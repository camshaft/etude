# etude-bigint
[![crates.io](https://img.shields.io/crates/v/etude-bigint.svg)](https://crates.io/crates/etude-bigint) [![docs.rs](https://docs.rs/etude-bigint/badge.svg)](https://docs.rs/etude-bigint)

> Arbitrary-precision signed integers: a small `no_std` limb library with a canonical
> sign-magnitude form.

A small, hand-written `no_std` limb library. Pure over `alloc::vec::Vec`, no I/O, no dependency.
The surface is small (add/sub/mul/divmod/gcd/cmp + from/to i64 + two byte encodings) over
`Vec<u64>` limbs — schoolbook algorithms. Independently unit-testable, with a differential test
against `num-bigint` (a dev-dependency) as the safety net.

## Representation

`Big` is `{ neg: bool, mag: Vec<u64> }` — base-2⁶⁴ limbs, little-endian (`mag[0]` is the
least-significant limb), with no trailing zero limbs. Zero is the canonical
`{ neg: false, mag: [] }`. Every operation `normalize`s its result (strips trailing zero limbs;
forces `neg = false` when the magnitude is zero), so a value has exactly one in-memory form. This
canonical form is required when a `Big` is used as a map key or compared for equality: the
sign-magnitude byte encoding (`Big::to_sign_magnitude_bytes`) is what such comparisons operate
on, so equal values must produce identical bytes.

Part of the [etude](https://github.com/camshaft/etude) workspace. See the
[API documentation on docs.rs](https://docs.rs/etude-bigint).

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](../../LICENSE)).
