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

use alloc::borrow::Cow;
use alloc::string::String;
use core::cmp::Ordering;
use etude_bigint::Big;

/// An exact rational number in canonical form. See the module doc for the invariant.
///
/// The internal `{ num, den }` representation is private and not part of the stable API. Construct values
/// through the constructors and inspect them through the accessors.
///
/// # Examples
/// ```
/// use etude_rational::Rational;
/// use etude_bigint::Big;
///
/// // Method-form arithmetic (no operator overloads); equality is by mathematical value.
/// let half = Rational::from_ratio_i64(1, 2).unwrap();
/// let third = Rational::from_ratio_i64(1, 3).unwrap();
/// assert_eq!(half.add(&third), Rational::from_ratio_i64(5, 6).unwrap()); // 1/2 + 1/3 = 5/6
///
/// // Values are kept in lowest terms, so equal fractions are equal and share identical fields:
/// // 2/4 is the same value as 1/2.
/// let reduced = Rational::from_ratio_i64(2, 4).unwrap();
/// assert_eq!(reduced, half);
/// assert_eq!(reduced.numer(), &Big::from_i64(1));
/// assert_eq!(reduced.denom(), &Big::from_i64(2));
///
/// // Division is fallible only on a zero divisor; the sign lives on the numerator.
/// assert!(Rational::one().div(&Rational::zero()).is_none());
/// assert!(Rational::from_ratio_i64(-1, 2).unwrap().is_negative());
/// ```
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

    /// The additive inverse `-self`, consuming `self`. Same result as [`Rational::neg`], but **zero
    /// allocation**: the numerator's sign flips in place (`Big::negate`, an `O(1)` sign-bit toggle — no
    /// magnitude copy) and the already-positive denominator is moved unchanged. The value stays canonical.
    pub fn into_neg(self) -> Rational {
        let Rational { mut num, den } = self;
        num.negate();
        Rational { num, den }
    }

    /// The absolute value `|self|`, consuming `self`. Same result as [`Rational::abs`], but **zero
    /// allocation**: the numerator's sign clears in place (`Big::abs_assign`, an `O(1)` sign-bit clear —
    /// no magnitude copy) and the already-positive denominator is moved unchanged.
    pub fn into_abs(self) -> Rational {
        let Rational { mut num, den } = self;
        num.abs_assign();
        Rational { num, den }
    }

    /// The reciprocal `1/self` (`den/num`), consuming `self`. Returns `None` when `self` is zero.
    ///
    /// Same result as [`Rational::recip`] (no gcd is needed — the reciprocal of a canonical value is
    /// canonical), but it reuses the owned components instead of cloning them. For a positive numerator
    /// the two fields just swap, so the common case allocates nothing at all; a negative numerator negates
    /// both terms to keep the denominator positive (as `recip` does).
    pub fn into_recip(self) -> Option<Rational> {
        if self.num.is_zero() {
            return None;
        }
        let Rational { num, den } = self;
        if num.is_negative() {
            // num < 0 ⇒ raw `den/num` has a negative denominator; handled out of line (see
            // `recip_negative`). Cold + not inlined so this rare branch's locals do not perturb the hot
            // positive-swap codegen — inlining it in place cost the positive branch ~6 ns (#299 → this fix).
            Some(recip_negative(num, den))
        } else {
            // num > 0 ⇒ `den/num` is already canonical: swap the owned components, zero allocation.
            Some(Rational { num: den, den: num })
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
        addsub_big(&self.num, &self.den, &other.num, &other.den, false)
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
        addsub_big(&self.num, &self.den, &other.num, &other.den, true)
    }

    /// Native `i128` add (`subtract == false`) or subtract for the general different-denominator case when
    /// all components fit `i64`: `(a*d ± c*b)/(b*d)`. `a*d`, `c*b`, `b*d` each fit `i128`, but the
    /// numerator `a*d ± c*b` (two terms up to `2^126`) can overflow `i128`, so it is checked — on overflow
    /// (or any component exceeding `i64`) return `None` to fall back to the `Big` path.
    fn addsub_small(&self, other: &Rational, subtract: bool) -> Option<Rational> {
        let a = self.num.to_i64_checked()?;
        let b = self.den.to_i64_checked()?;
        let c = other.num.to_i64_checked()?;
        let d = other.den.to_i64_checked()?;
        let ad = a as i128 * d as i128; // fits i128 (|a*d| <= 2^126)
        let cb = c as i128 * b as i128; // fits i128
        let num = if subtract {
            ad.checked_sub(cb)?
        } else {
            ad.checked_add(cb)?
        };
        let den = b as i128 * d as i128; // b, d > 0 ⇒ den > 0
        // Reduce over gcd(b, d) on the DENOMINATORS (a u64 hardware-divide gcd), as in the Big `add`/`sub`
        // path: when the denominators are coprime (the common case) `(a*d ± c*b)/(b*d)` is already in lowest
        // terms, so skip the wide gcd over the ~127-bit product entirely (canonicalizing zero to 0/1).
        if gcd_u64(b.unsigned_abs(), d.unsigned_abs()) == 1 {
            if num == 0 {
                return Some(Rational::zero());
            }
            return Some(Rational {
                num: big_from_i128(num),
                den: big_from_i128(den),
            });
        }
        // Shared denominator factor (rare): reduce by gcd(num, den). gcd(0, den) = den ⇒ zero → 0/1.
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
        // Native u128 path for the ~1-limb `64b` band (magnitudes fit u64 but exceed i64, so `mul_small`
        // misses them): the cross-reduced products fit u128 (see `mul_small_u128`).
        if let Some(r) = self.mul_small_u128(other) {
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
        let a = self.num.to_i64_checked()?;
        let b = self.den.to_i64_checked()?;
        let c = other.num.to_i64_checked()?;
        let d = other.den.to_i64_checked()?;
        // Cross-reduce on the i64 ORIGINALS (`gcd(a,d)`, `gcd(c,b)`) rather than one gcd over the ~126-bit
        // products: `a`/`b` and `c`/`d` are canonical (coprime), so cancelling the cross-pairs leaves the
        // result already in lowest terms — and the gcds run on u64 (hardware divide) instead of u128 (an
        // `__umodti3` libcall on aarch64). `b, d > 0`; `a, c` nonzero (the `mul` zero-guard ran first).
        let g1 = gcd_u64(a.unsigned_abs(), d.unsigned_abs()) as i64; // gcd(|a|, d)
        let g2 = gcd_u64(c.unsigned_abs(), b.unsigned_abs()) as i64; // gcd(|c|, b)
        // Coprime cross-pairs are the common case; skip the four `x/1` cancellations (each a hardware
        // `sdiv` even when the divisor is 1). `a⊥b`, `c⊥d`, `a⊥d`, `c⊥b` ⇒ the product is already reduced.
        let (num, den) = if g1 == 1 && g2 == 1 {
            (a as i128 * c as i128, b as i128 * d as i128)
        } else {
            (
                (a / g1) as i128 * (c / g2) as i128, // |·| <= 2^126, no overflow
                (b / g2) as i128 * (d / g1) as i128, // b, d > 0 ⇒ den > 0
            )
        };
        Some(Rational {
            num: big_from_i128(num),
            den: big_from_i128(den),
        })
    }

    /// Native product when every component's magnitude fits `u64` but a component exceeds `i64` (so
    /// `mul_small` misses it — the ~1-limb `64b` band). After cross-reducing on the u64 magnitudes, each
    /// reduced factor is `<= u64::MAX`, so the products `num`/`den` fit `u128` (`<= (2^64-1)^2 < 2^128`) with
    /// no overflow — but they can exceed `i128`, so the sign is carried separately and boxed via
    /// [`big_from_u128`]. Returns `None` (fall back to `Big`) when any magnitude exceeds `u64`. `self`/`other`
    /// are nonzero (the `mul` zero-guard ran first) and canonical, so `b, d > 0`.
    fn mul_small_u128(&self, other: &Rational) -> Option<Rational> {
        let a = self.num.to_i128_checked()?;
        let b = self.den.to_i128_checked()?;
        let c = other.num.to_i128_checked()?;
        let d = other.den.to_i128_checked()?;
        let max = u64::MAX as u128;
        let (am, cm) = (a.unsigned_abs(), c.unsigned_abs());
        let (bm, dm) = (b as u128, d as u128); // b, d > 0 (canonical)
        if am > max || bm > max || cm > max || dm > max {
            return None;
        }
        // Cross-reduce on the u64 magnitudes (`gcd(|a|,d)`, `gcd(|c|,b)`) — see `mul_small`.
        let g1 = gcd_u64(am as u64, dm as u64) as u128;
        let g2 = gcd_u64(cm as u64, bm as u64) as u128;
        let (num_mag, den_mag) = if g1 == 1 && g2 == 1 {
            (am * cm, bm * dm) // coprime: skip the four `x/1` u128 divisions
        } else {
            ((am / g1) * (cm / g2), (bm / g2) * (dm / g1))
        };
        // den > 0; result sign is `sign(a) XOR sign(c)`.
        Some(Rational {
            num: big_from_u128(num_mag, (a < 0) != (c < 0)),
            den: big_from_u128(den_mag, false),
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
        // Native u128 path for the ~1-limb `64b` band (see `div_small_u128`).
        if let Some(r) = self.div_small_u128(other) {
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
        let a = self.num.to_i64_checked()?;
        let b = self.den.to_i64_checked()?;
        let c = other.num.to_i64_checked()?;
        let d = other.den.to_i64_checked()?;
        // `a/b ÷ c/d = a*d / (b*c)`. Cross-reduce on the i64 originals (`gcd(a,c)`, `gcd(d,b)`) — u64 gcds,
        // no gcd over the 126-bit product (see [`Rational::mul_small`]). `b, d > 0`; `a, c` nonzero (the
        // `div` guards ran first). The divisor numerator `c` may be negative, so the sign is moved onto
        // the numerator.
        let g1 = gcd_u64(a.unsigned_abs(), c.unsigned_abs()) as i64; // gcd(|a|, |c|)
        let g2 = gcd_u64(d.unsigned_abs(), b.unsigned_abs()) as i64; // gcd(d, b)
        // Coprime cross-pairs (common): skip the four `x/1` cancellations (a hardware `sdiv` each). The
        // product `a*d / (b*c)` is then already reduced (`a⊥b`, `c⊥d`, `a⊥c`, `d⊥b`).
        let (mut num, mut den) = if g1 == 1 && g2 == 1 {
            (a as i128 * d as i128, b as i128 * c as i128) // sign(den) = sign(c)
        } else {
            (
                (a / g1) as i128 * (d / g2) as i128,
                (b / g2) as i128 * (c / g1) as i128, // sign(den) = sign(c)
            )
        };
        if den < 0 {
            num = -num;
            den = -den;
        }
        Some(Rational {
            num: big_from_i128(num),
            den: big_from_i128(den),
        })
    }

    /// Native quotient for the ~1-limb `64b` band (magnitudes fit `u64`, a component exceeds `i64`, so
    /// `div_small` misses it). `a/b ÷ c/d = a*d / (b*c)`; cross-reduce on the u64 magnitudes (`gcd(|a|,|c|)`,
    /// `gcd(d,b)`), after which the products fit `u128` (see [`Rational::mul_small_u128`]). The divisor
    /// numerator `c` may be negative, so the sign is carried onto the numerator (denominator stays positive).
    /// Returns `None` (fall back to `Big`) when any magnitude exceeds `u64`. `self`/`other` are nonzero.
    fn div_small_u128(&self, other: &Rational) -> Option<Rational> {
        let a = self.num.to_i128_checked()?;
        let b = self.den.to_i128_checked()?;
        let c = other.num.to_i128_checked()?;
        let d = other.den.to_i128_checked()?;
        let max = u64::MAX as u128;
        let (am, cm) = (a.unsigned_abs(), c.unsigned_abs());
        let (bm, dm) = (b as u128, d as u128); // b, d > 0 (canonical)
        if am > max || bm > max || cm > max || dm > max {
            return None;
        }
        let g1 = gcd_u64(am as u64, cm as u64) as u128; // gcd(|a|, |c|)
        let g2 = gcd_u64(dm as u64, bm as u64) as u128; // gcd(d, b)
        let (num_mag, den_mag) = if g1 == 1 && g2 == 1 {
            (am * dm, bm * cm)
        } else {
            ((am / g1) * (dm / g2), (bm / g2) * (cm / g1))
        };
        // Denominator magnitude `b*c` is positive; the true denominator sign is `sign(c)`, moved onto the
        // numerator so the stored denominator is positive. Result sign = `sign(a) XOR sign(c)`.
        Some(Rational {
            num: big_from_u128(num_mag, (a < 0) != (c < 0)),
            den: big_from_u128(den_mag, false),
        })
    }

    /// The decimal string `"num/den"` (e.g. `"-3/10"`), or just `"num"` when the value is an integer.
    ///
    /// Writes the components directly into one `String` via `Big::write_decimal` (a sink writer), so it
    /// allocates only the result — no intermediate per-component `String`s. Equivalent to `self.to_string()`.
    pub fn to_decimal_string(&self) -> String {
        use core::fmt::Write;
        // Pre-size the buffer so `write_decimal` never reallocates mid-render. A `b`-byte magnitude has at
        // most `ceil(b * log10(256)) < b * 2.41` decimal digits; `b * 5 / 2` is a safe over-estimate. Add
        // room for a leading `-` and the `/` separator. `byte_len` is O(1).
        let integer = self.is_integer();
        let mut cap = self.num.byte_len() * 5 / 2 + 2; // digits + sign
        if !integer {
            cap += self.den.byte_len() * 5 / 2 + 1; // digits + `/`
        }
        let mut s = String::with_capacity(cap);
        // Write the components straight into the sized buffer (writing into a `String` is infallible).
        let _ = self.num.write_decimal(&mut s);
        if !integer {
            let _ = s.write_str("/");
            let _ = self.den.write_decimal(&mut s);
        }
        s
    }
}

impl core::fmt::Display for Rational {
    /// `num/den` (e.g. `-3/10`), or just `num` when the value is an integer.
    ///
    /// # Format flags
    /// Honors the `Formatter` **padding** flags — width, fill, alignment, the `+` sign flag, and sign-aware
    /// zero-padding — via [`Formatter::pad_integral`](core::fmt::Formatter::pad_integral), matching `num-rational`'s `Ratio` byte-for-byte (this
    /// crate is a drop-in): the numerator's sign is the value's sign, and the `num/den` magnitude is padded
    /// as a single integral field (e.g. `{:>8}` → `"    3/10"`, `{:+}` → `"+3/10"`, `{:08}` → `"00003/10"`,
    /// `{:08}` on `-3/10` → `"-0003/10"`). **Precision is ignored** — a `Rational` is an *exact* fraction
    /// with no inherent decimal expansion, so `{:.2}` cannot mean "two fractional digits" without silently
    /// choosing a rounding (num-rational ignores it too). This is the shared contract with `etude-decimal`.
    ///
    /// The common flag-free case (`"{}"`, `to_string`) keeps a zero-allocation fast path, streaming the
    /// components straight into the sink via `Big::write_decimal`; only a flagged format renders to a
    /// scratch `String` first (which `pad_integral` requires, since it needs the whole magnitude).
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Fast path: no width/sign/zero-pad flag ⇒ nothing to pad, so stream with no allocation. (Fill and
        // alignment are inert without a width; precision does not apply — see above.)
        if f.width().is_none() && !f.sign_plus() && !f.sign_aware_zero_pad() {
            self.num.write_decimal(f)?;
            if !self.is_integer() {
                f.write_str("/")?;
                self.den.write_decimal(f)?;
            }
            return Ok(());
        }
        // A padding flag is set: render the UNSIGNED magnitude to a scratch buffer, then let `pad_integral`
        // apply the sign, width, fill, alignment, `+`, and sign-aware zero-padding exactly as num-rational.
        let mut buf = String::new();
        let _ = self.num.abs().write_decimal(&mut buf);
        if !self.is_integer() {
            buf.push('/');
            let _ = self.den.write_decimal(&mut buf);
        }
        f.pad_integral(!self.num.is_negative(), "", &buf)
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
        // Native u128 path when every magnitude fits u64 (the ~1-limb `64b` tier, whose top magnitude bit is
        // set so the components exceed i64 and miss `cmp_small`): the cross-products `|a|*d`, `|c|*b` fit
        // u128, so compare magnitudes natively and apply the (equal) sign — no `Big` multiply, no allocation.
        if let Some(ord) = self.cmp_small_u128(other) {
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
/// the continued-fraction method. Measured crossover after the CF `q ∈ {0,1}` fast path (which made CF
/// cheap): cross-multiply still wins at ~1 limb (the 64b tier, 8-byte components), but CF wins from ~32
/// bytes up (256b: 62 ns via CF vs 94 ns cross-multiply). 16 bytes sits in the crossover gap.
const CMP_SMALL_BYTES: usize = 16;

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

    /// Native comparison when every magnitude fits `u64` but a component exceeds `i64` (so `cmp_small`
    /// misses it — the ~1-limb `64b` tier). The cross-products `|a|*d` and `|c|*b` are `u64 * u64`, which fit
    /// `u128` with no overflow, so magnitudes compare natively. The caller has already returned on
    /// differing signs, so `a` and `c` share a sign; denominators are strictly positive. For two negatives
    /// the magnitude ordering is reversed. Returns `None` (fall back to the `Big`/CF path) when any magnitude
    /// exceeds `u64`.
    fn cmp_small_u128(&self, other: &Rational) -> Option<Ordering> {
        let a = self.num.to_i128_checked()?;
        let b = self.den.to_i128_checked()?;
        let c = other.num.to_i128_checked()?;
        let d = other.den.to_i128_checked()?;
        let max = u64::MAX as u128;
        // `b, d > 0` (canonical); their magnitudes are the values themselves.
        if a.unsigned_abs() > max || b as u128 > max || c.unsigned_abs() > max || d as u128 > max {
            return None;
        }
        let ord = (a.unsigned_abs() * d as u128).cmp(&(c.unsigned_abs() * b as u128));
        // `a` and `c` share a sign; if both are negative the larger magnitude is the smaller value.
        Some(if a < 0 { ord.reverse() } else { ord })
    }
}

/// `(⌊a/b⌋, a mod b)` for non-negative `a` and strictly positive `b`, fast-pathing the quotients 0 and 1
/// that dominate the continued-fraction comparison of similar-magnitude operands (each step's operands are
/// within a factor of ~2, so `⌊a/b⌋ ∈ {0, 1}`). A full `divmod` — schoolbook `O(n²)` or a reciprocal
/// setup — is then replaced by a single `Big` comparison (`q = 0`) or a comparison plus one subtraction
/// (`q = 1`), both `O(n)`. Falls back to `divmod` only when `q >= 2`.
fn divmod_small_q(a: &Big, b: &Big) -> (Big, Big) {
    if a.cmp(b) == Ordering::Less {
        return (Big::zero(), a.clone()); // q = 0, remainder a
    }
    let r = a.sub(b);
    if r.cmp(b) == Ordering::Less {
        return (Big::from_i64(1), r); // q = 1, remainder a - b
    }
    a.divmod(b).expect("b > 0") // q >= 2: full division
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
    let (q1, r1) = divmod_small_q(a, b);
    let (q2, r2) = divmod_small_q(c, d);
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
        let (q1, r1) = divmod_small_q(&a, &b);
        let (q2, r2) = divmod_small_q(&c, &d);
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

/// Exact `a/b ± c/d` for two canonical fractions (`b, d > 0`, `gcd(a,b) = gcd(c,d) = 1`), reduced without a
/// wide `gcd`. Let `g = gcd(b, d)` (an n-bit gcd on the denominators only).
///
/// - **`g == 1` (coprime denominators, the common case):** `(a*d ± c*b)/(b*d)` is already in lowest terms —
///   the numerator is coprime to `b` (`≡ a*d (mod b)`, and `a ⊥ b`, `d ⊥ b`) and to `d` (symmetrically),
///   and `b ⊥ d`, so `gcd(num, b*d) == 1`. Construct directly, skipping any reduce gcd.
/// - **`g > 1`:** work over the lcm `b·(d/g)`. The numerator is `N = a·(d/g) ± c·(b/g)`, and the standard
///   identity `gcd(N, lcm) = gcd(N, g)` holds (`N` is coprime to `b/g` and `d/g`), so the final reduction
///   is a gcd against the *small* `g`, never the `2n`-bit product.
///
/// This replaces the previous "multiply out then `normalize` (a `2n`-bit gcd)" path — the reduction gcd is
/// now at most n-bit (on the denominators / on `g`), which is where nearly all of `add`/`sub`'s large-tier
/// cost lived.
fn addsub_big(a: &Big, b: &Big, c: &Big, d: &Big, subtract: bool) -> Rational {
    let combine = |x: Big, y: &Big| if subtract { x.sub(y) } else { x.add(y) };
    let g = b.gcd(d); // gcd(|b|, |d|)
    if g.bit_len() == 1 {
        // Coprime denominators ⇒ num/(b*d) already canonical (den = b*d > 0, gcd(num, den) == 1).
        let num = combine(a.mul(d), &c.mul(b));
        if num.is_zero() {
            return Rational::zero();
        }
        return Rational { num, den: b.mul(d) };
    }
    // Shared factor: reduce the denominators first so the product is the lcm, and the final gcd is on `g`.
    let d_over_g = d.div_exact(&g).expect("g divides d");
    let b_over_g = b.div_exact(&g).expect("g divides b");
    let num = combine(a.mul(&d_over_g), &c.mul(&b_over_g));
    if num.is_zero() {
        return Rational::zero();
    }
    let lcm = b.mul(&d_over_g); // b*(d/g) = lcm(b, d) > 0
    let h = num.gcd(&g); // gcd(num, lcm) == gcd(num, g) — reduce against the small g
    if h.bit_len() == 1 {
        return Rational { num, den: lcm };
    }
    let num = num.div_exact(&h).expect("h divides num");
    let den = lcm.div_exact(&h).expect("h divides lcm");
    Rational { num, den }
}

/// Reciprocal of a negative-numerator canonical rational (the rare `into_recip` branch): the raw `den/num`
/// would have a negative denominator, so swap the owned components and flip both signs in place via
/// `Big::negate` (`O(1)`, no magnitude copy) — zero allocation. Kept out of line (`#[cold]` +
/// `#[inline(never)]`) so its locals do not perturb the hot positive-swap branch's codegen (inlining it
/// there regressed the positive branch ~6 ns — a same-function co-regression).
#[cold]
#[inline(never)]
fn recip_negative(num: Big, den: Big) -> Rational {
    let (mut new_num, mut new_den) = (den, num);
    new_num.negate();
    new_den.negate();
    Rational {
        num: new_num,
        den: new_den,
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
    // Native i128 fast path for i64-fitting components — the common small-rational construction case
    // (`from_ratio_i64`, `new` on small `Big`s): native sign fixup + native `u128` gcd + box back, with no
    // `Big` gcd/divmod. The `add`/`sub` general path feeds large products here, so its `to_i64_checked`
    // fails fast (O(1)) and falls through. `n`/`d` fit i64, so every step is overflow-free in i128.
    if let (Some(n), Some(d)) = (num.to_i64_checked(), den.to_i64_checked()) {
        let mut n = n as i128;
        let mut d = d as i128; // d != 0 (den was nonzero)
        if d < 0 {
            n = -n; // move the sign onto the numerator so the denominator is strictly positive
            d = -d;
        }
        let g = gcd_u128(n.unsigned_abs(), d as u128) as i128; // g >= 1 divides both
        return Some(Rational {
            num: big_from_i128(n / g),
            den: big_from_i128(d / g),
        });
    }
    // Native path for the u64-magnitude band (a component exceeds i64 but both magnitudes fit u64 — the
    // `64b` construction tier): native sign fixup + `u64` hardware-divide gcd + box, bypassing the `Big`
    // gcd and `div_exact`. Division only shrinks, so the reduced magnitudes still fit u64. `n, d` are
    // nonzero (guarded above); `d`'s sign moves onto the numerator so the stored denominator is positive.
    if let (Some(n), Some(d)) = (num.to_i128_checked(), den.to_i128_checked()) {
        let max = u64::MAX as u128;
        let (nm, dm) = (n.unsigned_abs(), d.unsigned_abs());
        if nm <= max && dm <= max {
            let g = gcd_u64(nm as u64, dm as u64) as u128; // g >= 1 divides both magnitudes
            return Some(Rational {
                num: big_from_u128(nm / g, (n < 0) != (d < 0)),
                den: big_from_u128(dm / g, false),
            });
        }
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

/// Native Euclidean gcd of two `u128`s. `gcd(x, 0) = x`. When both operands fit `u64` (always the case for
/// the `normalize` small-construction path, and for arithmetic on genuinely small operands) it dispatches
/// to a `u64` gcd whose remainder step is a hardware divide — the `u128` `%` is an `__umodti3` libcall on
/// aarch64, so the fast path avoids a libcall per Euclidean step. Values only shrink, so the one-time
/// entry check suffices.
fn gcd_u128(mut a: u128, mut b: u128) -> u128 {
    if a <= u64::MAX as u128 && b <= u64::MAX as u128 {
        return gcd_u64(a as u64, b as u64) as u128;
    }
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// Native Euclidean gcd of two `u64`s (hardware divide, no `u128` libcall). `gcd(x, 0) = x`.
fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// Box an `i128` as a `Big` via [`Big::from_i128`], a direct limb build (≤ 2 limbs, no byte buffer) — the
/// limb-level twin of `from_i64`. Used to box the result of the native `i128` arithmetic fast paths, whose
/// products routinely exceed `i64` (e.g. a 48-bit × 48-bit numerator is ~96-bit), so a byte-encode/decode
/// round-trip would run on every native op.
fn big_from_i128(v: i128) -> Big {
    Big::from_i128(v)
}

/// Box a `u128` magnitude with an explicit sign as a `Big`. When the magnitude fits `i128` (all but the
/// `(2^127, 2^128)` top band that only the native `u128` mul/div products can reach) it uses the direct
/// limb constructor [`Big::from_i128`]; otherwise it falls back to the canonical sign-magnitude byte
/// encoding (`[sign] + 16 little-endian magnitude bytes`), the only path that can hold a full `u128`.
fn big_from_u128(mag: u128, negative: bool) -> Big {
    if mag <= i128::MAX as u128 {
        let v = mag as i128;
        return Big::from_i128(if negative { -v } else { v });
    }
    let mut buf = [0u8; 17]; // 1 sign byte + 16 magnitude bytes
    buf[0] = negative as u8;
    buf[1..].copy_from_slice(&mag.to_le_bytes());
    Big::from_sign_magnitude_bytes(&buf)
}

/// `n / g` where `g` is a known divisor of `n`, without allocating when the division is a no-op: returns a
/// borrow of `n` when `g == 1` (an O(1), allocation-free `bit_len() == 1` check — `g` is a non-negative gcd,
/// so `bit_len() == 1` ⟺ `g == 1`), else the owned `Big::div_exact` quotient (no discarded remainder). The
/// borrow avoids cloning the operand in the common coprime case, where the caller only reads it (to
/// multiply) and would otherwise drop the clone immediately.
fn reduce_ref<'a>(n: &'a Big, g: &Big) -> Cow<'a, Big> {
    if g.bit_len() == 1 {
        Cow::Borrowed(n)
    } else {
        Cow::Owned(n.div_exact(g).expect("g is a nonzero divisor of n"))
    }
}

/// Cross-reduced multiply of two canonical fractions `a/b` and `c/d` (`gcd(a,b) = gcd(c,d) = 1`, and both
/// nonzero). Cancels `g1 = gcd(a,d)` and `g2 = gcd(c,b)` before multiplying, returning `((a/g1)*(c/g2),
/// (b/g2)*(d/g1))` — which is already in lowest terms (the four cross-pairs are pairwise coprime), so no
/// further gcd-normalize is needed. `gcd` is sign-agnostic, so the quotients keep their operands' signs;
/// the caller owns any final sign placement. When a cross-gcd is 1 (the common coprime case) the factor is
/// borrowed rather than cloned — only the two products are allocated.
fn cross_reduce_mul(a: &Big, b: &Big, c: &Big, d: &Big) -> (Big, Big) {
    let g1 = a.gcd(d); // gcd(|a|, |d|)
    let g2 = c.gcd(b); // gcd(|c|, |b|)
    let num = reduce_ref(a, &g1).mul(&reduce_ref(c, &g2));
    let den = reduce_ref(b, &g2).mul(&reduce_ref(d, &g1));
    (num, den)
}

#[cfg(test)]
mod tests;
