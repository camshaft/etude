# etude

A workspace of small, focused crates for moving bytes around — copy-avoiding
buffer traits and the containers built on them.

## Crates

| Crate | Description |
|-------|-------------|
| [`etude-ensure`](crates/etude-ensure)   | Dependency-free `ensure!` / `assume!` control-flow macros. |
| [`etude-buffer`](crates/etude-buffer)   | Copy-avoiding `reader` / `writer` buffer traits, a family of storage adapters, and a stream-data testing model. Offsets are plain `u64`. |
| [`etude-bytevec`](crates/etude-bytevec) | `ByteVec`: a chunked, reference-counted byte buffer built on `etude-buffer`. |

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
