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
//! [`Decimal::new`] (and the [`Decimal::from_i64`] / [`Decimal::from_bigint`] conveniences), or PARSED
//! from a decimal number literal via [`Decimal::parse`] / [`Decimal::from_str`]. Parsing owns exactly the
//! decimal-number-literal grammar and consumes any `Iterator<Item = u8>`, so a rope- or chunk-backed
//! (non-contiguous) byte source is parsed in place with no flattening; [`Decimal::parse_prefix`] parses a
//! number embedded in a larger byte stream (a text decoder such as a JSON tokenizer hands its chunk
//! cursor straight in — this crate parses the *number*, the decoder owns the surrounding structure).
//!
//! # Representation and the canonical-form invariant
//! A [`Decimal`] is the exact value `coeff * 10^exp`, where `coeff` is an [`etude_bigint::Big`] signed
//! integer (it carries the sign of the whole value) and `exp` is a base-10 point shift. It is kept in a
//! SINGLE canonical form:
//! - the coefficient has NO trailing zero digit (`coeff % 10 != 0`), with `exp` raised to compensate, so
//!   `1.0`, `1`, and `1.00` all become the one value `coeff = 1, exp = 0`, and `100` becomes
//!   `coeff = 1, exp = 2`;
//! - zero is exactly `coeff = 0, exp = 0` (there is no `0 * 10^5`).
//!
//! Every constructor and operation renormalizes, so a value has exactly ONE in-memory form. This is
//! required for `Eq`/`Ord` to mean numeric equality: `0.1` and `0.10` and `1e-1` are the SAME value and
//! MUST have identical fields.
//!
//! The fields are PRIVATE and not part of the stable API — the coefficient repr and the exponent width
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
/// The internal `{ coeff, exp }` representation is PRIVATE and not part of the stable API. Construct
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
/// Mirrors the IEEE-754 / `bigdecimal` rounding modes. There is deliberately NO default — a
/// rounding-capable operation takes the mode as an EXPLICIT argument so the caller always chooses.
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
    /// with a coefficient of `~2^31` trailing zeros stays merely un-fully-stripped, never wrong).
    fn normalize(&mut self) {
        if self.coeff.is_zero() {
            self.exp = 0;
            return;
        }
        let ten = Big::from_i64(10);
        while self.exp < i64::MAX {
            // divmod by 10 is None only for a zero divisor, which `ten` is not.
            let (q, r) = self.coeff.divmod(&ten).expect("divisor 10 is nonzero");
            if !r.is_zero() {
                break;
            }
            self.coeff = q;
            self.exp += 1;
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
        if self.exp == other.exp {
            // Already aligned — the common fast path (e.g. equal-scale sums).
            return Decimal::new(self.coeff.add(&other.coeff), self.exp);
        }
        let e = self.exp.min(other.exp);
        let a = scale_pow10(&self.coeff, (self.exp - e) as u64);
        let b = scale_pow10(&other.coeff, (other.exp - e) as u64);
        Decimal::new(a.add(&b), e)
    }

    /// Exact difference `self - other`.
    pub fn sub(&self, other: &Decimal) -> Decimal {
        self.add(&other.neg())
    }

    /// Exact product `self * other`: multiply the coefficients and add the exponents. Exact — a decimal
    /// product is always representable (unlike a quotient).
    pub fn mul(&self, other: &Decimal) -> Decimal {
        if self.is_zero() || other.is_zero() {
            return Decimal::zero();
        }
        // Exponents come from parsing bounded to ≤18 digits, so their sum fits i64 for any realistic
        // input; saturate only in the astronomically-extreme case rather than wrap.
        let exp = self.exp.saturating_add(other.exp);
        Decimal::new(self.coeff.mul(&other.coeff), exp)
    }

    /// EXACT division `self / other`, or `None` if the quotient does not terminate as a finite decimal
    /// (or `other` is zero). A decimal quotient is exact exactly when the divisor's coefficient, reduced
    /// against the dividend's, has no prime factor other than 2 and 5 (`1/2`, `1/8`, `3/40` terminate;
    /// `1/3`, `1/7` do not). Because it cannot round, this operation takes NO rounding arguments — use
    /// [`Decimal::div_round`] for a rounded quotient to a chosen precision.
    pub fn div(&self, other: &Decimal) -> Option<Decimal> {
        if other.is_zero() {
            return None;
        }
        if self.is_zero() {
            return Some(Decimal::zero());
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

    /// Divide `self / other`, rounding the quotient to `precision` significant digits with the EXPLICIT
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
    /// stream is not a well-formed literal. The stream need NOT be contiguous — any `IntoIterator<Item =
    /// u8>` works, so a rope- or chunk-backed byte source is parsed in place with no flattening. The
    /// grammar is
    ///
    /// ```text
    /// -? ( 0 | [1-9][0-9]* ) ( . [0-9]+ )? ( [eE] [+-]? [0-9]+ )?
    /// ```
    ///
    /// so a leading `+`, a redundant leading zero (`01`), a bare `.5`, a trailing `1.`, a lone `-`, an
    /// empty exponent (`1e`), and any surrounding whitespace or trailing bytes are all REJECTED. The
    /// decode is LOSSLESS. An exponent of more than 18 digits (beyond `i64`) is rejected rather than
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

    /// Parse the maximal decimal-number-literal PREFIX from a peekable byte iterator, stopping at (and NOT
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
    /// DIRECTLY from the coefficient and exponent with no string round-trip. Magnitudes beyond `f64`
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
        let neg = self.coeff.is_negative();
        let sign = if neg { 1u64 << 63 } else { 0 };
        let inf = if neg {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };

        // Cheap magnitude estimate to short-circuit over/underflow WITHOUT materializing a huge power of
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

    /// Compare `|self|` against `|other|` (both assumed nonzero). Exact and allocation-light.
    ///
    /// When the exponents are EQUAL — the common case (same-scale decimals) — the magnitude order is
    /// exactly the coefficient magnitude order, so it compares the [`etude_bigint::Big`] coefficients
    /// directly (a limb-wise, top-limb-first compare) with no base-10 rendering. Otherwise it falls back
    /// to comparing the adjusted exponent (the base-10 order of magnitude of the most-significant digit),
    /// and only when those tie does it compare the digit strings aligned at that most-significant digit
    /// — so no power-of-ten scaling is ever materialized.
    fn cmp_magnitude(&self, other: &Decimal) -> Ordering {
        if self.exp == other.exp {
            // Equal scale: |a·10^e| vs |b·10^e| is |a| vs |b|. Compare the coefficient magnitudes via
            // `Big` (allocation-light, short-circuiting on the top limb) rather than rendering both to
            // decimal strings — the hot path the scoreboard flagged.
            return self.coeff.abs().cmp(&other.coeff.abs());
        }
        let da = self.coeff.abs().to_decimal_string();
        let db = other.coeff.abs().to_decimal_string();
        // Adjusted exponent = position of the most-significant digit = (#digits - 1) + exp. Computed in
        // i128 so a near-`i64::MAX` exponent cannot overflow the addition.
        let adj_a = da.len() as i128 - 1 + self.exp as i128;
        let adj_b = db.len() as i128 - 1 + other.exp as i128;
        if adj_a != adj_b {
            return adj_a.cmp(&adj_b);
        }
        // Same order of magnitude: compare digit strings left-to-right, treating the shorter as
        // right-padded with zeros (equal adjusted exponents align the leading digits).
        let ab = da.as_bytes();
        let bb = db.as_bytes();
        let len = ab.len().max(bb.len());
        for k in 0..len {
            let ca = ab.get(k).copied().unwrap_or(b'0');
            let cb = bb.get(k).copied().unwrap_or(b'0');
            if ca != cb {
                return ca.cmp(&cb);
            }
        }
        Ordering::Equal
    }

    /// Write the canonical decimal rendering DIRECTLY into a [`core::fmt::Write`] sink — the
    /// allocation-conscious rendering path that [`Display`](core::fmt::Display) uses (no intermediate `String` is built for
    /// the framing). The output is always a valid decimal literal and re-parses via [`Decimal::parse`]
    /// to the same value: a plain (point) form for modest exponents, and a bounded `<digits>e<exp>`
    /// scientific form for large magnitudes so the output stays small.
    ///
    /// One residual allocation remains: the coefficient's digits come from
    /// [`etude_bigint::Big::to_decimal_string`], which allocates. A sink-writing digit emitter on `Big`
    /// (requested from etude-bigint) would make this fully allocation-free; everything else here writes
    /// straight to `w`.
    pub fn write_to<W: core::fmt::Write>(&self, w: &mut W) -> core::fmt::Result {
        if self.is_zero() {
            return w.write_str("0");
        }
        if self.coeff.is_negative() {
            w.write_str("-")?;
        }
        let mag = self.coeff.abs().to_decimal_string(); // digits only, no sign, no leading zero
        // Threshold that bounds the plain-form length; beyond it, fall back to scientific notation. All
        // comparisons stay in i64 (narrowing to usize only once bounded ≤ PLAIN_PAD) so a large
        // exponent cannot truncate on a 32-bit-usize target like wasm32.
        const PLAIN_PAD: i64 = 30;
        if self.exp == 0 {
            w.write_str(&mag)
        } else if self.exp > 0 && self.exp <= PLAIN_PAD {
            w.write_str(&mag)?;
            for _ in 0..self.exp {
                w.write_str("0")?;
            }
            Ok(())
        } else if self.exp < 0 {
            let k = -self.exp; // positive point shift, in i64
            let l = mag.len() as i64;
            if k < l {
                // Point sits inside the digit string: "ddd.ddd".
                let cut = (l - k) as usize;
                w.write_str(&mag[..cut])?;
                w.write_str(".")?;
                w.write_str(&mag[cut..])
            } else if k <= l + PLAIN_PAD {
                // "0.00…ddd" — leading zeros before the significant digits.
                w.write_str("0.")?;
                for _ in 0..(k - l) {
                    w.write_str("0")?;
                }
                w.write_str(&mag)
            } else {
                write!(w, "{mag}e{}", self.exp)
            }
        } else {
            write!(w, "{mag}e{}", self.exp)
        }
    }
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

/// Accumulates ASCII decimal digits fed one at a time (streaming, across chunk boundaries) into a
/// nonnegative [`Big`] coefficient. Batches digits into ≤18-digit `i64` chunks (each flushed as one
/// `mul`+`add`) to avoid an O(n²) per-digit multiply chain.
struct CoeffBuilder {
    /// The accumulated high-order magnitude (everything already flushed).
    mag: Big,
    /// The current low-order chunk of up to 18 not-yet-flushed digits.
    chunk: i64,
    /// The number of digits in `chunk`.
    len: u32,
}

impl CoeffBuilder {
    fn new() -> CoeffBuilder {
        CoeffBuilder {
            mag: Big::zero(),
            chunk: 0,
            len: 0,
        }
    }

    /// Append one ASCII digit (`b'0'..=b'9'`), flushing the chunk into `mag` every 18 digits.
    fn push(&mut self, d: u8) {
        self.chunk = self.chunk * 10 + (d - b'0') as i64;
        self.len += 1;
        if self.len == 18 {
            self.mag = self.mag.mul(&pow10(18)).add(&Big::from_i64(self.chunk));
            self.chunk = 0;
            self.len = 0;
        }
    }

    /// Fold the trailing partial chunk in and return the assembled magnitude.
    fn finish(self) -> Big {
        if self.len == 0 {
            self.mag
        } else {
            self.mag
                .mul(&pow10(self.len as u64))
                .add(&Big::from_i64(self.chunk))
        }
    }
}

impl core::fmt::Display for Decimal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The Formatter is itself a `core::fmt::Write` sink — write straight into it, no String.
        self.write_to(f)
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
