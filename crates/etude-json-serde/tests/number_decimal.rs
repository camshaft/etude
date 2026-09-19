// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! The real number decode, end-to-end: a `Visitor` turns each JSON number into an `etude_decimal`
//! `Decimal` by feeding `Decimal::parse` the primary `NumberToken::lexeme` sub-rope's byte iterator —
//! `lexeme.chunks().flat_map(|c| c.iter().copied())`, which is cross-chunk safe with no pre-copy. The
//! adapter core stays value-type-agnostic; the choice of `Decimal` lives here, in the consumer.
//!
//! Differentially checked against `serde_json`: the decoded `Decimal` must agree with serde's number
//! on `to_f64`, and on checked-integer extraction (`to_i64`) where serde reports an integer.

use bytes::Bytes;
use etude_bytevec::ByteVec;
use etude_decimal::Decimal;
use etude_json_serde::from_rope;
use etude_serde::{Error, MapAccess, NumberToken, RopeBytes, RopeStr, SeqAccess, Visitor};
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

/// Decodes a single JSON number into a `Decimal` via the primary lexeme sub-rope.
struct DecimalNum;

impl Visitor for DecimalNum {
    type Value = Decimal;

    fn visit_number(self, n: NumberToken) -> Result<Decimal, Error> {
        // The whole point: parse straight off the lexeme rope, no intermediate String, cross-chunk safe.
        Decimal::parse(n.lexeme().chunks().flat_map(|c| c.iter().copied()))
            .ok_or_else(|| Error::custom("Decimal::parse rejected a tokenizer-validated number"))
    }
    fn visit_null(self) -> Result<Decimal, Error> {
        Err(Error::custom("not a number"))
    }
    fn visit_bool(self, _: bool) -> Result<Decimal, Error> {
        Err(Error::custom("not a number"))
    }
    fn visit_str(self, _: RopeStr) -> Result<Decimal, Error> {
        Err(Error::custom("not a number"))
    }
    fn visit_bytes(self, _: RopeBytes) -> Result<Decimal, Error> {
        Err(Error::custom("not a number"))
    }
    fn visit_seq<A: SeqAccess>(self, _: A) -> Result<Decimal, Error> {
        Err(Error::custom("not a number"))
    }
    fn visit_map<A: MapAccess>(self, _: A) -> Result<Decimal, Error> {
        Err(Error::custom("not a number"))
    }
}

/// Decode `bytes` (a bare JSON number) to a `Decimal` and cross-check against serde_json, under a few
/// chunk layouts so the lexeme sub-rope is exercised both single- and multi-chunk.
fn check_number(bytes: &[u8]) {
    let serde_num = match serde_json::from_slice::<Value>(bytes).unwrap() {
        Value::Number(n) => n,
        other => panic!(
            "test input {:?} is not a JSON number: {other:?}",
            String::from_utf8_lossy(bytes)
        ),
    };

    for chunk in [1usize, bytes.len().max(1)] {
        let dec = from_rope(&rope(bytes, chunk), DecimalNum).unwrap();

        // Value agreement via correctly-rounded f64 (both sides round the same digits).
        assert_eq!(
            dec.to_f64(),
            serde_num.as_f64().unwrap(),
            "f64 mismatch for {:?} at chunk={chunk}",
            String::from_utf8_lossy(bytes)
        );

        // Checked-integer extraction agrees with serde wherever serde reports an integer.
        if let Some(i) = serde_num.as_i64() {
            assert_eq!(
                dec.to_i64(),
                Some(i),
                "to_i64 mismatch for {:?} at chunk={chunk}",
                String::from_utf8_lossy(bytes)
            );
        }
    }
}

#[test]
fn numbers_decode_to_decimal_matching_serde_json() {
    for n in [
        &b"0"[..],
        b"42",
        b"-17",
        b"1000000",
        b"3.14",
        b"-2.5",
        b"1e3",
        b"1E+06",
        b"-6.022e23",
        b"0.0001",
        b"123456789012345",
    ] {
        check_number(n);
    }
}

#[test]
fn checked_integer_extraction() {
    // A plain integer decodes and extracts to i64/i128; a fractional one does not.
    let forty_two = from_rope(&rope(b"42", 1), DecimalNum).unwrap();
    assert_eq!(forty_two.to_i64(), Some(42));
    assert_eq!(forty_two.to_i128(), Some(42));

    let pi = from_rope(&rope(b"3.14", 1), DecimalNum).unwrap();
    assert_eq!(pi.to_i64(), None, "3.14 is not an integer");

    // 1e3 is integral (exp >= 0) → extractable.
    let thousand = from_rope(&rope(b"1e3", 2), DecimalNum).unwrap();
    assert_eq!(thousand.to_i64(), Some(1000));

    // Far out of i64 range → None, but the Decimal itself decoded fine.
    let huge = from_rope(&rope(b"1e40", 1), DecimalNum).unwrap();
    assert_eq!(huge.to_i64(), None);
}

#[test]
fn cross_chunk_lexeme_parses() {
    // A number split across many one-byte chunks: the lexeme sub-rope is multi-chunk, and the byte
    // iterator (flat_map over chunks) must still feed Decimal::parse correctly.
    let dec = from_rope(&rope(b"-123.456e2", 1), DecimalNum).unwrap();
    assert_eq!(dec.to_f64(), -12345.6);
}
