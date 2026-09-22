// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Copy-avoiding JSON over the etude byte-rope.
//!
//! The point of this crate is to read JSON without copying its bytes. [`Tokenizer`] walks a
//! [`ByteVec`] and yields [`Token`]s that each reference a byte range of that input rope (a
//! [`Span`]) rather than owning a copy of the bytes. A caller that wants the bytes takes an O(1)
//! structural-sharing [`ByteVec::slice`] of the span; only a caller that needs a transformed value
//! — an unescaped string, a parsed number — pays for materialization, and only then.
//!
//! # What the tokenizer does and does not do
//! It is a lexer, not a parser. It validates each token in isolation (a string's escapes, a number's
//! grammar, a keyword's spelling) and reports the byte offset of the first malformed byte, but it
//! does not enforce JSON's grammar between tokens: the sequence `] ,` tokenizes into two structural
//! tokens without complaint. Grammar and nesting are a parser's job (a later layer built on this
//! iterator). Whitespace (space, tab, CR, LF) between tokens is skipped.
//!
//! # Copy-avoiding strings
//! A [`TokenKind::String`] token carries the span of its raw content (between the quotes) and a flag
//! for whether that content contains escape sequences. A caller can:
//! - take the raw content span as a zero-copy rope slice when [`Token::string_has_escapes`] is
//!   `false` (the common case),
//! - call [`Token::decode_str_rope`] for the string value as a [`StrRope`] over the input's chunks —
//!   a zero-copy borrow when there are no escapes, a single built leaf when there are (the
//!   copy-avoiding string value), or
//! - call [`Token::decode_string`] to materialize a flat unescaped `String`.
//!
//! # Strictness
//! By default ([`Strictness::Strict`], via [`Tokenizer::new`]) the tokenizer enforces full JSON
//! string correctness — content must be valid UTF-8 and every `\u` surrogate must be paired — so its
//! accept/reject matches `serde_json`. [`Tokenizer::with_strictness`] can instead select
//! [`Strictness::Lenient`], which accepts a documented superset (non-UTF-8 content and lone
//! surrogates pass, to be resolved lossily on decode). See [`Strictness`].
//!
//! # Spans reference the input
//! Every [`Span`] is a half-open byte range `[start, end)` into the same rope the tokenizer was
//! created over. A span is meaningless against any other rope.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use etude_bytevec::ByteVec;
use etude_span::Cursor;
use etude_strrope::StrRope;

/// A half-open byte range into the input rope, re-exported from [`etude_span`]. A [`Token`]'s
/// [`Token::span`] is resolved against the originating rope (e.g. [`ByteVec::slice`]) to read bytes.
pub use etude_span::Span;

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

/// The kind-specific payload a [`Token`] carries. A token is at most one of a string or a number, so
/// this is an enum (its size is the larger arm) rather than two `Option` fields (whose sizes add) —
/// it keeps [`Token`] small on the hot path, where it is moved through the parser's lookahead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Payload {
    /// Structural / keyword tokens carry no extra data.
    None,
    /// A [`TokenKind::String`]: the span of its content between the quotes and whether that content
    /// contains any escape sequence.
    Str { content: Span, has_escapes: bool },
    /// A [`TokenKind::Number`]: its decomposition into sign / integer / fraction / exponent spans.
    Num(NumberParts),
}

/// The parts of a [`TokenKind::Number`] lexeme, recorded during the mandatory boundary scan as
/// byte-offset [`Span`]s into the input rope. This is representation-neutral: no value is parsed and
/// no digits are copied — a decoder reads the digit spans and folds them into whatever numeric type
/// it wants (e.g. `etude-decimal`) without re-scanning the bytes.
///
/// For `-12.34e-5`: `negative` is `true`, `integer` spans `12`, `fraction` spans `34`, `exponent`
/// spans `5`, and `exponent_negative` is `true`. The coefficient digits are `integer` then
/// `fraction`; the effective power of ten is `±exponent − fraction.len()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumberParts {
    /// The lexeme has a leading `-`.
    pub negative: bool,
    /// The integer-part digits, without the sign. Always a non-empty span.
    pub integer: Span,
    /// The fraction digits after `.` (the digits only, no `.`), or `None` if there is no fraction.
    pub fraction: Option<Span>,
    /// The exponent digits after `e`/`E` and its optional sign (digits only), or `None` if there is
    /// no exponent.
    pub exponent: Option<Span>,
    /// The exponent carries an explicit `-`. `false` when there is no exponent or it is `+`/unsigned.
    pub exponent_negative: bool,
}

/// A single JSON token: its class and the span of its bytes in the input rope.
///
/// The fields are private and not part of the stable API — construct tokens only by iterating a
/// [`Tokenizer`], and read them through the accessors so the representation stays free to change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    kind: TokenKind,
    span: Span,
    payload: Payload,
}

// A `Token` is moved through the parser's one-token lookahead on the hot path, so keep it small.
// Folding the mutually-exclusive string/number data into one `Payload` enum (rather than two `Option`
// fields whose sizes add) holds it here; a regression is a conscious change, not a silent regrowth.
const _: () = assert!(core::mem::size_of::<Token>() <= 96);

impl Token {
    /// The token's lexical class.
    pub fn kind(&self) -> TokenKind {
        self.kind
    }

    /// The span of the token's full lexeme in the input rope. For a string this includes the
    /// surrounding quotes; for every other token it is exactly the token's bytes.
    pub fn span(&self) -> Span {
        self.span
    }

    /// For a [`TokenKind::String`] token, the span of its content between the quotes (no quotes);
    /// `None` for any other kind. When [`Token::string_has_escapes`] is `false`, a rope slice of this
    /// span is the string's exact bytes with no decoding needed.
    pub fn string_span(&self) -> Option<Span> {
        match self.payload {
            Payload::Str { content, .. } => Some(content),
            _ => None,
        }
    }

    /// For a [`TokenKind::String`] token, whether its content contains any escape sequence (`\` …);
    /// `None` for any other kind. `false` means the content span is the literal string bytes.
    pub fn string_has_escapes(&self) -> Option<bool> {
        match self.payload {
            Payload::Str { has_escapes, .. } => Some(has_escapes),
            _ => None,
        }
    }

    /// Materialize the unescaped content of a [`TokenKind::String`] token as an owned `String`;
    /// `None` for any other kind.
    ///
    /// This is the on-demand, pay-only-when-asked path: escapes are decoded (`\n`, `\t`, `\uXXXX`
    /// with surrogate-pair joining, …) into a fresh `String`. When the token has no escapes, prefer a
    /// zero-copy rope slice of [`Token::string_span`] instead of paying for this allocation.
    ///
    /// `input` must be the rope this token was tokenized from; the content span is resolved against
    /// it. The token's content was validated during tokenization, so decoding does not fail.
    pub fn decode_string(&self, input: &ByteVec) -> Option<String> {
        match self.payload {
            Payload::Str { content, .. } => Some(decode_content(input, content)),
            _ => None,
        }
    }

    /// Materialize the content of a [`TokenKind::String`] token as a [`StrRope`] — a UTF-8 string
    /// rope over the input's chunks; `None` for any other kind. This is the copy-avoiding string
    /// value: prefer it over [`Token::decode_string`], which allocates a flat `String`.
    ///
    /// - No escapes (the common case): the returned `StrRope` is a zero-copy structural-share of the
    ///   input rope's chunks (via [`StrRope::from_utf8`] over a [`ByteVec::slice`] of the content
    ///   span) — no bytes are copied.
    /// - Escapes present: the unescaped content is built into a fresh `StrRope` (`\n`, `\t`, `\uXXXX`
    ///   with surrogate-pair joining, …), pushing whole runs of ordinary bytes at a time.
    ///
    /// `input` must be the rope this token was tokenized from. The content was validated during
    /// tokenization, so decoding is infallible.
    pub fn decode_str_rope(&self, input: &ByteVec) -> Option<StrRope> {
        let info = self.string?;
        if !info.has_escapes {
            // Zero-copy borrow: a rope slice is O(1) structural-share, and `from_utf8` reuses the
            // allocation (validation only). Valid JSON content is valid UTF-8, so this succeeds; the
            // rare error path falls through to the escape decoder (which is lossy-tolerant).
            if let Ok(rope) = StrRope::from_utf8(input.slice(info.content.range())) {
                return Some(rope);
            }
        }
        // Escapes present: decode the unescaped content into one contiguous buffer (the bulk-copy
        // path), then wrap it as a single rope leaf. Appending each run/escape to the rope separately
        // instead would allocate a leaf per piece — pathological for escape-dense strings. (Moving the
        // buffer in via `from_utf8` to skip the wrap's copy was measured slower here: its validation
        // scan plus a guard scan cost more than the single copy on many short strings.)
        Some(StrRope::from(decode_content(input, info.content)))
    }

    /// For a [`TokenKind::Number`] token, whether the literal is an integer — no fraction and no
    /// exponent, so its bytes are a plain `-?[0-9]+`; `None` for any other kind.
    ///
    /// This is derived from the parts recorded during the scan the tokenizer already had to perform,
    /// so a decoder can pick an integer fast path without re-reading the number's bytes.
    pub fn number_is_integer(&self) -> Option<bool> {
        match self.payload {
            Payload::Num(n) => Some(n.fraction.is_none() && n.exponent.is_none()),
            _ => None,
        }
    }

    /// For a [`TokenKind::Number`] token, its decomposition into sign / integer / fraction / exponent
    /// digit [`Span`]s (see [`NumberParts`]); `None` for any other kind.
    ///
    /// The parts are recorded during the mandatory scan, so a decoder builds its numeric value
    /// directly from the digit spans — resolve each span against the originating rope for its bytes —
    /// with no re-scan and no digits copied by the tokenizer.
    pub fn number_parts(&self) -> Option<NumberParts> {
        match self.payload {
            Payload::Num(n) => Some(n),
            _ => None,
        }
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
    /// A `\u` escape names a surrogate that is not part of a valid high-then-low pair — a lone or
    /// unpaired surrogate. Only reported in [`Strictness::Strict`]; in [`Strictness::Lenient`] a
    /// lone surrogate is accepted (and decoded lossily to U+FFFD).
    LoneSurrogate,
    /// A string's raw content bytes are not valid UTF-8. Only reported in [`Strictness::Strict`]; in
    /// [`Strictness::Lenient`] non-UTF-8 content is accepted (and decoded lossily).
    InvalidUtf8,
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
            ErrorKind::LoneSurrogate => "lone or unpaired surrogate in unicode escape",
            ErrorKind::InvalidUtf8 => "string content is not valid UTF-8",
            ErrorKind::ControlCharInString => "unescaped control character in string",
            ErrorKind::InvalidNumber => "invalid number",
            ErrorKind::InvalidKeyword => "invalid keyword",
        };
        write!(f, "{what} at byte offset {}", self.offset)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

/// How strictly the tokenizer enforces JSON string correctness.
///
/// The only axis on which JSON parsers legitimately differ is how they treat two malformed-string
/// conditions the byte-oriented scan can otherwise wave through: string content that is not valid
/// UTF-8, and a `\u` escape naming a lone (unpaired) surrogate. This selects between matching
/// `serde_json` / RFC 8259 §8.1 exactly and accepting a documented lenient superset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Strictness {
    /// Enforce full string correctness at lex time (the default): string content must be valid UTF-8
    /// and every `\u` surrogate must form a high-then-low pair. Accept/reject matches `serde_json`.
    ///
    /// Because a `Strict` tokenizer guarantees a [`TokenKind::String`] token's content span is valid
    /// UTF-8, a consumer may read that span with an unchecked O(1) conversion (no re-validation).
    #[default]
    Strict,
    /// Accept a lenient superset of JSON strings: string content that is not valid UTF-8 and lone
    /// `\u` surrogates are tolerated (a decoder resolves both lossily to U+FFFD).
    ///
    /// A consumer must not assume a string token's content is valid UTF-8 under this mode.
    Lenient,
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
    strictness: Strictness,
}

impl<'a> Tokenizer<'a> {
    /// Create a tokenizer over `input` in the default ([`Strictness::Strict`]) mode. Iterating it
    /// yields the JSON tokens of the rope's bytes.
    pub fn new(input: &'a ByteVec) -> Self {
        Self::with_strictness(input, Strictness::Strict)
    }

    /// Create a tokenizer over `input` with an explicit [`Strictness`] mode (see its variants for
    /// what each accepts).
    pub fn with_strictness(input: &'a ByteVec, strictness: Strictness) -> Self {
        Tokenizer {
            cursor: Cursor::new(input),
            done: false,
            strictness,
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
            span: Span::new(start, self.cursor.offset()),
            payload: Payload::None,
        }
    }

    /// Scan a `"..."` string beginning at the opening quote (the current position).
    fn scan_string(&mut self) -> Result<Token, Error> {
        let start = self.cursor.offset();
        self.cursor.bump(); // past the opening quote
        let mut has_escapes = false;
        let strict = self.strictness == Strictness::Strict;
        // In Strict mode the raw content bytes must be valid UTF-8. Content is delivered in leaf-sized
        // ordinary-byte runs; a multi-byte char can straddle a rope-leaf boundary but never one of the
        // significant bytes (`"`, `\`, control), which are all ASCII (`< 0x80`) and so never a piece of
        // a multi-byte sequence. `Utf8Check` therefore validates run-by-run, carrying an incomplete
        // trailing char only across leaf boundaries.
        let mut utf8 = Utf8Check::default();
        loop {
            // Bulk-skip a run of ordinary bytes within the current leaf up to the next significant
            // byte (`"`, `\`, or a control byte `< 0x20`). This one contiguous-slice scan replaces a
            // per-byte peek/bump over ordinary content — the dominant cost on large strings.
            let tail = self.cursor.chunk_tail();
            match tail
                .iter()
                .position(|&b| b == b'"' || b == b'\\' || b < 0x20)
            {
                Some(k) => {
                    if strict {
                        let run_start = self.cursor.offset();
                        utf8.feed(&tail[..k]).map_err(|off| Error {
                            offset: run_start + off,
                            kind: ErrorKind::InvalidUtf8,
                        })?;
                        // The upcoming significant byte is ASCII, so a still-pending multi-byte char
                        // was cut short by it — that is invalid UTF-8.
                        if !utf8.is_clean() {
                            return Err(Error {
                                offset: run_start + k,
                                kind: ErrorKind::InvalidUtf8,
                            });
                        }
                    }
                    self.cursor.skip_in_chunk(k);
                }
                None => {
                    if tail.is_empty() {
                        // End of input with a pending incomplete char is also invalid UTF-8, but the
                        // missing closing quote is the more fundamental defect — report that.
                        return Err(Error {
                            offset: start,
                            kind: ErrorKind::UnterminatedString,
                        });
                    }
                    if strict {
                        let run_start = self.cursor.offset();
                        utf8.feed(tail).map_err(|off| Error {
                            offset: run_start + off,
                            kind: ErrorKind::InvalidUtf8,
                        })?;
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
                    let content = Span::new(start + 1, self.cursor.offset());
                    self.cursor.bump(); // past the closing quote
                    return Ok(Token {
                        kind: TokenKind::String,
                        span: Span::new(start, self.cursor.offset()),
                        payload: Payload::Str {
                            content,
                            has_escapes,
                        },
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
                            let hi = self.read_hex4(esc_start)?;
                            if strict {
                                self.check_surrogate(hi, esc_start)?;
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

    /// Read exactly four hexadecimal digits at the cursor, consuming them, and return their value.
    /// `esc_start` is the offset of the escape's `\`, used to locate an [`ErrorKind::InvalidUnicodeEscape`].
    fn read_hex4(&mut self, esc_start: usize) -> Result<u32, Error> {
        let mut v = 0u32;
        for _ in 0..4 {
            match self.cursor.peek() {
                Some(h) if h.is_ascii_hexdigit() => {
                    v = (v << 4) | (h as char).to_digit(16).expect("ascii hex digit");
                    self.cursor.bump();
                }
                _ => {
                    return Err(Error {
                        offset: esc_start,
                        kind: ErrorKind::InvalidUnicodeEscape,
                    });
                }
            }
        }
        Ok(v)
    }

    /// In Strict mode, enforce `\u` surrogate pairing for a just-read escape value `hi` (the escape's
    /// `\` at `esc_start`): a high surrogate must be immediately followed by a `\u` low surrogate, and
    /// a lone low surrogate is rejected. A BMP scalar is fine and consumes nothing further.
    fn check_surrogate(&mut self, hi: u32, esc_start: usize) -> Result<(), Error> {
        let lone = Err(Error {
            offset: esc_start,
            kind: ErrorKind::LoneSurrogate,
        });
        if (0xD800..=0xDBFF).contains(&hi) {
            // High surrogate: require a following `\uXXXX` naming a low surrogate.
            if self.cursor.peek() != Some(b'\\') {
                return lone;
            }
            let lo_start = self.cursor.offset();
            self.cursor.bump(); // `\`
            if self.cursor.peek() != Some(b'u') {
                return lone;
            }
            self.cursor.bump(); // `u`
            let lo = self.read_hex4(lo_start)?;
            if !(0xDC00..=0xDFFF).contains(&lo) {
                return lone;
            }
            Ok(())
        } else if (0xDC00..=0xDFFF).contains(&hi) {
            // A low surrogate with no preceding high surrogate.
            lone
        } else {
            Ok(())
        }
    }

    /// Bulk-skip a run of ASCII digits (`0`–`9`) from the current position, crossing rope-leaf
    /// boundaries. This replaces a per-byte `peek`/`bump` loop with one contiguous-slice `position`
    /// scan per leaf — the same lever the string scan uses — so a long digit run (a big integer or a
    /// high-precision fraction) costs one scan per leaf rather than a call per digit.
    fn skip_digits(&mut self) {
        loop {
            let tail = self.cursor.chunk_tail();
            match tail.iter().position(|b| !b.is_ascii_digit()) {
                Some(k) => {
                    self.cursor.skip_in_chunk(k);
                    return;
                }
                None => {
                    if tail.is_empty() {
                        return;
                    }
                    // Whole leaf is digits — advance to its end and continue in the next leaf.
                    self.cursor.skip_in_chunk(tail.len());
                }
            }
        }
    }

    /// Scan a numeric literal beginning at `self.pos`, validating the JSON number grammar
    /// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`.
    fn scan_number(&mut self) -> Result<Token, Error> {
        let start = self.cursor.offset();

        let negative = self.cursor.peek() == Some(b'-');
        if negative {
            self.cursor.bump();
        }

        // Integer part: a lone `0`, or a nonzero digit followed by more digits. Record its span (no
        // sign) — always non-empty for a valid number.
        let int_start = self.cursor.offset();
        match self.cursor.peek() {
            Some(b'0') => self.cursor.bump(),
            Some(b'1'..=b'9') => {
                self.cursor.bump();
                self.skip_digits();
            }
            _ => {
                return Err(Error {
                    offset: self.cursor.offset(),
                    kind: ErrorKind::InvalidNumber,
                });
            }
        }
        let integer = Span::new(int_start, self.cursor.offset());

        // Optional fraction: `.` then at least one digit. Record the digit span (no `.`).
        let mut fraction = None;
        if self.cursor.peek() == Some(b'.') {
            self.cursor.bump();
            let frac_start = self.cursor.offset();
            if !matches!(self.cursor.peek(), Some(b'0'..=b'9')) {
                return Err(Error {
                    offset: self.cursor.offset(),
                    kind: ErrorKind::InvalidNumber,
                });
            }
            self.skip_digits();
            fraction = Some(Span::new(frac_start, self.cursor.offset()));
        }

        // Optional exponent: `e`/`E`, optional sign, at least one digit. Record the sign and the
        // digit span (no `e`/sign).
        let mut exponent = None;
        let mut exponent_negative = false;
        if matches!(self.cursor.peek(), Some(b'e') | Some(b'E')) {
            self.cursor.bump();
            match self.cursor.peek() {
                Some(b'-') => {
                    exponent_negative = true;
                    self.cursor.bump();
                }
                Some(b'+') => self.cursor.bump(),
                _ => {}
            }
            let exp_start = self.cursor.offset();
            if !matches!(self.cursor.peek(), Some(b'0'..=b'9')) {
                return Err(Error {
                    offset: self.cursor.offset(),
                    kind: ErrorKind::InvalidNumber,
                });
            }
            self.skip_digits();
            exponent = Some(Span::new(exp_start, self.cursor.offset()));
        }

        Ok(Token {
            kind: TokenKind::Number,
            span: Span::new(start, self.cursor.offset()),
            payload: Payload::Num(NumberParts {
                negative,
                integer,
                fraction,
                exponent,
                exponent_negative,
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
            span: Span::new(start, self.cursor.offset()),
            payload: Payload::None,
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

/// Incremental UTF-8 validator over a string's ordinary-content byte runs, used only in
/// [`Strictness::Strict`]. Runs arrive one rope-leaf piece at a time; because a run boundary that is
/// not a leaf boundary falls on an ASCII significant byte (`"`, `\`, control), a multi-byte char can
/// only be split across a leaf boundary — so at most a 3-byte incomplete-trailing `carry` need
/// survive between [`Utf8Check::feed`] calls. All of the hard validation (overlong forms, surrogate
/// range, `> U+10FFFF`) is delegated to `core::str::from_utf8`; this only manages the carry.
#[derive(Default)]
struct Utf8Check {
    carry: [u8; 4],
    carry_len: usize,
}

impl Utf8Check {
    /// Validate the next ordinary-content run. `Err(off)` on invalid UTF-8, where `off` is the byte
    /// offset within `run` at which the defect was detected (best-effort, for diagnostics).
    fn feed(&mut self, run: &[u8]) -> Result<(), usize> {
        let mut i = 0;
        // First, complete a char carried (incomplete) from the previous run's tail.
        while self.carry_len > 0 {
            if i == run.len() {
                return Ok(()); // run exhausted, char still incomplete — carry into the next run
            }
            self.carry[self.carry_len] = run[i];
            self.carry_len += 1;
            i += 1;
            match core::str::from_utf8(&self.carry[..self.carry_len]) {
                Ok(_) => {
                    self.carry_len = 0; // carried char completed and is valid
                    break;
                }
                // A definitively invalid sequence (not merely incomplete).
                Err(e) if e.error_len().is_some() => return Err(i - 1),
                // Still an incomplete-but-valid prefix; a char is at most four bytes.
                Err(_) if self.carry_len == 4 => return Err(i - 1),
                Err(_) => {}
            }
        }
        // Then validate the rest of the run in place.
        match core::str::from_utf8(&run[i..]) {
            Ok(_) => Ok(()),
            Err(e) if e.error_len().is_some() => Err(i + e.valid_up_to()),
            Err(e) => {
                // A valid prefix ending in an incomplete char — stash its (≤3) bytes for the next run.
                let tail = &run[i + e.valid_up_to()..];
                self.carry[..tail.len()].copy_from_slice(tail);
                self.carry_len = tail.len();
                Ok(())
            }
        }
    }

    /// Whether there is no pending incomplete char — must hold at an ASCII boundary and at end.
    fn is_clean(&self) -> bool {
        self.carry_len == 0
    }
}

/// Decode the (validated) content of a string token into an owned `String`, applying JSON escapes.
///
/// `content` is the span between the quotes. The bytes were validated during tokenization, so every
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
        // Bulk-copy the run of ordinary bytes up to the next escape in one `extend_from_slice`
        // instead of one `push` per byte — the same lever the tokenizer's string scan uses (#91).
        // Multi-byte UTF-8 sequences are copied byte-for-byte and reassembled by the final
        // `from_utf8` conversion; a string with no escapes is one scan plus one copy.
        match src[pos..].iter().position(|&b| b == b'\\') {
            None => {
                out.extend_from_slice(&src[pos..]);
                break;
            }
            Some(0) => {}
            Some(run) => {
                out.extend_from_slice(&src[pos..pos + run]);
                pos += run;
            }
        }
        // At an escape sequence (`src[pos] == b'\\'`).
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
    // The content was validated (well-formed escapes) during tokenization; raw bytes on the paths
    // that matter are valid UTF-8. `from_utf8_lossy` keeps decoding infallible for any residual bad
    // byte.
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
