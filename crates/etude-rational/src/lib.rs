// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Exact rational numbers — a normalized `num/den` pair over [`etude_bigint::Big`] arbitrary-precision
//! integers. Pure over `alloc`, no I/O, no dependency but `etude-bigint`. Exact `+ - * /` and comparison
//! over the normalized pair, with a differential test against `num-rational` (a dev-dependency) as the
//! safety net.
//!
//! # Representation and the canonical-form invariant
//! [`Rational`] is a `{ num: Big, den: Big }` pair kept in a SINGLE canonical form:
//! - the denominator is STRICTLY POSITIVE (`den >= 1`), so the sign lives entirely on the numerator;
//! - the pair is in LOWEST TERMS (`gcd(|num|, den) == 1`);
//! - zero is exactly `0/1`; an integer `n` is exactly `n/1`.
//!
//! Every constructor and operation renormalizes, so a value has exactly ONE in-memory form. This is
//! required for `Eq`/`Ord`/hashing to mean mathematical equality: `1/2` and `2/4` are the SAME value and
//! MUST have identical fields. There is no representation of a zero-denominator rational — the fallible
//! constructors return `None` and the total operations cannot produce one.
//!
//! The fields are PRIVATE and not part of the stable API. Construct through [`Rational::zero`],
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
/// The internal `{ num, den }` representation is PRIVATE and not part of the stable API. Construct values
/// through the constructors and inspect them through the accessors.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Rational {
    /// Numerator; carries the sign of the whole value. Canonical: `gcd(|num|, den) == 1`.
    num: Big,
    /// Denominator; always STRICTLY POSITIVE and coprime to `num`. Never zero.
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
        self.den == Big::from_i64(1)
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
    /// is symmetric, so `den/num` is ALREADY in lowest terms — only the sign is moved onto the numerator
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
        let num = self.num.mul(&other.den).add(&other.num.mul(&self.den));
        let den = self.den.mul(&other.den);
        // Both denominators are strictly positive, so the product is nonzero: normalize cannot fail.
        normalize(num, den).expect("product of positive denominators is nonzero")
    }

    /// Exact difference `self - other`.
    pub fn sub(&self, other: &Rational) -> Rational {
        let num = self.num.mul(&other.den).sub(&other.num.mul(&self.den));
        let den = self.den.mul(&other.den);
        normalize(num, den).expect("product of positive denominators is nonzero")
    }

    /// Exact product `self * other`. `(a/b) * (c/d) = (a*c)/(b*d)`, renormalized.
    pub fn mul(&self, other: &Rational) -> Rational {
        let num = self.num.mul(&other.num);
        let den = self.den.mul(&other.den);
        normalize(num, den).expect("product of positive denominators is nonzero")
    }

    /// Exact quotient `self / other`. `(a/b) / (c/d) = (a*d)/(b*c)`, renormalized. Returns `None` when
    /// `other` is zero.
    pub fn div(&self, other: &Rational) -> Option<Rational> {
        let num = self.num.mul(&other.den);
        let den = self.den.mul(&other.num);
        normalize(num, den)
    }

    /// The decimal string `"num/den"` (e.g. `"-3/10"`), or just `"num"` when the value is an integer.
    pub fn to_decimal_string(&self) -> String {
        if self.is_integer() {
            return self.num.to_decimal_string();
        }
        let mut s = self.num.to_decimal_string();
        s.push('/');
        s.push_str(&self.den.to_decimal_string());
        s
    }
}

impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Rational) -> Option<Ordering> {
        Some(Ord::cmp(self, other))
    }
}

impl Ord for Rational {
    /// Exact three-way comparison. Both denominators are strictly positive, so `a/b ? c/d` is decided by
    /// the cross-products `a*d ? c*b`.
    fn cmp(&self, other: &Rational) -> Ordering {
        let lhs = self.num.mul(&other.den);
        let rhs = other.num.mul(&self.den);
        lhs.cmp(&rhs)
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
    if g == Big::from_i64(1) {
        // Already coprime — skip the two divisions (the common case for freshly built pairs).
        return Some(Rational { num, den });
    }
    let (num_reduced, _) = num.divmod(&g).expect("gcd is nonzero");
    let (den_reduced, _) = den.divmod(&g).expect("gcd is nonzero");
    Some(Rational {
        num: num_reduced,
        den: den_reduced,
    })
}

#[cfg(test)]
mod tests;
