// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Correctness tests for the [`Tokenizer`].
//!
//! The safety net is a differential oracle against `serde_json` (the reference implementation).
//! `serde_json` supplies the ground-truth value for a document and is the arbiter of whether bytes
//! are valid JSON; the tokenizer must produce exactly the token stream that value implies, and must
//! never reject input `serde_json` accepts. A single growing harness generates documents (valid via a
//! `Doc` model, and arbitrary strings as malformed candidates), feeds them through ropes built with
//! several chunk layouts (so a token straddling a rope-leaf boundary is exercised), and asserts
//! agreement. When a case can't be expressed here, grow this harness rather than adding a one-off.

use super::*;
use bytes::Bytes;
use etude_bytevec::ByteVec;

// ─── rope construction ──────────────────────────────────────────────────────────────────────────

/// Build a rope holding `bytes`, split into chunks of at most `chunk` bytes. Small `chunk` values
/// force tokens to straddle rope-leaf boundaries, exercising the copy-avoiding byte-offset scan.
fn rope(bytes: &[u8], chunk: usize) -> ByteVec {
    let mut r = ByteVec::new();
    let chunk = chunk.max(1);
    let mut i = 0;
    while i < bytes.len() {
        let j = (i + chunk).min(bytes.len());
        r.push_back(Bytes::copy_from_slice(&bytes[i..j]));
        i = j;
    }
    r
}

/// Tokenize `bytes` from a single-chunk rope, collecting into a `Result`.
fn tokenize_all(bytes: &[u8]) -> Result<Vec<Token>, Error> {
    let r = rope(bytes, bytes.len().max(1));
    Tokenizer::new(&r).collect()
}

/// The bytes of `span` read back out of `input`.
fn span_bytes(input: &ByteVec, span: Span) -> Vec<u8> {
    (span.start()..span.end())
        .map(|i| input.byte_at(i).unwrap())
        .collect()
}

// ─── differential model ─────────────────────────────────────────────────────────────────────────

/// A normalized expectation for one token: a structural/keyword kind, a decoded string, or a number
/// normalized through `serde_json` (so equal values compare equal regardless of lexeme spelling).
#[derive(Debug, PartialEq, Eq)]
enum Expect {
    Kind(TokenKind),
    Str(String),
    Num(String),
}

/// The token stream a valid `serde_json` value implies, in serialized order.
fn expected(v: &serde_json::Value, out: &mut Vec<Expect>) {
    use serde_json::Value;
    match v {
        Value::Null => out.push(Expect::Kind(TokenKind::Null)),
        Value::Bool(true) => out.push(Expect::Kind(TokenKind::True)),
        Value::Bool(false) => out.push(Expect::Kind(TokenKind::False)),
        Value::Number(n) => out.push(Expect::Num(n.to_string())),
        Value::String(s) => out.push(Expect::Str(s.clone())),
        Value::Array(items) => {
            out.push(Expect::Kind(TokenKind::BeginArray));
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(Expect::Kind(TokenKind::Comma));
                }
                expected(item, out);
            }
            out.push(Expect::Kind(TokenKind::EndArray));
        }
        Value::Object(map) => {
            out.push(Expect::Kind(TokenKind::BeginObject));
            for (i, (k, val)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(Expect::Kind(TokenKind::Comma));
                }
                out.push(Expect::Str(k.clone()));
                out.push(Expect::Kind(TokenKind::Colon));
                expected(val, out);
            }
            out.push(Expect::Kind(TokenKind::EndObject));
        }
    }
}

/// The normalized expectation for the tokens the tokenizer actually produced.
fn actual(tokens: &[Token], input: &ByteVec) -> Vec<Expect> {
    tokens
        .iter()
        .map(|t| match t.kind() {
            TokenKind::String => {
                let decoded = t.decode_string(input).unwrap();
                // Cross-check the StrRope-backed value against the String path on every generated
                // doc and chunk layout: the zero-copy no-escape borrow and the escape builder must
                // both decode to identical content.
                assert_eq!(
                    t.decode_str_rope(input).unwrap(),
                    decoded.as_str(),
                    "decode_str_rope disagreed with decode_string for {decoded:?}"
                );
                Expect::Str(decoded)
            }
            // The Number token spans the exact bytes serde_json serialized (check_valid tokenizes
            // serde_json's own canonical output), which equals the expected `Number::to_string()`.
            // Compare the lexeme directly: a reparse-then-reserialize is not the identity for some
            // subnormal doubles (a serde_json f64 round-trip artifact, off by 1 ULP in the string),
            // which would spuriously fail even though the tokenizer spanned the correct bytes.
            TokenKind::Number => {
                Expect::Num(String::from_utf8(span_bytes(input, t.span())).unwrap())
            }
            k => Expect::Kind(k),
        })
        .collect()
}

/// Assert that valid JSON `bytes` tokenizes — under every chunk layout — into exactly the token
/// stream its `serde_json` value implies.
///
/// The input is canonicalized through `serde_json`'s own serialization first: `expected` walks the
/// parsed value (whose object keys are in `serde_json::Map` order), so the bytes we tokenize must be
/// in that same order. Canonicalizing decouples this check from the input document's key ordering,
/// which the tokenizer preserves faithfully but which need not match the parsed map's order.
fn check_valid(bytes: &[u8]) {
    let v: serde_json::Value = serde_json::from_slice(bytes).expect("caller passes valid json");
    let canon = serde_json::to_vec(&v).expect("re-serializes");
    let mut exp = Vec::new();
    expected(&v, &mut exp);

    for chunk in [1usize, 2, 3, 5, 13, canon.len().max(1)] {
        let r = rope(&canon, chunk);
        let toks: Vec<Token> = Tokenizer::new(&r)
            .collect::<Result<_, _>>()
            .unwrap_or_else(|e| {
                panic!(
                    "tokenizer rejected valid json {:?} at chunk={chunk}: {e}",
                    String::from_utf8_lossy(&canon)
                )
            });
        assert_eq!(
            actual(&toks, &r),
            exp,
            "token stream mismatch for {:?} at chunk={chunk}",
            String::from_utf8_lossy(&canon)
        );
    }
    // Independently: the exact input bytes must tokenize identically under any chunk layout.
    assert_chunk_invariant(bytes);
}

/// A structural fingerprint of a token stream: each token's kind, span, and (for strings) decoded
/// content. Two ropes holding the same bytes must yield identical fingerprints regardless of chunking.
fn token_repr(toks: &[Token], input: &ByteVec) -> Vec<(TokenKind, usize, usize, Option<String>)> {
    toks.iter()
        .map(|t| {
            let decoded = if t.kind() == TokenKind::String {
                t.decode_string(input)
            } else {
                None
            };
            (t.kind(), t.span().start(), t.span().end(), decoded)
        })
        .collect()
}

/// Assert the tokenizer produces the same result (same accept/reject, same token spans and decoded
/// strings) for `bytes` no matter how the input rope is chunked — the copy-avoiding byte-offset scan
/// must be correct when a token straddles a rope-leaf boundary.
fn assert_chunk_invariant(bytes: &[u8]) {
    let base_rope = rope(bytes, bytes.len().max(1));
    let base: Result<Vec<Token>, Error> = Tokenizer::new(&base_rope).collect();
    for chunk in [1usize, 2, 3, 5, 7, 11] {
        let r = rope(bytes, chunk);
        let got: Result<Vec<Token>, Error> = Tokenizer::new(&r).collect();
        match (&base, &got) {
            (Ok(a), Ok(b)) => assert_eq!(
                token_repr(a, &base_rope),
                token_repr(b, &r),
                "token stream changed at chunk={chunk} for {:?}",
                String::from_utf8_lossy(bytes)
            ),
            (Err(a), Err(b)) => assert_eq!(
                a,
                b,
                "error changed at chunk={chunk} for {:?}",
                String::from_utf8_lossy(bytes)
            ),
            _ => panic!(
                "chunk layout changed accept/reject at chunk={chunk} for {:?}",
                String::from_utf8_lossy(bytes)
            ),
        }
    }
}

/// The sound direction on arbitrary bytes: whatever `serde_json` accepts, the tokenizer must accept
/// and match. What `serde_json` rejects, the tokenizer may still tokenize (it does not enforce
/// grammar between tokens) — but it must not panic and must drain cleanly under every chunk layout.
fn check_arbitrary(s: &str) {
    let bytes = s.as_bytes();
    if serde_json::from_slice::<serde_json::Value>(bytes).is_ok() {
        check_valid(bytes);
    } else {
        for chunk in [1usize, 2, 7, bytes.len().max(1)] {
            let r = rope(bytes, chunk);
            let _ = Tokenizer::new(&r).collect::<Result<Vec<_>, _>>();
        }
    }
}

// ─── concrete unit tests ────────────────────────────────────────────────────────────────────────

/// Tokenize and expect a single successful token of `kind` spanning the whole input.
fn assert_single(bytes: &[u8], kind: TokenKind) {
    let toks = tokenize_all(bytes).expect("valid single token");
    assert_eq!(toks.len(), 1, "{:?}", String::from_utf8_lossy(bytes));
    assert_eq!(toks[0].kind(), kind);
    assert_eq!(toks[0].span(), Span::new(0, bytes.len()));
}

#[test]
fn tokenizes_literals_and_structure() {
    assert_single(b"null", TokenKind::Null);
    assert_single(b"true", TokenKind::True);
    assert_single(b"false", TokenKind::False);
    for kw in [&b" null "[..], b"\ttrue\n", b"\r\nfalse\t"] {
        assert!(tokenize_all(kw).is_ok());
    }

    let toks = tokenize_all(b"{}[]:,").unwrap();
    let kinds: Vec<_> = toks.iter().map(|t| t.kind()).collect();
    assert_eq!(
        kinds,
        vec![
            TokenKind::BeginObject,
            TokenKind::EndObject,
            TokenKind::BeginArray,
            TokenKind::EndArray,
            TokenKind::Colon,
            TokenKind::Comma,
        ]
    );
}

#[test]
fn tokenizes_numbers() {
    // (bytes, is_integer): the cheap scan-derived integer flag must match the presence of a
    // fraction or exponent.
    let cases: &[(&[u8], bool)] = &[
        (b"0", true),
        (b"-0", true),
        (b"123", true),
        (b"-123", true),
        (b"9223372036854775807", true),
        (b"3.14", false),
        (b"-2.5e10", false),
        (b"1E-5", false),
        (b"0.0", false),
        (b"1e+9", false),
    ];
    for (n, is_int) in cases {
        let toks = tokenize_all(n).unwrap();
        assert_eq!(toks.len(), 1, "{:?}", String::from_utf8_lossy(n));
        assert_eq!(toks[0].kind(), TokenKind::Number);
        assert_eq!(toks[0].span(), Span::new(0, n.len()));
        assert_eq!(
            toks[0].number_is_integer(),
            Some(*is_int),
            "{:?}",
            String::from_utf8_lossy(n)
        );
    }
}

#[test]
fn string_span_is_zero_copy_when_unescaped() {
    let toks = tokenize_all(b"\"hello\"").unwrap();
    assert_eq!(toks[0].kind(), TokenKind::String);
    assert_eq!(toks[0].string_has_escapes(), Some(false));
    let content = toks[0].string_span().unwrap();
    // The content span is the bytes between the quotes — no decoding needed.
    let r = rope(b"\"hello\"", 7);
    assert_eq!(span_bytes(&r, content), b"hello");
}

#[test]
fn decodes_string_escapes() {
    let cases: &[(&[u8], &str)] = &[
        (b"\"\"", ""),
        (b"\"abc\"", "abc"),
        (b"\"a\\nb\"", "a\nb"),
        (b"\"tab\\tend\"", "tab\tend"),
        (b"\"q\\\"q\"", "q\"q"),
        (b"\"back\\\\slash\"", "back\\slash"),
        (b"\"slash\\/\"", "slash/"),
        (b"\"\\b\\f\\r\"", "\u{08}\u{0C}\r"),
        (b"\"\\u0041\"", "A"),
        (b"\"\\uD83D\\uDE00\"", "\u{1F600}"),
    ];
    for (bytes, want) in cases {
        let r = rope(bytes, 1); // 1-byte chunks: every escape straddles a leaf boundary.
        let toks: Vec<Token> = Tokenizer::new(&r).collect::<Result<_, _>>().unwrap();
        assert_eq!(toks.len(), 1);
        assert_eq!(toks[0].kind(), TokenKind::String);
        assert_eq!(
            toks[0].decode_string(&r).as_deref(),
            Some(*want),
            "{:?}",
            String::from_utf8_lossy(bytes)
        );
        assert_eq!(toks[0].string_has_escapes(), Some(bytes.contains(&b'\\')));
    }
}

#[test]
fn decode_str_rope_escapes_and_borrows() {
    // Same escape cases as `decodes_string_escapes`, but through the StrRope-backed value path;
    // 1-byte chunks force every escape (and the borrow slice) to straddle rope-leaf boundaries.
    let cases: &[(&[u8], &str)] = &[
        (b"\"\"", ""),
        (b"\"abc\"", "abc"),
        (b"\"a\\nb\"", "a\nb"),
        (b"\"q\\\"q\"", "q\"q"),
        (b"\"back\\\\slash\"", "back\\slash"),
        (b"\"\\b\\f\\r\"", "\u{08}\u{0C}\r"),
        (b"\"\\u0041\"", "A"),
        (b"\"\\uD83D\\uDE00\"", "\u{1F600}"),
        (
            b"\"unicode \xc3\xa9 \xf0\x9f\x98\x80\"",
            "unicode \u{e9} \u{1F600}",
        ),
    ];
    for (bytes, want) in cases {
        let r = rope(bytes, 1);
        let toks: Vec<Token> = Tokenizer::new(&r).collect::<Result<_, _>>().unwrap();
        assert_eq!(toks.len(), 1);
        let value = toks[0]
            .decode_str_rope(&r)
            .expect("string token decodes to a StrRope");
        assert_eq!(
            value,
            *want,
            "decode_str_rope content for {:?}",
            String::from_utf8_lossy(bytes)
        );
    }

    // The no-escape path is a zero-copy borrow: its content still equals the raw bytes between the
    // quotes, even when the string spans several rope leaves.
    let r = rope(b"\"hello world\"", 3);
    let tok = &Tokenizer::new(&r)
        .collect::<Result<Vec<Token>, _>>()
        .unwrap()[0];
    assert_eq!(tok.string_has_escapes(), Some(false));
    assert_eq!(tok.decode_str_rope(&r).unwrap(), "hello world");
}

#[test]
fn rejects_malformed_and_reports_offset() {
    let cases: &[(&[u8], ErrorKind, usize)] = &[
        (b"\"abc", ErrorKind::UnterminatedString, 0),
        (b"\"a\\xb\"", ErrorKind::InvalidEscape, 2),
        (b"\"a\\u12\"", ErrorKind::InvalidUnicodeEscape, 2),
        (b"12x", ErrorKind::UnexpectedByte, 2),
        (b"-", ErrorKind::InvalidNumber, 1),
        (b"1.", ErrorKind::InvalidNumber, 2),
        (b"1e", ErrorKind::InvalidNumber, 2),
        (b"tru", ErrorKind::InvalidKeyword, 0),
        (b"nul", ErrorKind::InvalidKeyword, 0),
        (b"@", ErrorKind::UnexpectedByte, 0),
    ];
    for (bytes, kind, offset) in cases {
        let err = tokenize_all(bytes).expect_err(&format!("{:?}", String::from_utf8_lossy(bytes)));
        assert_eq!(err.kind(), *kind, "{:?}", String::from_utf8_lossy(bytes));
        assert_eq!(
            err.offset(),
            *offset,
            "{:?}",
            String::from_utf8_lossy(bytes)
        );
        // A lexical rejection implies serde_json also rejects the bytes.
        assert!(serde_json::from_slice::<serde_json::Value>(bytes).is_err());
    }
}

#[test]
fn rejects_control_char_in_string() {
    let bytes = b"\"a\x01b\"";
    let err = tokenize_all(bytes).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::ControlCharInString);
    assert_eq!(err.offset(), 2);
}

/// Tokenize `bytes` in a given [`Strictness`] under a specific rope chunk layout.
fn tokenize_mode(bytes: &[u8], strictness: Strictness, chunk: usize) -> Result<Vec<Token>, Error> {
    let r = rope(bytes, chunk);
    Tokenizer::with_strictness(&r, strictness).collect()
}

#[test]
fn strict_and_lenient_string_correctness() {
    // The two axes on which Strict (the default, = serde_json parity) and Lenient (accept-superset)
    // differ: string-content UTF-8 validity and `\u` surrogate pairing. `chunk == 1` forces every
    // multi-byte char to straddle a rope-leaf boundary, exercising the incremental UTF-8 carry.
    //
    // (bytes, strict_kind): a string that Strict must REJECT with `strict_kind` and Lenient must
    // ACCEPT (tokenize as one String). serde_json must agree with Strict and reject the bytes.
    let strict_rejects: &[(&[u8], ErrorKind)] = &[
        // Lone / unpaired `\u` surrogates.
        (br#""\uD800""#, ErrorKind::LoneSurrogate), // lone high
        (br#""\uDE00""#, ErrorKind::LoneSurrogate), // lone low
        (br#""\uD800\uD800""#, ErrorKind::LoneSurrogate), // high then high (not a low)
        (br#""\uD800n""#, ErrorKind::LoneSurrogate), // high not followed by an escape
        (br#""\uDC00\uDC00""#, ErrorKind::LoneSurrogate), // low first
        // Invalid UTF-8 content bytes.
        (b"\"a\xffb\"", ErrorKind::InvalidUtf8), // stray 0xFF
        (b"\"a\x80b\"", ErrorKind::InvalidUtf8), // stray continuation byte
        (b"\"\xc3\"", ErrorKind::InvalidUtf8),   // 2-byte lead cut short by the closing quote
        (b"\"\xc3\x28\"", ErrorKind::InvalidUtf8), // lead + non-continuation
        (b"\"\xed\xa0\x80\"", ErrorKind::InvalidUtf8), // UTF-8-encoded surrogate (D800)
        (b"\"\xf0\x9f\x98\"", ErrorKind::InvalidUtf8), // 4-byte emoji truncated to 3 bytes
        (b"\"\xc0\xaf\"", ErrorKind::InvalidUtf8), // overlong `/`
    ];
    for (bytes, kind) in strict_rejects {
        // serde_json (the reference) also rejects these.
        assert!(
            serde_json::from_slice::<serde_json::Value>(bytes).is_err(),
            "expected serde_json to reject {:?}",
            String::from_utf8_lossy(bytes)
        );
        for chunk in [1usize, 2, 3, bytes.len().max(1)] {
            let strict = tokenize_mode(bytes, Strictness::Strict, chunk);
            assert_eq!(
                strict.as_ref().map_err(|e| e.kind()),
                Err(*kind),
                "Strict should reject {:?} with {kind:?} at chunk={chunk}",
                String::from_utf8_lossy(bytes)
            );
            // Lenient accepts the superset: exactly one String token, no error.
            let lenient = tokenize_mode(bytes, Strictness::Lenient, chunk).unwrap_or_else(|e| {
                panic!(
                    "Lenient should accept {:?}: {e}",
                    String::from_utf8_lossy(bytes)
                )
            });
            assert_eq!(lenient.len(), 1, "{:?}", String::from_utf8_lossy(bytes));
            assert_eq!(lenient[0].kind(), TokenKind::String);
        }
    }

    // Strings both modes must accept identically (valid UTF-8, paired surrogate, BMP escape).
    let both_accept: &[&[u8]] = &[
        b"\"\\uD83D\\uDE00\"",   // a valid `\u` surrogate pair (U+1F600)
        b"\"\\u0041\"",          // BMP `\u` escape
        b"\"\xc3\xa9\"",         // é
        b"\"\xf0\x9f\x98\x80\"", // 😀 emoji (4 bytes)
        b"\"plain ascii\"",
    ];
    for bytes in both_accept {
        for chunk in [1usize, 2, 3, bytes.len().max(1)] {
            for mode in [Strictness::Strict, Strictness::Lenient] {
                let toks = tokenize_mode(bytes, mode, chunk).unwrap_or_else(|e| {
                    panic!(
                        "{mode:?} should accept {:?}: {e}",
                        String::from_utf8_lossy(bytes)
                    )
                });
                assert_eq!(toks.len(), 1, "{:?}", String::from_utf8_lossy(bytes));
                assert_eq!(toks[0].kind(), TokenKind::String);
            }
        }
    }
}

#[test]
fn chunk_layout_does_not_change_tokens() {
    // Keys deliberately in non-sorted insertion order — the tokenizer preserves document order, so
    // this exercises chunk-invariance without any dependence on the parsed map's key ordering.
    let doc = br#"{"name":"ada","vals":[1,-2.5,true,null],"nested":{"k":"v\n"}}"#;
    assert_chunk_invariant(doc);
}

#[test]
fn differential_concrete_documents() {
    let docs: &[&[u8]] = &[
        b"null",
        b"[]",
        b"{}",
        b"[1,2,3]",
        br#"{"a":1,"b":[true,false,null],"c":"str"}"#,
        br#"[{"x":[1.5e3]},{"y":"\u00e9"}]"#,
        b"\"unicode: \xc3\xa9 \xf0\x9f\x98\x80\"",
    ];
    for d in docs {
        check_valid(d);
    }
}

// ─── property tests ─────────────────────────────────────────────────────────────────────────────

/// A generatable JSON document model. Serialized through `serde_json`, it produces valid JSON that
/// the tokenizer must reproduce token-for-token. `Int` and `Float` exercise the integer and
/// fraction/exponent number-lexeme paths; strings carry arbitrary text, exercising escape emission
/// and decoding.
#[derive(Debug, Clone, bolero_generator::TypeGenerator)]
enum Doc {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<Doc>),
    Obj(Vec<(String, Doc)>),
}

fn doc_to_value(d: &Doc) -> serde_json::Value {
    use serde_json::Value;
    match d {
        Doc::Null => Value::Null,
        Doc::Bool(b) => Value::Bool(*b),
        Doc::Int(i) => Value::Number((*i).into()),
        // JSON has no NaN/Infinity; `from_f64` returns `None` for those, so a non-finite generated
        // float degrades to `null` (still a valid document to tokenize).
        Doc::Float(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Doc::Str(s) => Value::String(s.clone()),
        Doc::Arr(items) => Value::Array(items.iter().map(doc_to_value).collect()),
        Doc::Obj(kvs) => Value::Object(
            kvs.iter()
                .map(|(k, v)| (k.clone(), doc_to_value(v)))
                .collect(),
        ),
    }
}

#[test]
fn differential_valid_documents() {
    use bolero::check;

    check!().with_type::<Doc>().cloned().for_each(|doc| {
        let bytes = serde_json::to_vec(&doc_to_value(&doc)).expect("serializes");
        check_valid(&bytes);
    });
}

#[test]
fn tokenizer_never_panics_on_arbitrary_bytes() {
    use bolero::check;
    // `differential_arbitrary_input` fuzzes a `String`, so its bytes are always valid UTF-8 — it
    // never feeds the tokenizer a raw non-UTF-8 or high byte. This fuzzes fully arbitrary `Vec<u8>`
    // (invalid UTF-8, control bytes, anything) across rope-chunk layouts: draining the tokenizer must
    // always terminate in Ok/Err and never unwind (a tokenizer that panics on malformed input is a
    // DoS bug), and the accept-superset invariant must hold — whatever serde_json accepts, the
    // tokenizer must also tokenize. (The default `Tokenizer::new` is Strict, so its string accept/
    // reject matches serde_json; `Strictness::Lenient` accepts a documented superset, e.g. non-UTF-8
    // string content and lone surrogates — see `strict_and_lenient_string_correctness`.)
    check!().with_type::<Vec<u8>>().cloned().for_each(|bytes| {
        for chunk in [1usize, 2, 7, bytes.len().max(1)] {
            let r = rope(&bytes, chunk);
            let result = Tokenizer::new(&r).collect::<Result<Vec<_>, _>>();
            // A panic here fails the fuzz; the accept-superset direction reuses check_valid.
            if serde_json::from_slice::<serde_json::Value>(&bytes).is_ok() {
                assert!(
                    result.is_ok(),
                    "tokenizer rejected serde-valid input at chunk={chunk}: {bytes:?}"
                );
            }
        }
    });
}

/// The Strict-mode soundness contract, fuzzed in the accept-then-inspect direction.
///
/// `tokenizer_never_panics_on_arbitrary_bytes` fuzzes the *accept-superset* direction (whatever
/// serde_json accepts, the tokenizer must accept). This fuzzes the converse safety guarantee the
/// docs make in the other direction: a Strict tokenizer promises that every `String` token it
/// *produces* has a content span (`Token::string_span`) that is valid UTF-8, which is what makes an
/// escape-free string readable with an unchecked O(1) conversion (see `Strictness::Strict` and
/// `Token::string_span`). Nothing else asserts this over fuzzed input — the deterministic tables
/// pick specific malformed sequences, but a boundary-carry false-accept in the incremental
/// `Utf8Check` (a truncated or overlong sequence straddling a rope-leaf boundary that slips through
/// as an accepted token) would leave an invalid-UTF-8 span behind and be caught by nothing.
///
/// For arbitrary bytes across several rope-chunk layouts (small chunks force every multi-byte char
/// to straddle a leaf boundary, exercising the incremental carry), whenever Strict accepts, every
/// resulting `String` token's content span must pass `core::str::from_utf8` — the literal portions
/// are Strict-validated UTF-8 and any escape sequences are ASCII, so the raw span is valid UTF-8
/// with or without escapes. A violation is a soundness bug: it makes `from_utf8_unchecked` on that
/// span undefined behavior.
#[test]
fn strict_string_token_spans_are_valid_utf8() {
    use bolero::check;

    check!().with_type::<Vec<u8>>().cloned().for_each(|bytes| {
        for chunk in [1usize, 2, 7, bytes.len().max(1)] {
            let r = rope(&bytes, chunk);
            let Ok(tokens) = Tokenizer::new(&r).collect::<Result<Vec<_>, _>>() else {
                // Strict rejected this input; the guarantee only covers what it accepts.
                continue;
            };
            for t in &tokens {
                if t.kind() != TokenKind::String {
                    continue;
                }
                let span = t.string_span().expect("a String token has a content span");
                let content = span_bytes(&r, span);
                assert!(
                    core::str::from_utf8(&content).is_ok(),
                    "Strict accepted a String whose content span is not valid UTF-8 \
                     (has_escapes={:?}) at chunk={chunk}: span={content:?} input={bytes:?}",
                    t.string_has_escapes(),
                );
            }
        }
    });
}

#[test]
fn differential_arbitrary_input() {
    use bolero::check;

    check!()
        .with_type::<String>()
        .cloned()
        .for_each(|s| check_arbitrary(&s));
}

// ─── number_parts faithful-decomposition oracle ───────────────────────────────────────────────────
//
// `number_parts()`/[`NumberParts`] is the contract `etude_decimal::from_parts` consumes, and the
// differential oracle above canonicalizes every number through `serde_json` (an `f64`), so it never
// exercises a lexeme with more precision than `f64` carries. This harness assembles valid JSON number
// lexemes from structured components — including digit runs far past `f64` precision — tokenizes them
// across several rope chunk layouts (so a digit run straddling a rope-leaf boundary is exercised),
// calls `number_parts()`, resolves each digit span back to bytes, and asserts the decomposition
// faithfully matches the components the lexeme was built from: no digit is dropped or misassigned at
// any precision. `serde_json` cannot be the oracle here; the generator's own components are the
// ground truth, and canonical lexemes additionally self-reconstruct from the recorded parts.

/// A structured JSON number: the exact digit groups a lexeme is assembled from, so the parts
/// `number_parts()` returns can be checked against ground truth rather than against a re-parse.
struct NumberSpec {
    negative: bool,
    /// JSON integer digits, no sign: `"0"` or `[1-9][0-9]*`.
    integer: &'static str,
    /// Fraction digits after `.` (no `.`), or `None`.
    fraction: Option<&'static str>,
    /// `(explicit '-', digits)` after the exponent marker, or `None`. A `+`/unsigned exponent is
    /// `false`; [`NumberSpec::exp_plus`] controls whether a non-negative exponent is spelled with `+`.
    exponent: Option<(bool, &'static str)>,
    /// Spell the exponent marker `E` instead of `e` (parts records neither case).
    exp_upper: bool,
    /// Spell an explicit `+` on a non-negative exponent (parts records no `+`).
    exp_plus: bool,
}

impl NumberSpec {
    fn int(negative: bool, integer: &'static str) -> Self {
        Self {
            negative,
            integer,
            fraction: None,
            exponent: None,
            exp_upper: false,
            exp_plus: false,
        }
    }

    fn frac(negative: bool, integer: &'static str, fraction: &'static str) -> Self {
        Self {
            negative,
            integer,
            fraction: Some(fraction),
            exponent: None,
            exp_upper: false,
            exp_plus: false,
        }
    }

    fn exp(
        negative: bool,
        integer: &'static str,
        fraction: Option<&'static str>,
        exponent: (bool, &'static str),
        exp_upper: bool,
        exp_plus: bool,
    ) -> Self {
        Self {
            negative,
            integer,
            fraction,
            exponent: Some(exponent),
            exp_upper,
            exp_plus,
        }
    }

    /// Assemble the JSON lexeme these components spell.
    fn lexeme(&self) -> String {
        let mut s = String::new();
        if self.negative {
            s.push('-');
        }
        s.push_str(self.integer);
        if let Some(f) = self.fraction {
            s.push('.');
            s.push_str(f);
        }
        if let Some((neg, digits)) = self.exponent {
            s.push(if self.exp_upper { 'E' } else { 'e' });
            if neg {
                s.push('-');
            } else if self.exp_plus {
                s.push('+');
            }
            s.push_str(digits);
        }
        s
    }
}

/// Tokenize `spec`'s lexeme across several rope chunk layouts and assert `number_parts()` decomposes
/// it faithfully — sign, integer, fraction, exponent digits and exponent sign all match the
/// components, `number_is_integer()` agrees, and a canonical lexeme self-reconstructs from the parts.
fn assert_number_parts(spec: &NumberSpec) {
    let lexeme = spec.lexeme();
    let bytes = lexeme.as_bytes();
    for chunk in [1usize, 2, 3, 5, 8, bytes.len().max(1)] {
        let r = rope(bytes, chunk);
        let toks: Vec<Token> = Tokenizer::new(&r)
            .collect::<Result<_, _>>()
            .unwrap_or_else(|e| panic!("{lexeme:?} at chunk={chunk} failed to tokenize: {e:?}"));
        assert_eq!(
            toks.len(),
            1,
            "{lexeme:?} at chunk={chunk}: not a single token"
        );
        let t = &toks[0];
        assert_eq!(t.kind(), TokenKind::Number, "{lexeme:?} at chunk={chunk}");

        let parts = t.number_parts().expect("a Number token has parts");

        assert_eq!(
            parts.negative, spec.negative,
            "sign for {lexeme:?} at chunk={chunk}"
        );

        assert_eq!(
            span_bytes(&r, parts.integer),
            spec.integer.as_bytes(),
            "integer digits for {lexeme:?} at chunk={chunk}",
        );

        match (parts.fraction, spec.fraction) {
            (Some(span), Some(expect)) => assert_eq!(
                span_bytes(&r, span),
                expect.as_bytes(),
                "fraction digits for {lexeme:?} at chunk={chunk}",
            ),
            (None, None) => {}
            (got, want) => panic!(
                "fraction presence mismatch for {lexeme:?} at chunk={chunk}: got {got:?} want {want:?}"
            ),
        }

        match (parts.exponent, spec.exponent) {
            (Some(span), Some((neg, expect))) => {
                assert_eq!(
                    span_bytes(&r, span),
                    expect.as_bytes(),
                    "exponent digits for {lexeme:?} at chunk={chunk}",
                );
                assert_eq!(
                    parts.exponent_negative, neg,
                    "exponent sign for {lexeme:?} at chunk={chunk}",
                );
            }
            (None, None) => assert!(
                !parts.exponent_negative,
                "exponent_negative set without an exponent for {lexeme:?} at chunk={chunk}",
            ),
            (got, want) => panic!(
                "exponent presence mismatch for {lexeme:?} at chunk={chunk}: got {got:?} want {want:?}"
            ),
        }

        // number_is_integer() is derived from the same recorded parts and must agree.
        assert_eq!(
            t.number_is_integer(),
            Some(spec.fraction.is_none() && spec.exponent.is_none()),
            "number_is_integer for {lexeme:?} at chunk={chunk}",
        );

        // Canonical self-reconstruction (the dep-free consumer oracle): a lexeme spelled canonically
        // (lowercase `e`, no explicit `+`) re-assembles exactly from the recorded parts, so no digit
        // is dropped or misassigned regardless of precision.
        if !spec.exp_upper && !spec.exp_plus {
            let mut rebuilt = String::new();
            if parts.negative {
                rebuilt.push('-');
            }
            rebuilt.push_str(core::str::from_utf8(&span_bytes(&r, parts.integer)).unwrap());
            if let Some(span) = parts.fraction {
                rebuilt.push('.');
                rebuilt.push_str(core::str::from_utf8(&span_bytes(&r, span)).unwrap());
            }
            if let Some(span) = parts.exponent {
                rebuilt.push('e');
                if parts.exponent_negative {
                    rebuilt.push('-');
                }
                rebuilt.push_str(core::str::from_utf8(&span_bytes(&r, span)).unwrap());
            }
            assert_eq!(rebuilt, lexeme, "self-reconstruction at chunk={chunk}");
        }
    }
}

#[test]
fn number_parts_faithful_decomposition() {
    // Digit runs past f64 precision: the etude_decimal domain the serde_json oracle cannot reach.
    const INT_40: &str = "1234567890123456789012345678901234567890";
    const INT_80: &str =
        "12345678901234567890123456789012345678901234567890123456789012345678901234567890";
    const FRAC_40: &str = "0987654321098765432109876543210987654321";

    let specs = [
        // Integers, both signs, including -0 and long runs.
        NumberSpec::int(false, "0"),
        NumberSpec::int(true, "0"),
        NumberSpec::int(false, "7"),
        NumberSpec::int(true, "7"),
        NumberSpec::int(false, "10"),
        NumberSpec::int(false, "1000000000000000000000"),
        NumberSpec::int(false, INT_40),
        NumberSpec::int(true, INT_80),
        // Fractions, including a leading-zero fraction and a 40-digit fraction.
        NumberSpec::frac(false, "0", "5"),
        NumberSpec::frac(true, "3", "14159"),
        NumberSpec::frac(false, "0", "0000000001"),
        NumberSpec::frac(true, INT_40, FRAC_40),
        // Exponents: marker case and sign spelling vary; parts records only digits + the neg flag.
        NumberSpec::exp(false, "1", None, (false, "5"), false, false), // 1e5
        NumberSpec::exp(false, "1", None, (false, "5"), true, false),  // 1E5
        NumberSpec::exp(false, "1", None, (false, "5"), false, true),  // 1e+5
        NumberSpec::exp(false, "1", None, (true, "5"), false, false),  // 1e-5
        NumberSpec::exp(false, "6", None, (false, "308"), false, false), // past f64 max
        NumberSpec::exp(false, "1", None, (true, "400"), false, false), // past f64 min
        // High-precision combined shapes: integer + fraction + exponent, the from_parts consumer case.
        NumberSpec::exp(true, INT_40, Some(FRAC_40), (true, "30"), false, false),
        NumberSpec::exp(false, INT_80, Some(FRAC_40), (false, "12"), true, true), // E and +
        NumberSpec::exp(
            true,
            "9",
            Some("000000000000000000001"),
            (true, "123"),
            false,
            false,
        ),
    ];

    for spec in &specs {
        assert_number_parts(spec);
    }
}
