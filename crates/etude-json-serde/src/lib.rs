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
//! - Strings use the has-escapes split: an escape-free string is a zero-copy [`RopeStr::Borrowed`]
//!   (an O(1) [`StrRope`] slice of the source rope — the copy-avoidance payoff); only an escaped
//!   string is materialized into a [`RopeStr::Owned`] buffer via `Token::decode_string`. (etude-json
//!   #147's `decode_str_rope` will let the escaped case return a built leaf too; the common zero-copy
//!   case needs only `Token::string_span` + `string_has_escapes`, which exist today.)
//! - Numbers arrive as an [`etude_serde::NumberToken`]: the whole lexeme (`Token::span`) as the primary
//!   sub-rope plus the component runs (`Token::number_parts`) as structural metadata. No value is parsed
//!   here; a consumer decodes on demand — e.g. `Decimal::parse(lexeme.chunks().flat_map(…))`.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]

extern crate alloc;

use etude_bytevec::ByteVec;
use etude_json::{Span, Token, TokenKind, Tokenizer};
use etude_serde::{Deserializer, Error, MapAccess, NumberToken, RopeStr, SeqAccess, Visitor};
use etude_strrope::StrRope;

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

/// Maximum container nesting depth. Recursive descent uses one Rust stack frame per level, so an
/// adversarial deeply-nested input (`[[[[…`) would otherwise overflow the stack; past this depth the
/// parser returns an error instead. Matches `serde_json`'s default recursion limit so accept/reject
/// stays in parity with the differential oracle.
const MAX_DEPTH: usize = 128;

/// The token stream with one-token lookahead. Owns the tokenizer + the input rope (string decode is
/// resolved against it). A cached lookahead makes grammar decisions (is the next token `]`? `,`?).
/// `depth` tracks open-container nesting so unbounded recursion can't overflow the stack.
struct Stream<'a> {
    iter: Tokenizer<'a>,
    input: &'a ByteVec,
    peeked: Option<Result<Option<Token>, Error>>,
    depth: usize,
}

impl<'a> Stream<'a> {
    fn new(input: &'a ByteVec) -> Self {
        Stream {
            iter: Tokenizer::new(input),
            input,
            peeked: None,
            depth: 0,
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
                let input = self.stream.input;
                // Copy-avoidance payoff: an escape-free string's content is handed as an O(1) structural
                // share (RopeStr::Borrowed) — no unescape, no allocation; only an escaped string is
                // materialized into an Owned buffer, chosen from the cheap has-escapes flag.
                //
                // Content validity is INPUT-derived, so it is an Err path, never a panic: the tokenizer
                // currently accepts string content that is not valid UTF-8 (a raw non-UTF-8 byte lexes as
                // a String), which serde_json rejects. Until that is resolved at the lexer (the pending
                // strict-vs-lossy policy call), the adapter rejects it here — a Deserializer must return
                // Err, not panic. (When the lexer validates UTF-8, from_utf8 becomes infallible and this
                // Borrowed arm can move to the zero-copy from_utf8_unchecked.)
                let has_escapes = token
                    .string_has_escapes()
                    .expect("String token has an escapes flag");
                if has_escapes {
                    let s = token
                        .decode_string(input)
                        .ok_or_else(|| Error::custom("string content could not be decoded"))?;
                    visitor.visit_str(RopeStr::Owned(s))
                } else {
                    let span = token
                        .string_span()
                        .expect("String token has a content span");
                    let rope = StrRope::from_utf8(input.slice(span.range()))
                        .map_err(|_| Error::custom("string content is not valid UTF-8"))?;
                    visitor.visit_str(RopeStr::Borrowed(rope))
                }
            }
            TokenKind::Number => {
                // Slice the whole-lexeme span ONCE (the only eager cost, and it makes the token
                // self-contained), then record each component as an offset RANGE WITHIN the lexeme —
                // the Visitor materializes a component only if it asks. This collapses the old
                // four-slices-per-number handoff to one slice for a skip/scan consumer.
                let p = token.number_parts().expect("Number token has parts");
                let input = self.stream.input;
                let lex = token.span().range();
                let base = lex.start;
                let rel = |s: Span| (s.range().start - base)..(s.range().end - base);
                visitor.visit_number(NumberToken::new(
                    input.slice(lex),
                    p.negative,
                    rel(p.integer),
                    p.fraction.map(rel),
                    p.exponent.map(rel),
                    p.exponent_negative,
                ))
            }
            TokenKind::BeginArray => {
                self.stream.depth += 1;
                if self.stream.depth > MAX_DEPTH {
                    return Err(Error::custom("recursion limit exceeded"));
                }
                // Reborrow so the depth can be restored once the sub-parse returns (its Value is owned,
                // so it does not keep the borrow alive).
                let out = visitor.visit_seq(Seq {
                    stream: &mut *self.stream,
                    first: true,
                });
                self.stream.depth -= 1;
                out
            }
            TokenKind::BeginObject => {
                self.stream.depth += 1;
                if self.stream.depth > MAX_DEPTH {
                    return Err(Error::custom("recursion limit exceeded"));
                }
                let out = visitor.visit_map(Map {
                    stream: &mut *self.stream,
                    first: true,
                });
                self.stream.depth -= 1;
                out
            }
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
