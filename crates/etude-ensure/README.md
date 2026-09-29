# etude-ensure
[![crates.io](https://img.shields.io/crates/v/etude-ensure.svg)](https://crates.io/crates/etude-ensure) [![docs.rs](https://docs.rs/etude-ensure/badge.svg)](https://docs.rs/etude-ensure)

> Small, dependency-free control-flow macros (`ensure!` / `assume!`).

- `ensure!` bails out early (`return`/`break`/`continue`) unless a condition holds.
- `assume!` asserts an invariant: a `debug_assert!` in debug builds, an optimization
  hint (`unreachable_unchecked`) in release builds.

These are intentionally free of any instrumentation or dependencies, so they can be used
anywhere bytes are being pushed around.

Part of the [etude](https://github.com/camshaft/etude) workspace. See the
[API documentation on docs.rs](https://docs.rs/etude-ensure).

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](../../LICENSE)).
