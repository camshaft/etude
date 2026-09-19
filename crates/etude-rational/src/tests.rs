// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Correctness tests for [`Rational`].
//!
//! The safety net is a DIFFERENTIAL ORACLE against `num-rational`'s `BigRational` (the reference
//! implementation): a single growing op-driver generates operand seeds + an operation sequence, runs the
//! SAME ops on our [`Rational`] and on `BigRational` in lockstep, and asserts they agree — both on the
//! canonical `(numer, denom)` pair after every value-producing op and on the sign of every comparison.
//! When a case can't be expressed here, GROW this harness rather than adding a one-off test.

use super::*;
use num_bigint::BigInt;
use num_rational::BigRational;

/// Assert our value and the reference agree on the canonical `(numer, denom)` pair. Comparing the decimal
/// strings makes both the value AND the canonical-form invariant (positive den, lowest terms, `0/1` zero)
/// part of the assertion.
fn assert_same(r: &Rational, b: &BigRational) {
    assert_eq!(
        r.numer().to_decimal_string(),
        b.numer().to_string(),
        "numerator mismatch: ours={r:?} ref={b}"
    );
    assert_eq!(
        r.denom().to_decimal_string(),
        b.denom().to_string(),
        "denominator mismatch: ours={r:?} ref={b}"
    );
}

// ─── concrete unit tests ────────────────────────────────────────────────────────────────────────

#[test]
fn canonicalizes_on_construction() {
    // 2/4 -> 1/2; sign moves to the numerator; -6/-8 -> 3/4.
    assert_eq!(
        Rational::from_ratio_i64(2, 4).unwrap().to_decimal_string(),
        "1/2"
    );
    assert_eq!(
        Rational::from_ratio_i64(1, -2).unwrap().to_decimal_string(),
        "-1/2"
    );
    assert_eq!(
        Rational::from_ratio_i64(-6, -8)
            .unwrap()
            .to_decimal_string(),
        "3/4"
    );
    assert_eq!(
        Rational::from_ratio_i64(0, 5).unwrap().to_decimal_string(),
        "0"
    );
    assert_eq!(
        Rational::from_ratio_i64(10, 5).unwrap().to_decimal_string(),
        "2"
    );
}

#[test]
fn zero_denominator_is_rejected() {
    assert!(Rational::from_ratio_i64(1, 0).is_none());
    assert!(Rational::new(Big::from_i64(3), Big::zero()).is_none());
    assert!(Rational::zero().recip().is_none());
    let z = Rational::zero();
    assert!(z.div(&Rational::one()).is_some());
    assert!(Rational::one().div(&z).is_none());
}

#[test]
fn exact_addition_re_reduces() {
    // The canonical Cadenza example: 0.1 + 0.2 = 3/10 exactly (unlike Float64).
    let a = Rational::from_ratio_i64(1, 10).unwrap();
    let b = Rational::from_ratio_i64(2, 10).unwrap();
    assert_eq!(a.add(&b).to_decimal_string(), "3/10");
    // 1/6 + 1/3 = 1/2 (renormalizes to lowest terms across a common-denominator sum).
    let s = Rational::from_ratio_i64(1, 6)
        .unwrap()
        .add(&Rational::from_ratio_i64(1, 3).unwrap());
    assert_eq!(s.to_decimal_string(), "1/2");
}

#[test]
fn equal_denominator_add_sub() {
    // The equal-denominator fast path must agree with the general path AND still reduce/canonicalize.
    // 3/10 + 4/10 = 7/10 (coprime, stays).
    assert_eq!(
        Rational::from_ratio_i64(3, 10)
            .unwrap()
            .add(&Rational::from_ratio_i64(4, 10).unwrap())
            .to_decimal_string(),
        "7/10"
    );
    // 3/10 + 7/10 = 10/10 = 1 (reduces to an integer).
    assert_eq!(
        Rational::from_ratio_i64(3, 10)
            .unwrap()
            .add(&Rational::from_ratio_i64(7, 10).unwrap())
            .to_decimal_string(),
        "1"
    );
    // 1/6 + 1/6 = 2/6 = 1/3 (reduces).
    assert_eq!(
        Rational::from_ratio_i64(1, 6)
            .unwrap()
            .add(&Rational::from_ratio_i64(1, 6).unwrap())
            .to_decimal_string(),
        "1/3"
    );
    // 3/10 - 3/10 = 0 (canonical 0/1).
    assert_eq!(
        Rational::from_ratio_i64(3, 10)
            .unwrap()
            .sub(&Rational::from_ratio_i64(3, 10).unwrap())
            .to_decimal_string(),
        "0"
    );
    // 1/10 - 7/10 = -6/10 = -3/5 (sign + reduce).
    assert_eq!(
        Rational::from_ratio_i64(1, 10)
            .unwrap()
            .sub(&Rational::from_ratio_i64(7, 10).unwrap())
            .to_decimal_string(),
        "-3/5"
    );
    // Integers (den == 1) hit the same fast path: 5 + 7 = 12.
    assert_eq!(
        Rational::from_i64(5)
            .add(&Rational::from_i64(7))
            .to_decimal_string(),
        "12"
    );
}

#[test]
fn integer_and_sign_predicates() {
    assert!(Rational::from_i64(7).is_integer());
    assert!(!Rational::from_ratio_i64(1, 2).unwrap().is_integer());
    assert!(Rational::zero().is_zero());
    assert!(Rational::from_ratio_i64(-1, 2).unwrap().is_negative());
    assert!(!Rational::from_ratio_i64(1, -2).unwrap().is_zero());
    assert_eq!(
        Rational::from_ratio_i64(-1, 2)
            .unwrap()
            .abs()
            .to_decimal_string(),
        "1/2"
    );
    assert_eq!(
        Rational::from_ratio_i64(3, 4)
            .unwrap()
            .neg()
            .to_decimal_string(),
        "-3/4"
    );
}

#[test]
fn comparison_is_exact() {
    use core::cmp::Ordering::{Equal, Greater, Less};
    let cmp = |n1, d1, n2, d2| {
        Rational::from_ratio_i64(n1, d1)
            .unwrap()
            .cmp(&Rational::from_ratio_i64(n2, d2).unwrap())
    };
    let a = Rational::from_ratio_i64(1, 3).unwrap();
    let b = Rational::from_ratio_i64(1, 2).unwrap();
    assert_eq!(a.cmp(&b), Less);
    assert_eq!(b.cmp(&a), Greater);
    assert_eq!(a.cmp(&Rational::from_ratio_i64(2, 6).unwrap()), Equal);
    // Ord/PartialOrd delegate to cmp.
    assert!(a < b);

    // Sign handling (the sign short-circuit + magnitude reversal for two negatives).
    assert_eq!(cmp(-1, 2, 0, 1), Less);
    assert_eq!(cmp(0, 1, 1, 100), Less);
    assert_eq!(cmp(-1, 2, 1, 100), Less); // negative < positive
    assert_eq!(cmp(-1, 3, -1, 2), Greater); // -1/3 > -1/2 (magnitude reversed)
    assert_eq!(cmp(-2, 6, -1, 3), Equal); // same negative value, different representation
    assert_eq!(cmp(-5, 1, -5, 1), Equal);

    // Integer part decides (unequal ⌊·⌋) and integer-vs-fraction tie-break.
    assert_eq!(cmp(7, 2, 5, 2), Greater); // 3.5 vs 2.5
    assert_eq!(cmp(4, 2, 5, 2), Less); // 2 (exact) vs 2.5 → integer < fraction at equal ⌊·⌋
    assert_eq!(cmp(5, 2, 2, 1), Greater); // 2.5 vs 2 (exact) → fraction > integer
    assert_eq!(cmp(6, 3, 2, 1), Equal); // both exactly 2

    // Close values with distinct large-ish denominators (the hard cases for any comparison method).
    assert_eq!(cmp(22, 7, 355, 113), Greater); // 22/7 > 355/113 (both ≈ π, 22/7 is larger)
    assert_eq!(cmp(355, 113, 22, 7), Less);
    assert_eq!(cmp(1000000, 999999, 999999, 999998), Less); // 1 + 1/999999 < 1 + 1/999998
    assert_eq!(cmp(13, 11, 14, 12), Greater); // 1.1818… vs 1.1666…
}

#[test]
fn division_of_fractions() {
    // (3/4) / (2/1) = 3/8.
    let q = Rational::from_ratio_i64(3, 4)
        .unwrap()
        .div(&Rational::from_i64(2))
        .unwrap();
    assert_eq!(q.to_decimal_string(), "3/8");
    // recip round-trip.
    let r = Rational::from_ratio_i64(3, 7).unwrap();
    assert_eq!(r.recip().unwrap().to_decimal_string(), "7/3");
    assert_eq!(r.recip().unwrap().recip().unwrap(), r);
}

#[test]
fn mul_div_cross_reduction() {
    let s = |n, d| Rational::from_ratio_i64(n, d).unwrap();
    // Products that cancel cross-wise: (6/5)*(10/9) = 4/3 [gcd(6,9)=3, gcd(10,5)=5].
    assert_eq!(s(6, 5).mul(&s(10, 9)).to_decimal_string(), "4/3");
    // Sign travels through cancellation: (-2/3)*(3/4) = -1/2.
    assert_eq!(s(-2, 3).mul(&s(3, 4)).to_decimal_string(), "-1/2");
    assert_eq!(s(-2, 3).mul(&s(-3, 4)).to_decimal_string(), "1/2");
    // Multiply by zero canonicalizes to 0/1 (must NOT leave 0/den).
    assert_eq!(Rational::zero().mul(&s(7, 9)).to_decimal_string(), "0");
    assert_eq!(s(7, 9).mul(&Rational::zero()).to_decimal_string(), "0");

    // Division with cancellation: (3/4)/(9/8) = 2/3 [gcd(3,9)=3, gcd(8,4)=4].
    assert_eq!(s(3, 4).div(&s(9, 8)).unwrap().to_decimal_string(), "2/3");
    // Divisor's sign is placed on the numerator: (1/2)/(-3/4) = -2/3.
    assert_eq!(s(1, 2).div(&s(-3, 4)).unwrap().to_decimal_string(), "-2/3");
    assert_eq!(s(-1, 2).div(&s(-3, 4)).unwrap().to_decimal_string(), "2/3");
    // Zero dividend / zero divisor.
    assert_eq!(
        Rational::zero().div(&s(3, 4)).unwrap().to_decimal_string(),
        "0"
    );
    assert!(s(3, 4).div(&Rational::zero()).is_none());
    // Results are truly canonical (coprime): (4/6)*(9/8) = 3/4, not 36/48.
    assert_eq!(s(4, 6).mul(&s(9, 8)).to_decimal_string(), "3/4");
}

#[test]
fn cmp_large_continued_fraction() {
    // Force the continued-fraction cmp path with components well above the small-threshold (64 bytes),
    // and cross-check the ordering against num-rational for many pairs, incl. negatives and equality.
    fn from_seed(seed: u64, nbytes: usize) -> (Big, BigInt) {
        let mut x = seed | 1;
        let mut sm = alloc::vec![0u8]; // sign byte: non-negative
        for _ in 0..nbytes {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            sm.push((x >> 24) as u8);
        }
        *sm.last_mut().unwrap() |= 0x80; // exact width
        let big = Big::from_sign_magnitude_bytes(&sm);
        let bi = BigInt::from_signed_bytes_le(&big.to_le_twos_complement_bytes());
        (big, bi)
    }
    let nbytes = 96; // > CMP_SMALL_BYTES (64) ⇒ the continued-fraction branch
    for i in 0..40u64 {
        let (n1, bn1) = from_seed(i * 4 + 1, nbytes);
        let (d1, bd1) = from_seed(i * 4 + 2, nbytes);
        let (n2, bn2) = from_seed(i * 4 + 3, nbytes);
        let (d2, bd2) = from_seed(i * 4 + 4, nbytes);
        let a = Rational::new(n1, d1).unwrap();
        let b = Rational::new(n2, d2).unwrap();
        let ra = BigRational::new(bn1, bd1);
        let rb = BigRational::new(bn2, bd2);
        assert_eq!(a.cmp(&b), ra.cmp(&rb), "cmp mismatch at i={i}");
        assert_eq!(a.cmp(&a), core::cmp::Ordering::Equal, "self-cmp at i={i}");
        // Mixed / both-negative sign paths through the same large operands.
        assert_eq!(
            a.neg().cmp(&b),
            (-ra.clone()).cmp(&rb),
            "neg-a mismatch at i={i}"
        );
        assert_eq!(
            a.neg().cmp(&b.neg()),
            (-ra).cmp(&-rb),
            "both-neg mismatch at i={i}"
        );
    }
}

// ─── the differential op-driver (the growing oracle) ──────────────────────────────────────────────

/// One operation over a small register file of rationals. Binary ops read two registers and push the
/// result; unary ops read one. Fallible ops (`Div`, `Recip`) are no-ops when the reference agrees they
/// divide by zero. `Cmp` asserts the ordering sign matches and pushes nothing.
fn apply_op(
    code: u8,
    i: usize,
    j: usize,
    ours: &mut alloc::vec::Vec<Rational>,
    refs: &mut alloc::vec::Vec<BigRational>,
) {
    use num_traits::Zero;
    let a = ours[i].clone();
    let b = ours[j].clone();
    let ra = refs[i].clone();
    let rb = refs[j].clone();
    let (r, rr) = match code % 8 {
        0 => (a.add(&b), &ra + &rb),
        1 => (a.sub(&b), &ra - &rb),
        2 => (a.mul(&b), &ra * &rb),
        3 => {
            if rb.is_zero() {
                assert!(
                    a.div(&b).is_none(),
                    "our div by zero must be None: {a:?} / {b:?}"
                );
                return;
            }
            (a.div(&b).expect("nonzero divisor divides"), &ra / &rb)
        }
        4 => (a.neg(), -ra),
        5 => (a.abs(), if ra < BigRational::zero() { -ra } else { ra }),
        6 => {
            if ra.is_zero() {
                assert!(a.recip().is_none(), "our recip of zero must be None");
                return;
            }
            (a.recip().expect("recip of nonzero"), ra.recip())
        }
        _ => {
            // Comparison: sign must match; nothing is pushed.
            assert_eq!(a.cmp(&b), ra.cmp(&rb), "cmp mismatch: {a:?} vs {b:?}");
            return;
        }
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
fn differential_oracle() {
    // Seeds: (num, den) pairs (den==0 skipped). Ops: (opcode, reg_a, reg_b) triples indexed mod live regs.
    bolero::check!()
        .with_type::<(alloc::vec::Vec<(i64, i64)>, alloc::vec::Vec<(u8, u8, u8)>)>()
        .for_each(|(seeds, ops)| {
            let mut ours: alloc::vec::Vec<Rational> = alloc::vec::Vec::new();
            let mut refs: alloc::vec::Vec<BigRational> = alloc::vec::Vec::new();
            // Always have at least one register so indexing never divides by zero.
            ours.push(Rational::zero());
            refs.push(BigRational::new(BigInt::from(0), BigInt::from(1)));
            for &(n, d) in seeds.iter().take(16) {
                if let Some(r) = Rational::from_ratio_i64(n, d) {
                    // The reference agrees the pair is constructible (d != 0); mirror it.
                    let rr = BigRational::new(BigInt::from(n), BigInt::from(d));
                    assert_same(&r, &rr);
                    ours.push(r);
                    refs.push(rr);
                } else {
                    assert_eq!(d, 0, "our constructor only rejects a zero denominator");
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
