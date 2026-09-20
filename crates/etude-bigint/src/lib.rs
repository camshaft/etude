// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Arbitrary-precision signed integers — a small, hand-written `no_std` limb library. Pure over
//! `alloc::vec::Vec`, no I/O, no dependency. The surface is small (add/sub/mul/divmod/gcd/cmp +
//! from/to i64 + two byte encodings) over `Vec<u64>` limbs — schoolbook algorithms. Independently
//! unit-testable, with a differential test against `num-bigint` (a dev-dependency) as the safety net.
//!
//! # Representation
//! [`Big`] is `{ neg: bool, mag: Vec<u64> }` — base-2⁶⁴ limbs, little-endian (`mag[0]` is the
//! least-significant limb), with no trailing zero limbs. Zero is the canonical `{ neg: false, mag: [] }`.
//! Every operation `normalize`s its result (strips trailing zero limbs; forces `neg = false` when the
//! magnitude is zero), so a value has exactly one in-memory form. This canonical form is required when a
//! `Big` is used as a map key or compared for equality: the sign-magnitude byte encoding
//! ([`Big::to_sign_magnitude_bytes`]) is what such comparisons operate on, so equal values must produce
//! identical bytes.

#![cfg_attr(not(test), no_std)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::vec::Vec;
use core::cmp::Ordering;

/// An arbitrary-precision signed integer. See the module doc for the canonical-form invariant.
///
/// The internal representation is private and not part of the stable API — the limb width, endianness,
/// and any future small-value inlining may change without notice. Construct values through the
/// constructors ([`Big::zero`], [`Big::from_i64`], the `from_*_bytes` parsers) and inspect them through
/// the accessors ([`Big::is_zero`], [`Big::is_negative`], [`Big::cmp`], the `to_*` conversions).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Big {
    /// Sign: `true` = negative. Always `false` when `mag` is empty (zero is non-negative, canonical).
    neg: bool,
    /// Magnitude limbs, base 2⁶⁴, little-endian, no trailing zero limbs (empty = zero).
    mag: Vec<u64>,
}

impl Big {
    /// The canonical zero.
    pub fn zero() -> Big {
        Big {
            neg: false,
            mag: Vec::new(),
        }
    }

    /// Whether this is zero (canonical: empty magnitude).
    pub fn is_zero(&self) -> bool {
        self.mag.is_empty()
    }

    /// Whether this is strictly negative. `false` for zero (zero is non-negative, canonical).
    pub fn is_negative(&self) -> bool {
        self.neg
    }

    /// Whether this is even. `O(1)` — the low bit of the least-significant limb; zero is even.
    pub fn is_even(&self) -> bool {
        self.mag.first().is_none_or(|&lo| lo & 1 == 0)
    }

    /// Whether this is odd. `O(1)` (the complement of [`Big::is_even`]).
    pub fn is_odd(&self) -> bool {
        !self.is_even()
    }

    /// The least-significant decimal digit of `|self|` — `|self| mod 10` (zero returns `0`). So
    /// `last_decimal_digit() == 0` is a divisibility-by-10 test, and the digit itself drives rounding.
    ///
    /// Computed by a limb sum, not a division: since `2⁶⁴ ≡ 6 (mod 10)` and `6ᵏ ≡ 6 (mod 10)` for
    /// `k ≥ 1`, every limb above the lowest contributes its value times 6, so
    /// `|self| ≡ limb₀ + 6·Σ_{i≥1} limbᵢ (mod 10)`. Each per-limb step is a `% 10` and an add — no
    /// per-limb reciprocal multiply as [`Big::rem_u64`] uses — which is cheaper at the wide magnitudes
    /// where a decimal normalizes results (a divisibility-by-10 check on every even coefficient).
    pub fn last_decimal_digit(&self) -> u8 {
        let (lo, rest) = match self.mag.split_first() {
            Some(pair) => pair,
            None => return 0, // zero
        };
        // `Σ (limbᵢ mod 10)` over the high limbs. Each term is `≤ 9`, so this overflows a `u64` only past
        // ~2⁶⁰ limbs (exabytes of magnitude) — unreachable; no per-iteration reduction needed.
        let mut high: u64 = 0;
        for &limb in rest {
            high += limb % 10;
        }
        ((lo % 10 + 6 * (high % 10)) % 10) as u8
    }

    /// The absolute value `|self|`.
    pub fn abs(&self) -> Big {
        Big {
            neg: false,
            mag: self.mag.clone(),
        }
    }

    /// The number of bits in the magnitude — `⌊log₂ |self|⌋ + 1`, and `0` for zero. `O(1)` (reads only
    /// the most-significant limb), so it is a cheap size probe for magnitude-thresholded algorithms.
    pub fn bit_len(&self) -> usize {
        bitlen_mag(&self.mag)
    }

    /// The number of significant bytes in the magnitude — the length of the little-endian magnitude in
    /// [`Big::to_sign_magnitude_bytes`] (i.e. excluding the sign byte). `0` for zero. `O(1)`.
    pub fn byte_len(&self) -> usize {
        self.bit_len().div_ceil(8)
    }

    /// The number of decimal digits in the base-10 representation of `|self|` — the length of
    /// [`Big::to_decimal_string`] ignoring any leading `-`. Zero has one digit (`"0"`).
    ///
    /// Exact, and it does not render the value. A single-limb value uses the native `u64::ilog10`; a
    /// wider one estimates the count from [`Big::bit_len`] (`≈ bits · log₁₀2`) and corrects it with a
    /// few `10^d` threshold comparisons. This is an order of magnitude cheaper than the decimal render a
    /// caller would otherwise run just to count digits — e.g. a decimal comparing adjusted exponents
    /// (digit count + scale) needs the count, not the digits.
    pub fn decimal_digit_count(&self) -> u64 {
        match self.mag.len() {
            0 => 1, // zero renders as "0"
            // Fits one limb: the native base-10 ilog is exact (`ilog10(v) + 1` digits for `v ≥ 1`).
            1 => self.mag[0].ilog10() as u64 + 1,
            _ => {
                let bits = self.bit_len() as u64;
                // `1233/4096 = 0.30102539… < log₁₀2`, so `⌊bits·1233/4096⌋ ≤ D` (the true digit count);
                // one less keeps `d` a strict lower bound, so `10^(d-1) ≤ |self|` and the loop runs.
                let mut d = ((bits * 1233) >> 12).saturating_sub(1).max(1);
                let mut p = pow10_mag(d); // 10^d
                // Raise `d` until `10^d` strictly exceeds `|self|`. The last raise established
                // `10^(d-1) ≤ |self|`, so at exit `10^(d-1) ≤ |self| < 10^d` — exactly `d` digits.
                while Big::cmp_mag(&self.mag, &p) != Ordering::Less {
                    mul_add_u64_inplace(&mut p, 10, 0); // p *= 10
                    d += 1;
                }
                d
            }
        }
    }

    /// Strip trailing zero limbs and force a zero magnitude to non-negative — re-establishes the
    /// canonical form after an operation that may have produced trailing zeros or a signed zero.
    fn normalize(&mut self) {
        while self.mag.last() == Some(&0) {
            self.mag.pop();
        }
        if self.mag.is_empty() {
            self.neg = false;
        }
    }

    // ─── magnitude helpers (unsigned, operate on limb slices) ─────────────────────────────────

    /// Compare two magnitudes (limb slices, little-endian, normalized) by value.
    fn cmp_mag(a: &[u64], b: &[u64]) -> Ordering {
        if a.len() != b.len() {
            return a.len().cmp(&b.len());
        }
        // Equal limb count → compare from the most-significant limb down.
        for i in (0..a.len()).rev() {
            match a[i].cmp(&b[i]) {
                Ordering::Equal => {}
                ord => return ord,
            }
        }
        Ordering::Equal
    }

    /// `a + b` over magnitudes (little-endian limbs), returning a normalized magnitude. A `u128`
    /// accumulator holds a limb sum plus carry without overflow (the carry is always 0 or 1).
    fn add_mag(a: &[u64], b: &[u64]) -> Vec<u64> {
        let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
        let mut carry = 0u128;
        for i in 0..a.len().max(b.len()) {
            let av = *a.get(i).unwrap_or(&0) as u128;
            let bv = *b.get(i).unwrap_or(&0) as u128;
            let s = av + bv + carry;
            out.push(s as u64);
            carry = s >> 64;
        }
        if carry != 0 {
            out.push(carry as u64);
        }
        strip(&mut out);
        out
    }

    /// `a - b` over magnitudes, requiring `a >= b` (caller ensures via `cmp_mag`). Returns normalized.
    /// Branchless `u64` borrow chain — two `overflowing_sub`s per limb (subtract the limb, then the
    /// incoming borrow), staying on native `u64` (no `i128` widen or per-limb branch). At most one of the
    /// two subtractions can borrow, so the outgoing borrow is their OR.
    fn sub_mag(a: &[u64], b: &[u64]) -> Vec<u64> {
        let mut out = Vec::with_capacity(a.len());
        let mut borrow = 0u64;
        for (i, &limb) in a.iter().enumerate() {
            let bv = *b.get(i).unwrap_or(&0);
            let (d1, b1) = limb.overflowing_sub(bv);
            let (d2, b2) = d1.overflowing_sub(borrow);
            out.push(d2);
            borrow = (b1 | b2) as u64;
        }
        strip(&mut out);
        out
    }

    /// `a -= b` in place over magnitudes, requiring `a >= b` (caller ensures). Reuses `a`'s buffer
    /// (no allocation) and re-strips the high zero limbs the subtraction exposes. Same branchless
    /// `u64` borrow chain as [`Big::sub_mag`]; used by the [`Big::gcd`] inner loop, which subtracts
    /// thousands of times and would otherwise allocate a fresh magnitude each step.
    fn sub_mag_inplace(a: &mut Vec<u64>, b: &[u64]) {
        let mut borrow = 0u64;
        for (i, ai) in a.iter_mut().enumerate() {
            let bv = *b.get(i).unwrap_or(&0);
            let (d1, b1) = ai.overflowing_sub(bv);
            let (d2, b2) = d1.overflowing_sub(borrow);
            *ai = d2;
            borrow = (b1 | b2) as u64;
        }
        strip(a);
    }

    /// `a += b` in place over magnitudes. Reuses `a`'s buffer, growing it only when `b` is wider or a
    /// carry escapes the top limb. A `u128` accumulator holds each limb sum plus carry (carry is 0 or 1).
    /// The in-place counterpart of [`Big::add_mag`]; used by the `&mut`-accumulator surface
    /// ([`Big::add_assign`]/[`Big::sub_assign`]) so a running sum grows in its own buffer with no fresh
    /// magnitude per step. Adding never exposes a trailing zero limb (the top limb only grows), so no
    /// re-strip is needed.
    fn add_mag_inplace(a: &mut Vec<u64>, b: &[u64]) {
        if a.len() < b.len() {
            a.resize(b.len(), 0);
        }
        let mut carry = 0u128;
        for (ai, &bv) in a.iter_mut().zip(b.iter()) {
            let s = *ai as u128 + bv as u128 + carry;
            *ai = s as u64;
            carry = s >> 64;
        }
        // Propagate the remaining carry through `a`'s higher limbs, then push a new top limb if it escapes.
        for ai in a.iter_mut().skip(b.len()) {
            if carry == 0 {
                break;
            }
            let s = *ai as u128 + carry;
            *ai = s as u64;
            carry = s >> 64;
        }
        if carry != 0 {
            a.push(carry as u64);
        }
    }

    /// `a = b - a` in place over magnitudes ("reverse subtract"), requiring `b >= a` (caller ensures via
    /// `cmp_mag`). Reuses `a`'s buffer, growing it to `b`'s width first, then runs the same branchless
    /// `u64` borrow chain as [`Big::sub_mag`] with the operands swapped. Lets the accumulator surface keep
    /// the result in `a`'s buffer when `|b| > |a|` (opposite-sign add where the addend is larger) instead
    /// of allocating a fresh magnitude. Because `b >= a`, the final borrow is always 0.
    fn rsub_mag_inplace(a: &mut Vec<u64>, b: &[u64]) {
        if a.len() < b.len() {
            a.resize(b.len(), 0);
        }
        let mut borrow = 0u64;
        for (i, ai) in a.iter_mut().enumerate() {
            let bv = *b.get(i).unwrap_or(&0);
            let (d1, b1) = bv.overflowing_sub(*ai);
            let (d2, b2) = d1.overflowing_sub(borrow);
            *ai = d2;
            borrow = (b1 | b2) as u64;
        }
        strip(a);
    }

    /// `a * b` over magnitudes, returning a normalized magnitude. Schoolbook O(n·m) below
    /// [`KARATSUBA_THRESHOLD`] limbs; Karatsuba (O(n^1.585)) once both operands are at least that wide.
    fn mul_mag(a: &[u64], b: &[u64]) -> Vec<u64> {
        if a.is_empty() || b.is_empty() {
            return Vec::new();
        }
        if a.len().min(b.len()) < KARATSUBA_THRESHOLD {
            mul_schoolbook(a, b)
        } else {
            mul_karatsuba(a, b)
        }
    }

    /// `out = a * b` over magnitudes, reusing `out`'s buffer. The in-place counterpart of [`Big::mul_mag`]
    /// for the [`Big::mul_into`] accumulator primitive. The schoolbook path (below [`KARATSUBA_THRESHOLD`])
    /// accumulates directly into `out`'s existing allocation ([`mul_schoolbook_into`]), so a caller
    /// multiplying repeatedly into one scratch `Big` (fraction reduction's cross terms) allocates nothing
    /// after the buffer reaches its steady size. The Karatsuba path is arithmetic-bound (O(n^1.585)) and
    /// its allocations are amortized over that work — an in-place variant was measured to give no win at
    /// those sizes — so it just replaces `out`'s magnitude with a freshly built one.
    fn mul_mag_into(a: &[u64], b: &[u64], out: &mut Vec<u64>) {
        if a.is_empty() || b.is_empty() {
            out.clear();
            return;
        }
        if a.len().min(b.len()) < KARATSUBA_THRESHOLD {
            mul_schoolbook_into(a, b, out);
        } else {
            *out = mul_karatsuba(a, b);
        }
    }

    // ─── signed arithmetic ────────────────────────────────────────────────────────────────────

    /// Signed comparison. Consistent with the value order (`-1 < 0 < 1`).
    // Kept as an inherent method to mirror the source surface; a `Big` is always canonical, so this is
    // consistent with the derived `Eq`, but `Ord`/`PartialOrd` are intentionally not implemented here.
    #[allow(clippy::should_implement_trait)]
    pub fn cmp(&self, other: &Big) -> Ordering {
        match (self.neg, other.neg) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => Big::cmp_mag(&self.mag, &other.mag),
            // Both negative: the LARGER magnitude is the SMALLER value.
            (true, true) => Big::cmp_mag(&other.mag, &self.mag),
        }
    }

    /// Three-way compare two values given their canonical sign-magnitude byte encodings directly — the
    /// same result as decoding both to `Big` and calling [`Big::cmp`], but with no limb `Vec` allocated.
    /// A comparison over encoded operands is therefore allocation-free. Bytes are
    /// `[sign][LE magnitude, trailing-zeros-stripped]` (the [`Big::to_sign_magnitude_bytes`] form); a
    /// canonical zero is `[0]` (sign byte only), and zero is never negative — so the sign-differ arms
    /// below cannot misfire on a zero (its sign byte is 0). Differential-tested against [`Big::cmp`].
    pub fn cmp_sign_magnitude_bytes(a: &[u8], b: &[u8]) -> Ordering {
        let a_neg = a.first().copied().unwrap_or(0) != 0;
        let b_neg = b.first().copied().unwrap_or(0) != 0;
        let a_mag = a.get(1..).unwrap_or(&[]); // LE magnitude bytes, trailing zeros already stripped
        let b_mag = b.get(1..).unwrap_or(&[]);
        match (a_neg, b_neg) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => Big::cmp_mag_bytes_le(a_mag, b_mag),
            // Both negative: the LARGER magnitude is the SMALLER value.
            (true, true) => Big::cmp_mag_bytes_le(b_mag, a_mag),
        }
    }

    /// Compare two little-endian, trailing-zero-stripped magnitude byte slices by value. Longer = larger
    /// (no trailing zeros, so the length is significant-byte count); equal length → compare from the
    /// most-significant (highest-index) byte down. The byte analogue of `cmp_mag` over limbs.
    fn cmp_mag_bytes_le(a: &[u8], b: &[u8]) -> Ordering {
        if a.len() != b.len() {
            return a.len().cmp(&b.len());
        }
        for i in (0..a.len()).rev() {
            match a[i].cmp(&b[i]) {
                Ordering::Equal => continue,
                ne => return ne,
            }
        }
        Ordering::Equal
    }

    /// Signed add/sub core over `(sign, magnitude)` operands. Same sign → add magnitudes; opposite →
    /// subtract the smaller from the larger with the larger's sign. Taking the second operand's sign as
    /// a parameter lets `sub` reuse this without allocating a negated copy of `other`.
    fn add_signed(a_neg: bool, a: &[u64], b_neg: bool, b: &[u64]) -> Big {
        let mut r = if a_neg == b_neg {
            Big {
                neg: a_neg,
                mag: Big::add_mag(a, b),
            }
        } else {
            match Big::cmp_mag(a, b) {
                Ordering::Equal => Big::zero(),
                Ordering::Greater => Big {
                    neg: a_neg,
                    mag: Big::sub_mag(a, b),
                },
                Ordering::Less => Big {
                    neg: b_neg,
                    mag: Big::sub_mag(b, a),
                },
            }
        };
        r.normalize();
        r
    }

    /// `self + other`.
    pub fn add(&self, other: &Big) -> Big {
        Big::add_signed(self.neg, &self.mag, other.neg, &other.mag)
    }

    /// `-self`.
    pub fn neg(&self) -> Big {
        if self.is_zero() {
            return Big::zero();
        }
        Big {
            neg: !self.neg,
            mag: self.mag.clone(),
        }
    }

    /// Negate in place — `O(1)`, just flips the sign bit (no reallocation, no limb copy). Zero stays
    /// non-negative (the canonical signed-zero invariant), so negating zero is a no-op. The in-place
    /// twin of [`Big::neg`], for a consumer that owns a `Big` and wants to reuse its allocation.
    pub fn negate(&mut self) {
        if !self.mag.is_empty() {
            self.neg = !self.neg;
        }
    }

    /// Set this to its absolute value in place — `O(1)`, just clears the sign bit (no reallocation, no
    /// limb copy). Always canonical (zero and positive values are already non-negative). The in-place
    /// twin of [`Big::abs`], for a consumer that owns a `Big` and wants to reuse its allocation.
    pub fn abs_assign(&mut self) {
        self.neg = false;
    }

    /// `self - other`. Computes `self + (-other)` by flipping `other`'s sign as a parameter — no negated
    /// copy of `other`'s magnitude is allocated. (`other`'s sign is irrelevant when it is zero: an empty
    /// magnitude normalizes to canonical zero regardless.)
    pub fn sub(&self, other: &Big) -> Big {
        Big::add_signed(self.neg, &self.mag, !other.neg, &other.mag)
    }

    /// `self += other`, in place. Same value as `self = self.add(other)` but reuses `self`'s magnitude
    /// buffer instead of allocating a fresh result, so a running accumulator sums a sequence with no
    /// per-step allocation (the `&mut`-accumulator surface consumed by fraction reduction). Same-sign adds
    /// the magnitudes in place; opposite-sign subtracts the smaller from the larger (in place, or
    /// reverse-in-place when the addend is larger) and takes the larger operand's sign. The result is
    /// canonical: the magnitude helpers strip trailing zeros, and an exact cancellation resets to zero.
    // Inherent method mirroring the by-value `add`; not the `core::ops::AddAssign` trait (the surface is a
    // deliberate set of caller-owned-scratch primitives, called by name, not operator sugar).
    #[allow(clippy::should_implement_trait)]
    pub fn add_assign(&mut self, other: &Big) {
        if other.is_zero() {
            return;
        }
        if self.is_zero() {
            self.neg = other.neg;
            self.mag.clear();
            self.mag.extend_from_slice(&other.mag);
            return;
        }
        if self.neg == other.neg {
            Big::add_mag_inplace(&mut self.mag, &other.mag);
        } else {
            match Big::cmp_mag(&self.mag, &other.mag) {
                Ordering::Equal => {
                    self.mag.clear();
                    self.neg = false;
                }
                Ordering::Greater => Big::sub_mag_inplace(&mut self.mag, &other.mag),
                Ordering::Less => {
                    Big::rsub_mag_inplace(&mut self.mag, &other.mag);
                    self.neg = other.neg;
                }
            }
        }
    }

    /// `self -= other`, in place. The subtracting counterpart of [`Big::add_assign`] — identical to
    /// `self + (-other)`, computed by flipping `other`'s sign as a local (no negated copy allocated). Same
    /// zero-alloc, canonical-result guarantees.
    #[allow(clippy::should_implement_trait)]
    pub fn sub_assign(&mut self, other: &Big) {
        if other.is_zero() {
            return;
        }
        let other_neg = !other.neg;
        if self.is_zero() {
            self.neg = other_neg;
            self.mag.clear();
            self.mag.extend_from_slice(&other.mag);
            return;
        }
        if self.neg == other_neg {
            Big::add_mag_inplace(&mut self.mag, &other.mag);
        } else {
            match Big::cmp_mag(&self.mag, &other.mag) {
                Ordering::Equal => {
                    self.mag.clear();
                    self.neg = false;
                }
                Ordering::Greater => Big::sub_mag_inplace(&mut self.mag, &other.mag),
                Ordering::Less => {
                    Big::rsub_mag_inplace(&mut self.mag, &other.mag);
                    self.neg = other_neg;
                }
            }
        }
    }

    /// `self * other`.
    pub fn mul(&self, other: &Big) -> Big {
        let mag = Big::mul_mag(&self.mag, &other.mag);
        let mut r = Big {
            neg: self.neg != other.neg,
            mag,
        };
        r.normalize();
        r
    }

    /// `out = self * other`, writing the product into a caller-owned `out` (the `&mut`-accumulator
    /// surface's multiply). Same value as `out = self.mul(other)` but reuses `out`'s magnitude buffer
    /// instead of allocating a fresh result, so a caller that multiplies repeatedly into one scratch `Big`
    /// (fraction reduction building `a·d`, `c·b`, `b·d` into reused scratch each step) allocates nothing in
    /// steady state. `out` may be any `Big`; its previous value is overwritten. The result is canonical:
    /// the in-place magnitude multiply strips trailing zeros and `normalize` fixes a zero product's sign,
    /// so it is byte-identical to the by-value [`mul`](Big::mul).
    pub fn mul_into(&self, other: &Big, out: &mut Big) {
        Big::mul_mag_into(&self.mag, &other.mag, &mut out.mag);
        out.neg = self.neg != other.neg;
        out.normalize();
    }

    /// Truncating division + remainder: returns `(quotient, remainder)` where
    /// `self = quotient * divisor + remainder`, the quotient truncates toward zero, and the remainder
    /// has the sign of `self` (Rust `/`/`%` semantics). `None` when `divisor` is zero.
    pub fn divmod(&self, divisor: &Big) -> Option<(Big, Big)> {
        if divisor.is_zero() {
            return None;
        }
        let (qmag, rmag) = divmod_mag(&self.mag, &divisor.mag);
        // Quotient sign = XOR of operand signs; remainder sign = dividend sign (truncated division).
        let mut q = Big {
            neg: self.neg != divisor.neg,
            mag: qmag,
        };
        let mut r = Big {
            neg: self.neg,
            mag: rmag,
        };
        q.normalize();
        r.normalize();
        Some((q, r))
    }

    /// The quotient only of the truncating division `self / divisor` (toward zero — the same quotient as
    /// [`Big::divmod`]`.0`), `None` when `divisor` is zero. Unlike `divmod` this never allocates the
    /// remainder, so callers that divide by a known factor and discard the remainder (e.g. reducing a
    /// fraction by its gcd, or cross-reduction) skip that allocation. The name reflects the intended use
    /// on exact divisions (remainder zero); a non-exact division still returns the truncating quotient.
    pub fn div_exact(&self, divisor: &Big) -> Option<Big> {
        if divisor.is_zero() {
            return None;
        }
        // want_rem = false → the a<b / single-limb / Knuth paths all skip building the remainder `Vec`.
        let (qmag, _rmag) = divmod_mag_impl(&self.mag, &divisor.mag, false);
        let mut q = Big {
            neg: self.neg != divisor.neg,
            mag: qmag,
        };
        q.normalize();
        Some(q)
    }

    /// `self /= divisor` in place (truncating toward zero, the same quotient as [`Big::div_exact`]),
    /// returning `false` (and leaving `self` unchanged) when `divisor` is zero. The `&mut`-accumulator
    /// surface's exact-divide: fraction reduction divides `num` and `den` by their gcd, discarding the
    /// (zero) remainder. A single-limb divisor — the common case, since a reducing gcd is often one limb —
    /// divides `self`'s magnitude in place (`div_rem_limb_inplace`, no allocation); a multi-limb divisor
    /// takes the shared Knuth core, which allocates its quotient internally. Canonical result: the sign is
    /// `self.neg != divisor.neg` and `normalize` strips trailing zeros / fixes a zero quotient's sign.
    pub fn div_exact_assign(&mut self, divisor: &Big) -> bool {
        if divisor.is_zero() {
            return false;
        }
        let result_neg = self.neg != divisor.neg;
        if divisor.mag.len() == 1 {
            div_rem_limb_inplace(&mut self.mag, divisor.mag[0]);
        } else {
            let (qmag, _rmag) = divmod_mag_impl(&self.mag, &divisor.mag, false);
            self.mag = qmag;
        }
        self.neg = result_neg;
        self.normalize();
        true
    }

    /// The remainder `|self| mod d` for a single-limb divisor `d`, or `None` when `d` is zero. Returns
    /// the magnitude remainder (`< d`, so it fits a `u64`) — sign-agnostic, since the common uses are
    /// divisibility tests and small-factor stripping. Allocation-free: scans the limbs with the
    /// reciprocal 2-by-1 division (no `Big` divisor/remainder, no per-limb `u128` divide libcall).
    pub fn rem_u64(&self, d: u64) -> Option<u64> {
        if d == 0 {
            return None;
        }
        Some(rem_by_limb(&self.mag, d))
    }

    /// Truncating division by a single-limb divisor `d`: `(quotient, |remainder|)`, or `None` when `d`
    /// is zero. The quotient is a `Big` carrying `self`'s sign (`d` is positive); the remainder is the
    /// magnitude remainder as a native `u64` (no `Big` divisor or `Big` remainder is allocated). Faster
    /// than [`Big::divmod`] with a one-limb `Big` divisor — it takes the reciprocal single-limb path.
    pub fn divmod_u64(&self, d: u64) -> Option<(Big, u64)> {
        if d == 0 {
            return None;
        }
        let mut qmag = self.mag.clone();
        let rem = div_rem_limb_inplace(&mut qmag, d);
        let mut q = Big {
            neg: self.neg,
            mag: qmag,
        };
        q.normalize();
        Some((q, rem))
    }

    /// The greatest common divisor of `|self|` and `|other|` — always non-negative (gcd is sign-agnostic:
    /// `gcd(a, b) = gcd(|a|, |b|)`). `gcd(0, 0) = 0`; `gcd(a, 0) = |a|`.
    ///
    /// Binary GCD (Stein's algorithm): factor out the common power of two, then repeatedly make the
    /// operands odd (shifting out twos) and replace the larger with their difference. Each step is a
    /// shift and a subtraction — no division — which beats Euclid-over-divmod at these magnitudes.
    pub fn gcd(&self, other: &Big) -> Big {
        let mut a = self.mag.clone(); // |self|
        let mut b = other.mag.clone(); // |other|
        // gcd(x, 0) = |x| (covers gcd(0, 0) = 0).
        if a.is_empty() || b.is_empty() {
            let mut g = Big {
                neg: false,
                mag: if a.is_empty() { b } else { a },
            };
            g.normalize();
            return g;
        }
        // Fast path for a single-limb operand (the common `gcd(huge, small)`, e.g. a small-factor
        // reduction). One Euclid step — `huge mod small` via the single-limb reciprocal scan (`O(n)`) —
        // collapses the wide operand to a `u64`, then a native `u64` binary gcd finishes it. Stein alone
        // would grind the wide operand down in `O(bit_len)` shift-and-subtract steps of `O(n)` limbs each.
        if a.len() == 1 || b.len() == 1 {
            // Both limbs are nonzero here (canonical single-limb magnitudes have a nonzero top limb).
            let (wide, small) = if a.len() == 1 { (&b, a[0]) } else { (&a, b[0]) };
            let g = gcd_u64(small, rem_by_limb(wide, small));
            return Big {
                neg: false,
                mag: alloc::vec![g], // g ≥ 1 (gcd of two positive values), so this is canonical
            };
        }
        // Pull out the common factor of two; then keep `a` odd for the loop.
        let tz_a = trailing_zeros_mag(&a);
        let shift = tz_a.min(trailing_zeros_mag(&b));
        shr_bits(&mut a, tz_a);
        loop {
            let tz_b = trailing_zeros_mag(&b);
            shr_bits(&mut b, tz_b); // b is now odd
            // Both odd → order them so the subtraction stays non-negative.
            if Big::cmp_mag(&a, &b) == Ordering::Greater {
                core::mem::swap(&mut a, &mut b);
            }
            Big::sub_mag_inplace(&mut b, &a); // odd − odd = even, ≥ 0; reuses b's buffer
            if b.is_empty() {
                break; // gcd of the odd parts is `a`
            }
        }
        shl_bits(&mut a, shift); // restore the common power of two
        let mut g = Big { neg: false, mag: a };
        g.normalize();
        g
    }

    /// `out = gcd(|self|, |other|)`, writing the result into a caller-owned `out` (the `&mut`-accumulator
    /// surface's gcd). Same value as `out = self.gcd(other)` but reuses `out`'s buffer as the algorithm's
    /// `a` scratch instead of cloning `self`'s magnitude into a fresh one — one fewer allocation per call,
    /// so fraction reduction (which takes a gcd of `num`/`den` every step) reuses one gcd scratch across
    /// the loop. The binary-GCD steps are identical to [`Big::gcd`]; `out` is always non-negative and
    /// canonical.
    pub fn gcd_into(&self, other: &Big, out: &mut Big) {
        // Reuse out's allocation as the `a` scratch (= |self|); `b` (= |other|) is the one unavoidable
        // clone (the algorithm consumes it).
        out.neg = false;
        let mut a = core::mem::take(&mut out.mag);
        a.clear();
        a.extend_from_slice(&self.mag);
        let mut b = other.mag.clone();
        // gcd(x, 0) = |x| (covers gcd(0, 0) = 0).
        if a.is_empty() || b.is_empty() {
            out.mag = if a.is_empty() { b } else { a };
            out.normalize();
            return;
        }
        let tz_a = trailing_zeros_mag(&a);
        let shift = tz_a.min(trailing_zeros_mag(&b));
        shr_bits(&mut a, tz_a);
        loop {
            let tz_b = trailing_zeros_mag(&b);
            shr_bits(&mut b, tz_b); // b is now odd
            if Big::cmp_mag(&a, &b) == Ordering::Greater {
                core::mem::swap(&mut a, &mut b);
            }
            Big::sub_mag_inplace(&mut b, &a); // odd − odd = even, ≥ 0; reuses b's buffer
            if b.is_empty() {
                break; // gcd of the odd parts is `a`
            }
        }
        shl_bits(&mut a, shift); // restore the common power of two
        out.mag = a;
        out.normalize();
    }

    // ─── conversions ──────────────────────────────────────────────────────────────────────────

    /// The decimal string of this value (leading `-` if negative), size-independent. `0` → `"0"`.
    ///
    /// A thin allocating wrapper over [`Big::write_decimal`] — the same digits written into a fresh
    /// `String`. When rendering into an existing sink (e.g. a `Display` impl), prefer `write_decimal`.
    pub fn to_decimal_string(&self) -> alloc::string::String {
        let mut s = alloc::string::String::new();
        self.write_decimal(&mut s)
            .expect("writing decimal digits into a String is infallible");
        s
    }

    /// Write this value's decimal digits (leading `-` if negative) straight into a `core::fmt::Write`
    /// sink — the allocation-free core of [`Big::to_decimal_string`]. `0` writes `"0"`.
    ///
    /// Lets a downstream `Display` (e.g. a decimal or rational built on `Big`) render into any sink —
    /// a `String`, a `core::fmt::Formatter`, a byte buffer — with no intermediate `String`/`Vec`. Same
    /// digits as [`Big::to_decimal_string`].
    ///
    /// Small magnitudes take the linear chunk method (peel 19 digits per single-limb division by
    /// `10^19`); wide magnitudes take a recursive divide-and-conquer split (halve by a power of ten),
    /// which turns the linear method's O(n²) into the same subquadratic shape `num-bigint` uses.
    pub fn write_decimal<W: core::fmt::Write>(&self, w: &mut W) -> core::fmt::Result {
        if self.is_zero() {
            return w.write_str("0");
        }
        if self.neg {
            w.write_str("-")?;
        }
        write_decimal_mag(&self.mag, w)
    }

    /// Build a non-negative `Big` from base-`10ᵏ` limbs, most-significant first.
    ///
    /// The value is `Σ limbs[i] · (10ᵏ)^(len−1−i)` — `limbs[0]` is the most-significant chunk. `k` is
    /// the decimal digits per limb (`1..=19`, since `10¹⁹ < 2⁶⁴`); every limb must be `< 10ᵏ`. Returns
    /// `None` if `k` is out of range or any limb is `≥ 10ᵏ`.
    ///
    /// This is the inverse of the `10¹⁹`-chunked decimal render: a decimal parser that has already
    /// grouped its digits into `k`-digit chunks (all but the leading chunk exactly `k` wide) feeds them
    /// here and gets the `Big` without re-stringifying. It runs Horner's method — `acc = acc·10ᵏ + limb`
    /// per chunk — over an in-place `u64` multiply-add, so no intermediate `Big` is allocated per chunk.
    pub fn from_base_10_pow_k_limbs(k: u32, limbs: &[u64]) -> Option<Big> {
        if !(1..=19).contains(&k) {
            return None;
        }
        let base = 10u64.pow(k); // ≤ 10¹⁹ < 2⁶⁴
        let mut mag: Vec<u64> = Vec::new();
        for &limb in limbs {
            if limb >= base {
                return None; // a chunk carrying ≥ k digits is a caller error
            }
            mul_add_u64_inplace(&mut mag, base, limb);
        }
        strip(&mut mag); // leading-zero limbs (e.g. a `[0, 0, 5]` stream) leave no trailing zeros, but
        // an all-zero stream must land on the canonical empty magnitude
        Some(Big { neg: false, mag })
    }

    /// Box a signed 64-bit int as a `Big`.
    pub fn from_i64(v: i64) -> Big {
        if v == 0 {
            return Big::zero();
        }
        let neg = v < 0;
        let m = v.unsigned_abs(); // u64; handles i64::MIN without overflow, fits one limb
        Big {
            neg,
            mag: alloc::vec![m],
        }
    }

    /// Box a signed 128-bit int as a `Big`. The limb-level twin of [`Big::from_i64`] (no byte buffer),
    /// for a small-operand arithmetic fast path that computes in `i128` and boxes the result directly.
    pub fn from_i128(v: i128) -> Big {
        if v == 0 {
            return Big::zero();
        }
        let neg = v < 0;
        let m = v.unsigned_abs(); // u128; exact for i128::MIN (= 2¹²⁷), fits ≤ 2 limbs
        let (lo, hi) = (m as u64, (m >> 64) as u64);
        // Canonical: drop the high limb when it is zero (the low limb is then nonzero, since m ≠ 0).
        let mag = if hi == 0 {
            alloc::vec![lo]
        } else {
            alloc::vec![lo, hi]
        };
        Big { neg, mag }
    }

    /// Narrow to `i64` if it fits, else `None`.
    pub fn to_i64_checked(&self) -> Option<i64> {
        if self.mag.len() > 1 {
            return None; // needs >64 bits — cannot fit i64
        }
        let m = *self.mag.first().unwrap_or(&0); // magnitude as u64
        if self.neg {
            // Negative: fits iff m <= 2^63 (that boundary is exactly i64::MIN).
            if m <= (i64::MAX as u64) + 1 {
                // `-(m as i128) as i64` is exact for m up to 2^63.
                Some(-(m as i128) as i64)
            } else {
                None
            }
        } else if m <= i64::MAX as u64 {
            Some(m as i64)
        } else {
            None
        }
    }

    /// Narrow to `i128` if it fits, else `None`. The limb-level twin of [`Big::to_i64_checked`] (no byte
    /// buffer) — reads the ≤ 2 magnitude limbs directly, for a small-operand arithmetic fast path that
    /// widens its inputs to `i128`. Same result as `i128_from_sign_magnitude_bytes(&to_sign_magnitude_bytes())`.
    pub fn to_i128_checked(&self) -> Option<i128> {
        if self.mag.len() > 2 {
            return None; // needs >128 bits — cannot fit i128
        }
        let lo = self.mag.first().copied().unwrap_or(0) as u128;
        let hi = self.mag.get(1).copied().unwrap_or(0) as u128;
        let m = lo | (hi << 64); // magnitude as u128
        if self.neg {
            // Negative: fits iff m ≤ 2¹²⁷ (that boundary is exactly i128::MIN). `-(m as i128)` would
            // overflow at exactly m == 2¹²⁷, so handle that endpoint explicitly.
            if m < (i128::MAX as u128) + 1 {
                Some(-(m as i128))
            } else if m == (i128::MAX as u128) + 1 {
                Some(i128::MIN)
            } else {
                None
            }
        } else if m <= i128::MAX as u128 {
            Some(m as i128)
        } else {
            None
        }
    }

    /// The checked i64 narrowing directly from the canonical sign-magnitude byte encoding — the same
    /// result as `from_sign_magnitude_bytes(bytes).to_i64_checked()` but with no limb `Vec` allocated,
    /// so the read-only narrowing is allocation-free (like [`Big::cmp_sign_magnitude_bytes`]). Bytes are
    /// `[sign][LE magnitude, trailing-zeros-stripped]`; a value needing >8 magnitude bytes cannot fit
    /// i64. Differential-tested against [`Big::to_i64_checked`].
    pub fn i64_checked_from_sign_magnitude_bytes(bytes: &[u8]) -> Option<i64> {
        let neg = bytes.first().copied().unwrap_or(0) != 0;
        let mag = bytes.get(1..).unwrap_or(&[]); // LE magnitude bytes
        if mag.len() > 8 {
            return None; // needs >64 bits — cannot fit i64
        }
        let mut m: u64 = 0;
        for (i, &byte) in mag.iter().enumerate() {
            m |= (byte as u64) << (8 * i);
        }
        if neg {
            // Negative: fits iff m <= 2^63 (that boundary is exactly i64::MIN).
            if m <= (i64::MAX as u64) + 1 {
                Some(-(m as i128) as i64)
            } else {
                None
            }
        } else if m <= i64::MAX as u64 {
            Some(m as i64)
        } else {
            None
        }
    }

    /// Read the value directly from its canonical sign-magnitude byte encoding as an `i128`, or `None`
    /// if it needs more than 127 magnitude bits (i.e. cannot fit `i128`). Like
    /// [`Big::i64_checked_from_sign_magnitude_bytes`] but into the wider `i128`, so a small-operand
    /// arithmetic fast path can compute with native `checked_*` ops and no limb `Vec`. Bytes are
    /// `[sign][LE magnitude, trailing-zeros-stripped]`; >16 magnitude bytes cannot fit i128, and exactly
    /// 16 bytes fit only if the magnitude ≤ `i128::MAX`+1 (that boundary is `i128::MIN`). A canonical zero
    /// is `[0]`/empty → `Some(0)`. No allocation.
    pub fn i128_from_sign_magnitude_bytes(bytes: &[u8]) -> Option<i128> {
        let neg = bytes.first().copied().unwrap_or(0) != 0;
        let mag = bytes.get(1..).unwrap_or(&[]); // LE magnitude bytes, trailing zeros stripped
        if mag.len() > 16 {
            return None; // needs >128 bits — cannot fit i128
        }
        let mut m: u128 = 0;
        for (i, &byte) in mag.iter().enumerate() {
            m |= (byte as u128) << (8 * i);
        }
        if neg {
            // Negative: fits iff m <= 2^127 (that boundary is exactly i128::MIN). `-(m as i128)` would
            // overflow at exactly m == 2^127, so handle that endpoint explicitly.
            if m < (i128::MAX as u128) + 1 {
                Some(-(m as i128))
            } else if m == (i128::MAX as u128) + 1 {
                Some(i128::MIN)
            } else {
                None
            }
        } else if m <= i128::MAX as u128 {
            Some(m as i128)
        } else {
            None
        }
    }

    /// Serialize an `i128` directly to the canonical sign-magnitude byte encoding in `buf`, returning the
    /// byte length — or `None` if they don't fit `buf`. The write half of a small-operand arithmetic fast
    /// path: an `i128` result serializes with no intermediate `Big`/`Vec`. Byte-identical to
    /// `Big::from_i128(v).to_sign_magnitude_bytes()` — `[sign][LE magnitude, trailing-zeros-stripped]`,
    /// zero → `[0]`, never negative-zero. `unsigned_abs` handles `i128::MIN`.
    pub fn i128_to_sign_magnitude_bytes_into(v: i128, buf: &mut [u8]) -> Option<usize> {
        if buf.is_empty() {
            return None;
        }
        let neg = v < 0;
        let m = v.unsigned_abs(); // u128; exact for i128::MIN (= 2^127)
        let le = m.to_le_bytes(); // 16 bytes, little-endian
        // Significant magnitude length = 16 minus the trailing (high) zero bytes.
        let mut mlen = 16;
        while mlen > 0 && le[mlen - 1] == 0 {
            mlen -= 1;
        }
        if 1 + mlen > buf.len() {
            return None; // caller falls back to the heap `Big` path
        }
        // Zero is never negative on the wire (matches the `Big` canonical form).
        buf[0] = (neg && mlen > 0) as u8;
        buf[1..1 + mlen].copy_from_slice(&le[..mlen]);
        Some(1 + mlen)
    }

    // ─── sign-magnitude bytes (the canonical byte encoding) ─────────────────────────────────────
    // byte 0: sign (0 = non-negative, 1 = negative). bytes 1..: magnitude as LITTLE-ENDIAN bytes with
    // no trailing zero bytes. Zero is exactly `[0x00]` (sign 0, empty magnitude). Canonical.

    /// Serialize to the canonical sign-magnitude byte encoding.
    pub fn to_sign_magnitude_bytes(&self) -> Vec<u8> {
        // Sign byte + 8 bytes/limb, reserved up front so the byte-at-a-time growth never reallocates.
        let mut out = Vec::with_capacity(1 + self.mag.len() * 8);
        out.push(self.neg as u8);
        // Limbs (LE u64) → LE bytes, then strip trailing zero bytes for canonicality.
        for &limb in &self.mag {
            out.extend_from_slice(&limb.to_le_bytes());
        }
        while out.len() > 1 && *out.last().unwrap() == 0 {
            out.pop();
        }
        out
    }

    /// Serialize the sign-magnitude bytes directly into `buf` (no heap Vec), returning the byte length —
    /// or `None` if they don't fit (`buf` too small). A small-value fast path: a single-limb value is
    /// `[sign] + ≤8 magnitude bytes` = ≤9 bytes, so it serializes into a caller-provided buffer without
    /// a transient `Vec`. Byte-identical to [`Big::to_sign_magnitude_bytes`].
    pub fn to_sign_magnitude_bytes_into(&self, buf: &mut [u8]) -> Option<usize> {
        let need = 1 + self.mag.len() * 8; // upper bound before the trailing-zero strip
        if need > buf.len() {
            return None; // caller falls back to the heap `to_sign_magnitude_bytes`
        }
        buf[0] = self.neg as u8;
        let mut n = 1;
        for &limb in &self.mag {
            buf[n..n + 8].copy_from_slice(&limb.to_le_bytes());
            n += 8;
        }
        // Strip trailing zero bytes (keep at least the sign byte), matching the canonical form.
        while n > 1 && buf[n - 1] == 0 {
            n -= 1;
        }
        Some(n)
    }

    /// Parse from the canonical sign-magnitude byte encoding (the inverse of
    /// [`Big::to_sign_magnitude_bytes`]). A malformed/empty input decodes as zero (total).
    pub fn from_sign_magnitude_bytes(bytes: &[u8]) -> Big {
        let Some((&sign, mag_bytes)) = bytes.split_first() else {
            return Big::zero();
        };
        let mut mag = Vec::with_capacity(mag_bytes.len().div_ceil(8));
        let mut i = 0;
        while i < mag_bytes.len() {
            let mut limb = [0u8; 8];
            let k = (mag_bytes.len() - i).min(8);
            limb[..k].copy_from_slice(&mag_bytes[i..i + k]);
            mag.push(u64::from_le_bytes(limb));
            i += 8;
        }
        let mut b = Big {
            neg: sign != 0,
            mag,
        };
        b.normalize();
        b
    }

    // ─── two's-complement bytes (a signed little-endian interchange form) ────────────────────────

    /// Serialize to little-endian two's-complement bytes. The minimal length that round-trips: the sign
    /// bit of the top byte must equal the value's sign, so a positive value whose top byte is ≥0x80 gets
    /// a `0x00` guard byte, and a negative one whose top byte is <0x80 gets a `0xff` guard byte. Zero is
    /// the empty slice.
    pub fn to_le_twos_complement_bytes(&self) -> Vec<u8> {
        if self.is_zero() {
            return Vec::new();
        }
        // Magnitude → LE bytes (strip trailing zeros). Reserve up front (8 bytes/limb + a possible sign
        // guard/extension byte) so the byte-at-a-time growth never reallocates.
        let mut mbytes = Vec::with_capacity(self.mag.len() * 8 + 1);
        for &limb in &self.mag {
            mbytes.extend_from_slice(&limb.to_le_bytes());
        }
        while mbytes.last() == Some(&0) {
            mbytes.pop();
        }
        if !self.neg {
            // Non-negative: bytes are the magnitude; add a 0x00 guard if the top bit is set.
            if mbytes.last().map(|&b| b & 0x80 != 0).unwrap_or(false) {
                mbytes.push(0x00);
            }
            mbytes
        } else {
            // Negative: two's complement = invert all bytes + 1, over enough bytes that the sign bit is set.
            // Ensure a byte exists whose top bit will be 1 after negation: if the magnitude's top bit is
            // already set we still need a leading 0x00 in the magnitude so negation yields 0xff… — handled
            // by widening one byte, then trimming redundant 0xff at the end.
            let mut ext = mbytes;
            if ext.last().map(|&b| b & 0x80 != 0).unwrap_or(true) {
                ext.push(0x00);
            }
            // two's complement: invert + add 1
            let mut carry = 1u16;
            for byte in ext.iter_mut() {
                let v = (!*byte) as u16 + carry;
                *byte = (v & 0xff) as u8;
                carry = v >> 8;
            }
            // Trim redundant 0xff top bytes (keep one that preserves the sign bit).
            while ext.len() > 1 && *ext.last().unwrap() == 0xff && ext[ext.len() - 2] & 0x80 != 0 {
                ext.pop();
            }
            ext
        }
    }

    /// Parse little-endian two's-complement bytes (the inverse of [`Big::to_le_twos_complement_bytes`]).
    /// Empty = zero. The sign is the top bit of the most-significant (last) byte.
    pub fn from_le_twos_complement_bytes(bytes: &[u8]) -> Big {
        let Some(&top) = bytes.last() else {
            return Big::zero();
        };
        let neg = top & 0x80 != 0;
        // Magnitude bytes: if negative, take the two's complement (invert + 1) to recover the magnitude.
        let mut mbytes: Vec<u8> = bytes.to_vec();
        if neg {
            let mut carry = 1u16;
            for byte in mbytes.iter_mut() {
                let v = (!*byte) as u16 + carry;
                *byte = (v & 0xff) as u8;
                carry = v >> 8;
            }
        }
        // LE bytes → LE u64 limbs.
        let mut mag = Vec::with_capacity(mbytes.len().div_ceil(8));
        let mut i = 0;
        while i < mbytes.len() {
            let mut limb = [0u8; 8];
            let k = (mbytes.len() - i).min(8);
            limb[..k].copy_from_slice(&mbytes[i..i + k]);
            mag.push(u64::from_le_bytes(limb));
            i += 8;
        }
        let mut b = Big { neg, mag };
        b.normalize();
        b
    }
}

/// Widening multiply `u64 * u64 -> (high, low)`.
///
/// On 64-bit targets this is one native `u128` multiply. On 32-bit targets — including `wasm32`, which
/// has native 64-bit integers but emulates 128-bit ones (a `u64 * u64 -> u128` lowers to a `__multi3`
/// libcall) — it is synthesized from four native `u32 * u32 -> u64` partial products, so it stays on
/// native wasm `i64` ops with no 128-bit intrinsic. The limb storage is `u64` on both (native on wasm);
/// only this intermediate differs. The `synth-mul` feature forces the synthesized path for testing.
#[cfg(all(
    not(feature = "synth-mul"),
    not(target_pointer_width = "16"),
    not(target_pointer_width = "32")
))]
#[inline]
fn wide_mul(a: u64, b: u64) -> (u64, u64) {
    let p = (a as u128) * (b as u128);
    ((p >> 64) as u64, p as u64)
}

#[cfg(any(
    feature = "synth-mul",
    target_pointer_width = "16",
    target_pointer_width = "32"
))]
#[inline]
fn wide_mul(a: u64, b: u64) -> (u64, u64) {
    // Split each operand into 32-bit halves; every partial product is a native u32*u32 -> u64.
    let (a_lo, a_hi) = (a & 0xffff_ffff, a >> 32);
    let (b_lo, b_hi) = (b & 0xffff_ffff, b >> 32);
    let ll = a_lo * b_lo;
    let lh = a_lo * b_hi;
    let hl = a_hi * b_lo;
    let hh = a_hi * b_hi;
    // Sum the four partials at their 2^0 / 2^32 / 2^64 weights with native u64 carries. `x << 32` keeps
    // a partial's low half (its high half feeds `hi` via `x >> 32`); the true high 64 bits fit u64, so
    // the `hi` additions never overflow.
    let mut lo = ll;
    let mut hi = hh;
    let (s, c1) = lo.overflowing_add(lh << 32);
    lo = s;
    hi += (lh >> 32) + c1 as u64;
    let (s, c2) = lo.overflowing_add(hl << 32);
    lo = s;
    hi += (hl >> 32) + c2 as u64;
    (hi, lo)
}

/// Divide the double word `(u1·2⁶⁴ + u0)` by a normalized single-word divisor `d` (top bit set), using a
/// precomputed reciprocal `v = ⌊(2¹²⁸−1)/d⌋ − 2⁶⁴` — returns `(quotient, remainder)`. Requires `u1 < d`
/// (so the quotient fits one word). Möller & Granlund, "Improved division by invariant integers",
/// Algorithm 4 (DIV2BY1): a widening multiply plus two conditional corrections replace the hardware
/// `u128 / u64` divide (which lowers to a slow `__udivti3` libcall on aarch64 and wasm).
#[inline]
fn udiv_qrnnd_preinv(u1: u64, u0: u64, d: u64, v: u64) -> (u64, u64) {
    // (q1, q0) = v·u1 + (u1·2⁶⁴ + u0); then q1+1 is the quotient estimate.
    let (mut q1, q0) = wide_mul(v, u1);
    let (q0, carry) = q0.overflowing_add(u0);
    q1 = q1.wrapping_add(u1).wrapping_add(carry as u64);
    let mut qhat = q1.wrapping_add(1);
    let mut r = u0.wrapping_sub(qhat.wrapping_mul(d));
    // First correction: qhat was one too large iff r ran past q0.
    if r > q0 {
        qhat = qhat.wrapping_sub(1);
        r = r.wrapping_add(d);
    }
    // Second correction: a final overshoot (r ≥ d) means qhat was one too small.
    if r >= d {
        qhat += 1;
        r -= d;
    }
    (qhat, r)
}

/// Append the base-10 digits of `v` (most-significant first) to `digits`, zero-padded to at least
/// `pad` digits. `pad == 0` emits the natural length (used for the most-significant chunk); a following
/// chunk uses `pad == 19` so its leading zeros are preserved in the concatenation.
/// Largest power of ten below `2^64` (fits one limb — the single-limb divisor fast path), and its
/// decimal-digit count. Peeling by it extracts 19 digits per division step.
const DECIMAL_CHUNK: u64 = 10_000_000_000_000_000_000; // 10^19 < 2^64
const DECIMAL_CHUNK_DIGITS: usize = 19;

/// Precomputed 2-by-1 reciprocal of [`DECIMAL_CHUNK`] for [`udiv_qrnnd_preinv`]: `⌊(2¹²⁸−1)/d⌋ − 2⁶⁴`
/// (Möller & Granlund, "Improved division by invariant integers"). `10¹⁹ = 0x8AC7230489E80000` already
/// has its top bit set (normalized), so no shift is needed and the reciprocal division is exact.
const DECIMAL_CHUNK_RECIP: u64 = ((u128::MAX / DECIMAL_CHUNK as u128) - (1u128 << 64)) as u64;

/// At or below this many limbs the linear chunk method wins: the recursive split's per-node divmods
/// (each with its own quotient/remainder/scratch allocations) cost more than they save until the
/// magnitude is very wide. The linear peel divides by `10^19` in place with a precomputed reciprocal
/// (a `wide_mul`, no `u128` divide), so its per-chunk step is cheap enough that it beats the recursive
/// path through at least 64 limbs — measured on the `to_decimal` benchmark: linear vs recursive is
/// 1.64 vs 3.21 µs at 1024b (16 limbs) and 14.3 vs 17.2 µs at 4096b (64 limbs). Recursive is kept for
/// the wider magnitudes above this, where its subquadratic split finally pays off.
const DECIMAL_RECURSIVE_THRESHOLD: usize = 64;

/// Upper bound on the number of `10^19` chunks a `≤DECIMAL_RECURSIVE_THRESHOLD`-limb value produces:
/// each limb is `< 2^64 ≈ 10^19.27`, so `L` limbs are `< 10^(19.27·L)` → `⌈19.27·L / 19⌉` chunks
/// (11 at `L = 10`); `+2` is slack. Sizes the linear path's stack scratch so it needs no heap.
const MAX_LINEAR_CHUNKS: usize = DECIMAL_RECURSIVE_THRESHOLD + 2;

/// Render a nonzero canonical magnitude `mag` as decimal digits into the sink `w` (no leading zeros).
///
/// A single-limb value (the common small case) writes directly in one chunk — no scratch allocation.
/// Wider narrow magnitudes use [`emit_decimal_linear`] (peel `10^19` chunks). Wide ones use a recursive
/// divide-and-conquer split: with `pow[i] = 10^(19·2^i)`, dividing by the half-width power `pow[level-1]`
/// splits the value into a high and low half of ≈equal digit width, each converted recursively. The
/// linear method is O(n²) (each of the n/19 chunk divisions scans the whole shrinking magnitude); the
/// split makes the sub-divisions operate on geometrically smaller operands, the same subquadratic
/// base conversion `num-bigint` uses.
fn write_decimal_mag<W: core::fmt::Write>(mag: &[u64], w: &mut W) -> core::fmt::Result {
    // Single-limb value (≤ ~1.8·10¹⁹, ≤20 digits): its one `u64` writes in a single chunk with no
    // magnitude clone and no chunk-index `Vec` — the fast path for the common small-value case.
    if mag.len() <= 1 {
        return write_decimal_chunk(w, mag.first().copied().unwrap_or(0), 0);
    }
    if mag.len() <= DECIMAL_RECURSIVE_THRESHOLD {
        return emit_decimal_linear(mag, w);
    }
    // Power stack: pow[0] = 10^19, pow[i] = pow[i-1]² = 10^(19·2^i). Grow it until pow.last()² > value;
    // that pow.last() is the top divisor pow[level-1], and pow.last()² is the bound pow[level].
    //
    // pow[level] is used ONLY as the bound (value < pow[level]) — the recursion divides by pow[level-1]
    // downward and never touches it. So we avoid materializing that last (widest, costliest) squaring
    // whenever the bit widths alone prove pow.last()² > value: with `top` of `b` bits, `top² ≥ 2^(2b-2)`,
    // so `2b-2 ≥ value_bits` (i.e. `2b-1 > value_bits`) already forces `top² > value`. Only a one-bit-wide
    // ambiguous band still needs the actual square computed to decide.
    let value_bits = bitlen_mag(mag);
    let mut pow: Vec<Vec<u64>> = alloc::vec![alloc::vec![DECIMAL_CHUNK]];
    let level = loop {
        let top = pow.last().unwrap();
        if 2 * bitlen_mag(top) - 1 > value_bits {
            break pow.len(); // top² > value proven by width — do not build it; top is pow[level-1]
        }
        let sq = Big::mul_mag(top, top);
        if Big::cmp_mag(&sq, mag) == Ordering::Greater {
            break pow.len(); // ambiguous band: had to build sq (= pow[level]); discard it, keep pow[level-1]
        }
        pow.push(sq);
    };
    write_decimal_rec(mag, w, true, level, &pow)
}

/// Recursively render `value(mag) < pow[level]` into the sink `w`. `top` nodes render at natural width
/// (no leading zeros); non-`top` nodes render at exactly `19·2^level` digits (left-zero-padded), their
/// fixed slot in the parent split. `pow[i] = 10^(19·2^i)`.
fn write_decimal_rec<W: core::fmt::Write>(
    mag: &[u64],
    w: &mut W,
    top: bool,
    level: usize,
    pow: &[Vec<u64>],
) -> core::fmt::Result {
    if level == 0 {
        // value < 10^19 fits a single limb → one chunk (width 19 unless it is the leading chunk).
        let v = mag.first().copied().unwrap_or(0);
        return write_decimal_chunk(w, v, if top { 0 } else { DECIMAL_CHUNK_DIGITS });
    }
    // Split at the half-width power: hi = value / pow[level-1], lo = value % pow[level-1]. Since
    // value < pow[level] = pow[level-1]², both halves are < pow[level-1] (handled at level-1).
    let (hi, lo) = divmod_mag(mag, &pow[level - 1]);
    if top && hi.is_empty() {
        // The high half is empty — the value is narrower than the balanced split; lo is the new top.
        write_decimal_rec(&lo, w, true, level - 1, pow)
    } else {
        write_decimal_rec(&hi, w, top, level - 1, pow)?;
        write_decimal_rec(&lo, w, false, level - 1, pow)
    }
}

/// Linear base conversion for a narrow magnitude (`2..=DECIMAL_RECURSIVE_THRESHOLD` limbs — the
/// single-limb case is handled upstream): peel 19-digit chunks (value mod `10^19`) off `mag`, dividing
/// the running quotient in place, then emit most-significant chunk first at natural width, the rest
/// zero-padded to 19. Both the quotient and the chunk list live in stack buffers (bounded by the limb
/// threshold), so this path allocates nothing. `mag` nonzero.
fn emit_decimal_linear<W: core::fmt::Write>(mag: &[u64], w: &mut W) -> core::fmt::Result {
    let mut cur = [0u64; DECIMAL_RECURSIVE_THRESHOLD];
    cur[..mag.len()].copy_from_slice(mag);
    let mut len = mag.len();
    let mut chunks = [0u64; MAX_LINEAR_CHUNKS];
    let mut n = 0;
    while len > 0 {
        // Divide cur[..len] by 10^19 in place (most-significant limb first); `rem` is the peeled chunk.
        // Each step is a 128÷64 via the precomputed reciprocal (a `wide_mul`, no `u128` hardware divide).
        let mut rem = 0u64;
        for limb in cur[..len].iter_mut().rev() {
            let (q, r) = udiv_qrnnd_preinv(rem, *limb, DECIMAL_CHUNK, DECIMAL_CHUNK_RECIP);
            *limb = q;
            rem = r;
        }
        while len > 0 && cur[len - 1] == 0 {
            len -= 1; // strip high zero limbs off the shrinking quotient
        }
        chunks[n] = rem;
        n += 1;
    }
    // Emit most-significant chunk first at natural width, the rest zero-padded to 19.
    for idx in (0..n).rev() {
        let pad = if idx + 1 == n {
            0
        } else {
            DECIMAL_CHUNK_DIGITS
        };
        write_decimal_chunk(w, chunks[idx], pad)?;
    }
    Ok(())
}

/// The 100 two-digit strings `"00".."99"` packed as bytes, so `DIGIT_PAIRS[2d..2d+2]` is the ASCII of
/// `d` (`0..=99`) with its leading zero. Lets [`write_decimal_chunk`] emit two digits per `/100` step
/// instead of one per `/10` — halving the divisions and stores in the digit loop. Built at compile time.
const DIGIT_PAIRS: [u8; 200] = {
    let mut t = [0u8; 200];
    let mut d = 0usize;
    while d < 100 {
        t[d * 2] = b'0' + (d / 10) as u8;
        t[d * 2 + 1] = b'0' + (d % 10) as u8;
        d += 1;
    }
    t
};

/// Format one `10^19` chunk `v` into a fixed stack buffer — most-significant digit first, left-padded
/// with `'0'` to at least `pad` digits — and write it to the sink in a single `write_str` (the bytes
/// are ASCII digits, hence valid UTF-8). No allocation.
fn write_decimal_chunk<W: core::fmt::Write>(
    w: &mut W,
    mut v: u64,
    pad: usize,
) -> core::fmt::Result {
    // Fill one buffer from the right (least-significant digit written last), so the digits land
    // most-significant first with no separate reversal pass. A u64 is at most 20 decimal digits.
    // Two digits per iteration via `DIGIT_PAIRS` (one `/100` + one table lookup replaces two `/10`s).
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    if v == 0 {
        i -= 1;
        buf[i] = b'0';
    } else {
        while v >= 100 {
            let p = ((v % 100) * 2) as usize;
            i -= 2;
            buf[i] = DIGIT_PAIRS[p];
            buf[i + 1] = DIGIT_PAIRS[p + 1];
            v /= 100;
        }
        // 1 or 2 remaining digits (v < 100): emit the pair without its leading zero.
        if v >= 10 {
            let p = (v * 2) as usize;
            i -= 2;
            buf[i] = DIGIT_PAIRS[p];
            buf[i + 1] = DIGIT_PAIRS[p + 1];
        } else {
            i -= 1;
            buf[i] = b'0' + v as u8;
        }
    }
    // Left-pad with '0' up to `pad` digits (pad ≤ 19 ≤ 20; a full chunk already has ≥ pad digits, so
    // this adds nothing there). `buf.len() - i` is the digit count written so far.
    while buf.len() - i < pad {
        i -= 1;
        buf[i] = b'0';
    }
    w.write_str(core::str::from_utf8(&buf[i..]).expect("ascii digits"))
}

/// Below this many limbs (in the smaller operand), schoolbook multiply beats Karatsuba (whose
/// splitting/recombination overhead dominates for small inputs). Tuned on the `mul` benchmark.
const KARATSUBA_THRESHOLD: usize = 40;

/// Schoolbook `a * b` over magnitudes (O(n·m)), returning a normalized magnitude. Each limb product is
/// a widening `u64 * u64 -> (high, low)` via [`wide_mul`], accumulated with `u64` carries — no `u128`
/// on the multiply path, so it is native on wasm (see [`wide_mul`]). `a`, `b` are non-empty.
fn mul_schoolbook(a: &[u64], b: &[u64]) -> Vec<u64> {
    let mut out = alloc::vec![0u64; a.len() + b.len()];
    for (i, &av) in a.iter().enumerate() {
        let mut carry = 0u64;
        for (j, &bv) in b.iter().enumerate() {
            let (hi, lo) = wide_mul(av, bv);
            // out[i+j] += lo + carry; the two carry-outs and hi form the next carry (which the
            // schoolbook invariant keeps < 2^64, so this add cannot overflow).
            let (s1, c1) = out[i + j].overflowing_add(lo);
            let (s2, c2) = s1.overflowing_add(carry);
            out[i + j] = s2;
            carry = hi + c1 as u64 + c2 as u64;
        }
        // Propagate the final carry into the next limb (and beyond, if it cascades).
        let mut k = i + b.len();
        while carry != 0 {
            let (s, c) = out[k].overflowing_add(carry);
            out[k] = s;
            carry = c as u64;
            k += 1;
        }
    }
    strip(&mut out);
    out
}

/// Schoolbook `a * b` accumulated into `out`'s existing buffer (the in-place counterpart of
/// [`mul_schoolbook`]). Clears `out` and resizes it to `a.len() + b.len()` zeroed limbs — reusing the
/// allocation when its capacity already suffices, so a caller reusing one scratch `Big` across many
/// products allocates only until the buffer reaches steady size. Identical accumulation to
/// [`mul_schoolbook`]; `a`, `b` are non-empty.
fn mul_schoolbook_into(a: &[u64], b: &[u64], out: &mut Vec<u64>) {
    out.clear();
    out.resize(a.len() + b.len(), 0);
    for (i, &av) in a.iter().enumerate() {
        let mut carry = 0u64;
        for (j, &bv) in b.iter().enumerate() {
            let (hi, lo) = wide_mul(av, bv);
            let (s1, c1) = out[i + j].overflowing_add(lo);
            let (s2, c2) = s1.overflowing_add(carry);
            out[i + j] = s2;
            carry = hi + c1 as u64 + c2 as u64;
        }
        let mut k = i + b.len();
        while carry != 0 {
            let (s, c) = out[k].overflowing_add(carry);
            out[k] = s;
            carry = c as u64;
            k += 1;
        }
    }
    strip(out);
}

/// Karatsuba `a * b` over magnitudes. Split each operand at `k` limbs (`a = a1·Bᵏ + a0`), then
/// `a·b = z2·B²ᵏ + z1·Bᵏ + z0` with `z0 = a0·b0`, `z2 = a1·b1`, and
/// `z1 = (a0+a1)(b0+b1) − z0 − z2` — three half-size products instead of four. Recurses through
/// [`Big::mul_mag`], so sub-products below the threshold fall back to schoolbook. `a`, `b` non-empty.
fn mul_karatsuba(a: &[u64], b: &[u64]) -> Vec<u64> {
    let k = a.len().max(b.len()).div_ceil(2);
    let (a0, a1) = a.split_at(k.min(a.len()));
    let (b0, b1) = b.split_at(k.min(b.len()));

    let z0 = Big::mul_mag(a0, b0);
    let z2 = Big::mul_mag(a1, b1);
    // z1 = (a0+a1)(b0+b1) - z0 - z2. Both subtractions are non-negative: (a0+a1)(b0+b1) = z0+z1+z2.
    let sa = Big::add_mag(a0, a1);
    let sb = Big::add_mag(b0, b1);
    let mut z1 = Big::mul_mag(&sa, &sb);
    z1 = Big::sub_mag(&z1, &z0);
    z1 = Big::sub_mag(&z1, &z2);

    // result = z0 + z1·Bᵏ + z2·B²ᵏ.
    let mut out = z0;
    add_into_at(&mut out, &z1, k);
    add_into_at(&mut out, &z2, 2 * k);
    strip(&mut out);
    out
}

/// `acc += addend · B^offset` in place (little-endian limbs, `B = 2⁶⁴`), with `u64` carry propagation.
fn add_into_at(acc: &mut Vec<u64>, addend: &[u64], offset: usize) {
    if addend.is_empty() {
        return;
    }
    if acc.len() < offset + addend.len() + 1 {
        acc.resize(offset + addend.len() + 1, 0);
    }
    let mut carry = false;
    for (i, &x) in addend.iter().enumerate() {
        let (s1, c1) = acc[offset + i].overflowing_add(x);
        let (s2, c2) = s1.overflowing_add(carry as u64);
        acc[offset + i] = s2;
        carry = c1 || c2;
    }
    let mut j = offset + addend.len();
    while carry {
        let (s, c) = acc[j].overflowing_add(1);
        acc[j] = s;
        carry = c;
        j += 1;
    }
}

/// Strip trailing zero limbs from a magnitude (little-endian).
fn strip(v: &mut Vec<u64>) {
    while v.last() == Some(&0) {
        v.pop();
    }
}

/// In-place `mag = mag · mul + add` over a little-endian magnitude — one word of multiply-add per limb,
/// with the high half of each `wide_mul` carried up. `add` seeds the carry (it is `< mul`, so it fits).
/// The single-word Horner step behind [`Big::from_base_10_pow_k_limbs`]. Leaves no trailing zero limbs
/// so long as the input had none (the top limb only grows), but callers `strip` for the all-zero case.
fn mul_add_u64_inplace(mag: &mut Vec<u64>, mul: u64, add: u64) {
    let mut carry = add;
    for limb in mag.iter_mut() {
        let (hi, lo) = wide_mul(*limb, mul);
        let (s, c) = lo.overflowing_add(carry);
        *limb = s;
        // hi ≤ 2⁶⁴−2 (product of two `u64`s), so hi + carry-bit never overflows a `u64`.
        carry = hi + c as u64;
    }
    if carry != 0 {
        mag.push(carry);
    }
}

/// `10^e` as a little-endian magnitude, by binary exponentiation (`O(log e)` multiplies over the
/// squaring `base = 10^(2ⁱ)`). The decimal-order threshold builder for [`Big::decimal_digit_count`].
fn pow10_mag(mut e: u64) -> Vec<u64> {
    let mut result = alloc::vec![1u64];
    let mut base = alloc::vec![10u64];
    while e > 0 {
        if e & 1 == 1 {
            result = Big::mul_mag(&result, &base);
        }
        e >>= 1;
        if e > 0 {
            base = Big::mul_mag(&base, &base);
        }
    }
    result
}

/// Bit width of a little-endian magnitude — `⌊log₂ value⌋ + 1`, and `0` for the empty (zero) magnitude.
/// `O(1)`: canonical form keeps the top limb nonzero, so `leading_zeros` on it gives the exact width.
fn bitlen_mag(m: &[u64]) -> usize {
    match m.last() {
        Some(&top) => m.len() * 64 - top.leading_zeros() as usize,
        None => 0,
    }
}

/// Count of trailing zero bits in a nonzero little-endian magnitude (its 2-adic valuation).
fn trailing_zeros_mag(m: &[u64]) -> usize {
    for (i, &limb) in m.iter().enumerate() {
        if limb != 0 {
            return i * 64 + limb.trailing_zeros() as usize;
        }
    }
    0 // all-zero magnitude — callers guard against this
}

/// Binary GCD of two `u64`s (Stein's algorithm on native words). `gcd(x, 0) = x`, `gcd(0, 0) = 0`.
/// Finishes the single-limb [`Big::gcd`] fast path after one big-by-small remainder step.
fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
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

/// `m >>= k` bits (little-endian; normalized out).
fn shr_bits(m: &mut Vec<u64>, k: usize) {
    if k == 0 {
        return;
    }
    let limb_shift = k / 64;
    if limb_shift >= m.len() {
        m.clear();
        return;
    }
    m.drain(0..limb_shift); // drop whole limbs off the low end
    let bit_shift = (k % 64) as u32;
    if bit_shift > 0 {
        // Each limb takes its high bits down and the next-higher limb's low bits into its top.
        let mut carry = 0u64;
        for limb in m.iter_mut().rev() {
            let next_carry = *limb << (64 - bit_shift);
            *limb = (*limb >> bit_shift) | carry;
            carry = next_carry;
        }
    }
    strip(m);
}

/// `m <<= k` bits (little-endian; normalized out). No-op on an empty (zero) magnitude.
fn shl_bits(m: &mut Vec<u64>, k: usize) {
    if k == 0 || m.is_empty() {
        return;
    }
    let bit_shift = (k % 64) as u32;
    if bit_shift > 0 {
        let mut carry = 0u64;
        for limb in m.iter_mut() {
            let next_carry = *limb >> (64 - bit_shift);
            *limb = (*limb << bit_shift) | carry;
            carry = next_carry;
        }
        if carry != 0 {
            m.push(carry);
        }
    }
    let limb_shift = k / 64;
    if limb_shift > 0 {
        m.splice(0..0, core::iter::repeat_n(0u64, limb_shift)); // insert whole zero limbs at the low end
    }
    strip(m);
}

/// Unsigned division of magnitudes: `(quotient, remainder)` with `a = quotient * b + remainder`,
/// `0 <= remainder < b`. `b` must be non-empty (nonzero — the caller checks). Both results normalized.
fn divmod_mag(a: &[u64], b: &[u64]) -> (Vec<u64>, Vec<u64>) {
    divmod_mag_impl(a, b, true)
}

/// The shared division core. When `want_rem` is false the remainder is not built (returned empty) and
/// no allocation for it is made — the quotient-only path behind [`Big::div_exact`]. Dispatches by
/// divisor width: `a < b` is trivial, a single-limb divisor uses a linear scan, and a multi-limb
/// divisor uses Knuth's Algorithm D (word-at-a-time long division).
fn divmod_mag_impl(a: &[u64], b: &[u64], want_rem: bool) -> (Vec<u64>, Vec<u64>) {
    // a < b → quotient 0, remainder a (only materialized when wanted).
    if Big::cmp_mag(a, b) == Ordering::Less {
        return (Vec::new(), if want_rem { a.to_vec() } else { Vec::new() });
    }
    if b.len() == 1 {
        return divmod_by_limb(a, b[0], want_rem);
    }
    knuth_divmod(a, b, want_rem)
}

/// Divide a magnitude by a single nonzero limb: `(quotient, remainder)`. One `u128` division per limb,
/// most-significant first, carrying the running remainder (always `< d`, so it fits a single limb). When
/// `want_rem` is false the (single-limb) remainder is not allocated.
fn divmod_by_limb(a: &[u64], d: u64, want_rem: bool) -> (Vec<u64>, Vec<u64>) {
    let mut q = a.to_vec();
    let rem = div_rem_limb_inplace(&mut q, d);
    let r = if want_rem && rem != 0 {
        alloc::vec![rem]
    } else {
        Vec::new()
    };
    (q, r)
}

/// The 2-by-1 reciprocal `⌊(2¹²⁸−1)/d⌋ − 2⁶⁴` of a normalized `d` (top bit set), for
/// [`udiv_qrnnd_preinv`]. One `u128` divide, amortized over a whole multi-limb division.
fn reciprocal_2by1(d: u64) -> u64 {
    ((u128::MAX / d as u128) - (1u128 << 64)) as u64
}

/// Divide `m` by a single nonzero limb in place — `m` becomes the quotient (normalized) — returning the
/// remainder (`< d`, so it fits one limb). Uses the reciprocal 2-by-1 division ([`udiv_qrnnd_preinv`]):
/// one `u128` divide to build the reciprocal, then a `wide_mul` per limb — no per-limb `u128` divide
/// (which is a `__udivti3` libcall on aarch64/wasm). The divisor is first normalized (shifted so its top
/// bit is set); the dividend is processed as if shifted left by the same amount, and the remainder is
/// shifted back at the end.
fn div_rem_limb_inplace(m: &mut Vec<u64>, d: u64) -> u64 {
    if m.is_empty() {
        return 0;
    }
    if m.len() == 1 {
        // Single limb: a native `u64 / u64` (one hardware `udiv`, no `u128` and no reciprocal setup —
        // the reciprocal's own `u128` divide would not amortize over a lone limb).
        let r = m[0] % d;
        m[0] /= d;
        strip(m);
        return r;
    }
    let sh = d.leading_zeros();
    let dn = d << sh; // normalized divisor (top bit set)
    let v = reciprocal_2by1(dn);
    let n = m.len();
    let mut rem;
    if sh == 0 {
        rem = 0u64;
        for limb in m.iter_mut().rev() {
            let (q, r) = udiv_qrnnd_preinv(rem, *limb, dn, v);
            *limb = q;
            rem = r;
        }
    } else {
        // Dividing `(m << sh)` by `dn` gives the same quotient as `m / d`; the remainder comes out
        // scaled by `2^sh`, undone at the end. `m << sh`'s top overflow seeds the running remainder.
        rem = m[n - 1] >> (64 - sh);
        for i in (0..n).rev() {
            let lo = if i == 0 { 0 } else { m[i - 1] };
            let shifted = (m[i] << sh) | (lo >> (64 - sh));
            let (q, r) = udiv_qrnnd_preinv(rem, shifted, dn, v);
            m[i] = q;
            rem = r;
        }
        rem >>= sh;
    }
    strip(m);
    rem
}

/// `mag mod d` for a single nonzero limb `d`, without allocating a quotient — the remainder-only twin of
/// [`div_rem_limb_inplace`] (same reciprocal scan, quotient limbs discarded). `mag` is read-only.
fn rem_by_limb(mag: &[u64], d: u64) -> u64 {
    if mag.is_empty() {
        return 0;
    }
    if mag.len() == 1 {
        return mag[0] % d;
    }
    let sh = d.leading_zeros();
    let dn = d << sh;
    let v = reciprocal_2by1(dn);
    let n = mag.len();
    let mut rem;
    if sh == 0 {
        rem = 0u64;
        for &limb in mag.iter().rev() {
            (_, rem) = udiv_qrnnd_preinv(rem, limb, dn, v);
        }
    } else {
        rem = mag[n - 1] >> (64 - sh);
        for i in (0..n).rev() {
            let lo = if i == 0 { 0 } else { mag[i - 1] };
            let shifted = (mag[i] << sh) | (lo >> (64 - sh));
            (_, rem) = udiv_qrnnd_preinv(rem, shifted, dn, v);
        }
        rem >>= sh;
    }
    rem
}

/// Knuth's Algorithm D (TAOCP Vol. 2, §4.3.1) over base-2⁶⁴ limbs, for a divisor of ≥2 limbs. Requires
/// `a >= b` and `b`'s top limb nonzero (both hold via `divmod_mag`'s dispatch). The `u32` version in
/// Hacker's Delight §9-2 (`divmnu`) is the model, widened to `u64` limbs with `u128`/`i128` intermediates.
/// When `want_rem` is false, D8 (the remainder unnormalization) is skipped and an empty remainder is
/// returned — no remainder `Vec` is allocated.
fn knuth_divmod(a: &[u64], b: &[u64], want_rem: bool) -> (Vec<u64>, Vec<u64>) {
    let n = b.len(); // ≥ 2
    let m = a.len() - n; // a.len() ≥ n, so m ≥ 0
    let base = 1u128 << 64;

    // D1. Normalize so the divisor's top limb has its high bit set — this bounds the quotient-digit
    // estimate to at most 2 over the true digit. Shift both operands left by `s` bits.
    let s = b[n - 1].leading_zeros();
    let shl = |src: &[u64], dst: &mut [u64]| {
        if s == 0 {
            dst[..src.len()].copy_from_slice(src);
        } else {
            for i in (1..src.len()).rev() {
                dst[i] = (src[i] << s) | (src[i - 1] >> (64 - s));
            }
            dst[0] = src[0] << s;
        }
    };
    let mut vn = alloc::vec![0u64; n];
    shl(b, &mut vn);
    // `un` carries an extra high limb for the shift overflow (length m+n+1).
    let mut un = alloc::vec![0u64; m + n + 1];
    shl(a, &mut un);
    if s != 0 {
        un[m + n] = a[a.len() - 1] >> (64 - s);
    }

    // D1 leaves `vn[n-1]` normalized (top bit set), so its 2-by-1 reciprocal (built once, amortized over
    // all m+1 quotient digits) turns each qhat estimate's `128 ÷ 64` into a `wide_mul` — no per-digit
    // `u128` divide (a `__udivti3` libcall on aarch64/wasm).
    let vtop = vn[n - 1];
    let vrecip = reciprocal_2by1(vtop);

    let mut q = alloc::vec![0u64; m + 1];
    // D2–D7. One quotient digit per iteration, most-significant first.
    for j in (0..=m).rev() {
        // D3. Estimate qhat = ⌊(un[j+n]·B + un[j+n-1]) / vn[n-1]⌋, then correct it down. The `||`
        // short-circuit keeps qhat < B before the multiply test, so no intermediate overflows u128.
        let num = ((un[j + n] as u128) << 64) | (un[j + n - 1] as u128);
        // `udiv_qrnnd_preinv` needs the high word `< vtop`; after normalization `un[j+n] ≤ vtop`, and the
        // rare equality (qhat would reach B) falls back to the exact divide — same (qhat, rhat) domain.
        let (mut qhat, mut rhat): (u128, u128) = if un[j + n] < vtop {
            let (qh, rh) = udiv_qrnnd_preinv(un[j + n], un[j + n - 1], vtop, vrecip);
            (qh as u128, rh as u128)
        } else {
            (num / vtop as u128, num % vtop as u128)
        };
        loop {
            if qhat >= base || qhat * (vn[n - 2] as u128) > (rhat << 64) + (un[j + n - 2] as u128) {
                qhat -= 1;
                rhat += vn[n - 1] as u128;
                if rhat < base {
                    continue;
                }
            }
            break;
        }

        // D4. Multiply and subtract: un[j..=j+n] -= qhat · vn. `k`/`t` are signed (i128) borrow chains.
        let mut k: i128 = 0;
        for i in 0..n {
            let p = qhat * (vn[i] as u128);
            let t = un[j + i] as i128 - k - (p & 0xffff_ffff_ffff_ffff) as i128;
            un[j + i] = t as u64;
            k = (p >> 64) as i128 - (t >> 64);
        }
        let t = un[j + n] as i128 - k;
        un[j + n] = t as u64;

        // D5/D6. If the subtraction went negative, qhat was one too big: add the divisor back.
        if t < 0 {
            q[j] = qhat as u64 - 1;
            let mut carry: i128 = 0;
            for i in 0..n {
                let t = un[j + i] as i128 + vn[i] as i128 + carry;
                un[j + i] = t as u64;
                carry = t >> 64;
            }
            un[j + n] = (un[j + n] as i128 + carry) as u64; // final carry cancels the borrow
        } else {
            q[j] = qhat as u64;
        }
    }
    strip(&mut q);

    if !want_rem {
        return (q, Vec::new()); // quotient-only caller — skip building the remainder
    }
    // D8. Unnormalize the remainder (the low n limbs of un), shifting right by `s`.
    let mut r = alloc::vec![0u64; n];
    if s == 0 {
        r.copy_from_slice(&un[..n]);
    } else {
        for i in 0..n - 1 {
            r[i] = (un[i] >> s) | (un[i + 1] << (64 - s));
        }
        r[n - 1] = un[n - 1] >> s;
    }
    strip(&mut r);
    (q, r)
}

#[cfg(test)]
mod tests;
