// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Exact base-10 arbitrary-precision decimal numbers — a `coeff * 10^exp` value over
//! [`etude_bigint::Big`]. Pure over `alloc`, no I/O, no dependency but `etude-bigint`. Built for the
//! JSON decoder: a JSON number literal (`-?int(.frac)?([eE][+-]?exp)?`) decodes into a [`Decimal`]
//! LOSSLESSLY, unlike an `f64` (which loses precision) or a rational (which would need gcd reduction and
//! cannot preserve scale). Exact arithmetic ([`Decimal::add`]/[`Decimal::sub`]/[`Decimal::mul`]) never
//! rounds a digit away; division (which needs a rounding policy) is a later addition. Correctness is
//! pinned by a differential test against `bigdecimal` (a dev-dependency) as the reference.
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
//! [`Decimal::new`], [`Decimal::from_ascii`]/[`Decimal::from_str`]; inspect through
//! [`Decimal::coefficient`], [`Decimal::exponent`], [`Decimal::is_zero`], [`Decimal::is_negative`],
//! [`Decimal::is_integer`], [`Decimal::to_f64`], the [`Ord`]/[`PartialOrd`] comparison, and `Display`.

#![cfg_attr(not(test), no_std)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::string::String;
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

    /// Parse a JSON number from ASCII bytes into an exact `Decimal`, or `None` if the bytes are not a
    /// well-formed JSON number. The accepted grammar is exactly JSON's:
    ///
    /// ```text
    /// -? ( 0 | [1-9][0-9]* ) ( . [0-9]+ )? ( [eE] [+-]? [0-9]+ )?
    /// ```
    ///
    /// so a leading `+`, a leading zero (`01`), a bare `.5`, a trailing `1.`, a lone `-`, an empty
    /// exponent (`1e`), and any surrounding whitespace or trailing garbage are all REJECTED. The decode
    /// is LOSSLESS: every significant digit becomes part of the coefficient and the decimal-point /
    /// exponent set `exp`. An exponent literal with more than 18 digits (beyond `i64` range) is
    /// rejected (returns `None`) rather than silently wrapping.
    pub fn from_ascii(bytes: &[u8]) -> Option<Decimal> {
        let mut i = 0;
        let n = bytes.len();
        if n == 0 {
            return None;
        }

        // Optional leading minus (JSON forbids a leading plus).
        let neg = bytes[0] == b'-';
        if neg {
            i += 1;
        }

        // Integer part: "0" alone, or [1-9] followed by any digits. No leading zeros.
        let int_start = i;
        if i >= n || !bytes[i].is_ascii_digit() {
            return None;
        }
        if bytes[i] == b'0' {
            i += 1;
            // A "0" may not be followed by another digit (no "00", "01").
            if i < n && bytes[i].is_ascii_digit() {
                return None;
            }
        } else {
            while i < n && bytes[i].is_ascii_digit() {
                i += 1;
            }
        }
        let int_digits = &bytes[int_start..i];

        // Optional fractional part: a dot followed by at least one digit.
        let mut frac_digits: &[u8] = &[];
        if i < n && bytes[i] == b'.' {
            i += 1;
            let frac_start = i;
            while i < n && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i == frac_start {
                return None; // a dot must be followed by at least one digit
            }
            frac_digits = &bytes[frac_start..i];
        }

        // Optional exponent: e/E, optional sign, at least one digit.
        let mut exp_val: i64 = 0;
        if i < n && (bytes[i] == b'e' || bytes[i] == b'E') {
            i += 1;
            let exp_neg = if i < n && (bytes[i] == b'+' || bytes[i] == b'-') {
                let s = bytes[i] == b'-';
                i += 1;
                s
            } else {
                false
            };
            let exp_start = i;
            while i < n && bytes[i].is_ascii_digit() {
                // An exponent with more than 18 digits cannot be reasoned about in an i64; reject it
                // rather than silently wrapping. (18 nines ≈ 1e18 < i64::MAX.)
                if i - exp_start >= 18 {
                    return None;
                }
                exp_val = exp_val * 10 + (bytes[i] - b'0') as i64;
                i += 1;
            }
            if i == exp_start {
                return None; // an exponent marker must be followed by at least one digit
            }
            if exp_neg {
                exp_val = -exp_val;
            }
        }

        // No trailing garbage.
        if i != n {
            return None;
        }

        // The coefficient is the concatenation of the integer and fractional digits; each fractional
        // digit shifts the point right, i.e. lowers the exponent by one.
        let mag = big_from_ascii_digits(int_digits, frac_digits);
        let exp = exp_val.checked_sub(frac_digits.len() as i64)?;
        let coeff = if neg { mag.neg() } else { mag };
        Some(Decimal::new(coeff, exp))
    }

    /// Convert to the nearest `f64` (correctly rounded via the standard library's float parser).
    /// Magnitudes beyond `f64` range become `±∞`, matching `f64` decimal parsing.
    pub fn to_f64(&self) -> f64 {
        // Building the canonical decimal string and letting the (correctly-rounded) std parser do the
        // work is exact-input rounding without reimplementing decimal-to-binary conversion.
        self.render().parse::<f64>().unwrap_or(f64::NAN)
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

    /// Compare `|self|` against `|other|` (both assumed nonzero). Exact and allocation-light: it first
    /// compares the adjusted exponent (the base-10 order of magnitude of the most-significant digit),
    /// and only when those tie does it compare the digit strings aligned at that most-significant digit
    /// — so no power-of-ten scaling is ever materialized.
    fn cmp_magnitude(&self, other: &Decimal) -> Ordering {
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

    /// Render the canonical decimal string. The output is always a valid JSON number and re-parses via
    /// [`Decimal::from_ascii`] to the same value. A plain (point) form is used for modest exponents; a
    /// bounded `<digits>e<exp>` scientific form is used for large magnitudes so the string stays small.
    fn render(&self) -> String {
        use core::fmt::Write;
        if self.is_zero() {
            return String::from("0");
        }
        let neg = self.coeff.is_negative();
        let mag = self.coeff.abs().to_decimal_string(); // digits only, no sign, no leading zero
        let mut out = String::new();
        if neg {
            out.push('-');
        }
        // Threshold that bounds the plain-form length; beyond it, fall back to scientific notation. All
        // comparisons stay in i64 (narrowing to usize only once bounded ≤ PLAIN_PAD) so a large
        // exponent cannot truncate on a 32-bit-usize target like wasm32.
        const PLAIN_PAD: i64 = 30;
        if self.exp == 0 {
            out.push_str(&mag);
        } else if self.exp > 0 && self.exp <= PLAIN_PAD {
            out.push_str(&mag);
            for _ in 0..self.exp {
                out.push('0');
            }
        } else if self.exp < 0 {
            let k = -self.exp; // positive point shift, in i64
            let l = mag.len() as i64;
            if k < l {
                // Point sits inside the digit string: "ddd.ddd".
                let cut = (l - k) as usize;
                out.push_str(&mag[..cut]);
                out.push('.');
                out.push_str(&mag[cut..]);
            } else if k <= l + PLAIN_PAD {
                // "0.00…ddd" — leading zeros before the significant digits.
                out.push_str("0.");
                for _ in 0..(k - l) {
                    out.push('0');
                }
                out.push_str(&mag);
            } else {
                let _ = write!(out, "{mag}e{}", self.exp);
            }
        } else {
            let _ = write!(out, "{mag}e{}", self.exp);
        }
        out
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

/// Build a nonnegative [`Big`] from the concatenation of two ASCII digit slices (integer part, then
/// fractional part). Accumulates in ≤18-digit chunks (each fits an `i64`) to avoid an O(n²) per-digit
/// multiply chain.
fn big_from_ascii_digits(int_digits: &[u8], frac_digits: &[u8]) -> Big {
    let mut acc = Big::zero();
    let mut chunk_val: i64 = 0;
    let mut chunk_len: u32 = 0;
    let flush = |acc: &mut Big, chunk_val: &mut i64, chunk_len: &mut u32| {
        if *chunk_len == 0 {
            return;
        }
        // acc = acc * 10^chunk_len + chunk_val
        let scale = Big::from_i64(10i64.pow(*chunk_len));
        *acc = acc.mul(&scale).add(&Big::from_i64(*chunk_val));
        *chunk_val = 0;
        *chunk_len = 0;
    };
    for &d in int_digits.iter().chain(frac_digits.iter()) {
        chunk_val = chunk_val * 10 + (d - b'0') as i64;
        chunk_len += 1;
        if chunk_len == 18 {
            flush(&mut acc, &mut chunk_val, &mut chunk_len);
        }
    }
    flush(&mut acc, &mut chunk_val, &mut chunk_len);
    acc
}

impl core::fmt::Display for Decimal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.render())
    }
}

/// Parse error for [`Decimal::from_str`] / [`str::parse`]. Carries no detail — the input was not a
/// well-formed JSON number (see [`Decimal::from_ascii`] for the grammar).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ParseDecimalError;

impl core::fmt::Display for ParseDecimalError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("invalid decimal (not a well-formed JSON number)")
    }
}

impl FromStr for Decimal {
    type Err = ParseDecimalError;

    fn from_str(s: &str) -> Result<Decimal, ParseDecimalError> {
        Decimal::from_ascii(s.as_bytes()).ok_or(ParseDecimalError)
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
