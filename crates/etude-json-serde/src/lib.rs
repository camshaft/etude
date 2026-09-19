// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! An [`etude_serde`] `Deserializer` over the [`etude_json`] `Tokenizer` — the grammar/parser layer
//! on top of the lexer.
//!
//! **DRAFT / SPIKE.** This is the first real consumer of the rope-native serde seam
//! (`DESIGN-etude-serde-zerocopy-ecosystem.md` §5, cadenza PR #9315), built to validate the seam
//! against a live tokenizer and feed fit-feedback into the design before it locks. It rides alongside
//! the `etude-serde` draft in the same spike; neither is merged while Decision 0 (no `'de`) is under
//! review.
//!
//! # What it adds over the lexer
//! [`etude_json::Tokenizer`] is a lexer: it validates each token but not the grammar *between* tokens
//! (`[1,]`, `1 2`, `{"a" 1}` all lex). This adapter is the parser: a grammar-enforcing recursive
//! descent that drives an [`etude_serde::Visitor`], so its accept/reject matches `serde_json`. Because
//! the seam has no `'de` (Decision 0), nested [`SeqAccess`]/[`MapAccess`] values thread with no
//! lifetime plumbing — each element/value re-enters [`Deserializer::deserialize_any`] over the shared
//! token stream.
//!
//! # Strings and numbers
//! - Strings arrive as [`RopeStr::Owned`] via `Token::decode_string` for now. The zero-copy
//!   [`RopeStr::Borrowed`] arm (driven by the token's has-escapes flag) lands when
//!   `Token::decode_str_rope` merges (etude-json #147).
//! - Numbers arrive as an [`etude_serde::NumberToken`] built from `Token::number_parts` (component
//!   spans) — no value is parsed here; a value type consumes the spans on demand (§6).

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]

extern crate alloc;

use etude_bytevec::ByteVec;
use etude_json::{Token, TokenKind, Tokenizer};
use etude_serde::{Deserializer, Error, MapAccess, NumberToken, RopeStr, SeqAccess, Visitor};

/// Deserialize the single JSON value in `input` (a rope), driving `visitor`. Errors if the bytes are
/// not exactly one grammatical JSON value (trailing tokens, unterminated containers, missing
/// separators, …) — i.e. this enforces the grammar the lexer does not.
pub fn from_rope<V: Visitor>(input: &ByteVec, visitor: V) -> Result<V::Value, Error> {
    let mut stream = Stream::new(input);
    let value = de(&mut stream).deserialize_any(visitor)?;
    match stream.next()? {
        None => Ok(value),
        Some(_) => Err(Error::custom("trailing tokens after a complete value")),
    }
}

/// Map a lexer error into a seam error (its `Display` carries the kind + byte offset).
fn map_err(e: etude_json::Error) -> Error {
    Error::custom(e)
}

/// The token stream with one-token lookahead. Owns the tokenizer + the input rope (string decode is
/// resolved against it). A cached lookahead makes grammar decisions (is the next token `]`? `,`?).
struct Stream<'a> {
    iter: Tokenizer<'a>,
    input: &'a ByteVec,
    peeked: Option<Result<Option<Token>, Error>>,
}

impl<'a> Stream<'a> {
    fn new(input: &'a ByteVec) -> Self {
        Stream {
            iter: Tokenizer::new(input),
            input,
            peeked: None,
        }
    }

    fn fill(&mut self) {
        if self.peeked.is_none() {
            self.peeked = Some(match self.iter.next() {
                None => Ok(None),
                Some(Ok(t)) => Ok(Some(t)),
                Some(Err(e)) => Err(map_err(e)),
            });
        }
    }

    /// The kind of the next token, `None` at end of input.
    fn peek_kind(&mut self) -> Result<Option<TokenKind>, Error> {
        self.fill();
        match self.peeked.as_ref().expect("filled") {
            Ok(opt) => Ok(opt.as_ref().map(Token::kind)),
            Err(e) => Err(e.clone()),
        }
    }

    /// Consume and return the next token, `None` at end of input.
    fn next(&mut self) -> Result<Option<Token>, Error> {
        self.fill();
        self.peeked.take().expect("filled")
    }
}

/// Build a one-shot [`Deserializer`] positioned at the stream's current token.
fn de<'a, 'b>(stream: &'b mut Stream<'a>) -> JsonDeserializer<'a, 'b> {
    JsonDeserializer { stream }
}

/// Deserializes exactly one value at the stream's current position.
struct JsonDeserializer<'a, 'b> {
    stream: &'b mut Stream<'a>,
}

impl Deserializer for JsonDeserializer<'_, '_> {
    fn deserialize_any<V: Visitor>(self, visitor: V) -> Result<V::Value, Error> {
        let token = self
            .stream
            .next()?
            .ok_or_else(|| Error::custom("expected a value, found end of input"))?;
        match token.kind() {
            TokenKind::Null => visitor.visit_null(),
            TokenKind::True => visitor.visit_bool(true),
            TokenKind::False => visitor.visit_bool(false),
            TokenKind::String => {
                // Owned for now; the zero-copy Borrowed arm lands with decode_str_rope (#147).
                let s = token
                    .decode_string(self.stream.input)
                    .expect("String token decodes");
                visitor.visit_str(RopeStr::Owned(s))
            }
            TokenKind::Number => {
                // Slice each component span into an O(1) sub-rope so the NumberToken is self-contained
                // (the Visitor needs no handle on the source rope to read the digits).
                let p = token.number_parts().expect("Number token has parts");
                let input = self.stream.input;
                visitor.visit_number(NumberToken {
                    negative: p.negative,
                    integer: input.slice(p.integer.range()),
                    fraction: p.fraction.map(|s| input.slice(s.range())),
                    exponent: p.exponent.map(|s| input.slice(s.range())),
                    exponent_negative: p.exponent_negative,
                })
            }
            TokenKind::BeginArray => visitor.visit_seq(Seq {
                stream: self.stream,
                first: true,
            }),
            TokenKind::BeginObject => visitor.visit_map(Map {
                stream: self.stream,
                first: true,
            }),
            TokenKind::EndArray | TokenKind::EndObject | TokenKind::Colon | TokenKind::Comma => {
                Err(Error::custom(
                    "unexpected structural token where a value was expected",
                ))
            }
        }
    }
}

struct Seq<'a, 'b> {
    stream: &'b mut Stream<'a>,
    first: bool,
}

impl SeqAccess for Seq<'_, '_> {
    fn next_element<V: Visitor>(&mut self, visitor: V) -> Result<Option<V::Value>, Error> {
        match self.stream.peek_kind()? {
            None => return Err(Error::custom("unterminated array")),
            Some(TokenKind::EndArray) => {
                self.stream.next()?;
                return Ok(None);
            }
            _ => {}
        }
        if self.first {
            self.first = false;
        } else {
            // Elements after the first are comma-separated; a trailing comma (comma then `]`) is invalid.
            match self.stream.next()? {
                Some(t) if t.kind() == TokenKind::Comma => {}
                _ => return Err(Error::custom("expected ',' between array elements")),
            }
            if self.stream.peek_kind()? == Some(TokenKind::EndArray) {
                return Err(Error::custom("trailing comma in array"));
            }
        }
        de(self.stream).deserialize_any(visitor).map(Some)
    }
}

struct Map<'a, 'b> {
    stream: &'b mut Stream<'a>,
    first: bool,
}

impl MapAccess for Map<'_, '_> {
    fn next_key<V: Visitor>(&mut self, visitor: V) -> Result<Option<V::Value>, Error> {
        match self.stream.peek_kind()? {
            None => return Err(Error::custom("unterminated object")),
            Some(TokenKind::EndObject) => {
                self.stream.next()?;
                return Ok(None);
            }
            _ => {}
        }
        if self.first {
            self.first = false;
        } else {
            match self.stream.next()? {
                Some(t) if t.kind() == TokenKind::Comma => {}
                _ => return Err(Error::custom("expected ',' between object members")),
            }
            if self.stream.peek_kind()? == Some(TokenKind::EndObject) {
                return Err(Error::custom("trailing comma in object"));
            }
        }
        // A JSON member key must be a string.
        if self.stream.peek_kind()? != Some(TokenKind::String) {
            return Err(Error::custom("object member key must be a string"));
        }
        de(self.stream).deserialize_any(visitor).map(Some)
    }

    fn next_value<V: Visitor>(&mut self, visitor: V) -> Result<V::Value, Error> {
        match self.stream.next()? {
            Some(t) if t.kind() == TokenKind::Colon => {}
            _ => return Err(Error::custom("expected ':' after object key")),
        }
        de(self.stream).deserialize_any(visitor)
    }
}

#[cfg(test)]
mod tests;
