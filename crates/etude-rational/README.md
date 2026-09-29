# etude-rational
[![crates.io](https://img.shields.io/crates/v/etude-rational.svg)](https://crates.io/crates/etude-rational) [![docs.rs](https://docs.rs/etude-rational/badge.svg)](https://docs.rs/etude-rational)

> Exact rational numbers: a normalized (reduced, positive-denominator) `num/den` pair over
> `etude-bigint`'s arbitrary-precision integers.

Exact `+ - * /` and comparison over the normalized pair. Pure over `alloc`, no I/O, no dependency
but `etude-bigint`, with a differential test against `num-rational` (a dev-dependency) as the
safety net.

## Representation and the canonical-form invariant

`Rational` is a `{ num: Big, den: Big }` pair kept in a single canonical form:

- the denominator is strictly positive (`den >= 1`), so the sign lives entirely on the numerator;
- the pair is in lowest terms (`gcd(|num|, den) == 1`);
- zero is exactly `0/1`; an integer `n` is exactly `n/1`.

Every constructor and operation renormalizes, so a value has exactly one in-memory form. This is
required for `Eq`/`Ord`/hashing to mean mathematical equality: `1/2` and `2/4` are the same value
and must have identical fields. There is no representation of a zero-denominator rational — the
fallible constructors return `None` and the total operations cannot produce one.

The fields are private and not part of the stable API. Construct through `Rational::zero`,
`Rational::from_i64`, `Rational::from_bigint`, `Rational::new`; inspect through `Rational::numer`,
`Rational::denom`, `Rational::is_zero`, `Rational::is_negative`, `Rational::is_integer`,
`Rational::cmp`.

Part of the [etude](https://github.com/camshaft/etude) workspace. See the
[API documentation on docs.rs](https://docs.rs/etude-rational).

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](../../LICENSE)).
