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
/// part of the assertion. Also asserts **`Display` parity** vs num-rational — bare and under representative
/// padding/sign flags — so the `pad_integral` flag handling (#220) is locked against silent regression
/// across the whole differential operand space, not just the static `display_matches_numrational_under_flags`
/// cases (the harness formerly checked only the `(numer, denom)` pair, never the formatted whole value).
fn assert_same(r: &Rational, b: &BigRational) {
    use alloc::format;
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
    // Display parity: the whole `num/den` rendering must match num-rational byte-for-byte, both bare and
    // under a width flag (fill/align), the `+` flag, and sign-aware zero-padding (exercises `pad_integral`).
    assert_eq!(
        format!("{r}"),
        format!("{b}"),
        "Display bare: ours={r:?} ref={b}"
    );
    assert_eq!(
        format!("{r:>24}"),
        format!("{b:>24}"),
        "Display width: ours={r:?}"
    );
    assert_eq!(
        format!("{r:+025}"),
        format!("{b:+025}"),
        "Display sign+zeropad: ours={r:?}"
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
fn consuming_sign_transforms_match_borrowing() {
    // into_neg / into_abs / into_recip reuse the owned allocations but must be value- and
    // canonical-form-identical to their borrowing counterparts across every sign, including zero.
    for (n, d) in [
        (3, 10),
        (-3, 10),
        (7, 1),
        (-7, 1),
        (0, 1),
        (1, -2),
        (-6, -8),
    ] {
        let r = Rational::from_ratio_i64(n, d).unwrap();
        assert_eq!(r.clone().into_neg(), r.neg(), "into_neg {n}/{d}");
        assert_eq!(r.clone().into_abs(), r.abs(), "into_abs {n}/{d}");
        assert_eq!(r.clone().into_recip(), r.recip(), "into_recip {n}/{d}");
    }
    // Zero has no reciprocal.
    assert!(Rational::zero().into_recip().is_none());
    // Positive-numerator reciprocal is the zero-allocation swap path: 3/7 -> 7/3.
    assert_eq!(
        Rational::from_ratio_i64(3, 7)
            .unwrap()
            .into_recip()
            .unwrap()
            .to_decimal_string(),
        "7/3"
    );
}

#[test]
fn display_matches_to_decimal_string() {
    use alloc::format;
    for (n, d) in [(3, 10), (-3, 10), (10, 5), (0, 7), (1, -2), (-6, -8)] {
        let r = Rational::from_ratio_i64(n, d).unwrap();
        assert_eq!(format!("{r}"), r.to_decimal_string());
    }
    assert_eq!(
        format!("{}", Rational::from_ratio_i64(-3, 10).unwrap()),
        "-3/10"
    );
    assert_eq!(format!("{}", Rational::from_i64(2)), "2");
    assert_eq!(format!("{}", Rational::zero()), "0");
}

#[test]
fn display_matches_numrational_under_flags() {
    // Contract (shared with etude-decimal): Display honors the `Formatter` padding flags — width, fill,
    // alignment, the `+` sign flag, and sign-aware zero-padding — via `pad_integral`, exactly as
    // num-rational does; precision is ignored (an exact fraction has no decimal expansion). Cross-check
    // byte-for-byte against num-rational's `Ratio` (this crate is a drop-in) across flags and sign/width.
    use alloc::format;
    for (n, d) in [
        (3, 10),
        (-3, 10),
        (10, 5),
        (0, 7),
        (7, 1),
        (-7, 1),
        (123, 4),
    ] {
        let r = Rational::from_ratio_i64(n, d).unwrap();
        let rr = BigRational::new(BigInt::from(n), BigInt::from(d));
        assert_eq!(format!("{r}"), format!("{rr}"), "bare {n}/{d}");
        assert_eq!(format!("{r:>8}"), format!("{rr:>8}"), "right/width {n}/{d}");
        assert_eq!(format!("{r:<10}"), format!("{rr:<10}"), "left {n}/{d}");
        assert_eq!(format!("{r:^12}"), format!("{rr:^12}"), "center {n}/{d}");
        assert_eq!(format!("{r:*>8}"), format!("{rr:*>8}"), "fill {n}/{d}");
        assert_eq!(format!("{r:+}"), format!("{rr:+}"), "sign+ {n}/{d}");
        assert_eq!(format!("{r:08}"), format!("{rr:08}"), "zero-pad {n}/{d}");
        assert_eq!(
            format!("{r:.2}"),
            format!("{rr:.2}"),
            "precision ignored {n}/{d}"
        );
        assert_eq!(
            format!("{r:+012}"),
            format!("{rr:+012}"),
            "sign+zeropad {n}/{d}"
        );
    }
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
fn mul_div_native_u64_magnitude() {
    // Exercise the native u128 mul/div paths: components whose MAGNITUDE fits u64 but exceeds i64 (the `64b`
    // band), so `mul_small`/`div_small` (i64) miss them and `mul_small_u128`/`div_small_u128` take over. The
    // cross-reduced products can reach ~2^128 (exceeding i128), so this also exercises `big_from_u128`.
    // Cross-check the canonical (numer, denom) pair against num-rational across sign combinations.
    fn sm_bytes(mag: u64, neg: bool) -> alloc::vec::Vec<u8> {
        let mut sm = alloc::vec![if neg { 1u8 } else { 0u8 }];
        sm.extend_from_slice(&mag.to_le_bytes());
        *sm.last_mut().unwrap() |= 0x80; // pin exact width so the value stays in the u64>i64 band
        sm
    }
    let mk = |nmag: u64, dmag: u64, neg: bool| {
        let nb = Big::from_sign_magnitude_bytes(&sm_bytes(nmag, neg));
        let db = Big::from_sign_magnitude_bytes(&sm_bytes(dmag, false));
        let bn = BigInt::from_signed_bytes_le(&nb.to_le_twos_complement_bytes());
        let bd = BigInt::from_signed_bytes_le(&db.to_le_twos_complement_bytes());
        (Rational::new(nb, db).unwrap(), BigRational::new(bn, bd))
    };
    let vals: [u64; 4] = [
        u64::MAX,              // 2^64 - 1 (max magnitude ⇒ products near 2^128)
        (i64::MAX as u64) + 1, // 2^63
        0xFFFF_FFFF_0000_0001,
        0x9E37_79B9_7F4A_7C15,
    ];
    for &nm in &vals {
        for &dm in &vals {
            for &nm2 in &vals {
                for &dm2 in &vals {
                    for (s1, s2) in [(false, false), (true, false), (false, true), (true, true)] {
                        let (a, ra) = mk(nm, dm, s1);
                        let (b, rb) = mk(nm2, dm2, s2);
                        assert_same(&a.mul(&b), &(&ra * &rb));
                        assert_same(&a.div(&b).unwrap(), &(&ra / &rb));
                    }
                }
            }
        }
    }
}

#[test]
fn cmp_native_u64_magnitude() {
    // Exercise the native u128 cmp path: components whose MAGNITUDE fits u64 but exceeds i64 (the `64b`
    // bench tier), so `cmp_small` (i64) misses them and `cmp_small_u128` takes over. Cross-check the
    // ordering against num-rational across sign combinations, including the both-negative magnitude reversal
    // and equal-value / different-representation cases. Values in (i64::MAX, u64::MAX] force the path.
    let vals: [u64; 5] = [
        (i64::MAX as u64) + 1, // 2^63, just over i64
        u64::MAX,              // 2^64 - 1, max magnitude
        0xFFFF_FFFF_0000_0001,
        0x8000_0000_0000_0003,
        0xC0FF_EE00_1234_5678,
    ];
    // Sign-magnitude bytes for a u64 magnitude (exact width via the 0x80 high marker so the top byte, which
    // may be a low value, isn't trimmed as leading zero).
    fn sm_bytes(mag: u64, neg: bool) -> alloc::vec::Vec<u8> {
        let mut sm = alloc::vec![if neg { 1u8 } else { 0u8 }];
        sm.extend_from_slice(&mag.to_le_bytes());
        *sm.last_mut().unwrap() |= 0x80;
        sm
    }
    // Build (numerator magnitude `nmag`, sign `neg`) / (positive denominator magnitude `dmag`) as both our
    // Rational and the num-rational reference, from the u64-band magnitudes directly (no i64 round-trip).
    let mk = |nmag: u64, dmag: u64, neg: bool| {
        let nb = Big::from_sign_magnitude_bytes(&sm_bytes(nmag, neg));
        let db = Big::from_sign_magnitude_bytes(&sm_bytes(dmag, false));
        let bn = BigInt::from_signed_bytes_le(&nb.to_le_twos_complement_bytes());
        let bd = BigInt::from_signed_bytes_le(&db.to_le_twos_complement_bytes());
        (Rational::new(nb, db).unwrap(), BigRational::new(bn, bd))
    };
    for &nm in &vals {
        for &dm in &vals {
            for &nm2 in &vals {
                for &dm2 in &vals {
                    for (s1, s2) in [(false, false), (true, false), (false, true), (true, true)] {
                        let (a, ra) = mk(nm, dm, s1);
                        let (b, rb) = mk(nm2, dm2, s2);
                        assert_eq!(a.cmp(&b), ra.cmp(&rb), "cmp {a:?} vs {b:?}");
                    }
                }
            }
        }
    }
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
    let nbytes = 96; // > CMP_SMALL_BYTES (16) ⇒ the continued-fraction branch
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

#[test]
fn add_sub_large_shared_denominator_factor() {
    // Exercise the Big `add`/`sub` path (components > i64) for BOTH the coprime-denominator branch and the
    // shared-factor (gcd(b, d) > 1) branch, which take different reductions (skip-gcd vs lcm + gcd(N, g)).
    // Cross-check against num-rational for many operand pairs, including sign combinations.
    fn big(seed: u64, nbytes: usize) -> (Big, BigInt) {
        let mut x = seed | 1;
        let mut sm = alloc::vec![0u8]; // non-negative
        for _ in 0..nbytes {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            sm.push((x >> 24) as u8);
        }
        *sm.last_mut().unwrap() |= 0x80;
        let b = Big::from_sign_magnitude_bytes(&sm);
        let bi = BigInt::from_signed_bytes_le(&b.to_le_twos_complement_bytes());
        (b, bi)
    }
    let nbytes = 40; // > 8 bytes ⇒ exceeds i64 ⇒ the Big add/sub path (not the native i128 fast path)
    for i in 0..30u64 {
        let (n1, bn1) = big(i * 5 + 1, nbytes);
        let (n2, bn2) = big(i * 5 + 2, nbytes);
        // A shared factor `p` planted into both denominators forces gcd(b, d) > 1 (the lcm branch).
        let (p, bp) = big(i * 5 + 3, nbytes / 2);
        let (q, bq) = big(i * 5 + 4, nbytes / 2);
        let (r, br) = big(i * 5 + 5, nbytes / 2);
        let d1 = p.mul(&q);
        let d2 = p.mul(&r); // gcd(d1, d2) >= p > 1
        let bd1 = &bp * &bq;
        let bd2 = &bp * &br;
        for (na, ba, nb, bb, dx, bdx, dy, bdy) in [
            (&n1, &bn1, &n2, &bn2, &d1, &bd1, &d2, &bd2), // shared-factor denominators
            (&n1, &bn1, &n2, &bn2, &q, &bq, &r, &br),     // coprime-ish denominators
        ] {
            let a = Rational::new(na.clone(), dx.clone()).unwrap();
            let b = Rational::new(nb.clone(), dy.clone()).unwrap();
            let ra = BigRational::new(ba.clone(), bdx.clone());
            let rb = BigRational::new(bb.clone(), bdy.clone());
            for (ours, theirs) in [
                (a.add(&b), &ra + &rb),
                (a.sub(&b), &ra - &rb),
                (a.neg().add(&b), -&ra + &rb),
                (a.sub(&b.neg()), &ra - &(-&rb)),
            ] {
                assert_same(&ours, &theirs);
            }
        }
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
        4 => {
            // The consuming variant must match the borrowing one (same value, same canonical form).
            assert_eq!(a.clone().into_neg(), a.neg(), "into_neg != neg: {a:?}");
            (a.neg(), -ra)
        }
        5 => {
            assert_eq!(a.clone().into_abs(), a.abs(), "into_abs != abs: {a:?}");
            (a.abs(), if ra < BigRational::zero() { -ra } else { ra })
        }
        6 => {
            if ra.is_zero() {
                assert!(a.recip().is_none(), "our recip of zero must be None");
                assert!(
                    a.clone().into_recip().is_none(),
                    "into_recip of zero must be None"
                );
                return;
            }
            assert_eq!(
                a.clone().into_recip().expect("into_recip of nonzero"),
                a.recip().expect("recip of nonzero"),
                "into_recip != recip: {a:?}"
            );
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
