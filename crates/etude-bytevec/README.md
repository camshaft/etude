# etude-bytevec

> A chunked byte buffer backed by a relaxed-radix (RRB) rope: O(log₃₂) offset lookup,
> structural sharing, zero-copy slice/concat.

`ByteVec` is a tiered byte rope that degrades to a flat `bytes::Bytes` deque when small. It is
a drop-in for a chunked byte buffer (a `VecDeque<Bytes>` with a cached length):
`push_back`/`push_front`/`pop_front`/`pop_back` are chunk-granular and O(1); `advance` consumes
bytes from the front zero-copy at chunk boundaries. It matches the flat buffer's cheapest
properties exactly:

- a **single chunk holds no tracking state** — it lives in `head` with an unallocated deque, and
- the **shallow streaming path never touches a tree**.

Only once a rope accumulates many chunks does it promote to a *deep* representation — a
relaxed-radix (RRB-style) tree of chunk blocks with buffered head/tail deques — where `clone` is
O(1) (structural sharing) and `split`/`concat` are O(log₃₂ n) instead of O(chunks). It demotes
back to flat when it drains, so a buffer that spikes and drains returns to the cheap shape.

The tree carries a cumulative byte-size table per node, so it is *relaxed*: chunks (and blocks)
have varying byte lengths, and front-consumption does not require a strict left-full invariant.

Part of the [etude](https://github.com/camshaft/etude) workspace. See the
[API documentation on docs.rs](https://docs.rs/etude-bytevec).

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](../../LICENSE)).
