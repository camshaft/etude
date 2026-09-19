// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Differential validation of the adapter against `serde_json`. Because the adapter is a
//! grammar-enforcing parser (not the bare lexer), its accept/reject AND the value it builds must
//! match `serde_json`. A `Visitor` builds a `serde_json::Value` so equality is a direct comparison.
//! Cases run through several rope chunk layouts so the seam threads across leaf boundaries.

use super::*;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use bytes::Bytes;
use etude_bytevec::ByteVec;
use etude_serde::{Error, MapAccess, NumberToken, RopeStr, SeqAccess, Visitor};
use serde_json::Value;

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

/// The ASCII digits of a component sub-rope as an owned string.
fn digits(run: &ByteVec) -> String {
    let bytes = run.copy_to_bytes();
    String::from_utf8(bytes.as_ref().to_vec()).expect("digits are ASCII")
}

/// A [`Visitor`] that builds a `serde_json::Value`, so the adapter's output can be compared to
/// `serde_json::from_slice` directly. It recurses into containers, exercising the whole seam.
struct BuildValue;

impl Visitor for BuildValue {
    type Value = Value;

    fn visit_null(self) -> Result<Value, Error> {
        Ok(Value::Null)
    }
    fn visit_bool(self, b: bool) -> Result<Value, Error> {
        Ok(Value::Bool(b))
    }
    fn visit_str(self, s: RopeStr) -> Result<Value, Error> {
        Ok(Value::String(s.to_string()))
    }
    fn visit_bytes(self, _b: etude_serde::RopeBytes) -> Result<Value, Error> {
        Err(Error::custom("JSON has no bytes value"))
    }
    fn visit_number(self, n: NumberToken) -> Result<Value, Error> {
        // Reconstruct the lexeme from the component digit runs and let serde parse it, so both sides
        // of the differential produce the same `Number` value regardless of `e`/`E`/`+` spelling.
        let mut lex = String::new();
        if n.negative {
            lex.push('-');
        }
        lex.push_str(&digits(&n.integer));
        if let Some(frac) = &n.fraction {
            lex.push('.');
            lex.push_str(&digits(frac));
        }
        if let Some(exp) = &n.exponent {
            lex.push('e');
            if n.exponent_negative {
                lex.push('-');
            }
            lex.push_str(&digits(exp));
        }
        serde_json::from_str(&lex).map_err(Error::custom)
    }
    fn visit_seq<A: SeqAccess>(self, mut seq: A) -> Result<Value, Error> {
        let mut items = Vec::new();
        while let Some(v) = seq.next_element(BuildValue)? {
            items.push(v);
        }
        Ok(Value::Array(items))
    }
    fn visit_map<A: MapAccess>(self, mut map: A) -> Result<Value, Error> {
        let mut obj = serde_json::Map::new();
        while let Some(key) = map.next_key(BuildValue)? {
            let key = match key {
                Value::String(s) => s,
                _ => return Err(Error::custom("object key was not a string")),
            };
            let value = map.next_value(BuildValue)?;
            obj.insert(key, value);
        }
        Ok(Value::Object(obj))
    }
}

/// Assert the adapter agrees with `serde_json` on `bytes` — same accept/reject, and (when accepted)
/// the same value — under several rope chunk layouts.
fn check(bytes: &[u8]) {
    let expected = serde_json::from_slice::<Value>(bytes);
    for chunk in [1usize, 3, bytes.len().max(1)] {
        let got = from_rope(&rope(bytes, chunk), BuildValue);
        match &expected {
            Ok(exp) => assert_eq!(
                got.as_ref().ok(),
                Some(exp),
                "adapter disagreed on valid {:?} at chunk={chunk} (got {got:?})",
                String::from_utf8_lossy(bytes)
            ),
            Err(_) => assert!(
                got.is_err(),
                "adapter accepted invalid {:?} at chunk={chunk}",
                String::from_utf8_lossy(bytes)
            ),
        }
    }
}

#[test]
fn valid_documents_match_serde_json() {
    let docs: &[&[u8]] = &[
        b"null",
        b"true",
        b"false",
        b"0",
        b"-42",
        b"3.14",
        b"-2.5e10",
        b"1E+06",
        b"\"hello\"",
        b"\"esc\\n\\t\\\"\"",
        b"[]",
        b"{}",
        b"[1,2,3]",
        b"[null,true,\"x\",-1.5]",
        br#"{"a":1,"b":[true,false],"c":{"d":null}}"#,
        br#"{"dup":1,"dup":2}"#,
        b"  [ 1 , 2 ]  ",
        b"\"unicode \xc3\xa9 \xf0\x9f\x98\x80\"",
    ];
    for d in docs {
        check(d);
    }
}

#[test]
fn grammar_violations_the_lexer_accepts_are_rejected() {
    // These all LEX (the tokenizer has no between-token grammar) but are invalid JSON; the adapter,
    // as a parser, must reject them — matching serde_json.
    let bad: &[&[u8]] = &[
        b"",                  // empty
        b"   ",               // whitespace only
        b"1 2",               // two top-level values
        b"nulltrue",          // adjacent values
        b"[1,]",              // trailing comma
        b"[1 2]",             // missing comma
        b"[1,,2]",            // double comma
        b"{\"a\":1,}",        // trailing comma in object
        b"{\"a\" 1}",         // missing colon
        b"{\"a\":1 \"b\":2}", // missing comma in object
        b"{1:2}",             // non-string key
        b"[1,2",              // unterminated array
        b"{\"a\":1",          // unterminated object
        b"[",                 // unterminated
        b"}",                 // stray close
    ];
    for d in bad {
        check(d);
        // Belt and suspenders: serde_json really does reject each (so `check` asserts the adapter does).
        assert!(
            serde_json::from_slice::<Value>(d).is_err(),
            "test bug: serde accepts {:?}",
            String::from_utf8_lossy(d)
        );
    }
}

/// A [`Visitor`] that reports, for a string value, whether it arrived zero-copy ([`RopeStr::Borrowed`])
/// plus the decoded content — so a test can pin the has-escapes split.
struct StrArm;

impl Visitor for StrArm {
    type Value = (bool, String);

    fn visit_str(self, s: RopeStr) -> Result<(bool, String), Error> {
        Ok((s.is_borrowed(), s.to_string()))
    }
    fn visit_null(self) -> Result<(bool, String), Error> {
        Err(Error::custom("not a string"))
    }
    fn visit_bool(self, _: bool) -> Result<(bool, String), Error> {
        Err(Error::custom("not a string"))
    }
    fn visit_bytes(self, _: etude_serde::RopeBytes) -> Result<(bool, String), Error> {
        Err(Error::custom("not a string"))
    }
    fn visit_number(self, _: NumberToken) -> Result<(bool, String), Error> {
        Err(Error::custom("not a string"))
    }
    fn visit_seq<A: SeqAccess>(self, _: A) -> Result<(bool, String), Error> {
        Err(Error::custom("not a string"))
    }
    fn visit_map<A: MapAccess>(self, _: A) -> Result<(bool, String), Error> {
        Err(Error::custom("not a string"))
    }
}

#[test]
fn escape_free_strings_are_borrowed_escaped_are_owned() {
    // (input, expected_is_borrowed, expected_content)
    let cases: &[(&[u8], bool, &str)] = &[
        (b"\"hello\"", true, "hello"), // plain ASCII -> zero-copy
        (b"\"\"", true, ""),           // empty -> zero-copy
        (b"\"unicode \xc3\xa9\"", true, "unicode \u{e9}"), // literal UTF-8, no escapes -> zero-copy
        (b"\"a\\nb\"", false, "a\nb"), // \n escape -> materialized
        (b"\"\\u0041\"", false, "A"),  // \u escape -> materialized
        (b"\"tab\\tend\"", false, "tab\tend"), // \t escape -> materialized
    ];
    for (bytes, want_borrowed, want_content) in cases {
        for chunk in [1usize, bytes.len()] {
            let (is_borrowed, content) = from_rope(&rope(bytes, chunk), StrArm).unwrap();
            assert_eq!(
                is_borrowed,
                *want_borrowed,
                "borrow-arm mismatch for {:?}",
                String::from_utf8_lossy(bytes)
            );
            assert_eq!(&content, want_content);
        }
    }
}

#[test]
fn deep_nesting_is_bounded_like_serde_json() {
    // n nested arrays, innermost empty: `[`*n + `]`*n. Recursive descent uses one stack frame per
    // level, so without a depth guard a large n would overflow; the adapter must instead reject it —
    // and agree with serde_json on both a shallow (accepted) and an adversarial (rejected) depth.
    let nest = |n: usize| -> Vec<u8> {
        let mut v = Vec::with_capacity(2 * n);
        v.resize(n, b'[');
        v.resize(2 * n, b']');
        v
    };
    // Well under the limit: both accept and build the same (deeply nested empty array) value.
    check(&nest(100));
    // Far past the limit: both reject (the adapter returns "recursion limit exceeded", not a crash).
    check(&nest(1000));
    // The same for objects would need keys; arrays suffice to exercise the shared depth guard.
}

#[test]
fn nested_structure_threads_through_the_seam() {
    // A deeper document: the no-'de SeqAccess/MapAccess threading must carry nested containers.
    let doc = br#"[{"k":[1,{"n":null}]},[],{"z":"end"}]"#;
    check(doc);
    let got = from_rope(&rope(doc, 2), BuildValue).unwrap();
    assert_eq!(got, serde_json::from_slice::<Value>(doc).unwrap());
}
