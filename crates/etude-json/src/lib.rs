// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Copy-avoiding JSON over the etude byte-rope.
//!
//! The point of this crate is to read JSON WITHOUT copying its bytes. [`Tokenizer`] walks a
//! [`ByteVec`] and yields [`Token`]s that each reference a byte RANGE of that input rope (a
//! [`Span`]) rather than owning a copy of the bytes. A caller that wants the bytes takes an O(1)
//! structural-sharing [`ByteVec::slice`] of the span; only a caller that needs a transformed value
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
use etude_bytevec::ByteVec;

/// A half-open byte range `[start, end)` into the input rope a [`Token`] was produced from.
///
/// A span carries no bytes; resolve it against the originating rope (e.g. [`ByteVec::slice`]) to
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
    pub fn decode_string(&self, input: &ByteVec) -> Option<String> {
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

/// A forward cursor over a [`ByteVec`]'s chunks.
///
/// It walks each contiguous leaf (`chunks()`) with a local slice index and refills at chunk
/// boundaries, so reading the next byte is O(1) amortized — each leaf is touched once, for O(n) over
/// the whole input. A per-byte `byte_at(offset)` would instead be an O(log n) tree descent every
/// byte (O(n log n) total, cache-hostile). The absolute byte offset is tracked only to stamp token
/// span endpoints, never to fetch a byte.
struct Cursor<'a> {
    chunks: etude_bytevec::Chunks<'a>,
    /// The current leaf. Empty exactly when the cursor is exhausted (past the last byte).
    chunk: &'a [u8],
    /// Index of the current byte within `chunk`. Invariant: `pos < chunk.len()` unless exhausted.
    pos: usize,
    /// Absolute byte offset of `chunk[0]` in the input.
    base: usize,
}

impl<'a> Cursor<'a> {
    fn new(input: &'a ByteVec) -> Self {
        let mut cursor = Cursor {
            chunks: input.chunks(),
            chunk: &[],
            pos: 0,
            base: 0,
        };
        // Prime with the first non-empty leaf (empty leaves carry no bytes and no offset). `refill`
        // over the empty initial `chunk` adds 0 to `base` and finds the first non-empty leaf.
        cursor.refill();
        cursor
    }

    /// The current byte without advancing, or `None` at end of input.
    fn peek(&self) -> Option<u8> {
        self.chunk.get(self.pos).copied()
    }

    /// The absolute byte offset of the current position (equals the input length at end of input).
    fn offset(&self) -> usize {
        self.base + self.pos
    }

    /// The unread bytes of the current leaf (empty when exhausted). Borrows the input, not `self`,
    /// so a caller can scan it and then mutate the cursor (e.g. [`Cursor::skip_in_chunk`]).
    fn chunk_tail(&self) -> &'a [u8] {
        &self.chunk[self.pos..]
    }

    /// Advance past the current byte. The caller must have observed a byte via [`Cursor::peek`]
    /// first (so `pos < chunk.len()`); at a leaf boundary this refills to the next non-empty leaf.
    fn bump(&mut self) {
        self.pos += 1;
        if self.pos >= self.chunk.len() {
            self.refill();
        }
    }

    /// Advance `k` bytes within the current leaf. The caller guarantees `k <= chunk_tail().len()`
    /// (the skip stays inside the current leaf); refills when it lands exactly at the leaf end. This
    /// is the bulk path: a run of ordinary bytes is skipped in one step instead of `k` `bump`s.
    fn skip_in_chunk(&mut self, k: usize) {
        self.pos += k;
        if self.pos >= self.chunk.len() {
            self.refill();
        }
    }

    /// Move to the next non-empty leaf (or the exhausted state), crediting the leaving leaf's whole
    /// length to `base` so [`Cursor::offset`] stays absolute.
    fn refill(&mut self) {
        self.base += self.chunk.len();
        self.pos = 0;
        self.chunk = &[];
        loop {
            match self.chunks.next() {
                Some(next) if !next.is_empty() => {
                    self.chunk = &next[..];
                    break;
                }
                Some(_) => {}
                None => break,
            }
        }
    }
}

/// An iterator of [`Token`]s over a [`ByteVec`].
///
/// Create one with [`Tokenizer::new`] and iterate it. Each step yields `Ok(token)` for a well-formed
/// token or `Err(error)` for malformed input; after an `Err` the iterator is exhausted (it will only
/// yield `None`). When the input holds no more tokens (only trailing whitespace remains) iteration
/// ends with `None`.
///
/// The tokenizer streams the rope's chunks through a [`Cursor`], reading each leaf once, so it is
/// correct regardless of how the rope is chunked — a token whose bytes straddle a rope-leaf boundary
/// is read the same as one within a single leaf, and no byte costs a tree descent.
pub struct Tokenizer<'a> {
    cursor: Cursor<'a>,
    done: bool,
}

impl<'a> Tokenizer<'a> {
    /// Create a tokenizer over `input`. Iterating it yields the JSON tokens of the rope's bytes.
    pub fn new(input: &'a ByteVec) -> Self {
        Tokenizer {
            cursor: Cursor::new(input),
            done: false,
        }
    }

    /// Advance past JSON insignificant whitespace (space, tab, LF, CR).
    fn skip_whitespace(&mut self) {
        while let Some(b) = self.cursor.peek() {
            match b {
                b' ' | b'\t' | b'\n' | b'\r' => self.cursor.bump(),
                _ => break,
            }
        }
    }

    /// Emit a one-byte structural token of `kind` starting at the current position.
    fn structural(&mut self, kind: TokenKind) -> Token {
        let start = self.cursor.offset();
        self.cursor.bump();
        Token {
            kind,
            span: Span {
                start,
                end: self.cursor.offset(),
            },
            string: None,
            number: None,
        }
    }

    /// Scan a `"..."` string beginning at the opening quote (the current position).
    fn scan_string(&mut self) -> Result<Token, Error> {
        let start = self.cursor.offset();
        self.cursor.bump(); // past the opening quote
        let mut has_escapes = false;
        loop {
            // Bulk-skip a run of ordinary bytes within the current leaf up to the next significant
            // byte (`"`, `\`, or a control byte `< 0x20`). This one contiguous-slice scan replaces a
            // per-byte peek/bump over ordinary content — the dominant cost on large strings.
            let tail = self.cursor.chunk_tail();
            match tail
                .iter()
                .position(|&b| b == b'"' || b == b'\\' || b < 0x20)
            {
                Some(k) => self.cursor.skip_in_chunk(k),
                None => {
                    if tail.is_empty() {
                        return Err(Error {
                            offset: start,
                            kind: ErrorKind::UnterminatedString,
                        });
                    }
                    // No significant byte in this leaf — skip to its end, continue in the next leaf.
                    self.cursor.skip_in_chunk(tail.len());
                    continue;
                }
            }
            // The cursor now rests on a significant byte.
            let b = self
                .cursor
                .peek()
                .expect("skip landed on a significant byte");
            match b {
                b'"' => {
                    let content = Span {
                        start: start + 1,
                        end: self.cursor.offset(),
                    };
                    self.cursor.bump(); // past the closing quote
                    return Ok(Token {
                        kind: TokenKind::String,
                        span: Span {
                            start,
                            end: self.cursor.offset(),
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
                    let esc_start = self.cursor.offset();
                    self.cursor.bump(); // past the backslash
                    let esc = self.cursor.peek().ok_or(Error {
                        offset: start,
                        kind: ErrorKind::UnterminatedString,
                    })?;
                    match esc {
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {
                            self.cursor.bump();
                        }
                        b'u' => {
                            self.cursor.bump(); // past the `u`
                            for _ in 0..4 {
                                match self.cursor.peek() {
                                    Some(h) if h.is_ascii_hexdigit() => self.cursor.bump(),
                                    _ => {
                                        return Err(Error {
                                            offset: esc_start,
                                            kind: ErrorKind::InvalidUnicodeEscape,
                                        });
                                    }
                                }
                            }
                        }
                        _ => {
                            return Err(Error {
                                offset: esc_start,
                                kind: ErrorKind::InvalidEscape,
                            });
                        }
                    }
                }
                // `position` stops on nothing else, so the remaining case is a control byte `< 0x20`.
                _ => {
                    return Err(Error {
                        offset: self.cursor.offset(),
                        kind: ErrorKind::ControlCharInString,
                    });
                }
            }
        }
    }

    /// Scan a numeric literal beginning at `self.pos`, validating the JSON number grammar
    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`.
    fn scan_number(&mut self) -> Result<Token, Error> {
        let start = self.cursor.offset();

        if self.cursor.peek() == Some(b'-') {
            self.cursor.bump();
        }

        // Integer part: a lone `0`, or a nonzero digit followed by more digits.
        match self.cursor.peek() {
            Some(b'0') => self.cursor.bump(),
            Some(b'1'..=b'9') => {
                self.cursor.bump();
                while matches!(self.cursor.peek(), Some(b'0'..=b'9')) {
                    self.cursor.bump();
                }
            }
            _ => {
                return Err(Error {
                    offset: self.cursor.offset(),
                    kind: ErrorKind::InvalidNumber,
                });
            }
        }

        // Optional fraction: `.` then at least one digit.
        let mut has_fraction = false;
        if self.cursor.peek() == Some(b'.') {
            has_fraction = true;
            self.cursor.bump();
            if !matches!(self.cursor.peek(), Some(b'0'..=b'9')) {
                return Err(Error {
                    offset: self.cursor.offset(),
                    kind: ErrorKind::InvalidNumber,
                });
            }
            while matches!(self.cursor.peek(), Some(b'0'..=b'9')) {
                self.cursor.bump();
            }
        }

        // Optional exponent: `e`/`E`, optional sign, at least one digit.
        let mut has_exponent = false;
        if matches!(self.cursor.peek(), Some(b'e') | Some(b'E')) {
            has_exponent = true;
            self.cursor.bump();
            if matches!(self.cursor.peek(), Some(b'+') | Some(b'-')) {
                self.cursor.bump();
            }
            if !matches!(self.cursor.peek(), Some(b'0'..=b'9')) {
                return Err(Error {
                    offset: self.cursor.offset(),
                    kind: ErrorKind::InvalidNumber,
                });
            }
            while matches!(self.cursor.peek(), Some(b'0'..=b'9')) {
                self.cursor.bump();
            }
        }

        Ok(Token {
            kind: TokenKind::Number,
            span: Span {
                start,
                end: self.cursor.offset(),
            },
            string: None,
            number: Some(NumberInfo {
                has_fraction,
                has_exponent,
            }),
        })
    }

    /// Scan a bare-word keyword (`word`) beginning at the current position, emitting `kind` on an
    /// exact match.
    fn scan_keyword(&mut self, word: &[u8], kind: TokenKind) -> Result<Token, Error> {
        let start = self.cursor.offset();
        for &expected in word {
            if self.cursor.peek() != Some(expected) {
                return Err(Error {
                    offset: start,
                    kind: ErrorKind::InvalidKeyword,
                });
            }
            self.cursor.bump();
        }
        Ok(Token {
            kind,
            span: Span {
                start,
                end: self.cursor.offset(),
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
        let b = match self.cursor.peek() {
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
                offset: self.cursor.offset(),
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
fn decode_content(input: &ByteVec, content: Span) -> String {
    // Materialize the content span once into a contiguous buffer, then decode with plain indexing.
    // `copy_to_bytes` is zero-copy when the content lies within a single rope leaf (its fast path);
    // otherwise it is one O(len) copy. Either way decoding is O(len), versus O(len·log n) for a
    // per-byte `byte_at` tree descent on a deep rope.
    let buf = input.slice(content.range()).copy_to_bytes();
    let src: &[u8] = &buf;
    let mut out: Vec<u8> = Vec::with_capacity(src.len());
    let mut pos = 0;
    while pos < src.len() {
        let b = src[pos];
        if b != b'\\' {
            // Copy the raw byte. Multi-byte UTF-8 sequences are copied byte-for-byte and reassembled
            // by the final `from_utf8` conversion.
            out.push(b);
            pos += 1;
            continue;
        }
        // Escape sequence.
        let esc = src.get(pos + 1).copied().unwrap_or(0);
        match esc {
            b'"' => out.push(b'"'),
            b'\\' => out.push(b'\\'),
            b'/' => out.push(b'/'),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0C),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'u' => {
                let hi = hex4(src, pos + 2);
                if (0xD800..=0xDBFF).contains(&hi)
                    && src.get(pos + 6) == Some(&b'\\')
                    && src.get(pos + 7) == Some(&b'u')
                {
                    // High surrogate followed by a `\u` escape — try to join a surrogate pair.
                    let lo = hex4(src, pos + 8);
                    if (0xDC00..=0xDFFF).contains(&lo) {
                        let c = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                        push_scalar(&mut out, c);
                        pos += 12;
                        continue;
                    }
                }
                push_scalar(&mut out, hi);
                pos += 6;
                continue;
            }
            _ => {}
        }
        pos += 2;
    }
    // The content was validated (well-formed escapes) during tokenization; raw bytes on the paths that
    // matter are valid UTF-8. `from_utf8_lossy` keeps decoding infallible for any residual bad byte.
    match String::from_utf8(out) {
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
fn hex4(src: &[u8], at: usize) -> u32 {
    let mut v = 0u32;
    for i in 0..4 {
        let d = src.get(at + i).copied().unwrap_or(b'0');
        v = (v << 4) | (d as char).to_digit(16).unwrap_or(0);
    }
    v
}

#[cfg(test)]
mod tests;
