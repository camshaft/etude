# etude-sha2-const
[![crates.io](https://img.shields.io/crates/v/etude-sha2-const.svg)](https://crates.io/crates/etude-sha2-const) [![docs.rs](https://docs.rs/etude-sha2-const/badge.svg)](https://docs.rs/etude-sha2-const)

> SHA-256 as a `const fn`: digests of static data computed at compile time, with no dependencies.

- `Sha256::new`, `update` and `finalize` are `const fn`, so the compiler computes the digest
  of static data, such as a stable 64-bit id of a type or a method.
- `finalize_u64` returns the digest's first 8 bytes as a big-endian `u64`.
- The crate is `no_std` and uses only `core`, so it builds for `wasm32-unknown-unknown`.

```rust
use etude_sha2_const::Sha256;

const DIGEST: [u8; 32] = Sha256::new().update(b"abc").finalize();
const ID: u64 = Sha256::new().update(b"abc").finalize_u64();

assert_eq!(ID, 0xba78_16bf_8f01_cfea);
assert_eq!(ID.to_be_bytes(), DIGEST[..8]);
```

Part of the [etude](https://github.com/camshaft/etude) workspace. See the
[API documentation on docs.rs](https://docs.rs/etude-sha2-const).

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](../../LICENSE)).
