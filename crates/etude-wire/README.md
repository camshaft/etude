# etude-wire
[![crates.io](https://img.shields.io/crates/v/etude-wire.svg)](https://crates.io/crates/etude-wire) [![docs.rs](https://docs.rs/etude-wire/badge.svg)](https://docs.rs/etude-wire)

> A compact binary encoding: `Serialize` and `Deserialize` for the standard types, versioned
> derives for your own, and a tag-length-value form for messages that can't negotiate a version.

- `Serialize` writes a value into an `etude-bytevec` `Builder`, and `Deserialize` reads it
  back from a `ByteVec`.
- `serialize` writes the value's type's global id, then its encoding; `deserialize` reads
  it back.
- `#[derive(Wire)]` (re-exported from `etude-wire-derive`) encodes a struct's fields, or an
  enum's variant, in declaration order, and `#[wire(...)]` attributes declare a type's
  versions and the fields each version holds.
- `#[derive(TaggedWire)]` encodes each field with its tag and length, so a reader skips the
  tags it doesn't know.
- The crate uses `std`, since it encodes `std::io` and `std::net` types. It builds for
  `wasm32-unknown-unknown` with that target's `std`.

```rust
use etude_wire::Wire;

#[derive(Wire, Debug, PartialEq)]
struct Message {
    id: u64,
    payload: Vec<u8>,
}

let mut bytes = etude_wire::serialize(Message { id: 1, payload: b"hello".to_vec() });
let decoded: Message = etude_wire::deserialize(&mut bytes).unwrap();
assert_eq!(decoded, Message { id: 1, payload: b"hello".to_vec() });
```

Part of the [etude](https://github.com/camshaft/etude) workspace. See the
[API documentation on docs.rs](https://docs.rs/etude-wire).

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](../../LICENSE)).
