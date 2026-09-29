# etude
[![CI](https://github.com/camshaft/etude/actions/workflows/ci.yml/badge.svg)](https://github.com/camshaft/etude/actions/workflows/ci.yml) [![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

A workspace of small, focused crates: copy-avoiding byte buffers and the text,
scanning, and JSON layers built on them, plus an exact arbitrary-precision numeric
tower (integers, rationals, decimals).

## Crates

### Bytes & buffers

| Crate | Description |
|-------|-------------|
| [`etude-buffer`](crates/etude-buffer)   | Copy-avoiding `reader` / `writer` buffer traits, a family of storage adapters, and a stream-data testing model. |
| [`etude-bytevec`](crates/etude-bytevec) | A chunked byte buffer backed by a relaxed-radix (RRB) rope: O(log₃₂) offset lookup, structural sharing, zero-copy slice/concat. |

### Text

| Crate | Description |
|-------|-------------|
| [`etude-str`](crates/etude-str)         | A cheaply-clonable, `Bytes`-backed UTF-8 string (O(1) clone). |
| [`etude-strrope`](crates/etude-strrope) | A UTF-8 string rope: a validated-UTF-8 view over the `etude-bytevec` byte rope. |

### Scanning & parsing

| Crate | Description |
|-------|-------------|
| [`etude-span`](crates/etude-span) | Byte-scanning primitives for copy-avoiding tokenizers: a `Span` (byte range) and a chunk-streaming `Cursor`. |
| [`etude-json`](crates/etude-json) | Copy-avoiding JSON: a tokenizer whose tokens reference spans of the input rope rather than copying bytes. |

### Numeric (exact, arbitrary-precision)

| Crate | Description |
|-------|-------------|
| [`etude-bigint`](crates/etude-bigint)     | Arbitrary-precision signed integers with a canonical sign-magnitude form. |
| [`etude-rational`](crates/etude-rational) | Exact rational numbers: a normalized (reduced, positive-denominator) `num/den` pair over `etude-bigint`. |
| [`etude-decimal`](crates/etude-decimal)   | Exact base-10 decimals (`coeff × 10^exp`) over `etude-bigint`. |

### Utilities

| Crate | Description |
|-------|-------------|
| [`etude-ensure`](crates/etude-ensure) | Dependency-free `ensure!` / `assume!` control-flow macros. |

## no_std

Every crate is `no_std`-compatible (they build for `wasm32-unknown-unknown`, e.g. to run
as WebAssembly guests).

- The numeric and macro crates — `etude-bigint`, `etude-rational`, `etude-decimal`,
  `etude-ensure` — are `no_std` unconditionally (they use only `core` + `alloc`).
- The byte, text, and parsing crates — `etude-buffer`, `etude-bytevec`, `etude-str`,
  `etude-strrope`, `etude-span`, `etude-json` — carry a `std` feature that is **on by
  default** (it enables `std` on their byte-buffer dependencies). Depend on them with
  `default-features = false` for a `no_std` build:

  ```toml
  etude-strrope = { version = "0.1", default-features = false }
  etude-str     = { version = "0.1", default-features = false }
  etude-bytevec = { version = "0.1", default-features = false }
  ```

## Example

```rust
use etude_bytevec::ByteVec;
use bytes::Bytes;

let mut v = ByteVec::new();
v.push_back(Bytes::from_static(b"hello "));
v.push_back(Bytes::from_static(b"world"));
assert_eq!(v.len(), 11);

// Split off the front without copying the underlying chunks.
let head = v.split_to(6).unwrap();
assert_eq!(head, b"hello ");
assert_eq!(v, b"world");
```

## Development

```sh
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features
cargo fmt --all --check
```

## License

Licensed under the [Apache-2.0](LICENSE) license.
