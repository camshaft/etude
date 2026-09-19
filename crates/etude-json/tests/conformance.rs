// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! JSON conformance edge cases, driven through the public API as an external consumer.
//!
//! Complements `tests/public_api.rs` (API surface + number/string/error cases) by covering spec axes
//! it does not: non-recursive deep nesting, UTF-8 / BOM / surrogate handling, empty / whitespace-only
//! / trailing-token stream semantics, and duplicate keys. Where the tokenizer (a lexer, no
//! between-token grammar) is intentionally more permissive than `serde_json`, that split is asserted
//! explicitly. Cases run through several rope chunk layouts so straddling tokens are exercised.

use bytes::Bytes;
use etude_bytevec::ByteVec;
use etude_json::{Error, TokenKind, Tokenizer};

fn rope(bytes: &[u8], chunk: usize) -> ByteVec {
    let chunk = chunk.max(1);
    let mut r = ByteVec::new();
    let mut i = 0;
    while i < bytes.len() {
        let j = (i + chunk).min(bytes.len());
        r.push_back(Bytes::copy_from_slice(&bytes[i..j]));
        i = j;
    }
    r
}

/// Token kinds of `bytes` (single-chunk rope), or the first error.
fn kinds(bytes: &[u8]) -> Result<Vec<TokenKind>, Error> {
    let r = rope(bytes, bytes.len().max(1));
    Tokenizer::new(&r)
        .map(|res| res.map(|t| t.kind()))
        .collect()
}

#[test]
fn deep_nesting_is_iterative_no_stack_overflow() {
    // The tokenizer is a flat iterator with no recursion, so it tokenizes arbitrarily deep nesting
    // without a stack overflow — a depth that a recursive descent parser (serde_json's default 128
    // frame limit) rejects. This confirms the lexer imposes no depth limit of its own.
    let depth = 50_000;
    let mut doc = vec![b'['; depth];
    doc.push(b'1');
    doc.resize(doc.len() + depth, b']');

    let r = rope(&doc, 8 * 1024);
    let mut begins = 0usize;
    let mut ends = 0usize;
    let mut numbers = 0usize;
    for tok in Tokenizer::new(&r) {
        match tok.expect("deep nesting tokenizes without error").kind() {
            TokenKind::BeginArray => begins += 1,
            TokenKind::EndArray => ends += 1,
            TokenKind::Number => numbers += 1,
            other => panic!("unexpected token {other:?}"),
        }
    }
    assert_eq!((begins, ends, numbers), (depth, depth, 1));
    // serde_json's recursive parser rejects this depth rather than overflowing the stack.
    assert!(serde_json::from_slice::<serde_json::Value>(&doc).is_err());
}

#[test]
fn utf8_and_surrogate_escapes() {
    // Valid multi-byte UTF-8 (raw) and BMP/astral `\u` escapes both decode correctly.
    let r = rope(
        "\"CJK 日本語 and \\u00e9 and \\uD83D\\uDE00\"".as_bytes(),
        3,
    );
    let toks: Vec<_> = Tokenizer::new(&r).collect::<Result<_, _>>().unwrap();
    assert_eq!(toks.len(), 1);
    assert_eq!(
        toks[0].decode_string(&r).as_deref(),
        Some("CJK 日本語 and é and \u{1F600}")
    );

    // A lone high surrogate is valid escape *syntax* (four hex digits), so the lexer accepts the
    // string; decoding is infallible and yields U+FFFD for the non-scalar. serde_json parses the
    // value and rejects the lone surrogate — the intended lexer/parser split.
    let bytes = br#""lone\uD800end""#;
    for chunk in [1usize, 4, bytes.len()] {
        let r = rope(bytes, chunk);
        let toks: Vec<_> = Tokenizer::new(&r).collect::<Result<_, _>>().unwrap();
        assert_eq!(toks[0].kind(), TokenKind::String);
        assert_eq!(
            toks[0].decode_string(&r).as_deref(),
            Some("lone\u{FFFD}end")
        );
    }
    assert!(serde_json::from_slice::<serde_json::Value>(br#""lone\uD800end""#).is_err());
}

/// KNOWN SPEC DIVERGENCE (tracked repro, breaker-byterope): the tokenizer accepts a JSON string
/// whose content is not valid UTF-8 — a raw `0xFF` byte between the quotes, unescaped — as a normal
/// escape-free `String` token. RFC 8259 §8.1 requires JSON text to be UTF-8, and `serde_json`
/// rejects the same input. This is the byte-oriented lexer being more permissive than the spec, the
/// same strict-vs-lossy class as the still-open lone-surrogate ruling.
///
/// Downstream consequences this divergence causes (surfaced on their PRs):
/// - the etude-json-serde adapter (#184) does `StrRope::from_utf8(content).expect(...)` on the
///   escape-free arm and PANICS on this input — a deserializer must return `Err`;
/// - `Token::decode_str_rope` (#147) lossy-decodes the same content to U+FFFD.
///
/// So consumers disagree and neither matches `serde_json`. Pending the owner's design ruling: if the
/// tokenizer moves to validate string-content UTF-8 at lex time (matching serde_json / RFC, and
/// making the `from_utf8_unchecked` zero-copy wiring in #208/#213 sound by construction), this test
/// flips to assert rejection — it is the change-detector for that decision.
#[test]
fn string_content_invalid_utf8_is_accepted_diverging_from_serde() {
    let bytes: &[u8] = b"\"a\xffb\""; // a raw 0xFF byte inside the string, no escape
    let r = rope(bytes, bytes.len());
    let toks: Vec<_> = Tokenizer::new(&r)
        .collect::<Result<_, _>>()
        .expect("byte-oriented lexer currently accepts non-UTF-8 string content");
    assert_eq!(toks.len(), 1, "one String token");
    assert_eq!(toks[0].kind(), TokenKind::String);
    assert_eq!(
        toks[0].string_has_escapes(),
        Some(false),
        "escape-free, so a consumer takes the zero-copy borrow arm"
    );
    // The accepted content is genuinely not valid UTF-8 — the crux of the divergence.
    let span = toks[0]
        .string_span()
        .expect("string token has a content span");
    let content = r.slice(span.range()).copy_to_bytes();
    assert!(
        core::str::from_utf8(&content).is_err(),
        "content is not valid UTF-8 (the divergence)"
    );
    // serde_json rejects the same bytes outright.
    assert!(
        serde_json::from_slice::<serde_json::Value>(bytes).is_err(),
        "serde_json rejects non-UTF-8 JSON per RFC 8259"
    );
}

#[test]
fn byte_order_mark_is_rejected() {
    // JSON has no BOM; a leading U+FEFF (EF BB BF) is not a valid token start, so the tokenizer
    // reports UnexpectedByte at offset 0 — and serde_json rejects a leading BOM too.
    let bytes = b"\xef\xbb\xbf null";
    let err = kinds(bytes).expect_err("BOM is not valid JSON");
    assert_eq!(err.kind(), etude_json::ErrorKind::UnexpectedByte);
    assert_eq!(err.offset(), 0);
    assert!(serde_json::from_slice::<serde_json::Value>(bytes).is_err());
}

#[test]
fn empty_whitespace_and_trailing_stream_semantics() {
    // The tokenizer yields a token *stream*; empty or whitespace-only input is an empty stream (not
    // an error), whereas serde_json requires exactly one value and errors. The sound direction holds:
    // the tokenizer does not reject anything serde accepts.
    for empty in [&b""[..], b"   ", b"\t\n\r "] {
        assert_eq!(kinds(empty).unwrap(), Vec::<TokenKind>::new());
        assert!(serde_json::from_slice::<serde_json::Value>(empty).is_err());
    }

    // Trailing whitespace after a value is fine and does not add tokens (serde accepts too).
    assert_eq!(kinds(b"  null \t\n").unwrap(), vec![TokenKind::Null]);
    assert!(serde_json::from_slice::<serde_json::Value>(b"  null \t\n").is_ok());

    // Multiple top-level values lex as adjacent tokens (no between-token grammar); serde rejects.
    assert_eq!(
        kinds(b"null null").unwrap(),
        vec![TokenKind::Null, TokenKind::Null]
    );
    assert!(serde_json::from_slice::<serde_json::Value>(b"null null").is_err());

    // Trailing non-token garbage after a keyword is a lexical error at the offending byte.
    let err = kinds(b"truex").expect_err("truex");
    assert_eq!(err.offset(), 4);
    assert!(serde_json::from_slice::<serde_json::Value>(b"truex").is_err());
}

#[test]
fn duplicate_keys_are_preserved_in_the_stream() {
    use TokenKind::*;
    // The tokenizer preserves every key token in document order (it does not deduplicate — that is a
    // parser/value concern); serde_json accepts the document (last value wins).
    let doc = br#"{"a":1,"a":2}"#;
    for chunk in [1usize, 5, doc.len()] {
        let r = rope(doc, chunk);
        let ks: Vec<_> = Tokenizer::new(&r)
            .map(|res| res.map(|t| t.kind()))
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            ks,
            vec![
                BeginObject,
                String,
                Colon,
                Number,
                Comma,
                String,
                Colon,
                Number,
                EndObject
            ],
            "chunk={chunk}"
        );
    }
    assert!(serde_json::from_slice::<serde_json::Value>(doc).is_ok());
}
