// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Copy-avoiding JSON over the etude byte-rope.
//!
//! The point of this crate is to read JSON WITHOUT copying its bytes. [`Tokenizer`] walks a
//! [`ByteRope`] and yields [`Token`]s that each reference a byte RANGE of that input rope (a
//! [`Span`]) rather than owning a copy of the bytes. A caller that wants the bytes takes an O(1)
//! structural-sharing [`ByteRope::slice`] of the span; only a caller that needs a transformed value
//! — an unescaped string, a parsed number — pays for materialization, and only then.
//!
//! # What the tokenizer does and does not do
//! It is a LEXER, not a parser. It validates each token in isolation (a string's escapes, a number's
//! grammar, a keyword's spelling) and reports the byte offset of the first malformed byte, but it
//! does NOT enforce JSON's grammar between tokens: the sequence `] ,` tokenizes into two structural
//! tokens without complaint. Grammar and nesting are a parser's job (a later layer built on this
//! iterator). Whitespace (space, tab, CR, LF) between tokens is skipped.
//!
//! # Copy-avoiding strings
//! A [`TokenKind::String`] token carries the span of its raw content (between the quotes) and a flag
//! for whether that content contains escape sequences. A caller can:
//! - take the raw content span as a zero-copy rope slice when [`Token::string_has_escapes`] is
//!   `false` (the common case), or
//! - call [`Token::decode_string`] to materialize the unescaped `String` when escapes are present.
//!
//! # Spans reference the input
//! Every [`Span`] is a half-open byte range `[start, end)` into the SAME rope the tokenizer was
//! created over. A span is meaningless against any other rope.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use etude_byterope::ByteRope;

/// A half-open byte range `[start, end)` into the input rope a [`Token`] was produced from.
///
/// A span carries no bytes; resolve it against the originating rope (e.g. [`ByteRope::slice`]) to
/// read them. Resolving it against a different rope is meaningless.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    start: usize,
    end: usize,
}

impl Span {
    /// The inclusive start byte offset.
    pub fn start(&self) -> usize {
        self.start
    }

    /// The exclusive end byte offset.
    pub fn end(&self) -> usize {
        self.end
    }

    /// The number of bytes the span covers.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Whether the span is empty (`start == end`).
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// The span as a `start..end` range, for slicing the input rope.
    pub fn range(&self) -> core::ops::Range<usize> {
        self.start..self.end
    }
}

/// The lexical class of a [`Token`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// `{`
    BeginObject,
    /// `}`
    EndObject,
    /// `[`
    BeginArray,
    /// `]`
    EndArray,
    /// `:`
    Colon,
    /// `,`
    Comma,
    /// `null`
    Null,
    /// `true`
    True,
    /// `false`
    False,
    /// A `"..."` string literal.
    String,
    /// A numeric literal (its bytes match the JSON number grammar; it is not parsed here).
    Number,
}

/// The extra data a [`TokenKind::String`] token carries: the span of its content between the quotes
/// and whether that content contains escape sequences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StringInfo {
    content: Span,
    has_escapes: bool,
}

/// Cheap flags recorded for a [`TokenKind::Number`] token during the mandatory boundary scan, so a
/// later decoder need not re-inspect the bytes to know the number's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NumberInfo {
    has_fraction: bool,
    has_exponent: bool,
}

/// A single JSON token: its class and the span of its bytes in the input rope.
///
/// The fields are PRIVATE and not part of the stable API — construct tokens only by iterating a
/// [`Tokenizer`], and read them through the accessors so the representation stays free to change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    kind: TokenKind,
    span: Span,
    string: Option<StringInfo>,
    number: Option<NumberInfo>,
}

impl Token {
    /// The token's lexical class.
    pub fn kind(&self) -> TokenKind {
        self.kind
    }

    /// The span of the token's full lexeme in the input rope. For a string this INCLUDES the
    /// surrounding quotes; for every other token it is exactly the token's bytes.
    pub fn span(&self) -> Span {
        self.span
    }

    /// For a [`TokenKind::String`] token, the span of its CONTENT between the quotes (no quotes);
    /// `None` for any other kind. When [`Token::string_has_escapes`] is `false`, a rope slice of this
    /// span is the string's exact bytes with no decoding needed.
    pub fn string_span(&self) -> Option<Span> {
        self.string.map(|s| s.content)
    }

    /// For a [`TokenKind::String`] token, whether its content contains any escape sequence (`\` …);
    /// `None` for any other kind. `false` means the content span is the literal string bytes.
    pub fn string_has_escapes(&self) -> Option<bool> {
        self.string.map(|s| s.has_escapes)
    }

    /// Materialize the unescaped content of a [`TokenKind::String`] token as an owned `String`;
    /// `None` for any other kind.
    ///
    /// This is the on-demand, pay-only-when-asked path: escapes are decoded (`\n`, `\t`, `\uXXXX`
    /// with surrogate-pair joining, …) into a fresh `String`. When the token has no escapes, prefer a
    /// zero-copy rope slice of [`Token::string_span`] instead of paying for this allocation.
    ///
    /// `input` MUST be the rope this token was tokenized from; the content span is resolved against
    /// it. The token's content was validated during tokenization, so decoding does not fail.
    pub fn decode_string(&self, input: &ByteRope) -> Option<String> {
        let info = self.string?;
        Some(decode_content(input, info.content))
    }

    /// For a [`TokenKind::Number`] token, whether the literal is an integer — no fraction and no
    /// exponent, so its bytes are a plain `-?[0-9]+`; `None` for any other kind.
    ///
    /// This is derived from flags recorded during the scan the tokenizer already had to perform, so a
    /// decoder can pick an integer fast path without re-reading the number's bytes.
    pub fn number_is_integer(&self) -> Option<bool> {
        self.number.map(|n| !n.has_fraction && !n.has_exponent)
    }
}

/// What went wrong, and where, while tokenizing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// A byte that cannot begin any token appeared where a token was expected.
    UnexpectedByte,
    /// The input ended inside a string literal (no closing quote).
    UnterminatedString,
    /// A `\` escape names a character JSON does not define (not one of `" \ / b f n r t u`).
    InvalidEscape,
    /// A `\u` escape is not followed by exactly four hexadecimal digits.
    InvalidUnicodeEscape,
    /// A raw control byte (`< 0x20`) appeared inside a string, where it must be escaped.
    ControlCharInString,
    /// A numeric literal does not match the JSON number grammar.
    InvalidNumber,
    /// A bare word is not one of the keywords `true`, `false`, `null`.
    InvalidKeyword,
}

/// A tokenization failure: its [`ErrorKind`] and the byte offset in the input rope where it occurred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Error {
    offset: usize,
    kind: ErrorKind,
}

impl Error {
    /// The byte offset in the input rope where the error was detected.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// What kind of malformed input caused the error.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.kind {
            ErrorKind::UnexpectedByte => "unexpected byte",
            ErrorKind::UnterminatedString => "unterminated string",
            ErrorKind::InvalidEscape => "invalid escape",
            ErrorKind::InvalidUnicodeEscape => "invalid unicode escape",
            ErrorKind::ControlCharInString => "unescaped control character in string",
            ErrorKind::InvalidNumber => "invalid number",
            ErrorKind::InvalidKeyword => "invalid keyword",
        };
        write!(f, "{what} at byte offset {}", self.offset)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

/// An iterator of [`Token`]s over a [`ByteRope`].
///
/// Create one with [`Tokenizer::new`] and iterate it. Each step yields `Ok(token)` for a well-formed
/// token or `Err(error)` for malformed input; after an `Err` the iterator is exhausted (it will only
/// yield `None`). When the input holds no more tokens (only trailing whitespace remains) iteration
/// ends with `None`.
///
/// The tokenizer borrows the rope for its lifetime and reads it by byte offset, so it is correct
/// regardless of how the rope is chunked — a token whose bytes straddle a rope-leaf boundary is read
/// the same as one within a single chunk.
pub struct Tokenizer<'a> {
    input: &'a ByteRope,
    pos: usize,
    done: bool,
}

impl<'a> Tokenizer<'a> {
    /// Create a tokenizer over `input`. Iterating it yields the JSON tokens of the rope's bytes.
    pub fn new(input: &'a ByteRope) -> Self {
        Tokenizer {
            input,
            pos: 0,
            done: false,
        }
    }

    /// The byte at `offset`, or `None` past the end of the input.
    fn byte(&self, offset: usize) -> Option<u8> {
        self.input.byte_at(offset)
    }

    /// Advance `pos` past JSON insignificant whitespace (space, tab, LF, CR).
    fn skip_whitespace(&mut self) {
        while let Some(b) = self.byte(self.pos) {
            match b {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    /// Emit a one-byte structural token of `kind` starting at the current position.
    fn structural(&mut self, kind: TokenKind) -> Token {
        let start = self.pos;
        self.pos += 1;
        Token {
            kind,
            span: Span {
                start,
                end: self.pos,
            },
            string: None,
            number: None,
        }
    }

    /// Scan a `"..."` string beginning at the opening quote (`self.pos`).
    fn scan_string(&mut self) -> Result<Token, Error> {
        let start = self.pos;
        let mut pos = start + 1; // past the opening quote
        let mut has_escapes = false;
        loop {
            let b = match self.byte(pos) {
                Some(b) => b,
                None => {
                    return Err(Error {
                        offset: start,
                        kind: ErrorKind::UnterminatedString,
                    });
                }
            };
            match b {
                b'"' => {
                    let content = Span {
                        start: start + 1,
                        end: pos,
                    };
                    self.pos = pos + 1; // past the closing quote
                    return Ok(Token {
                        kind: TokenKind::String,
                        span: Span {
                            start,
                            end: self.pos,
                        },
                        string: Some(StringInfo {
                            content,
                            has_escapes,
                        }),
                        number: None,
                    });
                }
                b'\\' => {
                    has_escapes = true;
                    let esc = self.byte(pos + 1).ok_or(Error {
                        offset: start,
                        kind: ErrorKind::UnterminatedString,
                    })?;
                    match esc {
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {
                            pos += 2;
                        }
                        b'u' => {
                            for i in 0..4 {
                                let h = self.byte(pos + 2 + i).ok_or(Error {
                                    offset: pos,
                                    kind: ErrorKind::InvalidUnicodeEscape,
                                })?;
                                if !h.is_ascii_hexdigit() {
                                    return Err(Error {
                                        offset: pos,
                                        kind: ErrorKind::InvalidUnicodeEscape,
                                    });
                                }
                            }
                            pos += 6;
                        }
                        _ => {
                            return Err(Error {
                                offset: pos,
                                kind: ErrorKind::InvalidEscape,
                            });
                        }
                    }
                }
                0x00..=0x1F => {
                    return Err(Error {
                        offset: pos,
                        kind: ErrorKind::ControlCharInString,
                    });
                }
                _ => pos += 1,
            }
        }
    }

    /// Scan a numeric literal beginning at `self.pos`, validating the JSON number grammar
    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`.
    fn scan_number(&mut self) -> Result<Token, Error> {
        let start = self.pos;
        let mut pos = start;

        if self.byte(pos) == Some(b'-') {
            pos += 1;
        }

        // Integer part: a lone `0`, or a nonzero digit followed by more digits.
        match self.byte(pos) {
            Some(b'0') => pos += 1,
            Some(b'1'..=b'9') => {
                pos += 1;
                while matches!(self.byte(pos), Some(b'0'..=b'9')) {
                    pos += 1;
                }
            }
            _ => {
                return Err(Error {
                    offset: pos,
                    kind: ErrorKind::InvalidNumber,
                });
            }
        }

        // Optional fraction: `.` then at least one digit.
        let mut has_fraction = false;
        if self.byte(pos) == Some(b'.') {
            has_fraction = true;
            pos += 1;
            if !matches!(self.byte(pos), Some(b'0'..=b'9')) {
                return Err(Error {
                    offset: pos,
                    kind: ErrorKind::InvalidNumber,
                });
            }
            while matches!(self.byte(pos), Some(b'0'..=b'9')) {
                pos += 1;
            }
        }

        // Optional exponent: `e`/`E`, optional sign, at least one digit.
        let mut has_exponent = false;
        if matches!(self.byte(pos), Some(b'e') | Some(b'E')) {
            has_exponent = true;
            pos += 1;
            if matches!(self.byte(pos), Some(b'+') | Some(b'-')) {
                pos += 1;
            }
            if !matches!(self.byte(pos), Some(b'0'..=b'9')) {
                return Err(Error {
                    offset: pos,
                    kind: ErrorKind::InvalidNumber,
                });
            }
            while matches!(self.byte(pos), Some(b'0'..=b'9')) {
                pos += 1;
            }
        }

        self.pos = pos;
        Ok(Token {
            kind: TokenKind::Number,
            span: Span { start, end: pos },
            string: None,
            number: Some(NumberInfo {
                has_fraction,
                has_exponent,
            }),
        })
    }

    /// Scan a bare-word keyword (`word`) beginning at `self.pos`, emitting `kind` on an exact match.
    fn scan_keyword(&mut self, word: &[u8], kind: TokenKind) -> Result<Token, Error> {
        let start = self.pos;
        for (i, &expected) in word.iter().enumerate() {
            if self.byte(start + i) != Some(expected) {
                return Err(Error {
                    offset: start,
                    kind: ErrorKind::InvalidKeyword,
                });
            }
        }
        self.pos = start + word.len();
        Ok(Token {
            kind,
            span: Span {
                start,
                end: self.pos,
            },
            string: None,
            number: None,
        })
    }
}

impl Iterator for Tokenizer<'_> {
    type Item = Result<Token, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        self.skip_whitespace();
        let b = match self.byte(self.pos) {
            Some(b) => b,
            None => {
                self.done = true;
                return None;
            }
        };
        let result = match b {
            b'{' => Ok(self.structural(TokenKind::BeginObject)),
            b'}' => Ok(self.structural(TokenKind::EndObject)),
            b'[' => Ok(self.structural(TokenKind::BeginArray)),
            b']' => Ok(self.structural(TokenKind::EndArray)),
            b':' => Ok(self.structural(TokenKind::Colon)),
            b',' => Ok(self.structural(TokenKind::Comma)),
            b'"' => self.scan_string(),
            b'-' | b'0'..=b'9' => self.scan_number(),
            b't' => self.scan_keyword(b"true", TokenKind::True),
            b'f' => self.scan_keyword(b"false", TokenKind::False),
            b'n' => self.scan_keyword(b"null", TokenKind::Null),
            _ => Err(Error {
                offset: self.pos,
                kind: ErrorKind::UnexpectedByte,
            }),
        };
        if result.is_err() {
            self.done = true;
        }
        Some(result)
    }
}

impl core::iter::FusedIterator for Tokenizer<'_> {}

/// Decode the (validated) content of a string token into an owned `String`, applying JSON escapes.
///
/// `content` is the span BETWEEN the quotes. The bytes were validated during tokenization, so every
/// escape is well-formed here; a `\u` value that is not a scalar (a lone surrogate) is replaced with
/// U+FFFD rather than failing, since decoding is infallible by contract.
fn decode_content(input: &ByteRope, content: Span) -> String {
    let mut bytes: Vec<u8> = Vec::with_capacity(content.len());
    let mut pos = content.start;
    let end = content.end;
    while pos < end {
        let b = input.byte_at(pos).unwrap_or(0);
        if b != b'\\' {
            // Copy the raw byte. Multi-byte UTF-8 sequences are copied byte-for-byte and reassembled
            // by the final `from_utf8` conversion.
            bytes.push(b);
            pos += 1;
            continue;
        }
        // Escape sequence.
        let esc = input.byte_at(pos + 1).unwrap_or(0);
        match esc {
            b'"' => bytes.push(b'"'),
            b'\\' => bytes.push(b'\\'),
            b'/' => bytes.push(b'/'),
            b'b' => bytes.push(0x08),
            b'f' => bytes.push(0x0C),
            b'n' => bytes.push(b'\n'),
            b'r' => bytes.push(b'\r'),
            b't' => bytes.push(b'\t'),
            b'u' => {
                let hi = hex4(input, pos + 2);
                if (0xD800..=0xDBFF).contains(&hi)
                    && input.byte_at(pos + 6) == Some(b'\\')
                    && input.byte_at(pos + 7) == Some(b'u')
                {
                    // High surrogate followed by a `\u` escape — try to join a surrogate pair.
                    let lo = hex4(input, pos + 8);
                    if (0xDC00..=0xDFFF).contains(&lo) {
                        let c = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                        push_scalar(&mut bytes, c);
                        pos += 12;
                        continue;
                    }
                }
                push_scalar(&mut bytes, hi);
                pos += 6;
                continue;
            }
            _ => {}
        }
        pos += 2;
    }
    // The content was validated (well-formed escapes) during tokenization; raw bytes on the paths that
    // matter are valid UTF-8. `from_utf8_lossy` keeps decoding infallible for any residual bad byte.
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

/// Append the UTF-8 encoding of Unicode scalar value `c` to `bytes`, replacing a non-scalar (a lone
/// surrogate) with U+FFFD so decoding stays infallible.
fn push_scalar(bytes: &mut Vec<u8>, c: u32) {
    let ch = char::from_u32(c).unwrap_or('\u{FFFD}');
    let mut buf = [0u8; 4];
    bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
}

/// Read four hex digits at `at` and return their value; invalid/missing digits contribute nothing
/// (the tokenizer already validated them, so this only runs over well-formed input).
fn hex4(input: &ByteRope, at: usize) -> u32 {
    let mut v = 0u32;
    for i in 0..4 {
        let d = input.byte_at(at + i).unwrap_or(b'0');
        v = (v << 4) | (d as char).to_digit(16).unwrap_or(0);
    }
    v
}

#[cfg(test)]
mod tests;
