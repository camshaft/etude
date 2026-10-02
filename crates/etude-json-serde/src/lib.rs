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

/// Re-exported so a caller can select the parse mode for [`from_rope_with`] without depending on
/// [`etude_json`] directly.
pub use etude_json::Strictness;

/// Deserialize the single JSON value in `input` (a rope), driving `visitor`, in the default
/// [`Strictness::Strict`] mode (accept/reject matches `serde_json`). Errors if the bytes are not
/// exactly one grammatical JSON value (trailing tokens, unterminated containers, missing separators,
/// …) — i.e. this enforces the grammar the lexer does not.
pub fn from_rope<V: Visitor>(input: &ByteVec, visitor: V) -> Result<V::Value, Error> {
    from_rope_with(input, Strictness::Strict, visitor)
}

/// Deserialize the single JSON value in `input` with an explicit [`Strictness`] mode.
///
/// [`Strictness::Strict`] matches `serde_json` (string content must be valid UTF-8, `\u` surrogates
/// must be paired) and lets the escape-free string path take a zero-copy O(1) borrow of the source
/// rope. [`Strictness::Lenient`] accepts the documented superset — non-UTF-8 content and lone
/// surrogates — resolving both lossily (invalid content is rejected here rather than borrowed, since
/// a [`RopeStr::Borrowed`] must be valid UTF-8).
pub fn from_rope_with<V: Visitor>(
    input: &ByteVec,
    strictness: Strictness,
    visitor: V,
) -> Result<V::Value, Error> {
    let mut stream = Stream::with_strictness(input, strictness);
    let value = de(&mut stream).deserialize_any(visitor)?;
    match stream.peek()? {
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
    strictness: Strictness,
}

impl<'a> Stream<'a> {
    fn with_strictness(input: &'a ByteVec, strictness: Strictness) -> Self {
        Stream {
            iter: Tokenizer::with_strictness(input, strictness),
            input,
            peeked: None,
            depth: 0,
            strictness,
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

    /// Borrow the next token without consuming it (`None` at end of input). Combined with [`Stream::bump`]
    /// this dispatches on the token *by reference*: the deserializer reads it only through accessors and
    /// never needs to own it, so the (comparatively large) `Token` is not moved out of the lookahead on
    /// every value — only the small `Copy` metadata a given arm actually uses is read out.
    fn peek(&mut self) -> Result<Option<&Token>, Error> {
        self.fill();
        match self.peeked.as_ref().expect("filled") {
            Ok(opt) => Ok(opt.as_ref()),
            Err(e) => Err(e.clone()),
        }
    }

    /// Advance past the currently-peeked token. Cheap — it drops the lookahead slot so the next
    /// [`Stream::fill`] tokenizes the following token. Call only after a successful peek.
    fn bump(&mut self) {
        self.peeked = None;
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
        // Dispatch on the token BY REFERENCE: read its kind (Copy) from the lookahead without moving
        // the whole `Token` out, then let each arm read only the small `Copy` metadata it needs and
        // `bump` past it. This keeps the ~96-byte `Token` in the lookahead slot instead of memcpying
        // it per value.
        let kind = match self.stream.peek()? {
            Some(t) => t.kind(),
            None => return Err(Error::custom("expected a value, found end of input")),
        };
        match kind {
            TokenKind::Null => {
                self.stream.bump();
                visitor.visit_null()
            }
            TokenKind::True => {
                self.stream.bump();
                visitor.visit_bool(true)
            }
            TokenKind::False => {
                self.stream.bump();
                visitor.visit_bool(false)
            }
            TokenKind::String => {
                // Copy-avoidance payoff: an escape-free string's content is handed as an O(1) structural
                // share (RopeStr::Borrowed) — no unescape, no allocation; only an escaped string is
                // materialized into an Owned buffer, chosen from the cheap has-escapes flag. The built
                // `RopeStr` owns its data (rope handle / String), so the token borrow can end before we
                // `bump`.
                let input = self.stream.input;
                let strictness = self.stream.strictness;
                let token = self.stream.peek()?.expect("peeked a String token above");
                let has_escapes = token
                    .string_has_escapes()
                    .expect("String token has an escapes flag");
                let rope_str = if has_escapes {
                    RopeStr::Owned(
                        token
                            .decode_string(input)
                            .ok_or_else(|| Error::custom("string content could not be decoded"))?,
                    )
                } else {
                    let span = token
                        .string_span()
                        .expect("String token has a content span");
                    let content = input.slice(span.range());
                    let rope = match strictness {
                        // A Strict tokenizer validates string-content UTF-8 at lex, so this span is
                        // guaranteed valid UTF-8 and the O(1) unchecked conversion is the zero-copy
                        // fast path — no redundant re-scan of the borrowed content.
                        Strictness::Strict => {
                            // SAFETY: `content` is the content span of a String token produced by a
                            // `Strictness::Strict` tokenizer, which rejects non-UTF-8 string content at
                            // lex time; the bytes are therefore valid UTF-8.
                            unsafe { StrRope::from_utf8_unchecked(content) }
                        }
                        // A Lenient tokenizer accepts non-UTF-8 content, so the borrowed bytes are not
                        // known-valid: validate here and reject (never borrow) invalid content — a
                        // Deserializer must return Err, not construct an invalid `StrRope` or panic.
                        Strictness::Lenient => StrRope::from_utf8(content)
                            .map_err(|_| Error::custom("string content is not valid UTF-8"))?,
                    };
                    RopeStr::Borrowed(rope)
                };
                self.stream.bump();
                visitor.visit_str(rope_str)
            }
            TokenKind::Number => {
                // Slice the whole-lexeme span ONCE (the only eager cost, and it makes the token
                // self-contained), then record each component as an offset RANGE WITHIN the lexeme —
                // the Visitor materializes a component only if it asks. This collapses the old
                // four-slices-per-number handoff to one slice for a skip/scan consumer.
                let input = self.stream.input;
                let token = self.stream.peek()?.expect("peeked a Number token above");
                let p = token.number_parts().expect("Number token has parts");
                let lex = token.span().range();
                let base = lex.start;
                let rel = |s: Span| (s.range().start - base)..(s.range().end - base);
                let number = NumberToken::new(
                    input.slice(lex),
                    p.negative,
                    rel(p.integer),
                    p.fraction.map(rel),
                    p.exponent.map(rel),
                    p.exponent_negative,
                );
                self.stream.bump();
                visitor.visit_number(number)
            }
            TokenKind::BeginArray => {
                self.stream.bump();
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
                self.stream.bump();
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
                self.stream.bump();
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
                self.stream.bump();
                return Ok(None);
            }
            _ => {}
        }
        if self.first {
            self.first = false;
        } else {
            // Elements after the first are comma-separated; a trailing comma (comma then `]`) is invalid.
            match self.stream.peek_kind()? {
                Some(TokenKind::Comma) => self.stream.bump(),
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
                self.stream.bump();
                return Ok(None);
            }
            _ => {}
        }
        if self.first {
            self.first = false;
        } else {
            match self.stream.peek_kind()? {
                Some(TokenKind::Comma) => self.stream.bump(),
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
        match self.stream.peek_kind()? {
            Some(TokenKind::Colon) => self.stream.bump(),
            _ => return Err(Error::custom("expected ':' after object key")),
        }
        de(self.stream).deserialize_any(visitor)
    }
}

#[cfg(test)]
mod tests;
