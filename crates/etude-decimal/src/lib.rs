// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Exact base-10 arbitrary-precision decimal numbers — a `coeff * 10^exp` value over
//! [`etude_bigint::Big`]. Pure over `alloc`, no I/O, no dependency but `etude-bigint`. A decimal
//! preserves every significant digit and its scale exactly, unlike an `f64` (which loses precision) or a
//! rational (which would need gcd reduction and cannot distinguish `0.1` from `0.10` by scale). Exact
//! arithmetic ([`Decimal::add`]/[`Decimal::sub`]/[`Decimal::mul`]) never rounds a digit away; division is
//! split by that principle — [`Decimal::div`] is exact and returns `None` when the quotient does not
//! terminate, while [`Decimal::div_round`] rounds to a caller-chosen precision and [`RoundingMode`] (a
//! rounding-capable operation always takes explicit rounding arguments — there is no default). Correctness
//! is pinned by a differential test against `bigdecimal` (a dev-dependency) as the reference.
//!
//! A value is built either numerically from a [`etude_bigint::Big`] coefficient and an exponent via
//! [`Decimal::new`] (and the [`Decimal::from_i64`] / [`Decimal::from_bigint`] conveniences), or parsed
//! from a decimal number literal via [`Decimal::parse`] / [`Decimal::from_str`]. Parsing owns exactly the
//! decimal-number-literal grammar and consumes any `Iterator<Item = u8>`, so a rope- or chunk-backed
//! (non-contiguous) byte source is parsed in place with no flattening; [`Decimal::parse_prefix`] parses a
//! number embedded in a larger byte stream (a text decoder such as a JSON tokenizer hands its chunk
//! cursor straight in — this crate parses the *number*, the decoder owns the surrounding structure).
//!
//! # Representation and the canonical-form invariant
//! A [`Decimal`] is the exact value `coeff * 10^exp`, where `coeff` is an [`etude_bigint::Big`] signed
//! integer (it carries the sign of the whole value) and `exp` is a base-10 point shift. It is kept in a
//! single canonical form:
//! - the coefficient has no trailing zero digit (`coeff % 10 != 0`), with `exp` raised to compensate, so
//!   `1.0`, `1`, and `1.00` all become the one value `coeff = 1, exp = 0`, and `100` becomes
//!   `coeff = 1, exp = 2`;
//! - zero is exactly `coeff = 0, exp = 0` (there is no `0 * 10^5`).
//!
//! Every constructor and operation renormalizes, so a value has exactly one in-memory form. This is
//! required for `Eq`/`Ord` to mean numeric equality: `0.1` and `0.10` and `1e-1` are the same value and
//! must have identical fields.
//!
//! The fields are private and not part of the stable API — the coefficient repr and the exponent width
//! may change. Construct through [`Decimal::zero`], [`Decimal::from_i64`], [`Decimal::from_bigint`],
//! [`Decimal::new`], or by parsing via [`Decimal::parse`]/[`Decimal::parse_prefix`]/[`Decimal::from_str`];
//! inspect through
//! [`Decimal::coefficient`], [`Decimal::exponent`], [`Decimal::is_zero`], [`Decimal::is_negative`],
//! [`Decimal::is_integer`], [`Decimal::to_f64`], the [`Ord`]/[`PartialOrd`] comparison, and `Display` /
//! [`Decimal::write_to`] (the allocation-conscious rendering path — write straight into a
//! [`core::fmt::Write`] sink rather than building an intermediate `String`).

#![cfg_attr(not(test), no_std)]
#![deny(missing_docs)]

extern crate alloc;

use core::cmp::Ordering;
use core::str::FromStr;
use etude_bigint::Big;

/// An exact base-10 decimal number `coeff * 10^exp` in canonical form. See the module doc for the
/// invariant.
///
/// The internal `{ coeff, exp }` representation is private and not part of the stable API. Construct
/// values through the constructors and inspect them through the accessors.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Decimal {
    /// The signed coefficient (significand); carries the sign of the whole value. Canonical: not
    /// divisible by 10 unless it is zero.
    coeff: Big,
    /// The base-10 exponent: the value is `coeff * 10^exp`. Canonical zero has `exp == 0`. It is an
    /// `i64` (not `i32`) so that `mul` — which adds the two operands' exponents — has ample headroom and
    /// so the width matches the reference `bigdecimal`'s `i64` scale.
    exp: i64,
}

/// How [`Decimal::div_round`] (and any rounding-capable operation) breaks a tie / discards a remainder.
/// Mirrors the IEEE-754 / `bigdecimal` rounding modes. There is deliberately no default — a
/// rounding-capable operation takes the mode as an explicit argument so the caller always chooses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RoundingMode {
    /// Away from zero (round the magnitude up whenever anything is discarded).
    Up,
    /// Toward zero (truncate the discarded digits).
    Down,
    /// Toward positive infinity.
    Ceiling,
    /// Toward negative infinity.
    Floor,
    /// Nearest; a tie (exactly halfway) rounds away from zero.
    HalfUp,
    /// Nearest; a tie rounds toward zero.
    HalfDown,
    /// Nearest; a tie rounds to make the last kept digit even (banker's rounding).
    HalfEven,
}

impl Decimal {
    /// The canonical zero, `0 * 10^0`.
    pub fn zero() -> Decimal {
        Decimal {
            coeff: Big::zero(),
            exp: 0,
        }
    }

    /// The canonical one, `1 * 10^0`.
    pub fn one() -> Decimal {
        Decimal {
            coeff: Big::from_i64(1),
            exp: 0,
        }
    }

    /// Box a signed 64-bit int (exp 0), canonicalized (e.g. `100` becomes `coeff = 1, exp = 2`).
    pub fn from_i64(v: i64) -> Decimal {
        Decimal::new(Big::from_i64(v), 0)
    }

    /// The big integer `n` as a decimal (exp 0), canonicalized.
    pub fn from_bigint(n: Big) -> Decimal {
        Decimal::new(n, 0)
    }

    /// Construct `coeff * 10^exp` and canonicalize it (strip trailing zero digits from the coefficient,
    /// raising `exp`; collapse a zero coefficient to the canonical zero). Total — every `(coeff, exp)`
    /// names a representable value.
    pub fn new(coeff: Big, exp: i64) -> Decimal {
        let mut d = Decimal { coeff, exp };
        d.normalize();
        d
    }

    /// Strip trailing zero digits from the coefficient (raising `exp` to keep the value), and collapse a
    /// zero coefficient to `exp == 0`. Stops early rather than overflowing `exp` (a pathological input
    /// with a coefficient of `~2^63` trailing zeros stays merely un-fully-stripped, never wrong).
    ///
    /// The hot path is the reject: a coefficient not divisible by ten is already canonical, and
    /// divisibility by ten requires an even coefficient, so [`etude_bigint::Big::is_odd`] (`O(1)`) returns
    /// half of all results with no work at all. A coefficient that fits `i128` — the common small decimal
    /// (a price/measurement like `1.50`) — then strips its trailing zeros entirely in native `u128`
    /// arithmetic, with no `Big` remainder or divide. Only a wider coefficient falls to the `Big` path:
    /// it tests the last decimal digit with [`etude_bigint::Big::last_decimal_digit`] (a limb-sum reduction,
    /// `2^64 ≡ 6 mod 10`, several times cheaper than a full reciprocal remainder), and only one that ends in
    /// zero peeks the low nine digits (a single allocation-free [`etude_bigint::Big::rem_u64`]`(10^9)`) to
    /// size a single [`etude_bigint::Big::divmod_u64`]`(10^tz)`; a run of nine or more zeros strips whole
    /// `10^9` chunks in a loop, so trailing-zero removal is `O(zeros / 9)` divides.
    fn normalize(&mut self) {
        if self.coeff.is_zero() {
            self.exp = 0;
            return;
        }
        const CHUNK_DIGITS: i64 = 9;
        const CHUNK: u64 = 1_000_000_000; // 10^9, a single-limb divisor
        loop {
            // Not divisible by 2 ⇒ not by 10 ⇒ already canonical. O(1), no division.
            if self.coeff.is_odd() {
                return;
            }
            // Native fast path for a small coefficient: one that fits `i128` strips its trailing zeros with
            // native `u128` divides — no `Big` remainder or divide at all. This is the common small decimal
            // (a price or measurement such as `1.50`, parsed to `150` and stripped to `15`), and mirrors
            // `etude-rational`'s small-value normalize (#222). A wider coefficient falls through to the
            // chunked `Big` strip below.
            if let Some(v) = self.coeff.to_i128_checked() {
                if !v.unsigned_abs().is_multiple_of(10) {
                    return; // even but not a multiple of ten → already canonical
                }
                let neg = v < 0;
                let mut m = v.unsigned_abs();
                let mut e = self.exp;
                while m.is_multiple_of(10) && e < i64::MAX {
                    m /= 10;
                    e += 1;
                }
                self.coeff = Big::from_i128(if neg { -(m as i128) } else { m as i128 });
                self.exp = e;
                return;
            }
            // Divisibility by ten from the last decimal digit alone — a cheap limb-sum reduction, not the
            // O(n) reciprocal remainder the strip needs. An even coefficient not ending in zero (the common
            // case among evens) returns here without that heavier peek.
            if self.coeff.last_decimal_digit() != 0 {
                return; // last digit nonzero → already canonical
            }
            if self.exp > i64::MAX - CHUNK_DIGITS {
                return; // refuse to overflow exp; leaving it un-fully-stripped is still correct
            }
            // Ends in zero, so it strips: now peek the low nine digits (a single allocation-free remainder,
            // paid only on the fraction of results that reach here) to size a single divide.
            let low = self.coeff.rem_u64(CHUNK).expect("divisor 10^9 is nonzero");
            if low == 0 {
                // All nine low digits are zero — strip the whole chunk and continue.
                self.coeff = self
                    .coeff
                    .divmod_u64(CHUNK)
                    .expect("divisor 10^9 is nonzero")
                    .0;
                self.exp += CHUNK_DIGITS;
                continue;
            }
            // `low` is nonzero and ends in zero, so the coefficient has exactly `tz ∈ 1..=8` trailing
            // zeros; strip them with a single divide (`10^tz` fits a `u64`) and stop.
            let mut v = low;
            let mut tz = 0u32;
            while v.is_multiple_of(10) {
                v /= 10;
                tz += 1;
            }
            self.coeff = self
                .coeff
                .divmod_u64(10u64.pow(tz))
                .expect("power of ten is nonzero")
                .0;
            self.exp += i64::from(tz); // tz ≤ 8, and exp ≤ i64::MAX - 9 was guarded above
            return;
        }
    }

    /// The signed coefficient (significand). Canonical: not divisible by 10 unless zero.
    pub fn coefficient(&self) -> &Big {
        &self.coeff
    }

    /// The base-10 exponent: the value is `coefficient() * 10^exponent()`.
    pub fn exponent(&self) -> i64 {
        self.exp
    }

    /// Whether the value is exactly zero.
    pub fn is_zero(&self) -> bool {
        self.coeff.is_zero()
    }

    /// Whether the value is strictly negative.
    pub fn is_negative(&self) -> bool {
        self.coeff.is_negative()
    }

    /// Whether the value is an exact integer. Canonical form makes this a pure exponent test: a nonzero
    /// coefficient is not divisible by 10, so the value has a fractional part exactly when `exp < 0`.
    pub fn is_integer(&self) -> bool {
        self.coeff.is_zero() || self.exp >= 0
    }

    /// This value as an `i64`, or `None` unless it is an exact integer that fits `i64`. `1.5` and `1e40`
    /// return `None`; `42`, `4.2e1`, and `100` return `Some`.
    pub fn to_i64(&self) -> Option<i64> {
        self.to_integer()?.to_i64_checked()
    }

    /// This value as an `i128`, or `None` unless it is an exact integer that fits `i128`.
    pub fn to_i128(&self) -> Option<i128> {
        self.to_integer()?.to_i128_checked()
    }

    /// This value as a `u64`, or `None` unless it is an exact integer in `0..=u64::MAX` (a negative value
    /// returns `None`).
    pub fn to_u64(&self) -> Option<u64> {
        let v = self.to_integer()?.to_i128_checked()?;
        (0..=u64::MAX as i128).contains(&v).then_some(v as u64)
    }

    /// The exact integer value as a [`Big`] when this decimal is an integer (`exp >= 0`), else `None`.
    /// Guards against materializing an oversized power of ten: a value with more decimal digits than the
    /// widest integer target (`i128` = 39 digits) cannot fit any of them, so it is rejected before the
    /// scaling multiply. The remaining fit check is left to the caller's checked conversion.
    fn to_integer(&self) -> Option<Big> {
        if self.exp < 0 {
            return None; // canonical: a nonzero coefficient with exp < 0 has a fractional part
        }
        if self.coeff.is_zero() {
            return Some(Big::zero());
        }
        if self.exp == 0 {
            return Some(self.coeff.clone());
        }
        let digits = self
            .coeff
            .decimal_digit_count()
            .saturating_add(self.exp as u64);
        if digits > 39 {
            return None; // wider than i128 — no integer target can hold it
        }
        Some(self.coeff.mul(&pow10(self.exp as u64)))
    }

    /// Negate the value (`-self`). Preserves canonical form (a sign flip cannot create a trailing zero).
    pub fn neg(&self) -> Decimal {
        Decimal {
            coeff: self.coeff.neg(),
            exp: self.exp,
        }
    }

    /// The absolute value (`|self|`). Preserves canonical form.
    pub fn abs(&self) -> Decimal {
        Decimal {
            coeff: self.coeff.abs(),
            exp: self.exp,
        }
    }

    /// Exact sum `self + other`. Aligns the exponents to the smaller of the two — scaling the
    /// larger-exponent operand's coefficient by the matching power of ten — adds the coefficients, and
    /// canonicalizes. Exact: no digit is ever rounded away.
    pub fn add(&self, other: &Decimal) -> Decimal {
        if self.is_zero() {
            return other.clone();
        }
        if other.is_zero() {
            return self.clone();
        }
        if let Some(d) = self.combine_small(other, i64::checked_add) {
            return d;
        }
        if let Some(d) = self.combine_wide(other, i128::checked_add) {
            return d;
        }
        self.combine(other, Big::add)
    }

    /// Exact difference `self - other`. Subtracts the coefficients directly (after aligning exponents),
    /// rather than adding a negated clone of `other`, so no intermediate negated value is allocated.
    pub fn sub(&self, other: &Decimal) -> Decimal {
        if self.is_zero() {
            return other.neg(); // 0 - other = -other
        }
        if other.is_zero() {
            return self.clone();
        }
        if let Some(d) = self.combine_small(other, i64::checked_sub) {
            return d;
        }
        if let Some(d) = self.combine_wide(other, i128::checked_sub) {
            return d;
        }
        self.combine(other, Big::sub)
    }

    /// Native fast path for [`Decimal::add`] / [`Decimal::sub`] on small values: when both coefficients
    /// fit an `i64` and aligning them to the smaller exponent (scaling by a power of ten) and combining
    /// with `op` (`checked_add` / `checked_sub`) all stay within `i64`, do the arithmetic natively — no
    /// `Big` power-of-ten, scale, or intermediate is allocated. Returns `None` on any overflow (including
    /// an exponent gap past 10^18), so the caller falls back to the exact `Big` path.
    fn combine_small(
        &self,
        other: &Decimal,
        op: impl Fn(i64, i64) -> Option<i64>,
    ) -> Option<Decimal> {
        let ca = self.coeff.to_i64_checked()?;
        let cb = other.coeff.to_i64_checked()?;
        let e = self.exp.min(other.exp);
        let sa = ca.checked_mul(pow10_i64((self.exp - e) as u32)?)?;
        let sb = cb.checked_mul(pow10_i64((other.exp - e) as u32)?)?;
        let r = op(sa, sb)?;
        Some(Decimal::new(Big::from_i64(r), e))
    }

    /// Wider native tier for [`Decimal::add`] / [`Decimal::sub`], between the `i64` fast path and the
    /// exact `Big` path: when the coefficients exceed `i64` but fit `i128` (a full 64-bit magnitude, say),
    /// align and combine in `i128`. Reads each coefficient's limbs straight into an `i128`
    /// ([`etude_bigint::Big::to_i128_checked`], no `Vec` allocated to inspect it) and boxes only the result
    /// ([`etude_bigint::Big::from_i128`]) — the direct limb conversions, not a sign-magnitude byte
    /// round-trip (which cost more than [`etude_bigint::Big::add`] itself). Returns `None` on any overflow
    /// (a coefficient past 127 bits, an exponent gap past `10^38`, or an `i128` combine overflow), so the
    /// caller falls back to the exact `Big` path.
    fn combine_wide(
        &self,
        other: &Decimal,
        op: impl Fn(i128, i128) -> Option<i128>,
    ) -> Option<Decimal> {
        let ca = self.coeff.to_i128_checked()?;
        let cb = other.coeff.to_i128_checked()?;
        let e = self.exp.min(other.exp);
        let sa = ca.checked_mul(pow10_i128((self.exp - e) as u32)?)?;
        let sb = cb.checked_mul(pow10_i128((other.exp - e) as u32)?)?;
        let r = op(sa, sb)?;
        Some(Decimal::new(Big::from_i128(r), e))
    }

    /// Align two nonzero operands to the smaller exponent — scaling the larger-exponent coefficient by
    /// the matching power of ten — combine their coefficients with `op` (`Big::add` or `Big::sub`), and
    /// canonicalize. Shared by [`Decimal::add`] and [`Decimal::sub`].
    fn combine(&self, other: &Decimal, op: impl Fn(&Big, &Big) -> Big) -> Decimal {
        if self.exp == other.exp {
            // Already aligned — the common fast path (e.g. equal-scale sums).
            return Decimal::new(op(&self.coeff, &other.coeff), self.exp);
        }
        let e = self.exp.min(other.exp);
        let a = scale_pow10(&self.coeff, (self.exp - e) as u64);
        let b = scale_pow10(&other.coeff, (other.exp - e) as u64);
        Decimal::new(op(&a, &b), e)
    }

    /// Exact product `self * other`: multiply the coefficients and add the exponents. Exact — a decimal
    /// product is always representable (unlike a quotient).
    pub fn mul(&self, other: &Decimal) -> Decimal {
        if self.is_zero() || other.is_zero() {
            return Decimal::zero();
        }
        // Native fast path: when both coefficients and their product fit an i64, multiply natively and
        // skip the Big multiply/allocation. Falls through on overflow.
        if let (Some(ca), Some(cb)) = (self.coeff.to_i64_checked(), other.coeff.to_i64_checked())
            && let (Some(coeff), Some(exp)) = (ca.checked_mul(cb), self.exp.checked_add(other.exp))
        {
            return Decimal::new(Big::from_i64(coeff), exp);
        }
        // Wider native tier: the product of two `i64`-fitting coefficients always fits `i128`, and a full
        // 64-bit coefficient fits `i128` (its product may too). Read the limbs straight into `i128` and box
        // only the result — no `Big` multiply or allocation — falling through on `i128` overflow.
        if let (Some(ca), Some(cb)) = (self.coeff.to_i128_checked(), other.coeff.to_i128_checked())
            && let Some(exp) = self.exp.checked_add(other.exp)
        {
            if let Some(coeff) = ca.checked_mul(cb) {
                return Decimal::new(Big::from_i128(coeff), exp);
            }
            // The signed product overflowed `i128`, but the magnitude may still fit `u128` — the top of the
            // `64b × 64b` range (both coefficients below `2^64`, so the product is below `2^128`). Multiply
            // the magnitudes in `u128` and box the result with its sign; only a truly wider product (a
            // coefficient past `2^64`) overflows here and falls to the exact `Big` multiply.
            if let Some(mag) = ca.unsigned_abs().checked_mul(cb.unsigned_abs()) {
                let neg = ca.is_negative() ^ cb.is_negative();
                return Decimal::new(big_from_u128(mag, neg), exp);
            }
        }
        // Exponents come from parsing bounded to ≤18 digits, so their sum fits i64 for any realistic
        // input; saturate only in the astronomically-extreme case rather than wrap.
        let exp = self.exp.saturating_add(other.exp);
        Decimal::new(self.coeff.mul(&other.coeff), exp)
    }

    /// exact division `self / other`, or `None` if the quotient does not terminate as a finite decimal
    /// (or `other` is zero). A decimal quotient is exact exactly when the divisor's coefficient, reduced
    /// against the dividend's, has no prime factor other than 2 and 5 (`1/2`, `1/8`, `3/40` terminate;
    /// `1/3`, `1/7` do not). Because it cannot round, this operation takes no rounding arguments — use
    /// [`Decimal::div_round`] for a rounded quotient to a chosen precision.
    pub fn div(&self, other: &Decimal) -> Option<Decimal> {
        if other.is_zero() {
            return None;
        }
        if self.is_zero() {
            return Some(Decimal::zero());
        }
        // Native fast path: when both coefficients fit `i128`, reduce and factor-strip in native `u128`
        // (gcd, trailing-zero 2-strip, 5-strip) rather than a chain of `Big` divmods. Falls back only when
        // the terminating quotient's coefficient overflows native width.
        if let (Some(n), Some(d)) = (self.coeff.to_i128_checked(), other.coeff.to_i128_checked()) {
            match div_exact_i128(n, self.exp, d, other.exp) {
                ExactDivI128::Terminates(dec) => return Some(dec),
                ExactDivI128::NonTerminating => return None,
                ExactDivI128::TooWide => {} // fall through to the exact Big path
            }
        }
        let neg = self.coeff.is_negative() ^ other.coeff.is_negative();
        // Reduce |num| / |den| to lowest terms, then strip all 2s and 5s from the denominator.
        let g = self.coeff.gcd(&other.coeff); // gcd ignores sign (magnitude gcd)
        let num = self.coeff.abs().divmod(&g).expect("gcd is nonzero").0;
        let mut den = other.coeff.abs().divmod(&g).expect("gcd is nonzero").0;
        let two = Big::from_i64(2);
        let five = Big::from_i64(5);
        let mut a2: u32 = 0;
        let mut a5: u32 = 0;
        loop {
            let (q, r) = den.divmod(&two).expect("2 is nonzero");
            if !r.is_zero() {
                break;
            }
            den = q;
            a2 += 1;
        }
        loop {
            let (q, r) = den.divmod(&five).expect("5 is nonzero");
            if !r.is_zero() {
                break;
            }
            den = q;
            a5 += 1;
        }
        if den != Big::from_i64(1) {
            return None; // a factor other than 2 or 5 remains → non-terminating
        }
        // 1/(2^a2·5^a5) = (2^(m-a2)·5^(m-a5)) / 10^m, m = max(a2,a5); exactly one of the two powers is >1.
        let m = a2.max(a5);
        let extra = if a2 >= a5 {
            pow5(a2 - a5)
        } else {
            pow2(a5 - a2)
        };
        let coeff = num.mul(&extra);
        let exp = (self.exp - other.exp) - m as i64;
        let coeff = if neg { coeff.neg() } else { coeff };
        Some(Decimal::new(coeff, exp))
    }

    /// Divide `self / other`, rounding the quotient to `precision` significant digits with the explicit
    /// [`RoundingMode`] (there is no default rounding — the caller always chooses). Returns `None` if
    /// `other` is zero or `precision` is zero. Unlike [`Decimal::div`] this always yields a value, at the
    /// cost of rounding a non-terminating (or over-long) quotient.
    pub fn div_round(
        &self,
        other: &Decimal,
        precision: u32,
        rounding: RoundingMode,
    ) -> Option<Decimal> {
        if other.is_zero() || precision == 0 {
            return None;
        }
        if self.is_zero() {
            return Some(Decimal::zero());
        }
        let neg = self.coeff.is_negative() ^ other.coeff.is_negative();
        let n = self.coeff.abs();
        let d = other.coeff.abs();
        let digits_n = n.to_decimal_string().len() as i64;
        let digits_d = d.to_decimal_string().len() as i64;
        // Choose k so that floor(n * 10^k / d) has `precision` digits, then round with the remainder.
        // Start near the answer and correct by ±1 (the count is monotonic in k, so this converges).
        let mut k = precision as i64 - 1 - (digits_n - digits_d);
        loop {
            let (num, den) = if k >= 0 {
                (n.mul(&pow10(k as u64)), d.clone())
            } else {
                (n.clone(), d.mul(&pow10((-k) as u64)))
            };
            let (q, r) = num.divmod(&den).expect("denominator is nonzero");
            let qd = if q.is_zero() {
                0
            } else {
                q.to_decimal_string().len() as i64
            };
            if qd < precision as i64 {
                k += 1;
                continue;
            }
            if qd > precision as i64 {
                k -= 1;
                continue;
            }
            let coeff = if round_up_magnitude(&q, &r, &den, neg, rounding) {
                q.add(&Big::from_i64(1))
            } else {
                q
            };
            let exp = (self.exp - other.exp) - k;
            let coeff = if neg { coeff.neg() } else { coeff };
            return Some(Decimal::new(coeff, exp));
        }
    }

    /// Parse a decimal number literal from a byte stream into an exact `Decimal`, or `None` if the whole
    /// stream is not a well-formed literal. The stream need not be contiguous — any `IntoIterator<Item =
    /// u8>` works, so a rope- or chunk-backed byte source is parsed in place with no flattening. The
    /// grammar is
    ///
    /// ```text
    /// -? ( 0 | [1-9][0-9]* ) ( . [0-9]+ )? ( [eE] [+-]? [0-9]+ )?
    /// ```
    ///
    /// so a leading `+`, a redundant leading zero (`01`), a bare `.5`, a trailing `1.`, a lone `-`, an
    /// empty exponent (`1e`), and any surrounding whitespace or trailing bytes are all rejected. The
    /// decode is lossless. An exponent of more than 18 digits (beyond `i64`) is rejected rather than
    /// silently wrapping. To parse a number embedded in a larger stream — stopping at the first byte that
    /// is not part of the number — use [`Decimal::parse_prefix`].
    pub fn parse<I: IntoIterator<Item = u8>>(bytes: I) -> Option<Decimal> {
        let mut it = bytes.into_iter().peekable();
        let d = Decimal::parse_prefix(&mut it)?;
        if it.peek().is_some() {
            return None; // trailing bytes after the number literal
        }
        Some(d)
    }

    /// Parse the maximal decimal-number-literal prefix from a peekable byte iterator, stopping at (and not
    /// consuming) the first byte that is not part of the number — leaving the iterator positioned right
    /// after the literal. This is the entry point for a decoder embedding a number in a larger chunked
    /// byte stream (e.g. a JSON tokenizer): it advances the shared cursor across chunk boundaries with no
    /// flattening. Returns `None` if no well-formed number literal starts at the cursor. Grammar and
    /// losslessness are as for [`Decimal::parse`].
    pub fn parse_prefix<I: Iterator<Item = u8>>(
        it: &mut core::iter::Peekable<I>,
    ) -> Option<Decimal> {
        // Optional leading minus (a leading plus is not accepted).
        let neg = if it.peek() == Some(&b'-') {
            it.next();
            true
        } else {
            false
        };

        let mut coeff = CoeffBuilder::new();

        // Integer part: "0" alone, or [1-9] followed by any digits. No redundant leading zeros.
        match it.peek() {
            Some(&b'0') => {
                it.next();
                coeff.push(b'0');
                // A leading "0" may not be followed by another digit ("00", "01").
                if matches!(it.peek(), Some(c) if c.is_ascii_digit()) {
                    return None;
                }
            }
            Some(&c) if (b'1'..=b'9').contains(&c) => {
                it.next();
                coeff.push(c);
                while let Some(&c) = it.peek() {
                    if !c.is_ascii_digit() {
                        break;
                    }
                    it.next();
                    coeff.push(c);
                }
            }
            _ => return None, // an integer digit is required
        }

        // Optional fractional part: a dot followed by at least one digit.
        let mut frac_len: i64 = 0;
        if it.peek() == Some(&b'.') {
            it.next();
            while let Some(&c) = it.peek() {
                if !c.is_ascii_digit() {
                    break;
                }
                it.next();
                coeff.push(c);
                frac_len += 1;
            }
            if frac_len == 0 {
                return None; // a dot must be followed by at least one digit
            }
        }

        // Optional exponent: e/E, optional sign, at least one digit.
        let mut exp_val: i64 = 0;
        if matches!(it.peek(), Some(&b'e') | Some(&b'E')) {
            it.next();
            let exp_neg = match it.peek() {
                Some(&b'+') => {
                    it.next();
                    false
                }
                Some(&b'-') => {
                    it.next();
                    true
                }
                _ => false,
            };
            let mut exp_digits = 0u32;
            while let Some(&c) = it.peek() {
                if !c.is_ascii_digit() {
                    break;
                }
                // More than 18 exponent digits cannot be reasoned about in i64; reject rather than wrap.
                if exp_digits >= 18 {
                    return None;
                }
                it.next();
                exp_val = exp_val * 10 + (c - b'0') as i64;
                exp_digits += 1;
            }
            if exp_digits == 0 {
                return None; // an exponent marker must be followed by at least one digit
            }
            if exp_neg {
                exp_val = -exp_val;
            }
        }

        // Each fractional digit lowered the exponent by one; fold that in.
        let exp = exp_val.checked_sub(frac_len)?;
        let mag = coeff.finish();
        let coeff = if neg { mag.neg() } else { mag };
        Some(Decimal::new(coeff, exp))
    }

    /// Convert to the nearest `f64` — correctly rounded (round-to-nearest, ties-to-even), computed
    /// directly from the coefficient and exponent with no string round-trip. Magnitudes beyond `f64`
    /// range become `±∞`, and magnitudes below the smallest subnormal round to `±0.0`, matching
    /// IEEE-754 decimal-to-binary conversion.
    ///
    /// The value `|coeff| * 10^exp` is the exact rational `N / D` (`N, D > 0`); the nearest `f64` is
    /// found by locating the binary exponent `e = ⌊log2(N/D)⌋`, dividing to obtain the 53-bit mantissa
    /// with the remainder as the round/sticky information, and assembling the IEEE-754 bits. An initial
    /// `log2` estimate short-circuits over/underflow so no oversized power of ten is ever materialized.
    pub fn to_f64(&self) -> f64 {
        if self.coeff.is_zero() {
            return 0.0;
        }
        // Fast path: a coefficient that fits f64's 53-bit mantissa exactly, times an exactly-representable
        // power of ten (`10^0..=10^22` are exact f64s), converts with a single correctly-rounded IEEE
        // operation — the common case for values that originate as ordinary decimal literals. Both
        // operands are exact, so the one multiply (or divide) yields the correctly-rounded result,
        // identical to the exact big-int path below.
        if (-22..=22).contains(&self.exp)
            && let Some(c) = self.coeff.to_i64_checked()
            && c.unsigned_abs() < (1u64 << 53)
        {
            let cf = c as f64; // exact: |c| < 2^53
            let p = POW10_F64[self.exp.unsigned_abs() as usize]; // exact: 10^0..=10^22
            return if self.exp >= 0 { cf * p } else { cf / p };
        }
        let neg = self.coeff.is_negative();
        let sign = if neg { 1u64 << 63 } else { 0 };
        let inf = if neg {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };

        // Cheap magnitude estimate to short-circuit over/underflow without materializing a huge power of
        // ten: log2(value) ≈ (bit_len(|coeff|) - 1) + exp*log2(10). The estimate under-counts the true
        // log2 by < 1, so the generous margins never shortcut a value the exact path would keep finite.
        let m = self.coeff.abs();
        let approx_log2 = (m.bit_len() as f64 - 1.0) + self.exp as f64 * core::f64::consts::LOG2_10;
        if approx_log2 > 1025.0 {
            return inf;
        }
        if approx_log2 < -1080.0 {
            return f64::from_bits(sign); // ±0.0
        }

        // Exact value = |coeff| * 10^exp = N / D. Within the window above |exp| is bounded, so these
        // powers of ten stay small.
        let (n, d) = if self.exp >= 0 {
            (m.mul(&pow10(self.exp as u64)), Big::from_i64(1))
        } else {
            (m, pow10((-self.exp) as u64))
        };

        // e = ⌊log2(N/D)⌋. The value lies in (2^(bn-bd-1), 2^(bn-bd+1)), so refine from that candidate.
        let bn = n.bit_len() as i64;
        let bd = d.bit_len() as i64;
        // N/D >= 2^k  ⇔  (k>=0: N >= D*2^k) or (k<0: N*2^-k >= D).
        let ge_pow2 = |k: i64| -> bool {
            if k >= 0 {
                n.cmp(&d.mul(&pow2(k as u32))) != Ordering::Less
            } else {
                n.mul(&pow2((-k) as u32)).cmp(&d) != Ordering::Less
            }
        };
        let mut e = bn - bd - 1;
        while ge_pow2(e + 1) {
            e += 1;
        }
        while !ge_pow2(e) {
            e -= 1;
        }

        // Rounded mantissa M = round(value * 2^shift). For normals shift = 52 - e (M carries the implicit
        // leading bit at position 52); for subnormals the exponent is pinned to the minimum (2^-1074), so
        // shift = 1074 and M directly encodes the [exponent-field, fraction] bit pattern.
        let normal = e >= -1022;
        let shift: i64 = if normal { 52 - e } else { 1074 };
        let (num, den) = if shift >= 0 {
            (n.mul(&pow2(shift as u32)), d.clone())
        } else {
            (n.clone(), d.mul(&pow2((-shift) as u32)))
        };
        let (q, r) = num.divmod(&den).expect("denominator is nonzero");
        // Round half to even: compare 2*r to den; a tie rounds toward the even mantissa.
        let two = Big::from_i64(2);
        let q_odd = !q.divmod(&two).expect("2 is nonzero").1.is_zero();
        let round_up = match r.add(&r).cmp(&den) {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => q_odd,
        };
        let m_big = if round_up {
            q.add(&Big::from_i64(1))
        } else {
            q
        };
        // M fits u64: normals ≤ 2^53, subnormals ≤ 2^52.
        let mant = m_big.to_i64_checked().expect("mantissa fits i64") as u64;

        if normal {
            let mut e = e;
            let mut mant = mant;
            if mant == 1u64 << 53 {
                // Rounded up across a power of two: renormalize (mantissa 2^53 → 2^52, exponent +1).
                mant = 1u64 << 52;
                e += 1;
            }
            if e > 1023 {
                return inf;
            }
            let biased = (e + 1023) as u64; // e ∈ [-1022, 1023] ⇒ biased ∈ [1, 2046]
            let frac = mant - (1u64 << 52);
            f64::from_bits(sign | (biased << 52) | frac)
        } else {
            // Subnormal (or a round-up to the smallest normal): the low 63 bits of `mant` are exactly the
            // exponent+fraction pattern (M < 2^52 ⇒ subnormal; M == 2^52 ⇒ smallest normal).
            f64::from_bits(sign | mant)
        }
    }

    /// Exact three-way comparison, consistent with the numeric value. Public callers use the [`Ord`] /
    /// [`PartialOrd`] impls (the `<`/`cmp` operators); this is their shared implementation.
    fn compare(&self, other: &Decimal) -> Ordering {
        match (self.is_zero(), other.is_zero()) {
            (true, true) => Ordering::Equal,
            (true, false) => {
                if other.coeff.is_negative() {
                    Ordering::Greater
                } else {
                    Ordering::Less
                }
            }
            (false, true) => {
                if self.coeff.is_negative() {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            }
            (false, false) => {
                let sa = self.coeff.is_negative();
                let sb = other.coeff.is_negative();
                if sa != sb {
                    return if sa {
                        Ordering::Less
                    } else {
                        Ordering::Greater
                    };
                }
                let mag = self.cmp_magnitude(other);
                if sa { mag.reverse() } else { mag }
            }
        }
    }

    /// Compare `|self|` against `|other|`. The caller ([`Decimal::compare`]) only reaches here with both
    /// nonzero and of the same sign, which lets a signed [`etude_bigint::Big`] compare stand in for a
    /// magnitude compare (flipped for negatives) — no `abs()` clone. Exact and allocation-light.
    ///
    /// When the exponents are equal — the common case (same-scale decimals) — the magnitude order is
    /// exactly the coefficient magnitude order, so it compares the [`etude_bigint::Big`] coefficients
    /// directly (a limb-wise, top-limb-first compare) with no base-10 rendering. Otherwise it compares the
    /// adjusted exponent (the base-10 order of magnitude of the most-significant digit) using an exact
    /// digit count from [`etude_bigint::Big::decimal_digit_count`] — no rendering — and only when those
    /// tie does it scale both magnitudes to a common exponent and compare the `Big` values directly. Every
    /// path is allocation-light: no decimal string is ever built.
    fn cmp_magnitude(&self, other: &Decimal) -> Ordering {
        // The caller (`compare`) only reaches here with `self` and `other` of the same sign (and both
        // nonzero), so a signed `Big` compare gives the magnitude order for two positives and its reverse
        // for two negatives: flip for negatives. This yields the magnitude order with no `abs()` clone.
        let neg = self.coeff.is_negative();
        let by_mag = |a: &Big, b: &Big| if neg { b.cmp(a) } else { a.cmp(b) };
        if self.exp == other.exp {
            // Equal scale: |a·10^e| vs |b·10^e| is |a| vs |b| — a direct coefficient compare.
            return by_mag(&self.coeff, &other.coeff);
        }
        // Native fast path for small unequal-scale values: when both coefficients fit `i128` and aligning
        // them to the smaller exponent (scaling by a power of ten) stays within `i128`, compare the scaled
        // integers directly — no digit count, `bit_len` estimate, or `Big` scale. `checked_mul` returns
        // `None` on overflow (a wide coefficient or a large scale gap), falling through to the exact paths
        // below.
        if let (Some(ca), Some(cb)) = (self.coeff.to_i128_checked(), other.coeff.to_i128_checked())
        {
            let e = self.exp.min(other.exp);
            if let (Some(pa), Some(pb)) = (
                pow10_i128((self.exp - e) as u32),
                pow10_i128((other.exp - e) as u32),
            ) && let (Some(sa), Some(sb)) = (ca.checked_mul(pa), cb.checked_mul(pb))
            {
                // `sa`/`sb` carry the shared sign, so `by_mag`'s flip applies to the scaled integers too.
                return if neg { sb.cmp(&sa) } else { sa.cmp(&sb) };
            }
        }
        // Adjusted exponent = position of the most-significant digit = (digit count - 1) + exp. Larger
        // adjusted exponent = larger magnitude (independent of the shared sign). Computed in i128 so a
        // near-`i64::MAX` exponent cannot overflow the addition.
        //
        // When at least one operand is multi-limb (so its exact `decimal_digit_count` would be
        // `O(magnitude)`), first bound the adjusted exponent from `bit_len` alone (`O(1)`): for a
        // magnitude of bit length `b`, the decimal digit count lies in `((b-1)·log10 2, b·log10 2 + 1]`,
        // so `adj` lies in a small interval. Padded to absorb f64 rounding, if the two intervals are
        // disjoint the order is decided with no `decimal_digit_count`. (Two single-limb values skip this:
        // their `decimal_digit_count` is a native `ilog10`, cheaper than the bound arithmetic.)
        if self.coeff.bit_len() > 64 || other.coeff.bit_len() > 64 {
            let adj_bounds = |coeff: &Big, exp: i64| -> (i128, i128) {
                let b = coeff.bit_len() as f64; // ≥ 1 (nonzero)
                // `as i128` truncates toward zero — floor here, since both products are non-negative (no
                // `f64::floor`, which is std-only and this crate is no_std).
                let dc_lo = ((b - 1.0) * core::f64::consts::LOG10_2) as i128 - 1; // ≤ true dc
                let dc_hi = (b * core::f64::consts::LOG10_2) as i128 + 2; // ≥ true dc
                (dc_lo - 1 + exp as i128, dc_hi - 1 + exp as i128)
            };
            let (a_lo, a_hi) = adj_bounds(&self.coeff, self.exp);
            let (b_lo, b_hi) = adj_bounds(&other.coeff, other.exp);
            if a_lo > b_hi {
                return Ordering::Greater; // |self| clearly larger
            }
            if a_hi < b_lo {
                return Ordering::Less; // |self| clearly smaller
            }
        }
        // Exact adjusted-exponent comparison (either both single-limb, or the bounds overlapped).
        let adj_a = self.coeff.decimal_digit_count() as i128 - 1 + self.exp as i128;
        let adj_b = other.coeff.decimal_digit_count() as i128 - 1 + other.exp as i128;
        if adj_a != adj_b {
            return adj_a.cmp(&adj_b);
        }
        // Same order of magnitude: scale both to the smaller exponent and compare. The scale gap equals
        // the digit-count difference (that is why the adjusted exponents tied), so the power of ten stays
        // proportional to the operands — never a runaway.
        let e = self.exp.min(other.exp);
        let a = scale_pow10(&self.coeff, (self.exp - e) as u64);
        let b = scale_pow10(&other.coeff, (other.exp - e) as u64);
        by_mag(&a, &b)
    }

    /// Write the canonical decimal rendering directly into a [`core::fmt::Write`] sink — the
    /// allocation-conscious rendering path that [`Display`](core::fmt::Display) uses. The output is always
    /// a valid decimal literal and re-parses via [`Decimal::parse`] to the same value: a plain (point)
    /// form for modest exponents, and a bounded `<digits>e<exp>` scientific form for large magnitudes so
    /// the output stays small.
    ///
    /// The coefficient's digits stream straight into `w` via [`etude_bigint::Big::write_decimal`] — no
    /// intermediate `String`. The common fractional case (a point shift that fits a `u64`) is split with a
    /// single-limb divide so the integer and fractional parts each write directly; only a deep fraction
    /// (point shift beyond 19 digits) falls back to rendering the digits once to slice them.
    pub fn write_to<W: core::fmt::Write>(&self, w: &mut W) -> core::fmt::Result {
        if self.is_zero() {
            return w.write_str("0");
        }
        // Threshold that bounds the plain-form length; beyond it, fall back to scientific notation. All
        // comparisons stay in i64 (narrowing to usize only once bounded) so a large exponent cannot
        // truncate on a 32-bit-usize target like wasm32.
        const PLAIN_PAD: i64 = 30;

        if self.exp >= 0 {
            // Integer, trailing-zeros, or (for a very large exponent) scientific — the digits are
            // contiguous, so write_decimal streams the sign and digits with no String and no clone.
            self.coeff.write_decimal(w)?;
            return if self.exp <= PLAIN_PAD {
                for _ in 0..self.exp {
                    w.write_str("0")?;
                }
                Ok(())
            } else {
                write!(w, "e{}", self.exp)
            };
        }

        let k = -self.exp; // fractional point shift, ≥ 1
        // Split-and-stream path: when the point shift fits a u64 AND the coefficient is large, a
        // single-limb divide splits the value into its integer part (a Big, written directly) and its low
        // k fractional digits (a native u64) — writing each straight to the sink, no rendered String. The
        // split pays a fixed divide plus two writes, which only beats rendering-once-and-slicing past a
        // few hundred bits (the to_string benchmark's crossover sits between the 256- and 1024-bit tiers);
        // below that the single render below is cheaper, so gate on the coefficient width.
        const SPLIT_MIN_BITS: usize = 512;
        if k <= 19 && self.coeff.bit_len() >= SPLIT_MIN_BITS {
            // The quotient carries the sign, so `write_decimal` renders "-ddd" for the integer part. A
            // coefficient this wide has far more than 19 digits, so with k ≤ 19 the point always sits
            // inside it (integer part nonzero) and the form is never scientific.
            let (int_part, frac) = self
                .coeff
                .divmod_u64(10u64.pow(k as u32))
                .expect("10^k is nonzero");
            int_part.write_decimal(w)?; // signed integer part
            w.write_str(".")?;
            return write_frac_u64(w, frac, k);
        }

        // Otherwise render the magnitude digits once and slice around the point. Handles small
        // coefficients, the `|value| < 1` leading-zeros form, deep fractions (k > 19), and scientific.
        if self.coeff.is_negative() {
            w.write_str("-")?;
        }
        let mag = self.coeff.abs().to_decimal_string();
        let l = mag.len() as i64;
        if k < l {
            let cut = (l - k) as usize;
            w.write_str(&mag[..cut])?;
            w.write_str(".")?;
            w.write_str(&mag[cut..])
        } else if k <= l + PLAIN_PAD {
            w.write_str("0.")?;
            for _ in 0..(k - l) {
                w.write_str("0")?;
            }
            w.write_str(&mag)
        } else {
            write!(w, "{mag}e{}", self.exp)
        }
    }
}

/// Write `frac` as exactly `width` decimal digits, left-padded with zeros, straight into `w` — the
/// fractional digits of a decimal whose point shift `width` (`1..=19`) fits a `u64`. `frac < 10^width`.
fn write_frac_u64<W: core::fmt::Write>(w: &mut W, frac: u64, width: i64) -> core::fmt::Result {
    let digits = if frac == 0 {
        1
    } else {
        frac.ilog10() as i64 + 1
    };
    for _ in 0..(width - digits) {
        w.write_str("0")?;
    }
    write!(w, "{frac}")
}

/// `10^0 ..= 10^22` as `f64`, each exactly representable (`10^k = 2^k · 5^k`, and `5^22 < 2^52`, so the
/// significand fits 53 bits). Used by the [`Decimal::to_f64`] fast path, where one multiply or divide by
/// an exact power of ten is correctly rounded. `10^23` is the first inexact power, so the table stops at
/// `10^22`.
const POW10_F64: [f64; 23] = [
    1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
    1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
];

/// `10^k` as an `i64`, or `None` when it overflows (`k > 18`, since `10^19 > i64::MAX`). Used by the
/// native small-value arithmetic fast paths to scale a coefficient by a power of ten.
fn pow10_i64(k: u32) -> Option<i64> {
    10i64.checked_pow(k)
}

/// `10^k` as an `i128`, or `None` when it overflows (`k > 38`, since `10^39 > i128::MAX`). Used by the
/// wider native arithmetic fast path to scale a coefficient by a power of ten.
fn pow10_i128(k: u32) -> Option<i128> {
    10i128.checked_pow(k)
}

/// Build a signed [`Big`] from a `u128` magnitude and a sign — for a product that fits `u128` but whose
/// magnitude may exceed `i128` (the top of the `64b × 64b` multiply range, `[2^127, 2^128)`). A magnitude
/// within `i64` boxes through the cheap [`Big::from_i64`]; a wider one writes its little-endian bytes into a
/// stack sign-magnitude buffer (one limb `Vec` allocated, no wider intermediate).
fn big_from_u128(mag: u128, negative: bool) -> Big {
    if mag <= i64::MAX as u128 {
        let v = mag as i64;
        return Big::from_i64(if negative { -v } else { v });
    }
    let mut buf = [0u8; 17]; // 1 sign byte + 16 magnitude bytes
    buf[0] = negative as u8;
    buf[1..].copy_from_slice(&mag.to_le_bytes());
    Big::from_sign_magnitude_bytes(&buf)
}

/// Greatest common divisor of two `u128`s by the binary (Stein) algorithm — native shifts and subtracts,
/// no division. `gcd(x, 0) == x`. Used by the native small-value exact-division fast path.
fn gcd_u128(mut a: u128, mut b: u128) -> u128 {
    if a == 0 {
        return b;
    }
    if b == 0 {
        return a;
    }
    let shift = (a | b).trailing_zeros();
    a >>= a.trailing_zeros();
    loop {
        b >>= b.trailing_zeros();
        if a > b {
            core::mem::swap(&mut a, &mut b);
        }
        b -= a;
        if b == 0 {
            return a << shift;
        }
    }
}

/// Outcome of the native `i128` exact-division fast path ([`Decimal::div`]).
enum ExactDivI128 {
    /// The quotient terminates and fit native arithmetic: the finished value.
    Terminates(Decimal),
    /// A prime factor other than 2 or 5 remains in the reduced divisor — no finite decimal quotient.
    NonTerminating,
    /// The reduced numerator times the compensating power of ten overflows `i128`/`u128` (or the exponent
    /// overflows `i64`): the caller must fall back to the exact `Big` path.
    TooWide,
}

/// Exact `n·10^ne / (d·10^de)` in native integer arithmetic, or a signal to defer. Mirrors the `Big` body
/// of [`Decimal::div`]: reduce `|n|/|d|` by their gcd, strip 2s and 5s from the divisor, and — if nothing
/// else remains — scale the numerator by the complementary power so the result is `coeff·10^exp`. All in
/// `u128` with checked widening; any overflow yields [`ExactDivI128::TooWide`]. `n` and `d` are nonzero.
fn div_exact_i128(n: i128, ne: i64, d: i128, de: i64) -> ExactDivI128 {
    let negative = (n < 0) ^ (d < 0);
    let mut num = n.unsigned_abs();
    let mut den = d.unsigned_abs();
    let g = gcd_u128(num, den);
    num /= g;
    den /= g;
    // Strip every factor of two (a native trailing-zero count), then every factor of five.
    let a2 = den.trailing_zeros();
    den >>= a2;
    let mut a5 = 0u32;
    while den.is_multiple_of(5) {
        den /= 5;
        a5 += 1;
    }
    if den != 1 {
        return ExactDivI128::NonTerminating; // a factor other than 2 or 5 remains
    }
    // 1/(2^a2·5^a5) = (2^(m-a2)·5^(m-a5)) / 10^m, m = max(a2,a5); one of the two powers is 1.
    let m = a2.max(a5);
    let extra = if a2 >= a5 {
        5u128.checked_pow(a2 - a5)
    } else {
        2u128.checked_pow(a5 - a2)
    };
    let Some(extra) = extra else {
        return ExactDivI128::TooWide;
    };
    let Some(mag) = num.checked_mul(extra) else {
        return ExactDivI128::TooWide;
    };
    let Some(exp) = (ne - de).checked_sub(m as i64) else {
        return ExactDivI128::TooWide;
    };
    ExactDivI128::Terminates(Decimal::new(big_from_u128(mag, negative), exp))
}

/// `10^k` as a nonnegative [`Big`], by binary exponentiation (base-10, squaring). `10^0 == 1`.
fn pow10(k: u64) -> Big {
    let mut result = Big::from_i64(1);
    let mut base = Big::from_i64(10);
    let mut e = k;
    while e > 0 {
        if e & 1 == 1 {
            result = result.mul(&base);
        }
        e >>= 1;
        if e > 0 {
            base = base.mul(&base);
        }
    }
    result
}

/// Multiply the signed `coeff` by `10^k` (shift its decimal point left by `k`), preserving sign. `k == 0`
/// is the identity.
fn scale_pow10(coeff: &Big, k: u64) -> Big {
    if k == 0 {
        coeff.clone()
    } else {
        coeff.mul(&pow10(k))
    }
}

/// `2^k` as a nonnegative [`Big`], built directly by setting bit `k` in a little-endian buffer (a
/// trailing zero byte keeps the two's-complement value positive). Used by the decimal→`f64` conversion.
fn pow2(k: u32) -> Big {
    let byte = (k / 8) as usize;
    let mut buf = alloc::vec![0u8; byte + 2]; // +1 for the set bit's byte, +1 zero byte = positive sign
    buf[byte] = 1u8 << (k % 8);
    Big::from_le_twos_complement_bytes(&buf)
}

/// `5^k` as a nonnegative [`Big`], by binary exponentiation. Used by exact decimal division.
fn pow5(k: u32) -> Big {
    let mut result = Big::from_i64(1);
    let mut base = Big::from_i64(5);
    let mut e = k;
    while e > 0 {
        if e & 1 == 1 {
            result = result.mul(&base);
        }
        e >>= 1;
        if e > 0 {
            base = base.mul(&base);
        }
    }
    result
}

/// Decide whether a rounded division should bump the quotient magnitude `q` up by one, given the nonzero
/// remainder `r` (`0 < r < den`) of the discarded fractional part `r/den`, the result sign, and the mode.
fn round_up_magnitude(q: &Big, r: &Big, den: &Big, neg: bool, mode: RoundingMode) -> bool {
    if r.is_zero() {
        return false; // exact division at this precision — nothing to round
    }
    // Compare 2*r to den: Less = below ½, Equal = exactly ½, Greater = above ½.
    let half = r.add(r).cmp(den);
    match mode {
        RoundingMode::Down => false,
        RoundingMode::Up => true,
        RoundingMode::Ceiling => !neg, // toward +∞: round the magnitude up only for a positive result
        RoundingMode::Floor => neg, // toward −∞: round the magnitude up only for a negative result
        RoundingMode::HalfUp => half != Ordering::Less,
        RoundingMode::HalfDown => half == Ordering::Greater,
        RoundingMode::HalfEven => match half {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => !q
                .divmod(&Big::from_i64(2))
                .expect("2 is nonzero")
                .1
                .is_zero(),
        },
    }
}

/// Number of decimal digits per base-`10ᵏ` limb fed to [`Big::from_base_10_pow_k_limbs`]. `10¹⁹ < 2⁶⁴`,
/// so a 19-digit chunk is the widest that fits a `u64`.
const COEFF_CHUNK: usize = 19;

/// Accumulates ASCII decimal digits fed one at a time (streaming, across chunk boundaries) into a
/// nonnegative [`Big`] coefficient. Collects the digit values and, at [`CoeffBuilder::finish`], groups
/// them most-significant-first into `COEFF_CHUNK`-digit base-`10ᵏ` limbs (the leading chunk carries the
/// remainder digits, every later chunk exactly `COEFF_CHUNK`) and builds the `Big` in one Horner pass via
/// [`Big::from_base_10_pow_k_limbs`] — a wide `u64` multiply-add per chunk, with no per-digit or
/// per-chunk `Big` allocated.
struct CoeffBuilder {
    /// The decimal digit values (`0..=9`), most-significant first, in read order.
    digits: alloc::vec::Vec<u8>,
}

impl CoeffBuilder {
    fn new() -> CoeffBuilder {
        CoeffBuilder {
            digits: alloc::vec::Vec::new(),
        }
    }

    /// Append one ASCII digit (`b'0'..=b'9'`).
    fn push(&mut self, d: u8) {
        self.digits.push(d - b'0');
    }

    /// Group the digits into base-`10ᵏ` limbs and assemble the magnitude.
    fn finish(self) -> Big {
        let digits = self.digits;
        if digits.is_empty() {
            return Big::zero();
        }
        // Most-significant-first grouping: the leading chunk holds `len % COEFF_CHUNK` digits (or a full
        // chunk when the count divides evenly), and every later chunk is exactly `COEFF_CHUNK` wide — so
        // each limb is `< 10^COEFF_CHUNK` and the uniform Horner shift lands every digit in its place.
        let mut limbs: alloc::vec::Vec<u64> =
            alloc::vec::Vec::with_capacity(digits.len() / COEFF_CHUNK + 1);
        let lead = digits.len() % COEFF_CHUNK;
        let mut i = 0;
        if lead != 0 {
            limbs.push(fold_digits(&digits[..lead]));
            i = lead;
        }
        while i < digits.len() {
            limbs.push(fold_digits(&digits[i..i + COEFF_CHUNK]));
            i += COEFF_CHUNK;
        }
        Big::from_base_10_pow_k_limbs(COEFF_CHUNK as u32, &limbs)
            .expect("COEFF_CHUNK is in 1..=19 and every limb is < 10^COEFF_CHUNK")
    }
}

/// Fold up to 19 decimal digit values (`0..=9`) into one `u64`: the value is `< 10^len ≤ 10¹⁹ < 2⁶⁴`, so
/// no step overflows.
fn fold_digits(ds: &[u8]) -> u64 {
    let mut v = 0u64;
    for &d in ds {
        v = v * 10 + d as u64;
    }
    v
}

impl core::fmt::Display for Decimal {
    /// Renders the canonical decimal form (see [`Decimal::write_to`]).
    ///
    /// # Format flags
    /// Honors the `Formatter` **padding** flags — width, fill, alignment, the `+` sign flag, and sign-aware
    /// zero-padding — via [`Formatter::pad_integral`](core::fmt::Formatter::pad_integral) (the value's
    /// magnitude is padded as a single integral field, e.g. `{:>8}` on `3.14` → `"    3.14"`, `{:+}` →
    /// `"+3.14"`, `{:08}` → `"00003.14"`, `{:08}` on
    /// `-0.5` → `"-00000.5"`). **Precision is ignored** — `{:.N}` cannot mean "`N` fractional digits"
    /// without silently choosing a rounding, and this crate never rounds without an explicit
    /// [`RoundingMode`] (use [`Decimal::div_round`], or render and reformat). This is the shared contract
    /// with [`etude-rational`](https://crates.io/crates/etude-rational), matching `num`'s numeric `Display`.
    ///
    /// The common flag-free case (`"{}"`, `to_string`) keeps a zero-allocation fast path, streaming the
    /// digits straight into the sink; only a flagged format renders to a scratch `String` first (which
    /// `pad_integral` requires, since it needs the whole magnitude).
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Fast path: no width/sign/zero-pad flag ⇒ nothing to pad, so stream with no allocation. (Fill and
        // alignment are inert without a width; precision does not apply — see above.)
        if f.width().is_none() && !f.sign_plus() && !f.sign_aware_zero_pad() {
            return self.write_to(f);
        }
        // A padding flag is set: render the UNSIGNED magnitude to a scratch buffer, then let `pad_integral`
        // apply the sign, width, fill, alignment, `+`, and sign-aware zero-padding (matching etude-rational).
        let mut buf = alloc::string::String::new();
        let _ = self.abs().write_to(&mut buf);
        f.pad_integral(!self.is_negative(), "", &buf)
    }
}

/// Parse error for [`Decimal::from_str`] / [`str::parse`]. Carries no detail — the input was not a
/// well-formed decimal literal (see [`Decimal::parse`] for the grammar).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ParseDecimalError;

impl core::fmt::Display for ParseDecimalError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("invalid decimal number literal")
    }
}

impl FromStr for Decimal {
    type Err = ParseDecimalError;

    fn from_str(s: &str) -> Result<Decimal, ParseDecimalError> {
        Decimal::parse(s.bytes()).ok_or(ParseDecimalError)
    }
}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Decimal) -> Option<Ordering> {
        Some(Ord::cmp(self, other))
    }
}

impl Ord for Decimal {
    /// Exact three-way comparison consistent with the numeric value.
    fn cmp(&self, other: &Decimal) -> Ordering {
        self.compare(other)
    }
}

#[cfg(test)]
mod tests;
