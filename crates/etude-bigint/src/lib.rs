// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Arbitrary-precision signed integers — a small, hand-written `no_std` limb library. Pure over
//! `alloc::vec::Vec`, no I/O, no dependency. The surface is small (add/sub/mul/divmod/gcd/cmp +
//! from/to i64 + two byte encodings) over `Vec<u64>` limbs — schoolbook algorithms. Independently
//! unit-testable, with a differential test against `num-bigint` (a dev-dependency) as the safety net.
//!
//! # Representation
//! [`Big`] is `{ neg: bool, mag: Vec<u64> }` — base-2⁶⁴ limbs, LITTLE-ENDIAN (`mag[0]` is the
//! least-significant limb), with NO trailing zero limbs. Zero is the canonical `{ neg: false, mag: [] }`.
//! Every operation `normalize`s its result (strips trailing zero limbs; forces `neg = false` when the
//! magnitude is zero), so a value has exactly ONE in-memory form. This canonical form is required when a
//! `Big` is used as a map key or compared for equality: the sign-magnitude byte encoding
//! ([`Big::to_sign_magnitude_bytes`]) is what such comparisons operate on, so equal values MUST produce
//! identical bytes.

#![cfg_attr(not(test), no_std)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::vec::Vec;
use core::cmp::Ordering;

/// An arbitrary-precision signed integer. See the module doc for the canonical-form invariant.
///
/// The internal representation is PRIVATE and not part of the stable API — the limb width, endianness,
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
        match self.mag.last() {
            // Canonical form keeps the top limb nonzero, so `leading_zeros` gives the exact bit width.
            Some(&top) => self.mag.len() * 64 - top.leading_zeros() as usize,
            None => 0,
        }
    }

    /// The number of significant bytes in the magnitude — the length of the little-endian magnitude in
    /// [`Big::to_sign_magnitude_bytes`] (i.e. excluding the sign byte). `0` for zero. `O(1)`.
    pub fn byte_len(&self) -> usize {
        self.bit_len().div_ceil(8)
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

    /// `a - b` over magnitudes, REQUIRING `a >= b` (caller ensures via `cmp_mag`). Returns normalized.
    /// An `i128` difference holds a full `u64` limb minus another minus the borrow without overflow.
    fn sub_mag(a: &[u64], b: &[u64]) -> Vec<u64> {
        let mut out = Vec::with_capacity(a.len());
        let mut borrow = 0i128;
        for (i, &limb) in a.iter().enumerate() {
            let av = limb as i128;
            let bv = *b.get(i).unwrap_or(&0) as i128;
            let mut d = av - bv - borrow;
            if d < 0 {
                d += 1i128 << 64;
                borrow = 1;
            } else {
                borrow = 0;
            }
            out.push(d as u64);
        }
        strip(&mut out);
        out
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

    /// Three-way compare TWO values given their canonical sign-magnitude byte encodings DIRECTLY — the
    /// same result as decoding both to `Big` and calling [`Big::cmp`], but with NO limb `Vec` allocated.
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
    /// a parameter lets `sub` reuse this WITHOUT allocating a negated copy of `other`.
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

    /// `self - other`. Computes `self + (-other)` by flipping `other`'s sign as a parameter — no negated
    /// copy of `other`'s magnitude is allocated. (`other`'s sign is irrelevant when it is zero: an empty
    /// magnitude normalizes to canonical zero regardless.)
    pub fn sub(&self, other: &Big) -> Big {
        Big::add_signed(self.neg, &self.mag, !other.neg, &other.mag)
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

    /// The greatest common divisor of `|self|` and `|other|` — always NON-NEGATIVE (gcd is sign-agnostic:
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
            b = Big::sub_mag(&b, &a); // odd − odd = even, ≥ 0
            if b.is_empty() {
                break; // gcd of the odd parts is `a`
            }
        }
        shl_bits(&mut a, shift); // restore the common power of two
        let mut g = Big { neg: false, mag: a };
        g.normalize();
        g
    }

    // ─── conversions ──────────────────────────────────────────────────────────────────────────

    /// The DECIMAL string of this value (leading `-` if negative), size-independent. `0` → `"0"`.
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

    /// The checked i64 narrowing DIRECTLY from the canonical sign-magnitude byte encoding — the same
    /// result as `from_sign_magnitude_bytes(bytes).to_i64_checked()` but with NO limb `Vec` allocated,
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

    /// Read the value DIRECTLY from its canonical sign-magnitude byte encoding as an `i128`, or `None`
    /// if it needs more than 127 magnitude bits (i.e. cannot fit `i128`). Like
    /// [`Big::i64_checked_from_sign_magnitude_bytes`] but into the wider `i128`, so a small-operand
    /// arithmetic fast path can compute with native `checked_*` ops and NO limb `Vec`. Bytes are
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

    /// Serialize an `i128` DIRECTLY to the canonical sign-magnitude byte encoding in `buf`, returning the
    /// byte length — or `None` if they don't fit `buf`. The write half of a small-operand arithmetic fast
    /// path: an `i128` result serializes with NO intermediate `Big`/`Vec`. Byte-IDENTICAL to
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
        let mut out = Vec::new();
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

    /// Serialize the sign-magnitude bytes DIRECTLY into `buf` (no heap Vec), returning the byte length —
    /// or `None` if they don't fit (`buf` too small). A small-value fast path: a single-limb value is
    /// `[sign] + ≤8 magnitude bytes` = ≤9 bytes, so it serializes into a caller-provided buffer without
    /// a transient `Vec`. Byte-IDENTICAL to [`Big::to_sign_magnitude_bytes`].
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

    /// Serialize to little-endian two's-complement bytes. The MINIMAL length that round-trips: the sign
    /// bit of the top byte must equal the value's sign, so a positive value whose top byte is ≥0x80 gets
    /// a `0x00` guard byte, and a negative one whose top byte is <0x80 gets a `0xff` guard byte. Zero is
    /// the empty slice.
    pub fn to_le_twos_complement_bytes(&self) -> Vec<u8> {
        if self.is_zero() {
            return Vec::new();
        }
        // Magnitude → LE bytes (strip trailing zeros).
        let mut mbytes = Vec::new();
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
/// has native 64-bit integers but EMULATES 128-bit ones (a `u64 * u64 -> u128` lowers to a `__multi3`
/// libcall) — it is synthesized from four native `u32 * u32 -> u64` partial products, so it stays on
/// native wasm `i64` ops with no 128-bit intrinsic. The limb STORAGE is `u64` on both (native on wasm);
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

/// Append the base-10 digits of `v` (most-significant first) to `digits`, zero-padded to at least
/// `pad` digits. `pad == 0` emits the natural length (used for the most-significant chunk); a following
/// chunk uses `pad == 19` so its leading zeros are preserved in the concatenation.
/// Largest power of ten below `2^64` (fits one limb — the single-limb divisor fast path), and its
/// decimal-digit count. Peeling by it extracts 19 digits per division step.
const DECIMAL_CHUNK: u64 = 10_000_000_000_000_000_000; // 10^19 < 2^64
const DECIMAL_CHUNK_DIGITS: usize = 19;

/// At or below this many limbs the linear chunk method wins: the recursive split's big divmods and
/// power-of-ten stack cost more than they save until the magnitude is wide. Tuned on the `to_decimal`
/// benchmark (the 64b/256b tiers stay linear; the 1024b+ tiers go recursive).
const DECIMAL_RECURSIVE_THRESHOLD: usize = 10;

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
    // Power stack: pow[0] = 10^19, pow[i] = pow[i-1]² = 10^(19·2^i). Square up until it strictly
    // exceeds the value, so the top entry bounds it (value < pow[level]).
    let mut pow: Vec<Vec<u64>> = alloc::vec![alloc::vec![DECIMAL_CHUNK]];
    while Big::cmp_mag(pow.last().unwrap(), mag) != Ordering::Greater {
        let top = pow.last().unwrap();
        let sq = Big::mul_mag(top, top);
        pow.push(sq);
    }
    let level = pow.len() - 1; // pow[level] > value ≥ pow[level-1]
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
/// the running quotient IN PLACE, then emit most-significant chunk first at natural width, the rest
/// zero-padded to 19. Both the quotient and the chunk list live in STACK buffers (bounded by the limb
/// threshold), so this path allocates nothing. `mag` nonzero.
fn emit_decimal_linear<W: core::fmt::Write>(mag: &[u64], w: &mut W) -> core::fmt::Result {
    let mut cur = [0u64; DECIMAL_RECURSIVE_THRESHOLD];
    cur[..mag.len()].copy_from_slice(mag);
    let mut len = mag.len();
    let mut chunks = [0u64; MAX_LINEAR_CHUNKS];
    let mut n = 0;
    let d = DECIMAL_CHUNK as u128;
    while len > 0 {
        // Divide cur[..len] by 10^19 in place (most-significant limb first); `rem` is the peeled chunk.
        let mut rem = 0u128;
        for limb in cur[..len].iter_mut().rev() {
            let c = (rem << 64) | *limb as u128; // rem < 10^19 ≤ 2^64, so this fits u128
            *limb = (c / d) as u64;
            rem = c % d;
        }
        while len > 0 && cur[len - 1] == 0 {
            len -= 1; // strip high zero limbs off the shrinking quotient
        }
        chunks[n] = rem as u64;
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

/// Format one `10^19` chunk `v` into a fixed stack buffer — most-significant digit first, left-padded
/// with `'0'` to at least `pad` digits — and write it to the sink in a single `write_str` (the bytes
/// are ASCII digits, hence valid UTF-8). No allocation.
fn write_decimal_chunk<W: core::fmt::Write>(
    w: &mut W,
    mut v: u64,
    pad: usize,
) -> core::fmt::Result {
    let mut lsf = [0u8; 20]; // digits least-significant first; a u64 is at most 20 decimal digits
    let mut n = 0;
    if v == 0 {
        lsf[0] = b'0';
        n = 1;
    } else {
        while v > 0 {
            lsf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
    }
    // Assemble most-significant first into `out`, left-padding with '0' up to `pad` (pad ≤ 19, n ≤ 20).
    let width = n.max(pad);
    let mut out = [0u8; 20];
    for (i, slot) in out[..width].iter_mut().enumerate() {
        let from_right = width - 1 - i; // position i from the left is digit `width-1-i` from the right
        *slot = if from_right < n {
            lsf[from_right]
        } else {
            b'0'
        };
    }
    w.write_str(core::str::from_utf8(&out[..width]).expect("ascii digits"))
}

/// Below this many limbs (in the SMALLER operand), schoolbook multiply beats Karatsuba (whose
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

/// Count of trailing zero BITS in a nonzero little-endian magnitude (its 2-adic valuation).
fn trailing_zeros_mag(m: &[u64]) -> usize {
    for (i, &limb) in m.iter().enumerate() {
        if limb != 0 {
            return i * 64 + limb.trailing_zeros() as usize;
        }
    }
    0 // all-zero magnitude — callers guard against this
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
/// `0 <= remainder < b`. `b` MUST be non-empty (nonzero — the caller checks). Both results normalized.
/// Dispatches by divisor width: `a < b` is trivial, a single-limb divisor uses a linear scan, and a
/// multi-limb divisor uses Knuth's Algorithm D (word-at-a-time long division).
fn divmod_mag(a: &[u64], b: &[u64]) -> (Vec<u64>, Vec<u64>) {
    // a < b → quotient 0, remainder a.
    if Big::cmp_mag(a, b) == Ordering::Less {
        return (Vec::new(), a.to_vec());
    }
    if b.len() == 1 {
        return divmod_by_limb(a, b[0]);
    }
    knuth_divmod(a, b)
}

/// Divide a magnitude by a single nonzero limb: `(quotient, remainder)`. One `u128` division per limb,
/// most-significant first, carrying the running remainder (always `< d`, so it fits a single limb).
fn divmod_by_limb(a: &[u64], d: u64) -> (Vec<u64>, Vec<u64>) {
    let mut q = a.to_vec();
    let rem = div_rem_limb_inplace(&mut q, d);
    let r = if rem == 0 {
        Vec::new()
    } else {
        alloc::vec![rem]
    };
    (q, r)
}

/// Divide `m` by a single nonzero limb IN PLACE — `m` becomes the quotient (normalized) — returning the
/// remainder (`< d`, so it fits one limb). One `u128` division per limb, most-significant first. Used by
/// the decimal render's chunk loop to avoid a fresh quotient `Vec` per step.
fn div_rem_limb_inplace(m: &mut Vec<u64>, d: u64) -> u64 {
    let d = d as u128;
    let mut rem = 0u128;
    for limb in m.iter_mut().rev() {
        let cur = (rem << 64) | *limb as u128; // rem < d ≤ 2^64, so this fits u128
        *limb = (cur / d) as u64;
        rem = cur % d;
    }
    strip(m);
    rem as u64
}

/// Knuth's Algorithm D (TAOCP Vol. 2, §4.3.1) over base-2⁶⁴ limbs, for a divisor of ≥2 limbs. Requires
/// `a >= b` and `b`'s top limb nonzero (both hold via `divmod_mag`'s dispatch). The `u32` version in
/// Hacker's Delight §9-2 (`divmnu`) is the model, widened to `u64` limbs with `u128`/`i128` intermediates.
fn knuth_divmod(a: &[u64], b: &[u64]) -> (Vec<u64>, Vec<u64>) {
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

    let mut q = alloc::vec![0u64; m + 1];
    // D2–D7. One quotient digit per iteration, most-significant first.
    for j in (0..=m).rev() {
        // D3. Estimate qhat = ⌊(un[j+n]·B + un[j+n-1]) / vn[n-1]⌋, then correct it down. The `||`
        // short-circuit keeps qhat < B before the multiply test, so no intermediate overflows u128.
        let num = ((un[j + n] as u128) << 64) | (un[j + n - 1] as u128);
        let mut qhat = num / vn[n - 1] as u128;
        let mut rhat = num % vn[n - 1] as u128;
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
