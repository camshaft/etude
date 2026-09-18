# byte-vec

A chunked, reference-counted byte buffer.

`ByteVec` is a deque of [`bytes::Bytes`] chunks that behaves like a single
contiguous byte buffer while avoiding copies on the hot path. It supports
cheap `split_to`, `push_front`/`push_back`, `truncate`, and zero-copy reads.

```rust
use byte_vec::ByteVec;
use bytes::Bytes;

let mut v = ByteVec::new();
v.push_back(Bytes::from_static(b"hello "));
v.push_back(Bytes::from_static(b"world"));
assert_eq!(v.len(), 11);

let head = v.split_to(6).unwrap();
assert_eq!(head, b"hello ");
assert_eq!(v, b"world");
```

## Layout

This is a Cargo workspace. Crates live under [`crates/`](crates):

- [`byte-vec`](crates/bytevec) — the `ByteVec` buffer.

## License

Licensed under the [Apache-2.0](LICENSE) license.

[`bytes::Bytes`]: https://docs.rs/bytes
