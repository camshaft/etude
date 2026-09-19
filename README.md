# etude

A workspace of small, focused Rust crates for moving bytes around and computing
with exact numbers — allocation-conscious data structures and formats that avoid
copying wherever they can, and numeric types that never silently lose precision.

The crates share a common style: a small public surface, a documented canonical
form or invariant, `no_std` support where it is practical, and correctness pinned
by property and differential tests against established references.

> Status: early. The crates are at `0.1.x`, their internal representations are
> private, and public APIs may still change between releases. Each crate documents
> every public item and is covered by tests, but treat the set as pre-1.0.

## Crates

### Foundations

| Crate | What it is |
|-------|------------|
| [`etude-ensure`](crates/etude-ensure)   | Dependency-free control-flow macros: `ensure!` (early return / `break` / `continue` unless a condition holds) and `assume!` (a debug assertion that becomes an optimization hint in release). |
| [`etude-buffer`](crates/etude-buffer)   | Copy-avoiding `reader` / `writer` buffer traits that move bytes as borrowed chunks, plus adapters (chain, limit, tracking) for composing them. |
| [`etude-span`](crates/etude-span)       | Byte-scanning primitives for copy-avoiding tokenizers: a `Span` (a byte range into an input) and a chunk-streaming `Cursor`. |

### Byte and string containers

| Crate | What it is |
|-------|------------|
| [`etude-bytevec`](crates/etude-bytevec)   | A chunked byte buffer backed by a relaxed-radix (RRB) rope: `O(log₃₂ n)` offset lookup, `O(1)` structural-sharing clone, and zero-copy slice and concat. It stays a flat `Bytes` deque while small and promotes to the tree only when it grows. |
| [`etude-strrope`](crates/etude-strrope)   | A UTF-8 string rope: a validated-UTF-8 view over the `etude-bytevec` byte rope, so large text is edited and shared without copying. |
| [`etude-str`](crates/etude-str)           | `Str`, a cheaply-clonable UTF-8 string backed by `bytes::Bytes`: cloning is a reference-count bump, and the text crosses the string/bytes boundary without re-allocating. |

### Exact numbers

| Crate | What it is |
|-------|------------|
| [`etude-bigint`](crates/etude-bigint)     | Arbitrary-precision signed integers — a small `no_std` limb library with a canonical sign-magnitude form. |
| [`etude-rational`](crates/etude-rational) | Exact rational numbers: a normalized (reduced, positive-denominator) `num/den` pair over `etude-bigint`. |
| [`etude-decimal`](crates/etude-decimal)   | Exact base-10 decimals, `coeff · 10^exp` over `etude-bigint` — every significant digit and its scale preserved, with exact arithmetic and explicit, caller-chosen rounding for division. |

### Formats

| Crate | What it is |
|-------|------------|
| [`etude-json`](crates/etude-json)         | A copy-avoiding JSON tokenizer whose tokens reference byte ranges of the input rope rather than copying the bytes; a caller materializes only the values it actually needs. |

Planned: a serde integration crate.

## Quick start

The crates are developed together in this workspace and are not yet published to
crates.io; depend on them by git until they are:

```toml
[dependencies]
etude-bytevec = { git = "https://github.com/camshaft/etude" }
```

```rust
use etude_bytevec::{ByteVec, Bytes}; // `Bytes` is re-exported, so no separate `bytes` dependency

let mut v = ByteVec::new();
v.push_back(Bytes::from_static(b"hello "));
v.push_back(Bytes::from_static(b"world"));
assert_eq!(v.len(), 11);

// Split off the front without copying the underlying chunks.
let head = v.split_to(6).unwrap();
assert_eq!(head, b"hello ");
assert_eq!(v, b"world");
```

## Requirements

- Rust 1.88 or newer (edition 2024).
- Many crates support `no_std`, depending only on `alloc` (the numeric types and
  the byte rope, among others); a few require `std`. Each crate's documentation
  states which, and which features it gates behind `std`.

## Development

```sh
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
cargo doc --workspace --all-features --no-deps
```

## License

Licensed under the [Apache-2.0](LICENSE) license. See [NOTICE](NOTICE) for
attribution.
