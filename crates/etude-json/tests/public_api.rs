// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Integration tests: drive `etude-json` as an external consumer.
//!
//! The unit tests in `src/tests.rs` are a property-based differential oracle (`use super::*`, private
//! access). These tests are complementary and deliberately outside the crate: they exercise only the
//! public API surface — [`Tokenizer`], the [`Token`] accessors, [`TokenKind`], [`Error`] /
//! [`ErrorKind`], [`Span`] — over a curated set of RFC 8259 edge cases, so a regression that narrows
//! or breaks the public surface (a lost accessor, a lifetime that no longer composes for a consumer)
//! is caught here. Every case is cross-checked against `serde_json` for accept/reject agreement and
//! run through several rope chunk layouts, so a token straddling a leaf boundary is exercised through
//! the public path.

use bytes::Bytes;
use etude_bytevec::ByteVec;
use etude_json::{Error, ErrorKind, TokenKind, Tokenizer};

/// Build a rope holding `bytes`, split into chunks of at most `chunk` bytes (min 1).
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

/// Drain a tokenizer over `bytes` (single-chunk rope), returning the token kinds or the first error.
/// Kinds are `Copy`, so nothing here depends on the `Token` type's lifetime — this stays valid for a
/// consumer regardless of whether a `Token` borrows the rope.
fn kinds(bytes: &[u8]) -> Result<Vec<TokenKind>, Error> {
    let r = rope(bytes, bytes.len().max(1));
    Tokenizer::new(&r)
        .map(|res| res.map(|t| t.kind()))
        .collect()
}

#[test]
fn number_grammar_edge_cases() {
    // (bytes, is_integer): the JSON number grammar the tokenizer accepts, with the scan-derived
    // integer flag (no fraction and no exponent).
    let accept: &[(&[u8], bool)] = &[
        (b"0", true),
        (b"-0", true),
        (b"123", true),
        (b"-9223372036854775808", true),
        (b"10000000000000000000000", true), // beyond u64 — still a valid integer lexeme
        (b"0.0", false),
        (b"3.14159", false),
        (b"-2.5e+10", false),
        (b"1E-5", false),
        (b"0e0", false),
    ];
    for (bytes, is_int) in accept {
        let r = rope(bytes, bytes.len().max(1));
        let toks: Vec<_> = Tokenizer::new(&r)
            .collect::<Result<_, _>>()
            .unwrap_or_else(|e| {
                panic!(
                    "rejected valid number {:?}: {e}",
                    String::from_utf8_lossy(bytes)
                )
            });
        assert_eq!(toks.len(), 1, "{:?}", String::from_utf8_lossy(bytes));
        assert_eq!(toks[0].kind(), TokenKind::Number);
        assert_eq!(toks[0].span().start(), 0);
        assert_eq!(toks[0].span().end(), bytes.len());
        assert_eq!(
            toks[0].number_is_integer(),
            Some(*is_int),
            "is_integer for {:?}",
            String::from_utf8_lossy(bytes)
        );
        // serde_json accepts these too (sound differential on the accept side).
        assert!(serde_json::from_slice::<serde_json::Value>(bytes).is_ok());
    }

    // Lexer vs parser: `1e999` is a grammar-valid number lexeme, so the tokenizer accepts it (a
    // lexer records the span; value overflow is a decode-layer concern), whereas serde_json parses
    // the value and rejects the f64 overflow. This is the intended lexer/parser split, not a bug.
    assert_eq!(kinds(b"1e999").unwrap(), vec![TokenKind::Number]);
    assert!(serde_json::from_slice::<serde_json::Value>(b"1e999").is_err());

    // Leading zeros are again the lexer/parser split: `0` is a complete number lexeme, so `01`/`00`
    // tokenize as two adjacent Number tokens (like `1 2` would), whereas serde_json rejects the
    // leading zero at the grammar level. The tokenizer does not enforce between-token grammar.
    for bytes in [&b"01"[..], b"00"] {
        assert_eq!(
            kinds(bytes).unwrap(),
            vec![TokenKind::Number, TokenKind::Number]
        );
        assert!(serde_json::from_slice::<serde_json::Value>(bytes).is_err());
    }

    // Malformed number lexemes the tokenizer must reject — and serde_json rejects them too.
    let reject: &[&[u8]] = &[b"1.", b".5", b"1e", b"-", b"+1", b"1.2.3", b"1e+"];
    for bytes in reject {
        assert!(
            kinds(bytes).is_err(),
            "tokenizer accepted malformed number {:?}",
            String::from_utf8_lossy(bytes)
        );
        assert!(
            serde_json::from_slice::<serde_json::Value>(bytes).is_err(),
            "serde_json accepted {:?}",
            String::from_utf8_lossy(bytes)
        );
    }
}

#[test]
fn string_escapes_decode_through_public_api() {
    let cases: &[(&[u8], &str)] = &[
        (br#""""#, ""),
        (br#""plain""#, "plain"),
        (br#""tab\tnl\nquote\"""#, "tab\tnl\nquote\""),
        (br#""back\\slash\/fwd""#, "back\\slash/fwd"),
        (br#""ctrl\b\f\r""#, "ctrl\u{08}\u{0C}\r"),
        (b"\"bmpA\xc3\xa9\"", "bmpAé"),
        (b"\"astral\xf0\x9f\x98\x80\"", "astral\u{1F600}"),
    ];
    for (bytes, want) in cases {
        // Exercise through chunk layouts so escapes straddle leaf boundaries via the public path.
        for chunk in [1usize, 2, bytes.len().max(1)] {
            let r = rope(bytes, chunk);
            let toks: Vec<_> = Tokenizer::new(&r).collect::<Result<_, _>>().unwrap();
            assert_eq!(toks.len(), 1);
            assert_eq!(toks[0].kind(), TokenKind::String);
            assert_eq!(toks[0].string_has_escapes(), Some(bytes.contains(&b'\\')));
            assert_eq!(
                toks[0].decode_string(&r).as_deref(),
                Some(*want),
                "decode {:?} at chunk={chunk}",
                String::from_utf8_lossy(bytes)
            );
            // A no-escape string's content span is the exact bytes — the zero-copy path.
            if !bytes.contains(&b'\\') {
                let content = toks[0].string_span().unwrap();
                assert_eq!(content.end() - content.start(), want.len());
            }
        }
    }
}

#[test]
fn error_kind_and_offset_are_reported() {
    let cases: &[(&[u8], ErrorKind, usize)] = &[
        (b"\"unterminated", ErrorKind::UnterminatedString, 0),
        (br#""bad\xescape""#, ErrorKind::InvalidEscape, 4),
        (br#""short\u12""#, ErrorKind::InvalidUnicodeEscape, 6),
        (b"\"ctrl\x01\"", ErrorKind::ControlCharInString, 5),
        (b"@", ErrorKind::UnexpectedByte, 0),
        (b"tru", ErrorKind::InvalidKeyword, 0),
        (b"12x", ErrorKind::UnexpectedByte, 2),
    ];
    for (bytes, kind, offset) in cases {
        let err = kinds(bytes).expect_err(&format!("{:?}", String::from_utf8_lossy(bytes)));
        assert_eq!(err.kind(), *kind, "{:?}", String::from_utf8_lossy(bytes));
        assert_eq!(
            err.offset(),
            *offset,
            "offset for {:?}",
            String::from_utf8_lossy(bytes)
        );
        // A lexical rejection implies serde_json rejects the bytes (sound direction).
        assert!(serde_json::from_slice::<serde_json::Value>(bytes).is_err());
    }
}

#[test]
fn structural_documents_tokenize_to_the_expected_stream() {
    use TokenKind::*;
    let doc = br#"{"a":[1,true,null],"b":"x"}"#;
    // Insertion order is preserved by the tokenizer (no dependence on serde_json's map ordering).
    let expected = [
        BeginObject,
        String,
        Colon,
        BeginArray,
        Number,
        Comma,
        True,
        Comma,
        Null,
        EndArray,
        Comma,
        String,
        Colon,
        String,
        EndObject,
    ];
    for chunk in [1usize, 4, doc.len()] {
        assert_eq!(
            kinds_chunked(doc, chunk),
            expected.to_vec(),
            "chunk={chunk}"
        );
    }
    // Whitespace between tokens is skipped and does not change the stream.
    assert_eq!(
        kinds(b" {\n\t\"a\" : 1 }\r\n").unwrap(),
        vec![BeginObject, String, Colon, Number, EndObject]
    );
}

/// Like [`kinds`] but with an explicit chunk size, unwrapping (caller passes valid JSON).
fn kinds_chunked(bytes: &[u8], chunk: usize) -> Vec<TokenKind> {
    let r = rope(bytes, chunk);
    Tokenizer::new(&r)
        .map(|res| res.map(|t| t.kind()))
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn accept_reject_agrees_with_serde_json() {
    // A grab-bag of whole documents: whatever serde_json accepts, the tokenizer must accept and drain
    // without error; a lexical rejection implies serde_json also rejects (the sound direction).
    let docs: &[&[u8]] = &[
        b"null",
        b"[]",
        b"{}",
        b"[1, 2, 3]",
        br#"{"nested":{"deep":[[[]]]}}"#,
        br#"[true,false,null,"s",1.5e3]"#,
        b"\"unicode \xc3\xa9 \xf0\x9f\x98\x80\"",
        b"[1,]", // trailing comma — serde rejects; tokenizer lexes structurally (no grammar)
        b"{'a':1}", // single quotes — both should reject at the quote
        b"1 2 3", // multiple top-level values — serde rejects; tokenizer lexes three numbers
    ];
    for d in docs {
        let serde_ok = serde_json::from_slice::<serde_json::Value>(d).is_ok();
        let tok = kinds(d);
        if serde_ok {
            assert!(
                tok.is_ok(),
                "serde accepted but tokenizer errored: {:?}",
                String::from_utf8_lossy(d)
            );
        } else if let Err(e) = &tok {
            // If the tokenizer rejects, serde must too (it does — serde_ok is false here).
            let _ = e.kind();
        }
        // Either way, tokenizing must not panic under any chunk layout.
        for chunk in [1usize, 3, d.len().max(1)] {
            let r = rope(d, chunk);
            let _ = Tokenizer::new(&r).collect::<Result<Vec<_>, _>>();
        }
    }
}

#[test]
fn number_parts_decompose_the_lexeme() {
    // Each number lexeme decomposes into representation-neutral digit spans: sign, integer digits,
    // fraction digits (no `.`), exponent digits (no `e`/sign) + exponent sign. The spans are byte
    // offsets into the input, so slicing the input at each span yields exactly those digits.
    // (bytes, negative, integer, fraction, exponent, exp_negative)
    type Case = (
        &'static [u8],
        bool,
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        bool,
    );
    let cases: &[Case] = &[
        (b"0", false, "0", None, None, false),
        (b"-0", true, "0", None, None, false),
        (b"123", false, "123", None, None, false),
        (b"-42", true, "42", None, None, false),
        (b"3.14", false, "3", Some("14"), None, false),
        (b"-0.500", true, "0", Some("500"), None, false),
        (b"1e10", false, "1", None, Some("10"), false),
        (b"1E+06", false, "1", None, Some("06"), false),
        (b"-2.5e-30", true, "2", Some("5"), Some("30"), true),
        (
            b"60221407600000000000000",
            false,
            "60221407600000000000000",
            None,
            None,
            false,
        ),
    ];
    for (bytes, neg, int, frac, exp, exp_neg) in cases {
        for chunk in [1usize, 2, bytes.len().max(1)] {
            let r = rope(bytes, chunk);
            let toks: Vec<_> = Tokenizer::new(&r).collect::<Result<_, _>>().unwrap();
            assert_eq!(toks.len(), 1, "{:?}", String::from_utf8_lossy(bytes));
            let p = toks[0].number_parts().expect("number token has parts");
            let at = |s: etude_json::Span| std::str::from_utf8(&bytes[s.start()..s.end()]).unwrap();
            assert_eq!(
                p.negative,
                *neg,
                "sign of {:?}",
                String::from_utf8_lossy(bytes)
            );
            assert_eq!(
                at(p.integer),
                *int,
                "integer of {:?}",
                String::from_utf8_lossy(bytes)
            );
            assert_eq!(
                p.fraction.map(at),
                *frac,
                "fraction of {:?}",
                String::from_utf8_lossy(bytes)
            );
            assert_eq!(
                p.exponent.map(at),
                *exp,
                "exponent of {:?}",
                String::from_utf8_lossy(bytes)
            );
            assert_eq!(
                p.exponent_negative,
                *exp_neg,
                "exp sign of {:?}",
                String::from_utf8_lossy(bytes)
            );
            // Consistency with the derived integer flag.
            assert_eq!(
                toks[0].number_is_integer(),
                Some(frac.is_none() && exp.is_none())
            );
        }
    }

    // number_parts is None for a non-number token.
    let r = rope(b"true", 4);
    assert!(
        Tokenizer::new(&r)
            .next()
            .unwrap()
            .unwrap()
            .number_parts()
            .is_none()
    );
}
