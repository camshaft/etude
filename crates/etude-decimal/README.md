# etude-decimal
[![crates.io](https://img.shields.io/crates/v/etude-decimal.svg)](https://crates.io/crates/etude-decimal) [![docs.rs](https://docs.rs/etude-decimal/badge.svg)](https://docs.rs/etude-decimal)

> Exact base-10 arbitrary-precision decimal numbers (`coeff * 10^exp`) over `etude-bigint`.

A decimal preserves every significant digit and its scale exactly, unlike an `f64` (which loses
precision) or a rational (which would need gcd reduction and cannot distinguish `0.1` from `0.10`
by scale). Pure over `alloc`, no I/O, no dependency but `etude-bigint`. Exact arithmetic
(`Decimal::add`/`Decimal::sub`/`Decimal::mul`) never rounds a digit away; division is split by
that principle — `Decimal::div` is exact and returns `None` when the quotient does not terminate,
while `Decimal::div_round` rounds to a caller-chosen precision and `RoundingMode` (a
rounding-capable operation always takes explicit rounding arguments — there is no default).
Correctness is pinned by a differential test against `bigdecimal` (a dev-dependency) as the
reference.

A value is built either numerically from a `Big` coefficient and an exponent via `Decimal::new`
(and the `Decimal::from_i64` / `Decimal::from_bigint` conveniences), or parsed from a decimal
number literal via `Decimal::parse` / `Decimal::from_str`. Parsing owns exactly the
decimal-number-literal grammar and consumes any `Iterator<Item = u8>`, so a rope- or chunk-backed
(non-contiguous) byte source is parsed in place with no flattening; `Decimal::parse_prefix`
parses a number embedded in a larger byte stream.

## Representation and the canonical-form invariant

A `Decimal` is the exact value `coeff * 10^exp`, where `coeff` is a `Big` signed integer (it
carries the sign of the whole value) and `exp` is a base-10 point shift. It is kept in a single
canonical form:

- the coefficient has no trailing zero digit (`coeff % 10 != 0`), with `exp` raised to
  compensate, so `1.0`, `1`, and `1.00` all become the one value `coeff = 1, exp = 0`, and `100`
  becomes `coeff = 1, exp = 2`;
- zero is exactly `coeff = 0, exp = 0` (there is no `0 * 10^5`).

Every constructor and operation renormalizes, so a value has exactly one in-memory form. This is
required for `Eq`/`Ord` to mean numeric equality: `0.1` and `0.10` and `1e-1` are the same value
and must have identical fields.

Part of the [etude](https://github.com/camshaft/etude) workspace. See the
[API documentation on docs.rs](https://docs.rs/etude-decimal).

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](../../LICENSE)).
