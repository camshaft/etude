// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Differential test suite: every operation is checked against `num-bigint` (the oracle). Two
//! engines drive it — a fast deterministic xorshift RNG for a dense fixed corpus, and a `bolero`
//! property harness for coverage-guided exploration + shrinking.

use super::*;
use num_bigint::BigInt as Ref;
use num_traits::{Signed, Zero};

// Convert a native `Big` to the reference `num_bigint::BigInt` for differential comparison.
fn to_ref(b: &Big) -> Ref {
    // Build from sign + LE u32 limbs.
    let mut bytes = Vec::new();
    for &limb in &b.mag {
        bytes.extend_from_slice(&limb.to_le_bytes());
    }
    let mag = num_bigint::BigUint::from_bytes_le(&bytes);
    let sign = if b.is_zero() {
        num_bigint::Sign::NoSign
    } else if b.neg {
        num_bigint::Sign::Minus
    } else {
        num_bigint::Sign::Plus
    };
    Ref::from_biguint(sign, mag)
}

fn from_i128(v: i128) -> Big {
    // Build a Big from an i128 for test seeding (covers > i64 range).
    if v == 0 {
        return Big::zero();
    }
    let neg = v < 0;
    let mut m = v.unsigned_abs();
    let mut mag = Vec::new();
    while m != 0 {
        mag.push(m as u64);
        m >>= 64;
    }
    let mut b = Big { neg, mag };
    b.normalize();
    b
}

/// Reference gcd via Euclid over non-negative num-bigint values (avoids a `num-integer` dep).
fn ref_gcd(mut a: Ref, mut b: Ref) -> Ref {
    while !b.is_zero() {
        let r = &a % &b;
        a = b;
        b = r;
    }
    a
}

/// Check every operation of a `(Big, Ref)` pair against the oracle. Shared by the RNG corpus and the
/// bolero harness so both engines exercise the identical assertions.
fn check_pair(a: &Big, b: &Big) {
    let (ra, rb) = (to_ref(a), to_ref(b));

    assert_eq!(to_ref(&a.add(b)), &ra + &rb, "add {a:?} {b:?}");
    assert_eq!(to_ref(&a.sub(b)), &ra - &rb, "sub {a:?} {b:?}");
    assert_eq!(to_ref(&a.mul(b)), &ra * &rb, "mul {a:?} {b:?}");
    // mul_into writes the product into caller-owned scratch: BYTE-IDENTICAL to by-value mul (canonical),
    // and pre-loading the scratch with unrelated data must not leak into the result.
    {
        let mut out = Big::zero();
        a.mul_into(b, &mut out);
        assert_eq!(out, a.mul(b), "mul_into (fresh out) == mul {a:?} {b:?}");
        let mut dirty = from_i128(0x1234_5678_9abc_def0);
        a.mul_into(b, &mut dirty);
        assert_eq!(dirty, a.mul(b), "mul_into (reused out) == mul {a:?} {b:?}");
    }
    assert_eq!(to_ref(&a.neg()), -&ra, "neg {a:?}");
    assert_eq!(to_ref(&a.abs()), ra.abs(), "abs {a:?}");

    // In-place accumulator ops: the result must be BYTE-IDENTICAL to the by-value form (derived `Eq`
    // compares sign+limbs), so `add_assign`/`sub_assign` land in canonical form — sacred for map keys.
    {
        let mut acc = a.clone();
        acc.add_assign(b);
        assert_eq!(acc, a.add(b), "add_assign == add {a:?} {b:?}");
        assert_eq!(to_ref(&acc), &ra + &rb, "add_assign value {a:?} {b:?}");
        let mut acc = a.clone();
        acc.sub_assign(b);
        assert_eq!(acc, a.sub(b), "sub_assign == sub {a:?} {b:?}");
        assert_eq!(to_ref(&acc), &ra - &rb, "sub_assign value {a:?} {b:?}");
    }
    // In-place sign flips must match their by-value twins exactly (canonical repr, so `==` is strict):
    // this pins the signed-zero invariant (negating zero is a no-op) and abs clearing the sign.
    let mut neg_ip = a.clone();
    neg_ip.negate();
    assert_eq!(neg_ip, a.neg(), "negate {a:?}");
    let mut abs_ip = a.clone();
    abs_ip.abs_assign();
    assert_eq!(abs_ip, a.abs(), "abs_assign {a:?}");
    assert_eq!(
        a.to_decimal_string(),
        ra.to_string(),
        "to_decimal_string {a:?}"
    );

    // O(1) size accessors: bit_len matches num-bigint's magnitude bit count; byte_len is the magnitude
    // byte count (the sign-magnitude encoding minus its sign byte).
    assert_eq!(a.bit_len(), ra.bits() as usize, "bit_len {a:?}");
    assert_eq!(
        a.byte_len(),
        a.to_sign_magnitude_bytes().len() - 1,
        "byte_len {a:?}"
    );

    let ord = a.cmp(b);
    assert_eq!(ord, ra.cmp(&rb), "cmp {a:?} {b:?}");
    // The byte-form compare (no `Big` decode) must give the SAME ordering as `Big::cmp`.
    assert_eq!(
        Big::cmp_sign_magnitude_bytes(&a.to_sign_magnitude_bytes(), &b.to_sign_magnitude_bytes()),
        ord,
        "byte-form cmp agrees with Big::cmp {a:?} {b:?}"
    );
    // The byte-form i64 narrowing (no `Big` decode) must match `Big::to_i64_checked`.
    assert_eq!(
        Big::i64_checked_from_sign_magnitude_bytes(&a.to_sign_magnitude_bytes()),
        a.to_i64_checked(),
        "byte-form i64-narrow agrees with to_i64_checked {a:?}"
    );
    // The byte-form i128 read: when Some, it must round-trip byte-identically and equal the value;
    // when None the value genuinely exceeds i128.
    let a_bytes = a.to_sign_magnitude_bytes();
    match Big::i128_from_sign_magnitude_bytes(&a_bytes) {
        Some(v) => {
            let mut buf = [0u8; 17];
            let n = Big::i128_to_sign_magnitude_bytes_into(v, &mut buf).unwrap();
            assert_eq!(
                &buf[..n],
                &a_bytes[..],
                "i128 byte round-trip is byte-identical {a:?}"
            );
            assert_eq!(
                Big::from_sign_magnitude_bytes(&buf[..n]),
                *a,
                "i128 round-trip value {a:?}"
            );
        }
        None => assert!(
            a_bytes.get(1..).map_or(0, |m| m.len()) >= 16,
            "i128 None only for >i64-ish wide {a:?}"
        ),
    }

    // Sign-magnitude + two's-complement byte round-trips.
    assert_eq!(
        Big::from_sign_magnitude_bytes(&a_bytes),
        *a,
        "sign-mag round-trip {a:?}"
    );
    let tc = a.to_le_twos_complement_bytes();
    assert_eq!(
        Big::from_le_twos_complement_bytes(&tc),
        *a,
        "2c round-trip {a:?}"
    );
    assert_eq!(
        Big::from_le_twos_complement_bytes(&ra.to_signed_bytes_le()),
        *a,
        "num-bigint 2c bytes parse to {a:?}"
    );

    if !b.is_zero() {
        let (q, r) = a.divmod(b).unwrap();
        // num-bigint's / and % are truncating (toward zero), matching our divmod.
        assert_eq!(to_ref(&q), &ra / &rb, "div {a:?} {b:?}");
        assert_eq!(to_ref(&r), &ra % &rb, "rem {a:?} {b:?}");
        // The quotient-only fast path returns EXACTLY divmod's quotient (just without the remainder).
        assert_eq!(
            a.div_exact(b),
            Some(q.clone()),
            "div_exact == divmod.0 {a:?} {b:?}"
        );
        // In-place exact divide: byte-identical to div_exact, returns true, works from a dirty buffer.
        let mut q_ip = a.clone();
        assert!(q_ip.div_exact_assign(b), "div_exact_assign nonzero → true");
        assert_eq!(q_ip, q, "div_exact_assign == divmod.0 {a:?} {b:?}");
        // The defining identity: a == q*b + r.
        assert_eq!(*a, q.mul(b).add(&r), "divmod identity {a:?} {b:?}");
        // |remainder| < |divisor|.
        assert_eq!(
            Big {
                neg: false,
                mag: r.mag.clone()
            }
            .cmp(&Big {
                neg: false,
                mag: b.mag.clone()
            }),
            Ordering::Less,
            "|rem| < |divisor| {a:?} {b:?}"
        );
    } else {
        assert!(a.divmod(b).is_none(), "div by zero → None");
        assert!(a.div_exact(b).is_none(), "div_exact by zero → None");
        // In-place exact divide by zero: returns false and leaves self unchanged.
        let mut unchanged = a.clone();
        assert!(
            !unchanged.div_exact_assign(b),
            "div_exact_assign zero → false"
        );
        assert_eq!(
            unchanged, *a,
            "div_exact_assign zero leaves self unchanged {a:?}"
        );
    }

    // gcd: sign-agnostic, non-negative; gcd(0,0)=0; divides both operands exactly.
    let g = a.gcd(b);
    assert!(!g.neg, "gcd is non-negative {a:?} {b:?}");
    assert_eq!(to_ref(&g), ref_gcd(ra.abs(), rb.abs()), "gcd {a:?} {b:?}");
    // gcd_into: byte-identical to the by-value gcd, from both a fresh and a dirty (pre-loaded) out.
    {
        let mut out = Big::zero();
        a.gcd_into(b, &mut out);
        assert_eq!(out, g, "gcd_into (fresh out) == gcd {a:?} {b:?}");
        let mut dirty = from_i128(-0x0fed_cba9_8765_4321);
        a.gcd_into(b, &mut dirty);
        assert_eq!(dirty, g, "gcd_into (reused out) == gcd {a:?} {b:?}");
    }
    if !g.is_zero() {
        assert!(a.divmod(&g).unwrap().1.is_zero(), "gcd divides a exactly");
        assert!(b.divmod(&g).unwrap().1.is_zero(), "gcd divides b exactly");
    } else {
        assert!(
            a.is_zero() && b.is_zero(),
            "gcd is 0 only when both operands are 0"
        );
    }
}

// A small deterministic PRNG (no rand dep; reproducibility matters for a differential corpus).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn big(&mut self) -> Big {
        self.big_upto(5) // 0..=4 limbs
    }
    /// A random `Big` with 0..`max_limbs` limbs (wider magnitudes exercise the divmod limb-boundary
    /// carry/borrow that ≤4-limb operands never reach).
    fn big_upto(&mut self, max_limbs: u64) -> Big {
        let limbs = (self.next() % max_limbs) as usize;
        let mut mag = Vec::new();
        for _ in 0..limbs {
            mag.push(self.next());
        }
        let neg = self.next() & 1 == 1;
        let mut b = Big { neg, mag };
        b.normalize();
        b
    }
}

#[test]
fn differential_arithmetic_vs_num_bigint() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    for _ in 0..5000 {
        let a = rng.big();
        let b = rng.big();
        check_pair(&a, &b);
    }
}

/// Coverage-guided + shrinking differential harness: build two `Big`s from arbitrary two's-complement
/// byte strings (giving arbitrary sign and magnitude) and run the full oracle check on the pair. The
/// bolero engine explores the operand space the fixed RNG corpus does not, and shrinks any failure to
/// a minimal reproducer.
#[test]
// The bolero property harness spins forever under miri (its slow interpreter times out on the many
// generated candidates); the same `check_pair` oracle path is covered under miri by the deterministic
// fixed-RNG harness (`differential_arithmetic_vs_num_bigint`, 5000 pairs) plus the targeted unit tests
// (`divmod_edge_cases_and_wide_operands`, `scalar_primitives_vs_num_bigint`, the byte round-trips).
// Operator directive 2026-09-19 (same pattern as etude-bytevec #242 / decimal #244).
#[cfg_attr(
    miri,
    ignore = "bolero property harness spins under miri; oracle path covered by deterministic tests"
)]
fn differential_bolero() {
    bolero::check!()
        .with_type::<(Vec<u8>, Vec<u8>)>()
        .cloned()
        .for_each(|(ab, bb)| {
            let a = Big::from_le_twos_complement_bytes(&ab);
            let b = Big::from_le_twos_complement_bytes(&bb);
            check_pair(&a, &b);
        });
}

/// `write_decimal` (the sink-writing, allocation-free core) must produce EXACTLY the same digits as
/// `to_decimal_string` — and hence match num-bigint — for every sign and width, including the wide
/// recursive path. Also confirms it drives an arbitrary `core::fmt::Write` sink (not only `String`).
#[test]
fn write_decimal_matches_to_decimal_string_and_num_bigint() {
    let mut rng = Rng(0xed17_dec1_a110_c1ce);
    let check = |b: &Big| {
        let mut s = alloc::string::String::new();
        b.write_decimal(&mut s).unwrap();
        assert_eq!(
            s,
            b.to_decimal_string(),
            "write_decimal == to_decimal_string {b:?}"
        );
        assert_eq!(
            s,
            to_ref(b).to_string(),
            "write_decimal == num-bigint {b:?}"
        );
    };
    // Zero, small signed values, and a few exact/boundary values.
    for &v in &[0i64, 1, -1, 9, -9, 10, -10, 42, -42, i64::MAX, i64::MIN] {
        check(&Big::from_i64(v));
    }
    // Random widths straddling the recursive threshold (10 limbs), both signs.
    for &n in &[1usize, 5, 10, 11, 17, 40, 100] {
        for &neg in &[false, true] {
            let mut mag: Vec<u64> = (0..n).map(|_| rng.next()).collect();
            if let Some(top) = mag.last_mut() {
                *top |= 0x8000_0000_0000_0000;
            }
            let mut b = Big { neg, mag };
            b.normalize();
            check(&b);
        }
    }
    // Drives a non-String sink: a `core::fmt::Write` wrapper that records the total byte length.
    struct CountingSink(usize);
    impl core::fmt::Write for CountingSink {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            self.0 += s.len();
            Ok(())
        }
    }
    let big = from_i128(i128::MIN); // negative, multi-limb
    let mut sink = CountingSink(0);
    big.write_decimal(&mut sink).unwrap();
    assert_eq!(
        sink.0,
        big.to_decimal_string().len(),
        "sink byte count matches"
    );
}

/// The native-scalar accessors — `is_even`/`is_odd` and the single-limb `rem_u64`/`divmod_u64` — vs
/// num-bigint, across signs and widths and a spread of divisors (powers of two, small primes, `10¹⁹`,
/// `u64::MAX`). The remainder is the magnitude remainder `|self| mod d`.
#[test]
fn scalar_primitives_vs_num_bigint() {
    use num_traits::ToPrimitive;
    let mut rng = Rng(0x5ca1_a400_0000_0001);
    let divisors: &[u64] = &[
        1,
        2,
        5,
        10,
        7,
        1_000_000_007,
        1u64 << 32,
        10u64.pow(19),
        u64::MAX,
    ];
    let two = Ref::from(2);
    for _ in 0..3000 {
        let a = rng.big_upto(8);
        let ra = to_ref(&a);
        // parity: even iff (value mod 2) == 0.
        let even = (&ra % &two) == Ref::from(0);
        assert_eq!(a.is_even(), even, "is_even {a:?}");
        assert_eq!(a.is_odd(), !even, "is_odd {a:?}");
        for &d in divisors {
            let rd = Ref::from(d);
            // magnitude remainder |self| mod d = |self mod d| (truncated division).
            let exp_rem = (&ra % &rd).magnitude().to_u64().unwrap();
            assert_eq!(a.rem_u64(d), Some(exp_rem), "rem_u64 {a:?} % {d}");
            let (q, r) = a.divmod_u64(d).unwrap();
            assert_eq!(r, exp_rem, "divmod_u64 rem {a:?} % {d}");
            assert_eq!(to_ref(&q), &ra / &rd, "divmod_u64 quotient {a:?} / {d}");
        }
        assert_eq!(a.rem_u64(0), None, "rem_u64 by zero");
        assert!(a.divmod_u64(0).is_none(), "divmod_u64 by zero");
        // last_decimal_digit = |self| mod 10 (the 2⁶⁴≡6 limb-sum identity), cross-checked against the
        // reciprocal rem_u64(10) it is meant to replace (which is itself checked vs num-bigint above).
        assert_eq!(
            a.last_decimal_digit() as u64,
            a.rem_u64(10).unwrap(),
            "last_decimal_digit {a:?}"
        );
    }
    // Zero and small explicit values.
    assert!(Big::zero().is_even());
    assert!(Big::from_i64(-4).is_even());
    assert!(Big::from_i64(-3).is_odd());
    assert_eq!(Big::from_i64(-17).rem_u64(5), Some(2)); // |−17| mod 5 = 2
    assert_eq!(
        Big::from_i64(-17).divmod_u64(5),
        Some((Big::from_i64(-3), 2))
    );
    assert_eq!(Big::zero().last_decimal_digit(), 0);
    assert_eq!(Big::from_i64(-17).last_decimal_digit(), 7); // |−17| = 17 → 7
    // Multi-limb, crossing the 6·high-limb term: i128::MAX = …105727 → 7.
    assert_eq!(
        Big::from_i128(i128::MAX).last_decimal_digit(),
        (i128::MAX % 10) as u8
    );
}

#[test]
fn canonical_form_invariants() {
    // Zero is unique and non-negative.
    assert!(Big::zero().is_zero());
    assert_eq!(
        Big::zero(),
        Big {
            neg: false,
            mag: Vec::new()
        }
    );
    // A "-0" or trailing-zero-limb input normalizes to canonical zero / minimal form.
    let mut z = Big {
        neg: true,
        mag: alloc::vec![0, 0],
    };
    z.normalize();
    assert_eq!(z, Big::zero());
    let mut t = Big {
        neg: false,
        mag: alloc::vec![5, 0, 0],
    };
    t.normalize();
    assert_eq!(t.mag, alloc::vec![5]);
    // Subtraction that reaches zero canonicalizes the sign.
    let five = Big::from_i64(5);
    assert_eq!(five.sub(&five), Big::zero());
    assert!(!five.sub(&five).neg);
}

/// The `Big` primitives compose into a correct RATIONAL normalization (lowest terms via `gcd`,
/// denominator strictly positive, sign on the numerator) — validating that `gcd` + `divmod` (exact
/// division by the gcd) + `neg` compose canonically.
#[test]
fn gcd_and_divmod_compose_into_rational_normalization() {
    // Normalize (num, den) → lowest terms, denominator > 0, sign on numerator. `den != 0`.
    fn normalize(num: &Big, den: &Big) -> (Big, Big) {
        assert!(
            !den.is_zero(),
            "the caller rejects a zero denominator before normalizing"
        );
        let g = num.gcd(den); // non-negative; gcd(0, d) = |d|
        // Divide both by the gcd (exact — g divides both). divmod's quotient carries each operand's sign.
        let (mut n, _) = num.divmod(&g).expect("gcd is nonzero when den != 0");
        let (mut d, _) = den.divmod(&g).expect("gcd is nonzero when den != 0");
        // Denominator strictly positive: if it came out negative, flip BOTH signs (value unchanged).
        if d.neg {
            n = n.neg();
            d = d.neg();
        }
        (n, d)
    }
    // a/b == c/d iff a*d == c*b.
    let cross_eq =
        |n1: &Big, d1: &Big, n2: &Big, d2: &Big| n1.mul(d2).cmp(&n2.mul(d1)) == Ordering::Equal;
    let cases: &[(i64, i64)] = &[
        (1, 2),
        (2, 4),
        (6, 8),
        (-1, 2),
        (1, -2),
        (-6, -8),
        (0, 5),
        (10, 5),
        (-10, 5),
        (7, 1),
        (100, -35),
        (-100, 35),
        (i64::MAX, 3),
        (3, i64::MAX),
    ];
    for &(n, d) in cases {
        let (nn, nd) = normalize(&Big::from_i64(n), &Big::from_i64(d));
        // (1) denominator strictly positive (never zero — den != 0 — and never negative).
        assert!(
            !nd.neg && !nd.is_zero(),
            "normalized denominator is strictly positive for {n}/{d}"
        );
        // (2) lowest terms: gcd(|num'|, den') == 1 (or num' == 0 with den' == 1).
        let g = nn.gcd(&nd);
        if nn.is_zero() {
            assert_eq!(nd, Big::from_i64(1), "0/d normalizes to 0/1 for {n}/{d}");
        } else {
            assert_eq!(
                g,
                Big::from_i64(1),
                "num'/den' is in lowest terms for {n}/{d}"
            );
        }
        // (3) value preserved: num'/den' == n/d (cross-multiply).
        assert!(
            cross_eq(&nn, &nd, &Big::from_i64(n), &Big::from_i64(d)),
            "value preserved for {n}/{d}"
        );
        // (4) canonical: normalizing an already-normalized pair is a fixpoint.
        let (nn2, nd2) = normalize(&nn, &nd);
        assert_eq!(
            (nn2, nd2),
            (nn, nd),
            "normalization is idempotent for {n}/{d}"
        );
    }
    // Two equal-value pairs normalize to the SAME canonical form (the map-key property).
    let (a_n, a_d) = normalize(&Big::from_i64(6), &Big::from_i64(8));
    let (b_n, b_d) = normalize(&Big::from_i64(-9), &Big::from_i64(-12)); // == 6/8 == 3/4
    assert_eq!(
        (a_n, a_d),
        (b_n, b_d),
        "6/8 and -9/-12 normalize identically (both 3/4)"
    );
}

#[test]
fn i64_round_trip_and_bounds() {
    for &v in &[
        0i64,
        1,
        -1,
        42,
        -42,
        i64::MAX,
        i64::MIN,
        1 << 40,
        -(1 << 40),
        0xffff_ffff,
        -0xffff_ffff,
    ] {
        let b = Big::from_i64(v);
        assert_eq!(b.to_i64_checked(), Some(v), "i64 round-trip {v}");
        assert_eq!(to_ref(&b), Ref::from(v), "i64 vs ref {v}");
    }
    // Out-of-range narrowing → None.
    let too_big = Big::from_i64(i64::MAX).add(&Big::from_i64(1)); // 2^63
    assert_eq!(too_big.to_i64_checked(), None, "2^63 does not fit i64");
    let way_big = from_i128((i64::MAX as i128) * 1000);
    assert_eq!(way_big.to_i64_checked(), None);
    // i64::MIN (= -2^63) DOES fit.
    assert_eq!(Big::from_i64(i64::MIN).to_i64_checked(), Some(i64::MIN));
}

/// `from_i128` / `to_i128_checked` (the limb-level pair) must round-trip, agree with num-bigint, and be
/// byte-identical to / agree with the sign-magnitude byte versions (`i128_to/from_sign_magnitude_bytes`)
/// they mirror — across the i64/i128 limb boundaries and the `i128::MIN`/`MAX` endpoints.
#[test]
fn i128_round_trip_bounds_and_vs_byte_versions() {
    for &v in &[
        0i128,
        1,
        -1,
        42,
        -42,
        i64::MAX as i128,
        i64::MIN as i128,
        i64::MAX as i128 + 1, // first value needing a 2nd limb
        i64::MIN as i128 - 1,
        1i128 << 64,
        -(1i128 << 64),
        (1i128 << 64) + 1,
        i128::MAX,
        i128::MIN,
        i128::MAX - 1,
        i128::MIN + 1,
        0x0123_4567_89ab_cdef_7654_3210_fedc_ba98,
        -0x0123_4567_89ab_cdef_7654_3210_fedc_ba98,
    ] {
        let b = Big::from_i128(v);
        assert_eq!(b.to_i128_checked(), Some(v), "i128 round-trip {v}");
        assert_eq!(to_ref(&b), Ref::from(v), "i128 vs ref {v}");
        // from_i128 builds the same value the byte serializer would.
        let mut buf = [0u8; 17];
        let n = Big::i128_to_sign_magnitude_bytes_into(v, &mut buf).unwrap();
        assert_eq!(
            Big::from_sign_magnitude_bytes(&buf[..n]),
            b,
            "from_i128 == byte build {v}"
        );
        // to_i128_checked agrees with the byte-level read.
        assert_eq!(
            b.to_i128_checked(),
            Big::i128_from_sign_magnitude_bytes(&b.to_sign_magnitude_bytes()),
            "to_i128_checked == byte read {v}"
        );
    }
    // Random Bigs (often exceeding i128): to_i128_checked must match the byte read exactly, and when it
    // fits, round-trip through from_i128 and equal the num-bigint value.
    let mut rng = Rng(0x1128_1128_1128_1128);
    for _ in 0..500 {
        let b = rng.big_upto(6); // up to 5 limbs — frequently past i128's 2 limbs
        assert_eq!(
            b.to_i128_checked(),
            Big::i128_from_sign_magnitude_bytes(&b.to_sign_magnitude_bytes()),
            "to_i128_checked matches byte read for {b:?}"
        );
        if let Some(v) = b.to_i128_checked() {
            assert_eq!(Big::from_i128(v), b, "from_i128 round-trip {v}");
            assert_eq!(Ref::from(v), to_ref(&b), "i128 value vs ref {v}");
        }
    }
    // A positive 2^127 (= i128::MAX + 1) exceeds i128 → None; the endpoints fit.
    let two_127 = Big::from_i128(i128::MAX).add(&Big::from_i64(1));
    assert_eq!(two_127.to_i128_checked(), None, "2^127 does not fit i128");
    assert_eq!(Big::from_i128(i128::MIN).to_i128_checked(), Some(i128::MIN));
    assert_eq!(Big::from_i128(i128::MAX).to_i128_checked(), Some(i128::MAX));
}

#[test]
fn sign_magnitude_bytes_round_trip_and_canonical() {
    let mut rng = Rng(0xdead_beef_cafe_0001);
    for _ in 0..2000 {
        let b = rng.big();
        let bytes = b.to_sign_magnitude_bytes();
        assert_eq!(
            Big::from_sign_magnitude_bytes(&bytes),
            b,
            "sign-mag round-trip {b:?}"
        );
        // Canonical: equal values → identical bytes (the map-key requirement).
        assert_eq!(bytes, b.clone().to_sign_magnitude_bytes());
    }
    // Zero is exactly [0x00].
    assert_eq!(Big::zero().to_sign_magnitude_bytes(), alloc::vec![0u8]);
    assert_eq!(Big::from_sign_magnitude_bytes(&[0]), Big::zero());
}

#[test]
fn twos_complement_bytes_round_trip_vs_num_bigint() {
    let mut rng = Rng(0x0badf00d_12345678);
    for _ in 0..3000 {
        let b = rng.big();
        let bytes = b.to_le_twos_complement_bytes();
        // Round-trips through our own parser.
        assert_eq!(
            Big::from_le_twos_complement_bytes(&bytes),
            b,
            "2c round-trip {b:?}"
        );
        // Matches num-bigint's signed LE two's-complement encoding.
        let rbytes = to_ref(&b).to_signed_bytes_le();
        // num-bigint encodes 0 as [0]; we encode 0 as [] — normalize both to "value" via re-parse.
        assert_eq!(
            Big::from_le_twos_complement_bytes(&rbytes),
            b,
            "num-bigint 2c bytes {rbytes:?} parse to {b:?}"
        );
    }
}

/// divmod is the algorithm with real subtlety (bit-at-a-time long division; a limb-boundary carry/
/// borrow bug hides only on LARGE operands the ≤4-limb random fuzzer never reaches). Two prongs:
/// (1) a WIDE differential vs num-bigint (up to ~20 limbs = ~640-bit); (2) structural corner cases —
/// powers of two (all-carry shifts), a single-limb divisor of a huge dividend, dividend just below /
/// at / above the divisor, all-`0xffffffff` limbs, and an EXACT multiple (`(k*d)/d == k`, rem 0).
#[test]
fn divmod_edge_cases_and_wide_operands() {
    // (1) Wide differential — magnitudes up to ~20 limbs, both signs.
    let mut rng = Rng(0xf00d_1234_5678_9abc);
    for _ in 0..3000 {
        let a = rng.big_upto(20);
        let b = rng.big_upto(20);
        let (ra, rb) = (to_ref(&a), to_ref(&b));
        if b.is_zero() {
            assert!(a.divmod(&b).is_none());
            continue;
        }
        let (q, r) = a.divmod(&b).unwrap();
        assert_eq!(to_ref(&q), &ra / &rb, "wide div {a:?} {b:?}");
        assert_eq!(to_ref(&r), &ra % &rb, "wide rem {a:?} {b:?}");
        // Quotient-only path on WIDE operands (exercises the Knuth want_rem=false branch).
        assert_eq!(
            a.div_exact(&b),
            Some(q.clone()),
            "wide div_exact {a:?} {b:?}"
        );
        assert_eq!(a, q.mul(&b).add(&r), "wide divmod identity");
        // |remainder| < |divisor| (the division invariant).
        assert_eq!(
            Big {
                neg: false,
                mag: r.mag.clone()
            }
            .cmp(&Big {
                neg: false,
                mag: b.mag.clone()
            }),
            Ordering::Less,
            "|rem| < |divisor| {a:?} {b:?}"
        );
    }

    // (2) Structural corners.
    let pow2 = |bits: u32| -> Big {
        // 2^bits as a Big (a single set bit — exercises the shift/carry path).
        let limb = (bits / 64) as usize;
        let mut mag = alloc::vec![0u64; limb + 1];
        mag[limb] = 1u64 << (bits % 64);
        let mut b = Big { neg: false, mag };
        b.normalize();
        b
    };
    // 2^200 / 2^64 = 2^136, remainder 0.
    let (q, r) = pow2(200).divmod(&pow2(64)).unwrap();
    assert_eq!(q, pow2(136), "2^200 / 2^64 = 2^136");
    assert!(r.is_zero(), "2^200 % 2^64 = 0");
    // (2^200 - 1) / 2^64 → quotient 2^136 - 1, remainder 2^64 - 1 (all low bits set).
    let big = pow2(200).sub(&Big::from_i64(1));
    let (q2, r2) = big.divmod(&pow2(64)).unwrap();
    assert_eq!(
        to_ref(&q2),
        to_ref(&big) / to_ref(&pow2(64)),
        "(2^200-1)/2^64 vs ref"
    );
    assert_eq!(
        to_ref(&r2),
        to_ref(&big) % to_ref(&pow2(64)),
        "(2^200-1)%2^64 vs ref"
    );

    // Single-limb divisor of a huge dividend (the common `n / small` shape).
    let huge = pow2(300).add(&Big::from_i64(12345));
    let small = Big::from_i64(7);
    let (qs, rs) = huge.divmod(&small).unwrap();
    assert_eq!(to_ref(&qs), to_ref(&huge) / to_ref(&small));
    assert_eq!(to_ref(&rs), to_ref(&huge) % to_ref(&small));

    // Dividend just-below / at / just-above the divisor.
    let d = pow2(128);
    let below = d.sub(&Big::from_i64(1));
    assert_eq!(
        below.divmod(&d).unwrap(),
        (Big::zero(), below.clone()),
        "a<d → (0, a)"
    );
    assert_eq!(
        d.divmod(&d).unwrap(),
        (Big::from_i64(1), Big::zero()),
        "a==d → (1, 0)"
    );
    let above = d.add(&Big::from_i64(1));
    assert_eq!(
        above.divmod(&d).unwrap(),
        (Big::from_i64(1), Big::from_i64(1)),
        "a=d+1 → (1, 1)"
    );

    // All-ones limbs (max limb values — carry propagation stress).
    let maxes = Big {
        neg: false,
        mag: alloc::vec![u64::MAX; 8],
    };
    let mref = to_ref(&maxes);
    for div in [Big::from_i64(3), pow2(32), pow2(100), maxes.clone()] {
        let (q, r) = maxes.divmod(&div).unwrap();
        assert_eq!(to_ref(&q), &mref / to_ref(&div), "maxes / {div:?}");
        assert_eq!(to_ref(&r), &mref % to_ref(&div), "maxes % {div:?}");
    }

    // Exact multiple: (k*d)/d == k, rem 0 — for random NON-NEGATIVE k, d.
    let abs = |mut b: Big| {
        b.neg = false;
        b
    };
    for _ in 0..500 {
        let k = abs(rng.big_upto(8));
        let dd = abs(rng.big_upto(8));
        if dd.is_zero() {
            continue;
        }
        let prod = k.mul(&dd);
        let (q, r) = prod.divmod(&dd).unwrap();
        assert_eq!(q, k, "(k*d)/d == k");
        assert!(r.is_zero(), "(k*d)%d == 0");
    }
}

/// Multiplication of WIDE operands vs num-bigint — the small random corpus never reaches the
/// Karatsuba threshold (32 limbs), so this pins the schoolbook↔Karatsuba boundary (31/32/33 limbs),
/// the recursive Karatsuba path (100/200 limbs split repeatedly), and unbalanced sizes (where the
/// dispatch drops back to schoolbook). Both signs.
/// `to_decimal_string` on WIDE magnitudes vs num-bigint — the small random corpus never reaches the
/// recursive divide-and-conquer threshold (10 limbs), so this pins the recursive base-conversion path:
/// sizes straddling the threshold, the balanced power-of-ten split, the recursion, the "high half
/// empty" narrowing (a value just under a split boundary), and both signs. Also exact powers of ten and
/// `10^k - 1` (all-nines) — the values most likely to expose a padding/leading-zero fencepost.
#[test]
fn to_decimal_string_recursive_vs_num_bigint() {
    let mut rng = Rng(0xdec1_3a17_c0de_5a5a);
    let mk = |rng: &mut Rng, n: usize, neg: bool| -> Big {
        let mut mag: Vec<u64> = (0..n).map(|_| rng.next()).collect();
        if let Some(top) = mag.last_mut() {
            *top |= 0x8000_0000_0000_0000; // force exact width n
        }
        let mut b = Big { neg, mag };
        b.normalize();
        b
    };
    // Widths straddling the recursive threshold and up through several split levels, both signs.
    for &n in &[9usize, 10, 11, 16, 17, 31, 32, 64, 100, 200] {
        for &neg in &[false, true] {
            let b = mk(&mut rng, n, neg);
            assert_eq!(b.to_decimal_string(), to_ref(&b).to_string(), "{n} limbs");
        }
    }
    // Values just below / at / above a balanced split boundary exercise the "high half empty" and the
    // zero-padding of a short low half: 10^k, 10^k - 1 (all nines), 10^k + 1, for a spread of k.
    let ten = Big::from_i64(10);
    let mut p = Big::from_i64(1);
    for k in 1..=400usize {
        p = p.mul(&ten); // p == 10^k
        if [1, 18, 19, 20, 37, 38, 39, 76, 77, 152, 300, 400].contains(&k) {
            for cand in [
                p.clone(),
                p.sub(&Big::from_i64(1)),
                p.add(&Big::from_i64(1)),
            ] {
                assert_eq!(
                    cand.to_decimal_string(),
                    to_ref(&cand).to_string(),
                    "10^{k} neighborhood"
                );
                let neg = cand.neg();
                assert_eq!(neg.to_decimal_string(), to_ref(&neg).to_string(), "-10^{k}");
            }
        }
    }
}

/// gcd on WIDE operands vs num-bigint — the ≤4-limb random corpus barely exercises Lehmer's
/// multiprecision cofactor path or its fallbacks. This drives: (1) random wide pairs; (2) pairs with a
/// planted large common factor `k` (so the gcd itself is multi-limb, stressing the cofactor combine and
/// the `k*x, k*y` reduction); (3) HIGHLY unbalanced widths (a ≫ b, which forces the `bh == 0` aligned-
/// leading-limb fallback to a full division step); (4) equal operands and one-off (coprime) pairs.
#[test]
fn gcd_wide_operands_vs_num_bigint() {
    let mut rng = Rng(0x9cd1_ea3b_7c50_0001);
    let mk = |rng: &mut Rng, n: usize| -> Big {
        let mut mag: Vec<u64> = (0..n).map(|_| rng.next()).collect();
        if let Some(top) = mag.last_mut() {
            *top |= 0x8000_0000_0000_0000; // force exact width n (top limb nonzero)
        }
        let mut b = Big { neg: false, mag };
        b.normalize();
        b
    };
    let check = |a: &Big, b: &Big| {
        let g = a.gcd(b);
        assert!(!g.is_negative(), "gcd non-negative");
        assert_eq!(to_ref(&g), ref_gcd(to_ref(a), to_ref(b)), "gcd {a:?} {b:?}");
        if !g.is_zero() {
            assert!(a.divmod(&g).unwrap().1.is_zero(), "gcd divides a");
            assert!(b.divmod(&g).unwrap().1.is_zero(), "gcd divides b");
        }
    };
    // (1) random wide pairs + (3) unbalanced widths (na ≫ nb triggers the bh==0 fallback).
    for &(na, nb) in &[
        (8usize, 8usize),
        (16, 15),
        (20, 3),
        (24, 1),
        (30, 12),
        (12, 30),
        (17, 17),
        (40, 5),
    ] {
        for _ in 0..40 {
            let a = mk(&mut rng, na);
            let b = mk(&mut rng, nb);
            check(&a, &b);
        }
    }
    // (2) planted common factor: gcd(k*x, k*y) is a multiple of k → a wide gcd result.
    for _ in 0..60 {
        let (nk, nx, ny) = (
            3 + (rng.next() % 6) as usize,
            4 + (rng.next() % 8) as usize,
            4 + (rng.next() % 8) as usize,
        );
        let k = mk(&mut rng, nk);
        let x = mk(&mut rng, nx);
        let y = mk(&mut rng, ny);
        check(&k.mul(&x), &k.mul(&y));
    }
    // (4) equal operands (gcd == |a|) and gcd(a, 1) == 1.
    let a = mk(&mut rng, 18);
    check(&a, &a);
    check(&a, &Big::from_i64(1));
}

#[test]
fn mul_wide_operands_vs_num_bigint() {
    let mut rng = Rng(0x51ee_7c0d_e1a5_9b3f);
    // A `Big` with exactly `n` u64 limbs (top limb forced nonzero so the width is exact), random sign.
    let mk = |rng: &mut Rng, n: usize| -> Big {
        let mut mag: Vec<u64> = (0..n).map(|_| rng.next()).collect();
        if let Some(top) = mag.last_mut() {
            *top |= 0x8000_0000_0000_0000;
        }
        let mut b = Big {
            neg: rng.next() & 1 == 1,
            mag,
        };
        b.normalize();
        b
    };
    // nb spans below-threshold (schoolbook dispatch) and >=threshold sizes both equal and unequal to
    // na, so balanced AND unbalanced Karatsuba (and its recursion at 100/200) are all exercised.
    for &na in &[39usize, 40, 41, 64, 65, 100, 200] {
        for &nb in &[1usize, 39, 40, 64, na] {
            let a = mk(&mut rng, na);
            let b = mk(&mut rng, nb);
            assert_eq!(
                to_ref(&a.mul(&b)),
                to_ref(&a) * to_ref(&b),
                "mul {na}x{nb} limbs"
            );
        }
    }
}

/// `decimal_digit_count` must equal the digit length of the rendered decimal (which is itself checked
/// against num-bigint), ignoring the sign. Sweeps the powers of ten and their ±1 neighbours across the
/// single/multi-limb boundary and into the recursive-render width, both signs, plus random and wide
/// magnitudes — the estimate-and-correct path must land exactly on every 10ᵏ boundary.
#[test]
fn decimal_digit_count_matches_render() {
    let check = |b: &Big| {
        let expected = b.to_decimal_string().trim_start_matches('-').len() as u64;
        assert_eq!(b.decimal_digit_count(), expected, "digit count of {b:?}");
    };
    check(&Big::zero());
    // 1, 10, 100, …, 10^79 (~266 bits, well into the multi-limb / recursive path) with ±1 and negation.
    let ten = Big::from_i64(10);
    let one = Big::from_i64(1);
    let mut p = one.clone();
    for _ in 0..80 {
        check(&p);
        check(&p.sub(&one)); // 10^k − 1 (all-nines, one fewer digit at the boundary)
        check(&p.add(&one)); // 10^k + 1
        check(&p.neg());
        p = p.mul(&ten);
    }
    // Random magnitudes up to 11 limbs (crosses the recursive threshold), both signs.
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..2000 {
        check(&rng.big_upto(12));
    }
    // A few very wide values (top limb forced set so the width is exact).
    for &n in &[16usize, 32, 64] {
        let mut mag: Vec<u64> = (0..n).map(|_| rng.next()).collect();
        *mag.last_mut().unwrap() |= 0x8000_0000_0000_0000;
        let mut b = Big { neg: false, mag };
        b.normalize();
        check(&b);
    }
}

/// `from_base_10_pow_k_limbs` must build exactly the value its base-`10ᵏ` limbs denote — checked against
/// num-bigint parsing the equivalent decimal string (each chunk zero-padded to `k`). Sweeps `k` across
/// its whole `1..=19` range and chunk counts from empty (zero) up through the recursive-render width,
/// including leading-zero and all-zero streams; also pins the canonical form and the `None` rejects.
#[test]
fn from_base_10_pow_k_limbs_vs_num_bigint() {
    use core::str::FromStr;
    let mut rng = Rng(0x0bad_c0de_1155_aa77);
    for k in 1u32..=19 {
        let base = 10u64.pow(k);
        for count in 0..=40usize {
            // Random valid chunks (each < 10ᵏ), most-significant first.
            let limbs: Vec<u64> = (0..count).map(|_| rng.next() % base).collect();
            let got = Big::from_base_10_pow_k_limbs(k, &limbs).expect("valid chunks build a value");
            // Reference: concatenate the chunks (each padded to k digits) and parse as one decimal.
            let mut s = alloc::string::String::new();
            for &limb in &limbs {
                s.push_str(&alloc::format!("{limb:0width$}", width = k as usize));
            }
            if s.is_empty() {
                s.push('0');
            }
            let expect = Ref::from_str(&s).expect("padded chunk string is valid decimal");
            assert_eq!(to_ref(&got), expect, "k={k} count={count} limbs={limbs:?}");
            // Result is non-negative and canonical (empty-magnitude zero, no trailing zero limbs).
            assert!(!got.is_negative());
            assert!(got.mag.last() != Some(&0));
            if expect.is_zero() {
                assert!(got.is_zero(), "all-zero stream must be canonical zero");
            }
        }
        // A chunk carrying ≥ k digits (`== 10ᵏ`) is rejected.
        assert!(Big::from_base_10_pow_k_limbs(k, &[base]).is_none());
        assert!(Big::from_base_10_pow_k_limbs(k, &[0, base, 1]).is_none());
    }
    // `k` out of the `1..=19` range is rejected (0 has no digits; 20 would overflow `10ᵏ` past `u64`).
    assert!(Big::from_base_10_pow_k_limbs(0, &[1]).is_none());
    assert!(Big::from_base_10_pow_k_limbs(20, &[1]).is_none());
    // A leading-zero stream and a single ragged leading chunk both land on the right value.
    assert_eq!(
        Big::from_base_10_pow_k_limbs(3, &[0, 0, 5])
            .unwrap()
            .to_decimal_string(),
        "5"
    );
    assert_eq!(
        Big::from_base_10_pow_k_limbs(3, &[1, 0, 7])
            .unwrap()
            .to_decimal_string(),
        "1000007"
    );
}
