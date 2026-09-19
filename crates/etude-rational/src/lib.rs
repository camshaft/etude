// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Exact rational numbers — a normalized `num/den` pair over [`etude_bigint::Big`] arbitrary-precision
//! integers. Pure over `alloc`, no I/O, no dependency but `etude-bigint`. Exact `+ - * /` and comparison
//! over the normalized pair, with a differential test against `num-rational` (a dev-dependency) as the
//! safety net.
//!
//! # Representation and the canonical-form invariant
//! [`Rational`] is a `{ num: Big, den: Big }` pair kept in a single canonical form:
//! - the denominator is strictly positive (`den >= 1`), so the sign lives entirely on the numerator;
//! - the pair is in lowest terms (`gcd(|num|, den) == 1`);
//! - zero is exactly `0/1`; an integer `n` is exactly `n/1`.
//!
//! Every constructor and operation renormalizes, so a value has exactly one in-memory form. This is
//! required for `Eq`/`Ord`/hashing to mean mathematical equality: `1/2` and `2/4` are the same value and
//! must have identical fields. There is no representation of a zero-denominator rational — the fallible
//! constructors return `None` and the total operations cannot produce one.
//!
//! The fields are private and not part of the stable API. Construct through [`Rational::zero`],
//! [`Rational::from_i64`], [`Rational::from_bigint`], [`Rational::new`]; inspect through [`Rational::numer`],
//! [`Rational::denom`], [`Rational::is_zero`], [`Rational::is_negative`], [`Rational::is_integer`],
//! [`Rational::cmp`].

#![cfg_attr(not(test), no_std)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::string::String;
use core::cmp::Ordering;
use etude_bigint::Big;

/// An exact rational number in canonical form. See the module doc for the invariant.
///
/// The internal `{ num, den }` representation is private and not part of the stable API. Construct values
/// through the constructors and inspect them through the accessors.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Rational {
    /// Numerator; carries the sign of the whole value. Canonical: `gcd(|num|, den) == 1`.
    num: Big,
    /// Denominator; always strictly positive and coprime to `num`. Never zero.
    den: Big,
}

impl Rational {
    /// The canonical zero, `0/1`.
    pub fn zero() -> Rational {
        Rational {
            num: Big::zero(),
            den: Big::from_i64(1),
        }
    }

    /// The canonical one, `1/1`.
    pub fn one() -> Rational {
        Rational {
            num: Big::from_i64(1),
            den: Big::from_i64(1),
        }
    }

    /// The integer `n` as `n/1`.
    pub fn from_i64(n: i64) -> Rational {
        Rational {
            num: Big::from_i64(n),
            den: Big::from_i64(1),
        }
    }

    /// The integer `n` (a [`Big`]) as `n/1`.
    pub fn from_bigint(n: Big) -> Rational {
        Rational {
            num: n,
            den: Big::from_i64(1),
        }
    }

    /// The rational `num/den`, normalized to canonical form. Returns `None` when `den` is zero.
    pub fn new(num: Big, den: Big) -> Option<Rational> {
        normalize(num, den)
    }

    /// The rational `num/den` from two `i64`s, normalized. Returns `None` when `den == 0`.
    pub fn from_ratio_i64(num: i64, den: i64) -> Option<Rational> {
        normalize(Big::from_i64(num), Big::from_i64(den))
    }

    /// The numerator (carries the sign; coprime to the denominator).
    pub fn numer(&self) -> &Big {
        &self.num
    }

    /// The denominator (strictly positive; coprime to the numerator).
    pub fn denom(&self) -> &Big {
        &self.den
    }

    /// Whether this is zero (canonical `0/1`).
    pub fn is_zero(&self) -> bool {
        self.num.is_zero()
    }

    /// Whether this is strictly negative. `false` for zero.
    pub fn is_negative(&self) -> bool {
        self.num.is_negative()
    }

    /// Whether this is an integer, i.e. the denominator is `1`.
    pub fn is_integer(&self) -> bool {
        // Canonical form: den >= 1 and coprime to num, so den == 1 iff the value is an integer.
        // `bit_len() == 1` ⟺ the value is exactly 1 (den is positive) — an O(1) check with no allocation.
        self.den.bit_len() == 1
    }

    /// The additive inverse `-self`. Already canonical (only the numerator's sign flips).
    pub fn neg(&self) -> Rational {
        Rational {
            num: self.num.neg(),
            den: self.den.clone(),
        }
    }

    /// The absolute value `|self|`. Already canonical (denominator is positive; only the numerator's
    /// sign is dropped).
    pub fn abs(&self) -> Rational {
        Rational {
            num: self.num.abs(),
            den: self.den.clone(),
        }
    }

    /// The reciprocal `1/self` (`den/num`). Returns `None` when `self` is zero.
    ///
    /// No gcd is needed: `self` is already canonical (`den > 0`, `gcd(|num|, den) == 1`), and coprimality
    /// is symmetric, so `den/num` is already in lowest terms — only the sign is moved onto the numerator
    /// to keep the denominator positive. This is an O(limbs) swap, not the O(gcd) full normalize.
    pub fn recip(&self) -> Option<Rational> {
        if self.num.is_zero() {
            return None;
        }
        if self.num.is_negative() {
            // num < 0 ⇒ the raw reciprocal `den/num` has a negative denominator; negate both terms.
            // Numerator `-den` is negative (den > 0), matching the sign of a negative input's reciprocal;
            // denominator `-num = |num|` is positive. Coprime because `gcd(den, |num|) == 1` already.
            Some(Rational {
                num: self.den.neg(),
                den: self.num.neg(),
            })
        } else {
            // num > 0 ⇒ `den/num` already has a positive, coprime denominator.
            Some(Rational {
                num: self.den.clone(),
                den: self.num.clone(),
            })
        }
    }

    /// Exact sum `self + other`. `a/b + c/d = (a*d + c*b)/(b*d)`, renormalized.
    pub fn add(&self, other: &Rational) -> Rational {
        // Native i128 fast path FIRST: for i64-fitting operands (the common small case) this covers the
        // equal-denominator case too — `(a*b + c*b)/(b*b)` reduces natively to `(a+c)/b` — and is faster
        // than the Big equal-denominator path below (one native add + native gcd vs a Big add + Big gcd).
        if let Some(r) = self.addsub_small(other, false) {
            return r;
        }
        if self.den == other.den {
            // Common denominator: `(a + c)/b`. Skips both cross-multiplies AND the `b*d` product — three
            // multiplies replaced by one add. The `den` equality test is an O(limbs) limb compare that
            // short-circuits on the first differing limb, so it is free when the denominators differ.
            return normalize(self.num.add(&other.num), self.den.clone())
                .expect("common denominator is positive");
        }
        let num = self.num.mul(&other.den).add(&other.num.mul(&self.den));
        let den = self.den.mul(&other.den);
        // Both denominators are strictly positive, so the product is nonzero: normalize cannot fail.
        normalize(num, den).expect("product of positive denominators is nonzero")
    }

    /// Exact difference `self - other`.
    pub fn sub(&self, other: &Rational) -> Rational {
        // Native i128 fast path FIRST (covers the equal-denominator small case too — see [`Rational::add`]).
        if let Some(r) = self.addsub_small(other, true) {
            return r;
        }
        if self.den == other.den {
            // Common denominator: `(a - c)/b` (see [`Rational::add`] for the fast-path rationale).
            return normalize(self.num.sub(&other.num), self.den.clone())
                .expect("common denominator is positive");
        }
        let num = self.num.mul(&other.den).sub(&other.num.mul(&self.den));
        let den = self.den.mul(&other.den);
        normalize(num, den).expect("product of positive denominators is nonzero")
    }

    /// Native `i128` add (`subtract == false`) or subtract for the general different-denominator case when
    /// all components fit `i64`: `(a*d ± c*b)/(b*d)`. `a*d`, `c*b`, `b*d` each fit `i128`, but the
    /// numerator `a*d ± c*b` (two terms up to `2^126`) can overflow `i128`, so it is checked — on overflow
    /// (or any component exceeding `i64`) return `None` to fall back to the `Big` path.
    fn addsub_small(&self, other: &Rational, subtract: bool) -> Option<Rational> {
        let a = self.num.to_i64_checked()? as i128;
        let b = self.den.to_i64_checked()? as i128;
        let c = other.num.to_i64_checked()? as i128;
        let d = other.den.to_i64_checked()? as i128;
        let ad = a * d; // fits i128 (|a*d| <= 2^126)
        let cb = c * b; // fits i128
        let num = if subtract {
            ad.checked_sub(cb)?
        } else {
            ad.checked_add(cb)?
        };
        let den = b * d; // b, d > 0 ⇒ den > 0
        // gcd(0, den) = den, so a zero numerator correctly normalizes to 0/1.
        let g = gcd_u128(num.unsigned_abs(), den as u128) as i128; // g >= 1
        Some(Rational {
            num: big_from_i128(num / g),
            den: big_from_i128(den / g),
        })
    }

    /// Exact product `self * other` = `(a/b) * (c/d)`.
    ///
    /// Cross-reduces before multiplying: since both operands are canonical (`gcd(a,b) = gcd(c,d) = 1`),
    /// the only common factors in `(a*c)/(b*d)` are between `a`&`d` and `c`&`b`. Cancelling `gcd(a,d)` and
    /// `gcd(c,b)` first leaves the result already in lowest terms — no final gcd-normalize — and both
    /// multiplies run on smaller operands. This trades one gcd over the `~2n`-bit product for two gcds
    /// over `~n`-bit operands (roughly half the work), and shrinks the products when factors do cancel.
    pub fn mul(&self, other: &Rational) -> Rational {
        if self.num.is_zero() || other.num.is_zero() {
            return Rational::zero();
        }
        // Native i128 fast path when all components fit i64 (the common small-rational case): avoids all
        // heap `Big` allocation and bignum ops. Falls back to the cross-reduced `Big` path otherwise.
        if let Some(r) = self.mul_small(other) {
            return r;
        }
        // `a*c / (b*d)`: cancel gcd(a,d) and gcd(c,b). `b,d > 0` and the cancelled factors are positive,
        // so the resulting denominator is positive — the sign stays on the numerator.
        let (num, den) = cross_reduce_mul(&self.num, &self.den, &other.num, &other.den);
        Rational { num, den }
    }

    /// Native-integer product when every component fits `i64`. `a*c` and `b*d` then fit `i128`
    /// (`|a*c| <= 2^126`), so there is no overflow; reduce with a native gcd and box the result. Returns
    /// `None` (fall back to the `Big` path) when any component exceeds `i64`. `self`/`other` are nonzero
    /// (the `mul` zero-guard ran first) and canonical, so `b, d > 0`.
    fn mul_small(&self, other: &Rational) -> Option<Rational> {
        let a = self.num.to_i64_checked()? as i128;
        let b = self.den.to_i64_checked()? as i128;
        let c = other.num.to_i64_checked()? as i128;
        let d = other.den.to_i64_checked()? as i128;
        let num = a * c;
        let den = b * d; // b, d > 0 ⇒ den > 0
        let g = gcd_u128(num.unsigned_abs(), den as u128) as i128; // g >= 1
        Some(Rational {
            num: big_from_i128(num / g),
            den: big_from_i128(den / g),
        })
    }

    /// Exact quotient `self / other` = `(a/b) / (c/d) = (a*d)/(b*c)`. Returns `None` when `other` is zero.
    ///
    /// Same cross-reduction as [`Rational::mul`] (dividing by `c/d` is multiplying by `d/c`): cancel
    /// `gcd(a,c)` and `gcd(d,b)` first, so no final gcd-normalize is needed. Because the divisor's
    /// numerator `c` can be negative, the resulting denominator's sign is fixed up onto the numerator.
    pub fn div(&self, other: &Rational) -> Option<Rational> {
        if other.num.is_zero() {
            return None;
        }
        if self.num.is_zero() {
            return Some(Rational::zero());
        }
        // Native i128 fast path (same overflow-freedom as `mul_small`).
        if let Some(r) = self.div_small(other) {
            return Some(r);
        }
        // Multiply `a/b` by `d/c` (both coprime pairs): cancel gcd(a,c) and gcd(d,b).
        let (mut num, mut den) = cross_reduce_mul(&self.num, &self.den, &other.den, &other.num);
        if den.is_negative() {
            num = num.neg();
            den = den.neg();
        }
        Some(Rational { num, den })
    }

    /// Native-integer quotient when every component fits `i64` (`self`/`other` nonzero, checked by the
    /// `div` caller). `a*d` and `b*c` fit `i128`, so no overflow; the divisor's numerator `c` may be
    /// negative, so the sign is moved onto the numerator. Returns `None` to fall back to the `Big` path.
    fn div_small(&self, other: &Rational) -> Option<Rational> {
        let a = self.num.to_i64_checked()? as i128;
        let b = self.den.to_i64_checked()? as i128;
        let c = other.num.to_i64_checked()? as i128;
        let d = other.den.to_i64_checked()? as i128;
        let mut num = a * d;
        let mut den = b * c; // b > 0, c != 0 ⇒ sign(den) = sign(c)
        if den < 0 {
            num = -num;
            den = -den;
        }
        let g = gcd_u128(num.unsigned_abs(), den as u128) as i128; // g >= 1
        Some(Rational {
            num: big_from_i128(num / g),
            den: big_from_i128(den / g),
        })
    }

    /// The decimal string `"num/den"` (e.g. `"-3/10"`), or just `"num"` when the value is an integer.
    ///
    /// Writes the components directly into one `String` via `Big::write_decimal` (a sink writer), so it
    /// allocates only the result — no intermediate per-component `String`s. Equivalent to `self.to_string()`.
    pub fn to_decimal_string(&self) -> String {
        use core::fmt::Write;
        let mut s = String::new();
        // Writing into a String is infallible.
        let _ = write!(s, "{self}");
        s
    }
}

impl core::fmt::Display for Rational {
    /// `num/den` (e.g. `-3/10`), or just `num` when the value is an integer. Writes the components
    /// straight into the formatter via `Big::write_decimal` — no intermediate `String` allocation.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.num.write_decimal(f)?;
        if !self.is_integer() {
            f.write_str("/")?;
            self.den.write_decimal(f)?;
        }
        Ok(())
    }
}

impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Rational) -> Option<Ordering> {
        Some(Ord::cmp(self, other))
    }
}

impl Ord for Rational {
    /// Exact three-way comparison.
    ///
    /// Fast paths before the general comparison: strictly-different signs decide in O(1), and equal
    /// denominators reduce to a direct numerator compare (both denominators are strictly positive). The
    /// general comparison is a size-thresholded hybrid: for small components a plain cross-multiply
    /// (`a*d ? c*b`) is cheapest, while for large components the continued-fraction / Euclidean method
    /// avoids the `O(n²)` double-width multiply — its per-step `divmod` quotients are typically tiny, so
    /// when the values differ in magnitude the answer falls out in one or two `O(n)` steps. The size probe
    /// is `etude_bigint::Big::byte_len` (O(1)).
    fn cmp(&self, other: &Rational) -> Ordering {
        // Strictly-different signs decide immediately (zero counts as non-negative, so `0 vs positive`
        // and `0 vs 0` fall through to the exact paths below — both handle them correctly).
        match (self.num.is_negative(), other.num.is_negative()) {
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
        // Equal denominators (both strictly positive): `a/b ? c/b` is just `a ? c`.
        if self.den == other.den {
            return self.num.cmp(&other.num);
        }
        // Native i128 path when all components fit i64 (the common small case): `a/b ? c/d` ⟺ `a*d ? c*b`,
        // and both products fit i128 — a direct integer compare, no Big multiply and no size probe.
        if let Some(ord) = self.cmp_small(other) {
            return ord;
        }
        // Small components: the cross-multiply is two cheap multiplies and beats the continued-fraction
        // bookkeeping (measured crossover between the 256b and 1024b tiers).
        if self.is_cmp_small() && other.is_cmp_small() {
            return self.num.mul(&other.den).cmp(&other.num.mul(&self.den));
        }
        // Large components: continued-fraction magnitude comparison. Signs are equal here (differing
        // signs returned above), so compare magnitudes and flip the result for two negatives. Denominators
        // are canonical (strictly positive), so only the numerators can need `abs` — and only when both
        // operands are negative. In the common non-negative case we borrow the components directly, so the
        // first Euclidean step allocates nothing (it usually decides on the integer parts alone).
        if self.num.is_negative() {
            let (na, nc) = (self.num.abs(), other.num.abs());
            cmp_magnitude(&na, &self.den, &nc, &other.den).reverse()
        } else {
            cmp_magnitude(&self.num, &self.den, &other.num, &other.den)
        }
    }
}

/// A component width (in significant magnitude bytes) at or below which a cross-multiply comparison beats
/// the continued-fraction method. Measured: cross-multiply wins the 64b/256b tiers, loses at 1024b.
const CMP_SMALL_BYTES: usize = 64;

impl Rational {
    /// Whether both components are small enough (by `Big::byte_len`, an O(1) probe) that a cross-multiply
    /// comparison is cheaper than the continued-fraction method.
    fn is_cmp_small(&self) -> bool {
        self.num.byte_len() <= CMP_SMALL_BYTES && self.den.byte_len() <= CMP_SMALL_BYTES
    }

    /// Native i128 comparison when every component fits i64: `a/b ? c/d` ⟺ `a*d ? c*b` (both `> 0`
    /// denominators). `a*d` and `c*b` fit i128 (`|·| <= 2^126`), so this is an exact integer compare with
    /// no `Big` multiply or allocation. Returns `None` (fall back to the `Big`/CF path) when any component
    /// exceeds i64.
    fn cmp_small(&self, other: &Rational) -> Option<Ordering> {
        let a = self.num.to_i64_checked()? as i128;
        let b = self.den.to_i64_checked()? as i128;
        let c = other.num.to_i64_checked()? as i128;
        let d = other.den.to_i64_checked()? as i128;
        Some((a * d).cmp(&(c * b)))
    }
}

/// Compare `a/b` vs `c/d` for non-negative `a`, `c` and strictly positive `b`, `d`, by the
/// continued-fraction method. Compares integer parts `⌊a/b⌋` vs `⌊c/d⌋`; on a tie compare the fractional
/// remainders `r1/b` vs `r2/d`, which — being in `[0, 1)` — reverse order under reciprocation, so the next
/// step compares `b/r1` vs `d/r2` with the running result negated.
///
/// The first Euclidean step takes the components by reference so it allocates nothing beyond the `divmod`
/// results — and for operands that differ in integer part (the common case) it decides immediately, never
/// touching the owned loop. Only a same-integer-part tie with fractional remainders on both sides recurses,
/// and only then are the (owned) denominators needed as the next step's numerators.
fn cmp_magnitude(a: &Big, b: &Big, c: &Big, d: &Big) -> Ordering {
    let (q1, r1) = a.divmod(b).expect("b > 0");
    let (q2, r2) = c.divmod(d).expect("d > 0");
    let qc = q1.cmp(&q2);
    if qc != Ordering::Equal {
        return qc; // top level: reverse == false
    }
    match (r1.is_zero(), r2.is_zero()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less, // a/b is an exact integer, c/d has a fraction ⇒ a/b < c/d
        (false, true) => Ordering::Greater, // a/b has a fraction, c/d is an exact integer ⇒ a/b > c/d
        // Recurse on reciprocals b/r1 vs d/r2 (order reverses): the old denominators become the new
        // numerators (clone them once — this is the only allocation of an operand), remainders the new
        // denominators.
        (false, false) => cmp_magnitude_owned(b.clone(), r1, d.clone(), r2, true),
    }
}

/// The owned continuation of [`cmp_magnitude`] once it has recursed at least once (so every value is
/// already an owned `divmod` result or a one-time denominator clone). Iterative to reuse the buffers.
fn cmp_magnitude_owned(
    mut a: Big,
    mut b: Big,
    mut c: Big,
    mut d: Big,
    mut reverse: bool,
) -> Ordering {
    loop {
        let (q1, r1) = a.divmod(&b).expect("b > 0");
        let (q2, r2) = c.divmod(&d).expect("d > 0");
        let qc = q1.cmp(&q2);
        if qc != Ordering::Equal {
            return if reverse { qc.reverse() } else { qc };
        }
        let frac = match (r1.is_zero(), r2.is_zero()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => {
                a = b;
                b = r1;
                c = d;
                d = r2;
                reverse = !reverse;
                continue;
            }
        };
        return if reverse { frac.reverse() } else { frac };
    }
}

/// Normalize a raw `num/den` pair into canonical form: strictly-positive denominator (sign moved to the
/// numerator), reduced to lowest terms by `gcd`, canonical `0/1` for zero. Returns `None` when `den == 0`.
fn normalize(mut num: Big, mut den: Big) -> Option<Rational> {
    if den.is_zero() {
        return None;
    }
    if num.is_zero() {
        return Some(Rational::zero());
    }
    // Move the sign onto the numerator so the denominator is strictly positive.
    if den.is_negative() {
        num = num.neg();
        den = den.neg();
    }
    // Reduce by gcd(|num|, den). `Big::gcd` is sign-agnostic and non-negative; den is positive here, so
    // the gcd is a positive divisor of both magnitudes — each exact division has remainder zero.
    let g = num.gcd(&den);
    if g.bit_len() == 1 {
        // g == 1 (an O(1), allocation-free check): already coprime — skip the two divisions (the common
        // case for freshly built pairs).
        return Some(Rational { num, den });
    }
    // `div_exact` returns just the quotient — no discarded-remainder allocation (the gcd divides both).
    let num_reduced = num.div_exact(&g).expect("gcd is nonzero");
    let den_reduced = den.div_exact(&g).expect("gcd is nonzero");
    Some(Rational {
        num: num_reduced,
        den: den_reduced,
    })
}

/// Native binary-free Euclidean gcd of two `u128`s. `gcd(x, 0) = x`.
fn gcd_u128(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// Box an `i128` as a `Big`: `from_i64` when it fits (the usual case), else via the canonical
/// sign-magnitude byte encoding (17 bytes holds any `i128`).
fn big_from_i128(v: i128) -> Big {
    if let Ok(v64) = i64::try_from(v) {
        Big::from_i64(v64)
    } else {
        let mut buf = [0u8; 17]; // 1 sign byte + up to 16 magnitude bytes
        let n =
            Big::i128_to_sign_magnitude_bytes_into(v, &mut buf).expect("17 bytes holds any i128");
        Big::from_sign_magnitude_bytes(&buf[..n])
    }
}

/// `n / g` where `g` is a known divisor of `n`. Skips the division entirely when `g == 1` (an O(1),
/// allocation-free `bit_len() == 1` check — `g` is a non-negative gcd, so `bit_len() == 1` ⟺ `g == 1`);
/// otherwise uses `Big::div_exact` (quotient only — no discarded-remainder allocation).
fn reduce_by(n: &Big, g: &Big) -> Big {
    if g.bit_len() == 1 {
        n.clone()
    } else {
        n.div_exact(g).expect("g is a nonzero divisor of n")
    }
}

/// Cross-reduced multiply of two canonical fractions `a/b` and `c/d` (`gcd(a,b) = gcd(c,d) = 1`, and both
/// nonzero). Cancels `g1 = gcd(a,d)` and `g2 = gcd(c,b)` before multiplying, returning `((a/g1)*(c/g2),
/// (b/g2)*(d/g1))` — which is already in lowest terms (the four cross-pairs are pairwise coprime), so no
/// further gcd-normalize is needed. `gcd` is sign-agnostic, so the quotients keep their operands' signs;
/// the caller owns any final sign placement.
fn cross_reduce_mul(a: &Big, b: &Big, c: &Big, d: &Big) -> (Big, Big) {
    let g1 = a.gcd(d); // gcd(|a|, |d|)
    let g2 = c.gcd(b); // gcd(|c|, |b|)
    let num = reduce_by(a, &g1).mul(&reduce_by(c, &g2));
    let den = reduce_by(b, &g2).mul(&reduce_by(d, &g1));
    (num, den)
}

#[cfg(test)]
mod tests;
