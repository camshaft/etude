// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Correctness tests for [`Decimal`].
//!
//! The safety net is a DIFFERENTIAL ORACLE against `bigdecimal`'s `BigDecimal` (the arbitrary-precision
//! reference): a single growing harness generates candidate byte-strings, parses them with BOTH our
//! [`Decimal::from_ascii`] and `BigDecimal`, and asserts they agree on the exact value (compared as the
//! canonical `(sign, magnitude-digits, exponent)` triple), that our output re-parses to the same value
//! (round-trip), and that our comparison sign matches the reference's on every pair. When a case can't
//! be expressed here, GROW this harness rather than adding a one-off test.

use super::*;
use bigdecimal::BigDecimal;
use std::str::FromStr;

/// The canonical `(is_negative, magnitude-digits, exponent)` triple of one of OUR values.
fn our_parts(d: &Decimal) -> (bool, String, i64) {
    (
        d.is_negative(),
        d.coefficient().abs().to_decimal_string(),
        d.exponent() as i64,
    )
}

/// The same triple for the REFERENCE, from its normalized (trailing-zeros-stripped) form. `bigdecimal`'s
/// `normalized()` also removes trailing zeros, so a matching triple means both the value AND our
/// canonical-form invariant agree with the reference.
fn ref_parts(b: &BigDecimal) -> (bool, String, i64) {
    let bn = b.normalized();
    let (int_val, scale) = bn.as_bigint_and_exponent();
    let neg = matches!(int_val.sign(), num_bigint::Sign::Minus);
    let mag = int_val.magnitude().to_str_radix(10);
    (neg, mag, -scale)
}

/// Assert our value and the reference denote the same exact number (and that both are canonical).
fn assert_same(d: &Decimal, b: &BigDecimal) {
    let ours = our_parts(d);
    let refs = ref_parts(b);
    // Any two zeros are equal regardless of how the reference reports a zero's exponent.
    if ours.1 == "0" && refs.1 == "0" {
        return;
    }
    assert_eq!(ours, refs, "value mismatch: ours={d:?} ({d}) ref={b}");
}

// ─── concrete unit tests ────────────────────────────────────────────────────────────────────────

#[test]
fn canonicalizes_trailing_zeros() {
    // 1.0 == 1 == 1.00 -> coeff 1, exp 0; 100 -> coeff 1, exp 2; 0.10 -> coeff 1, exp -1.
    assert_eq!(
        Decimal::from_str("1.0").unwrap(),
        Decimal::from_str("1").unwrap()
    );
    assert_eq!(
        Decimal::from_str("1.00").unwrap(),
        Decimal::from_str("1").unwrap()
    );
    assert_eq!(
        Decimal::from_str("0.10").unwrap(),
        Decimal::from_str("0.1").unwrap()
    );
    let hundred = Decimal::from_str("100").unwrap();
    assert_eq!(hundred.coefficient().to_decimal_string(), "1");
    assert_eq!(hundred.exponent(), 2);
    assert_eq!(hundred.to_string(), "100");
    // 1e-1 == 0.1.
    assert_eq!(
        Decimal::from_str("1e-1").unwrap(),
        Decimal::from_str("0.1").unwrap()
    );
    // A zero is always the one canonical zero regardless of scale/exponent/sign.
    for s in ["0", "0.0", "0.000", "0e5", "-0", "-0.0", "0E-3"] {
        let z = Decimal::from_str(s).unwrap();
        assert!(z.is_zero(), "{s} should be zero");
        assert_eq!(z, Decimal::zero());
        assert_eq!(z.to_string(), "0");
    }
}

#[test]
fn parses_exact_large_values() {
    // A 30-digit integer round-trips with no loss (well beyond i64/f64).
    let s = "123456789012345678901234567890";
    let d = Decimal::from_str(s).unwrap();
    assert_eq!(d.to_string(), s);
    assert!(d.is_integer());
    // A long fractional literal keeps every significant digit.
    let f = "0.123456789012345678901234567891";
    assert_eq!(Decimal::from_str(f).unwrap().to_string(), f);
    // A trailing zero in the fraction is not significant: it is stripped by canonicalization.
    assert_eq!(Decimal::from_str("0.1230").unwrap().to_string(), "0.123");
    // Exponent shifts: 1.5e3 = 1500, 15e-1 = 1.5.
    assert_eq!(Decimal::from_str("1.5e3").unwrap().to_string(), "1500");
    assert_eq!(Decimal::from_str("15e-1").unwrap().to_string(), "1.5");
    assert_eq!(Decimal::from_str("1.5E+3").unwrap().to_string(), "1500");
}

#[test]
fn rejects_malformed() {
    for bad in [
        "", "-", "+", "+1", "01", "00", "1.", ".5", "1..2", "1.2.3", "1e", "1e+", "1e-", "e5",
        "1.e5", "1.5e", "abc", "0x1", " 1", "1 ", "1,000", "--1", "1-", "Infinity", "NaN", ".",
        "1.2e3.4", "0..1", "+0",
    ] {
        assert!(
            Decimal::from_ascii(bad.as_bytes()).is_none(),
            "{bad:?} must be rejected"
        );
    }
    // Leading zero is only allowed as a bare "0" (optionally with a fraction/exponent).
    assert!(Decimal::from_str("0.5").is_ok());
    assert!(Decimal::from_str("0e3").is_ok());
    assert!(Decimal::from_str("07").is_err());
}

#[test]
fn comparison_is_exact() {
    use core::cmp::Ordering::{Equal, Greater, Less};
    let d = |s: &str| Decimal::from_str(s).unwrap();
    // The canonical Float pitfall: 0.1 < 0.2 exactly.
    assert_eq!(d("0.1").cmp(&d("0.2")), Less);
    // Different scale, same value.
    assert_eq!(d("1.10").cmp(&d("1.1")), Equal);
    assert_eq!(d("100").cmp(&d("1e2")), Equal);
    assert_eq!(d("0.5").cmp(&d("5e-1")), Equal);
    // Order of magnitude decides (adjusted-exponent fast path).
    assert_eq!(d("9.9").cmp(&d("10")), Less);
    assert_eq!(d("10").cmp(&d("9.9")), Greater);
    assert_eq!(d("1000").cmp(&d("999.9")), Greater);
    // Equal adjusted exponent, decided on aligned digits.
    assert_eq!(d("1.5").cmp(&d("1.49")), Greater);
    assert_eq!(d("1.49").cmp(&d("1.5")), Less);
    // Sign handling.
    assert_eq!(d("-0.1").cmp(&d("0")), Less);
    assert_eq!(d("-0.1").cmp(&d("0.1")), Less);
    assert_eq!(d("-2").cmp(&d("-1")), Less); // -2 < -1 (magnitude reversed)
    assert_eq!(d("-1.5").cmp(&d("-1.49")), Less);
    assert_eq!(d("0").cmp(&d("0")), Equal);
    // Ord/PartialOrd delegate to cmp.
    assert!(d("0.1") < d("0.2"));
    assert!(d("-1") <= d("-1"));
}

#[test]
fn predicates_and_sign() {
    assert!(Decimal::from_str("42").unwrap().is_integer());
    assert!(Decimal::from_str("1e3").unwrap().is_integer());
    assert!(!Decimal::from_str("1.5").unwrap().is_integer());
    assert!(Decimal::zero().is_integer());
    assert!(Decimal::from_str("-3.5").unwrap().is_negative());
    assert!(!Decimal::from_str("3.5").unwrap().is_negative());
    assert!(!Decimal::zero().is_negative());
    assert_eq!(Decimal::from_str("3.5").unwrap().neg().to_string(), "-3.5");
    assert_eq!(Decimal::from_str("-3.5").unwrap().abs().to_string(), "3.5");
    assert_eq!(Decimal::from_str("3.5").unwrap().abs().to_string(), "3.5");
    assert_eq!(Decimal::zero().neg(), Decimal::zero());
}

#[test]
fn to_f64_matches_float_parse() {
    for s in [
        "0",
        "1",
        "-1",
        "0.5",
        "0.1",
        "3.14159",
        "-2.5",
        "1e10",
        "1.5e-3",
        "123456.789",
    ] {
        let expected: f64 = s.parse().unwrap();
        let got = Decimal::from_str(s).unwrap().to_f64();
        assert_eq!(got, expected, "to_f64 mismatch for {s}");
    }
    // Overflow to infinity like f64 decimal parsing.
    assert!(Decimal::from_str("1e400").unwrap().to_f64().is_infinite());
}

#[test]
fn scientific_render_round_trips_large_exponents() {
    // Large positive/negative exponents fall back to a compact scientific form that re-parses exactly.
    let big = Decimal::from_str("1e1000").unwrap();
    assert_eq!(big.to_string(), "1e1000");
    assert_eq!(Decimal::from_str(&big.to_string()).unwrap(), big);
    let small = Decimal::from_str("1e-1000").unwrap();
    assert_eq!(small.to_string(), "1e-1000");
    assert_eq!(Decimal::from_str(&small.to_string()).unwrap(), small);
}

// ─── the differential harness (the growing oracle) ────────────────────────────────────────────────

/// The JSON-number character set. Random strings over it hit valid numbers, near-misses (leading zeros,
/// stray dots/signs), and pure garbage — exercising both the acceptance and the rejection paths.
const CHARSET: &[u8] = b"0123456789.-+eE";

fn map_to_charset(raw: &[u8]) -> String {
    // Cap the length so a pathological candidate can't build a giant coefficient string.
    let mut s = String::with_capacity(raw.len().min(48));
    for &b in raw.iter().take(48) {
        s.push(CHARSET[(b as usize) % CHARSET.len()] as char);
    }
    s
}

/// Parse `s` with both implementations and, when we accept it, assert the reference agrees on the value
/// and that our rendering round-trips. Returns the parsed pair when both accept, for downstream cmp.
fn check_parse(s: &str) -> Option<(Decimal, BigDecimal)> {
    let ours = Decimal::from_ascii(s.as_bytes());
    let refs = BigDecimal::from_str(s).ok();
    match (ours, refs) {
        (Some(d), Some(b)) => {
            assert_same(&d, &b);
            // Round-trip: our own output re-parses to the same value in both implementations.
            let rendered = d.to_string();
            let reparsed = Decimal::from_ascii(rendered.as_bytes())
                .unwrap_or_else(|| panic!("our output {rendered:?} must re-parse (from {s:?})"));
            assert_eq!(
                d, reparsed,
                "round-trip changed value: {s:?} -> {rendered:?}"
            );
            let ref_reparsed = BigDecimal::from_str(&rendered)
                .unwrap_or_else(|_| panic!("bigdecimal must parse our output {rendered:?}"));
            assert_same(&d, &ref_reparsed);
            Some((d, b))
        }
        (Some(d), None) => {
            // We accept a strict JSON number; the lenient reference should accept a superset. If this
            // ever fires, our acceptance diverged from a real decimal — surface it.
            panic!("we accepted {s:?} as {d:?} but bigdecimal rejected it");
        }
        // We reject (stricter than / same as the reference), or both reject — nothing to compare.
        (None, _) => None,
    }
}

#[test]
fn differential_parse_and_cmp() {
    bolero::check!()
        .with_type::<alloc::vec::Vec<alloc::vec::Vec<u8>>>()
        .for_each(|candidates| {
            let mut parsed: alloc::vec::Vec<(Decimal, BigDecimal)> = alloc::vec::Vec::new();
            for raw in candidates.iter().take(16) {
                let s = map_to_charset(raw);
                if let Some(pair) = check_parse(&s) {
                    parsed.push(pair);
                }
            }
            // Every ordered pair must agree in comparison sign with the reference.
            for (da, ba) in &parsed {
                for (db, bb) in &parsed {
                    assert_eq!(
                        da.cmp(db),
                        ba.cmp(bb),
                        "cmp mismatch: {da} ? {db} (ours) vs {ba} ? {bb} (ref)"
                    );
                }
            }
        });
}

#[test]
fn differential_structured_numbers() {
    // Guaranteed-valid JSON numbers built from typed fields, for dense cmp/round-trip coverage across
    // sign, integer, fractional, and exponent components.
    bolero::check!()
        .with_type::<alloc::vec::Vec<(bool, u64, u32, bool, i16, bool)>>()
        .for_each(|specs| {
            let mut parsed: alloc::vec::Vec<(Decimal, BigDecimal)> = alloc::vec::Vec::new();
            for &(neg, int, frac, has_frac, exp, has_exp) in specs.iter().take(16) {
                let mut s = String::new();
                if neg {
                    s.push('-');
                }
                s.push_str(&int.to_string()); // u64 Display never has a leading zero
                if has_frac {
                    s.push('.');
                    s.push_str(&frac.to_string());
                }
                if has_exp {
                    s.push('e');
                    s.push_str(&exp.to_string()); // i16 Display carries its own sign
                }
                if let Some(pair) = check_parse(&s) {
                    parsed.push(pair);
                }
            }
            for (da, ba) in &parsed {
                for (db, bb) in &parsed {
                    assert_eq!(da.cmp(db), ba.cmp(bb), "cmp mismatch: {da} ? {db}");
                }
            }
        });
}
