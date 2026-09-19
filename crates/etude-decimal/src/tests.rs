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
        d.exponent(),
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
fn from_parts_and_components() {
    // Component construction matches the equivalent literal parse (the decoder path == the string path).
    // value = (-1)^neg * (int ++ frac) * 10^(exp - frac.len())
    assert_eq!(
        Decimal::from_parts(false, b"1", b"5", 0).unwrap(),
        Decimal::from_str("1.5").unwrap()
    );
    assert_eq!(
        Decimal::from_parts(true, b"3", b"14", 0).unwrap(),
        Decimal::from_str("-3.14").unwrap()
    );
    // Explicit exponent folds with the fractional shift: 1.5e3 = 1500, and 15 with exp -1 = 1.5.
    assert_eq!(
        Decimal::from_parts(false, b"1", b"5", 3).unwrap(),
        Decimal::from_str("1500").unwrap()
    );
    assert_eq!(
        Decimal::from_parts(false, b"15", b"", -1).unwrap(),
        Decimal::from_str("1.5").unwrap()
    );
    // Empty coefficient is zero; sign on zero is dropped (canonical).
    assert_eq!(
        Decimal::from_parts(true, b"", b"", 5).unwrap(),
        Decimal::zero()
    );
    assert_eq!(
        Decimal::from_parts(true, b"0", b"", 0).unwrap(),
        Decimal::zero()
    );
    // from_components: (-1)^neg * digits * 10^exp (no fractional part).
    assert_eq!(
        Decimal::from_components(false, b"123", 4).unwrap(),
        Decimal::from_str("123e4").unwrap()
    );
    assert_eq!(
        Decimal::from_components(true, b"7", 0).unwrap(),
        Decimal::from_str("-7").unwrap()
    );
    // A big coefficient survives exactly.
    assert_eq!(
        Decimal::from_components(false, b"123456789012345678901234567890", 0)
            .unwrap()
            .to_string(),
        "123456789012345678901234567890"
    );
    // Non-digit bytes are rejected (defensive — the caller is expected to pass validated digits).
    assert!(Decimal::from_parts(false, b"1a", b"", 0).is_none());
    assert!(Decimal::from_parts(false, b"1", b"2.3", 0).is_none());
    assert!(Decimal::from_components(false, b"-5", 0).is_none()); // sign belongs in the flag, not digits
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
    // Correctly-rounded direct conversion must agree bit-for-bit with the std float parser (itself
    // correctly rounded) across normals, rounding boundaries, subnormals, overflow, and underflow.
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
        // Rounding boundaries around the 53-bit mantissa.
        "9007199254740992",   // 2^53
        "9007199254740993",   // 2^53 + 1 (not representable → rounds to even)
        "9007199254740995",   // 2^53 + 3
        "0.3",                // classic non-terminating binary fraction
        "1.0000000000000002", // 1 + 2^-52 (the next double after 1.0)
        // Near the top of the normal range and just over it.
        "1.7976931348623157e308", // f64::MAX
        "1e308",
        "1e309", // overflow → inf
        "-1e309",
        // Subnormals and underflow.
        "5e-324",                  // smallest positive subnormal
        "2.5e-324",                // rounds to the smallest subnormal
        "1e-320",                  // a subnormal
        "2.2250738585072014e-308", // smallest positive normal
        "1e-400",                  // underflow → 0
        "-1e-400",
    ] {
        let expected: f64 = s.parse().unwrap();
        let got = Decimal::from_str(s).unwrap().to_f64();
        // `==` treats +0.0 and -0.0 as equal (our canonical zero is unsigned) and compares inf exactly.
        assert!(
            got == expected,
            "to_f64 mismatch for {s}: got {got:?} ({:#x}) expected {expected:?} ({:#x})",
            got.to_bits(),
            expected.to_bits()
        );
    }
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

#[test]
fn exact_arithmetic() {
    let d = |s: &str| Decimal::from_str(s).unwrap();
    // The Float pitfall done exactly: 0.1 + 0.2 = 0.3.
    assert_eq!(d("0.1").add(&d("0.2")).to_string(), "0.3");
    // Different scales align (smaller exponent wins).
    assert_eq!(d("1.5").add(&d("2.25")).to_string(), "3.75");
    assert_eq!(d("100").add(&d("0.001")).to_string(), "100.001");
    // Subtraction: cancellation to canonical zero, and crossing zero.
    assert_eq!(d("1.5").sub(&d("1.5")), Decimal::zero());
    assert_eq!(d("0.3").sub(&d("0.1")).to_string(), "0.2");
    assert_eq!(d("1").sub(&d("0.9")).to_string(), "0.1");
    assert_eq!(d("0.1").sub(&d("0.3")).to_string(), "-0.2");
    // Multiplication: exponents add, coefficients multiply, result re-canonicalizes trailing zeros.
    assert_eq!(d("1.5").mul(&d("2")).to_string(), "3"); // 3.0 -> 3
    assert_eq!(d("0.1").mul(&d("0.1")).to_string(), "0.01");
    assert_eq!(d("12").mul(&d("12")).to_string(), "144");
    assert_eq!(d("2.5").mul(&d("4")).to_string(), "10"); // 10.0 -> 10
    assert_eq!(d("-1.5").mul(&d("2")).to_string(), "-3");
    assert_eq!(d("-1.5").mul(&d("-2")).to_string(), "3");
    // Identities with zero.
    assert_eq!(d("1.23").mul(&Decimal::zero()), Decimal::zero());
    assert_eq!(d("3.14").add(&Decimal::zero()).to_string(), "3.14");
    assert_eq!(Decimal::zero().sub(&d("3.14")).to_string(), "-3.14");
    // Exact well beyond f64/i64 range.
    let big = d("123456789012345678901234567890");
    assert_eq!(
        big.add(&d("1")).to_string(),
        "123456789012345678901234567891"
    );
    assert_eq!(
        big.mul(&d("10")).to_string(),
        "1234567890123456789012345678900"
    );
}

#[test]
fn write_to_renders_into_a_stack_sink() {
    // A fixed-capacity, heap-free `core::fmt::Write` sink: proves `write_to` renders into an arbitrary
    // sink with no allocation (the operator's Display-allocation concern), and matches `Display`.
    struct StackSink {
        buf: [u8; 128],
        len: usize,
    }
    impl core::fmt::Write for StackSink {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let b = s.as_bytes();
            let end = self.len + b.len();
            if end > self.buf.len() {
                return Err(core::fmt::Error);
            }
            self.buf[self.len..end].copy_from_slice(b);
            self.len = end;
            Ok(())
        }
    }

    for s in [
        "0", "1", "-1", "100", "0.1", "1.5", "-3.14", "0.001", "123.456", "1e1000", "1e-1000",
    ] {
        let d = Decimal::from_str(s).unwrap();
        let mut sink = StackSink {
            buf: [0; 128],
            len: 0,
        };
        d.write_to(&mut sink).unwrap();
        let rendered = core::str::from_utf8(&sink.buf[..sink.len]).unwrap();
        assert_eq!(
            rendered,
            d.to_string(),
            "write_to vs Display mismatch for {s}"
        );
    }
}

// ─── the differential harness (the growing oracle) ────────────────────────────────────────────────

/// The decimal-literal character set. Random strings over it hit valid numbers, near-misses (leading
/// zeros, stray dots/signs), and pure garbage — exercising both the acceptance and the rejection paths.
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
            // Direct decimal→f64 must match the (correctly-rounded) std float parse of the same literal.
            // Every literal we accept is a valid f64 literal, so the parse succeeds; `==` treats ±0.0 as
            // equal (our zero is unsigned) and compares infinities exactly.
            let our_f = d.to_f64();
            let ref_f = s
                .parse::<f64>()
                .expect("our accepted literal is a valid f64 literal");
            assert!(
                our_f == ref_f,
                "to_f64 mismatch for {s:?}: {our_f:?} vs {ref_f:?}"
            );
            Some((d, b))
        }
        (Some(d), None) => {
            // We accept a strict decimal literal; the lenient reference should accept a superset. If this
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

/// Build a guaranteed-valid decimal literal string from typed components. `int` (a `u64`) has no leading
/// zero by construction; `frac`/`exp` are appended only when present.
fn make_num(neg: bool, int: u64, frac: Option<u32>, exp: Option<i64>) -> String {
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    s.push_str(&int.to_string());
    if let Some(f) = frac {
        s.push('.');
        s.push_str(&f.to_string());
    }
    if let Some(e) = exp {
        s.push('e');
        s.push_str(&e.to_string()); // Display carries its own sign
    }
    s
}

#[test]
fn differential_structured_numbers() {
    // Guaranteed-valid decimal literals built from typed fields, for dense cmp/round-trip coverage across
    // sign, integer, fractional, and exponent components.
    bolero::check!()
        .with_type::<alloc::vec::Vec<(bool, u64, u32, bool, i16, bool)>>()
        .for_each(|specs| {
            let mut parsed: alloc::vec::Vec<(Decimal, BigDecimal)> = alloc::vec::Vec::new();
            for &(neg, int, frac, has_frac, exp, has_exp) in specs.iter().take(16) {
                let s = make_num(
                    neg,
                    int,
                    has_frac.then_some(frac),
                    has_exp.then_some(exp as i64),
                );
                if let Some(pair) = check_parse(&s) {
                    // The component path (what a decoder feeds) must match the string parse exactly:
                    // from_parts(neg, int_digits, frac_digits, raw eE exp).
                    let int_s = int.to_string();
                    let frac_s = frac.to_string();
                    let frac_bytes: &[u8] = if has_frac { frac_s.as_bytes() } else { b"" };
                    let raw_exp = if has_exp { exp as i64 } else { 0 };
                    let via_parts = Decimal::from_parts(neg, int_s.as_bytes(), frac_bytes, raw_exp)
                        .expect("valid components");
                    assert_eq!(via_parts, pair.0, "from_parts != from_ascii for {s}");
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

/// Apply one arithmetic op over a small register file, in lockstep with the reference, asserting the
/// results agree exactly. Binary ops read registers `i`,`j`; the unary negate reads `i`.
fn apply_op(
    code: u8,
    i: usize,
    j: usize,
    ours: &mut alloc::vec::Vec<Decimal>,
    refs: &mut alloc::vec::Vec<BigDecimal>,
) {
    let a = ours[i].clone();
    let b = ours[j].clone();
    let ra = refs[i].clone();
    let rb = refs[j].clone();
    let (r, rr) = match code % 4 {
        0 => (a.add(&b), &ra + &rb),
        1 => (a.sub(&b), &ra - &rb),
        2 => (a.mul(&b), &ra * &rb),
        _ => (a.neg(), -ra),
    };
    assert_same(&r, &rr);
    // Bound register growth so a long op sequence stays O(cap) in memory.
    if ours.len() < 64 {
        ours.push(r);
        refs.push(rr);
    } else {
        ours[i] = r;
        refs[i] = rr;
    }
}

#[test]
fn differential_arithmetic() {
    // Seeds are valid decimal literals (exponent bounded to i8 so exponent-alignment scaling stays modest);
    // ops are (opcode, reg_a, reg_b) triples. add/sub/mul are all EXACT in bigdecimal too, so the
    // oracle asserts exact agreement after every operation.
    bolero::check!()
        .with_type::<(
            alloc::vec::Vec<(bool, u64, u32, bool, i8, bool)>,
            alloc::vec::Vec<(u8, u8, u8)>,
        )>()
        .for_each(|(seeds, ops)| {
            let mut ours: alloc::vec::Vec<Decimal> = alloc::vec::Vec::new();
            let mut refs: alloc::vec::Vec<BigDecimal> = alloc::vec::Vec::new();
            // Always keep at least one register so indexing never divides by zero.
            ours.push(Decimal::zero());
            refs.push(BigDecimal::from_str("0").unwrap());
            for &(neg, int, frac, has_frac, exp, has_exp) in seeds.iter().take(16) {
                let s = make_num(
                    neg,
                    int,
                    has_frac.then_some(frac),
                    has_exp.then_some(exp as i64),
                );
                if let Some(pair) = check_parse(&s) {
                    ours.push(pair.0);
                    refs.push(pair.1);
                }
            }
            for &(code, a, b) in ops.iter() {
                let len = ours.len();
                let i = (a as usize) % len;
                let j = (b as usize) % len;
                apply_op(code, i, j, &mut ours, &mut refs);
            }
        });
}
