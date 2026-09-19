// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Arbitrary-precision signed integers — a small, hand-written `no_std` limb library. Pure over
//! `alloc::vec::Vec`, no I/O, no dependency. The surface is small (add/sub/mul/divmod/gcd/cmp +
//! from/to i64 + two byte encodings) over `Vec<u32>` limbs — schoolbook algorithms. Independently
//! unit-testable, with a differential test against `num-bigint` (a dev-dependency) as the safety net.
//!
//! # Representation
//! [`Big`] is `{ neg: bool, mag: Vec<u32> }` — base-2³² limbs, LITTLE-ENDIAN (`mag[0]` is the
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
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Big {
    /// Sign: `true` = negative. Always `false` when `mag` is empty (zero is non-negative, canonical).
    pub neg: bool,
    /// Magnitude limbs, base 2³², little-endian, no trailing zero limbs (empty = zero).
    pub mag: Vec<u32>,
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
    fn cmp_mag(a: &[u32], b: &[u32]) -> Ordering {
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

    /// `a + b` over magnitudes (little-endian limbs), returning a normalized magnitude.
    fn add_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
        let mut out = Vec::with_capacity(a.len().max(b.len()) + 1);
        let mut carry = 0u64;
        for i in 0..a.len().max(b.len()) {
            let av = *a.get(i).unwrap_or(&0) as u64;
            let bv = *b.get(i).unwrap_or(&0) as u64;
            let s = av + bv + carry;
            out.push((s & 0xffff_ffff) as u32);
            carry = s >> 32;
        }
        if carry != 0 {
            out.push(carry as u32);
        }
        strip(&mut out);
        out
    }

    /// `a - b` over magnitudes, REQUIRING `a >= b` (caller ensures via `cmp_mag`). Returns normalized.
    fn sub_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
        let mut out = Vec::with_capacity(a.len());
        let mut borrow = 0i64;
        for (i, &limb) in a.iter().enumerate() {
            let av = limb as i64;
            let bv = *b.get(i).unwrap_or(&0) as i64;
            let mut d = av - bv - borrow;
            if d < 0 {
                d += 1i64 << 32;
                borrow = 1;
            } else {
                borrow = 0;
            }
            out.push(d as u32);
        }
        strip(&mut out);
        out
    }

    /// `a * b` over magnitudes (O(n·m) schoolbook), returning a normalized magnitude.
    fn mul_mag(a: &[u32], b: &[u32]) -> Vec<u32> {
        if a.is_empty() || b.is_empty() {
            return Vec::new();
        }
        let mut out = alloc::vec![0u32; a.len() + b.len()];
        for (i, &av) in a.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &bv) in b.iter().enumerate() {
                let cur = out[i + j] as u64 + (av as u64) * (bv as u64) + carry;
                out[i + j] = (cur & 0xffff_ffff) as u32;
                carry = cur >> 32;
            }
            // Propagate the final carry into the next limb (and beyond, if it cascades).
            let mut k = i + b.len();
            while carry != 0 {
                let cur = out[k] as u64 + carry;
                out[k] = (cur & 0xffff_ffff) as u32;
                carry = cur >> 32;
                k += 1;
            }
        }
        strip(&mut out);
        out
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

    /// `self + other`.
    pub fn add(&self, other: &Big) -> Big {
        let mut r = if self.neg == other.neg {
            // Same sign: add magnitudes, keep the sign.
            Big {
                neg: self.neg,
                mag: Big::add_mag(&self.mag, &other.mag),
            }
        } else {
            // Opposite signs: subtract the smaller magnitude from the larger; sign follows the larger.
            match Big::cmp_mag(&self.mag, &other.mag) {
                Ordering::Equal => Big::zero(),
                Ordering::Greater => Big {
                    neg: self.neg,
                    mag: Big::sub_mag(&self.mag, &other.mag),
                },
                Ordering::Less => Big {
                    neg: other.neg,
                    mag: Big::sub_mag(&other.mag, &self.mag),
                },
            }
        };
        r.normalize();
        r
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

    /// `self - other`.
    pub fn sub(&self, other: &Big) -> Big {
        self.add(&other.neg())
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
    /// `gcd(a, b) = gcd(|a|, |b|)`). `gcd(0, 0) = 0`; `gcd(a, 0) = |a|`. Euclid over magnitudes via
    /// `divmod_mag` (the remainder shrinks each step).
    pub fn gcd(&self, other: &Big) -> Big {
        let mut a = self.mag.clone(); // |self|
        let mut b = other.mag.clone(); // |other|
        while !b.is_empty() {
            let (_q, r) = divmod_mag(&a, &b); // r = a mod b, normalized (no trailing zeros)
            a = b;
            b = r;
        }
        // `a` is the gcd magnitude (empty iff both inputs were zero). Non-negative by construction.
        let mut g = Big { neg: false, mag: a };
        g.normalize();
        g
    }

    // ─── conversions ──────────────────────────────────────────────────────────────────────────

    /// The DECIMAL string of this value (leading `-` if negative), size-independent — extracts digits by
    /// repeated division by 10 over the magnitude. `0` → `"0"`.
    pub fn to_decimal_string(&self) -> alloc::string::String {
        use alloc::string::String;
        if self.is_zero() {
            return String::from("0");
        }
        let ten = Big::from_i64(10);
        let mut cur = Big {
            neg: false,
            mag: self.mag.clone(),
        }; // work over the magnitude
        let mut digits: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        while !cur.is_zero() {
            let (q, r) = cur.divmod(&ten).expect("divisor 10 is nonzero");
            let d = r.mag.first().copied().unwrap_or(0) as u8; // 0..=9 fits one limb
            digits.push(b'0' + d);
            cur = q;
        }
        if self.neg {
            digits.push(b'-');
        }
        digits.reverse();
        String::from_utf8(digits).expect("ascii digits")
    }

    /// Box a signed 64-bit int as a `Big`.
    pub fn from_i64(v: i64) -> Big {
        if v == 0 {
            return Big::zero();
        }
        let neg = v < 0;
        let m = v.unsigned_abs(); // handles i64::MIN without overflow
        let mut mag = Vec::new();
        mag.push((m & 0xffff_ffff) as u32);
        let hi = (m >> 32) as u32;
        if hi != 0 {
            mag.push(hi);
        }
        Big { neg, mag }
    }

    /// Narrow to `i64` if it fits, else `None`.
    pub fn to_i64_checked(&self) -> Option<i64> {
        if self.mag.len() > 2 {
            return None;
        }
        let lo = *self.mag.first().unwrap_or(&0) as u64;
        let hi = *self.mag.get(1).unwrap_or(&0) as u64;
        let m = lo | (hi << 32); // magnitude as u64
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
        // Limbs (LE u32) → LE bytes, then strip trailing zero bytes for canonicality.
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
    /// `[sign] + ≤4 magnitude bytes` = ≤5 bytes, so it serializes into a caller-provided buffer without
    /// a transient `Vec`. Byte-IDENTICAL to [`Big::to_sign_magnitude_bytes`].
    pub fn to_sign_magnitude_bytes_into(&self, buf: &mut [u8]) -> Option<usize> {
        let need = 1 + self.mag.len() * 4; // upper bound before the trailing-zero strip
        if need > buf.len() {
            return None; // caller falls back to the heap `to_sign_magnitude_bytes`
        }
        buf[0] = self.neg as u8;
        let mut n = 1;
        for &limb in &self.mag {
            buf[n..n + 4].copy_from_slice(&limb.to_le_bytes());
            n += 4;
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
        let mut mag = Vec::with_capacity(mag_bytes.len().div_ceil(4));
        let mut i = 0;
        while i < mag_bytes.len() {
            let mut limb = [0u8; 4];
            let k = (mag_bytes.len() - i).min(4);
            limb[..k].copy_from_slice(&mag_bytes[i..i + k]);
            mag.push(u32::from_le_bytes(limb));
            i += 4;
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
        // LE bytes → LE u32 limbs.
        let mut mag = Vec::with_capacity(mbytes.len().div_ceil(4));
        let mut i = 0;
        while i < mbytes.len() {
            let mut limb = [0u8; 4];
            let k = (mbytes.len() - i).min(4);
            limb[..k].copy_from_slice(&mbytes[i..i + k]);
            mag.push(u32::from_le_bytes(limb));
            i += 4;
        }
        let mut b = Big { neg, mag };
        b.normalize();
        b
    }
}

/// Strip trailing zero limbs from a magnitude (little-endian).
fn strip(v: &mut Vec<u32>) {
    while v.last() == Some(&0) {
        v.pop();
    }
}

/// Unsigned long division of magnitudes: `(quotient, remainder)` with `a = quotient * b + remainder`,
/// `0 <= remainder < b`. `b` MUST be non-empty (nonzero — the caller checks). Bit-at-a-time long
/// division (simple + obviously-correct). Both results normalized.
fn divmod_mag(a: &[u32], b: &[u32]) -> (Vec<u32>, Vec<u32>) {
    // a < b → quotient 0, remainder a.
    if Big::cmp_mag(a, b) == Ordering::Less {
        return (Vec::new(), a.to_vec());
    }
    let nbits = a.len() * 32;
    let mut q = alloc::vec![0u32; a.len()];
    let mut r: Vec<u32> = Vec::new(); // running remainder, normalized (no trailing zeros)
    // Process dividend bits from most-significant to least.
    for i in (0..nbits).rev() {
        // r <<= 1
        shl1(&mut r);
        // bring down bit i of a into r's bit 0
        let bit = (a[i / 32] >> (i % 32)) & 1;
        if bit != 0 {
            if r.is_empty() {
                r.push(1);
            } else {
                r[0] |= 1;
            }
        }
        // if r >= b { r -= b; set quotient bit i }
        if Big::cmp_mag(&r, b) != Ordering::Less {
            r = Big::sub_mag(&r, b);
            q[i / 32] |= 1 << (i % 32);
        }
    }
    strip(&mut q);
    strip(&mut r);
    (q, r)
}

/// `r <<= 1` over a little-endian limb magnitude (normalized in/out).
fn shl1(r: &mut Vec<u32>) {
    let mut carry = 0u32;
    for limb in r.iter_mut() {
        let hi = *limb >> 31;
        *limb = (*limb << 1) | carry;
        carry = hi;
    }
    if carry != 0 {
        r.push(carry);
    }
}

#[cfg(test)]
mod tests;
