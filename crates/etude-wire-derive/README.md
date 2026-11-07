# etude-wire-derive
[![crates.io](https://img.shields.io/crates/v/etude-wire-derive.svg)](https://crates.io/crates/etude-wire-derive) [![docs.rs](https://docs.rs/etude-wire-derive/badge.svg)](https://docs.rs/etude-wire-derive)

> The `Wire` and `TaggedWire` derive macros for `etude-wire`.

- `#[derive(Wire)]` implements `etude-wire`'s `Serialize`, `Deserialize` and `Wire` for a
  struct or an enum, with the versions its `#[wire(...)]` attributes declare.
- `#[derive(TaggedWire)]` implements `etude-wire`'s tag-length-value encoding, with a
  `#[wire(tag = V.S)]` tag on each field or variant.
- The generated code refers to the `etude_wire` crate. Where the crate is renamed or
  re-exported, give its path with `#[wire(crate = path)]`.

`etude-wire` re-exports both derives, so depend on `etude-wire` rather than on this crate.

Part of the [etude](https://github.com/camshaft/etude) workspace. See the
[API documentation on docs.rs](https://docs.rs/etude-wire-derive).

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](../../LICENSE)).
