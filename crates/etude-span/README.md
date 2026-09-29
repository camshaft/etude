# etude-span
[![crates.io](https://img.shields.io/crates/v/etude-span.svg)](https://crates.io/crates/etude-span) [![docs.rs](https://docs.rs/etude-span/badge.svg)](https://docs.rs/etude-span)

> Byte-scanning primitives for copy-avoiding tokenizers over the etude byte-rope: a `Span`
> (byte range) and a chunk-streaming `Cursor`.

Two types, shared by every format tokenizer built on `ByteVec` (JSON, protobuf, decimal number
literals, …):

- `Span` — a half-open byte range `[start, end)` into a rope. A span carries no bytes; a token
  references its lexeme by span and the consumer resolves it against the originating rope
  (`ByteVec::slice`, O(1) structural sharing) only when it wants the bytes.
- `Cursor` — a forward cursor that streams the rope's leaves. It reads each contiguous leaf once
  with a local slice index and refills at leaf boundaries, so advancing is O(1) amortized (O(n)
  over the input) rather than the O(log n) tree descent a per-byte `ByteVec::byte_at` would cost.
  It tracks the absolute byte offset only to stamp span endpoints, never to fetch a byte, and
  allocates nothing.

Part of the [etude](https://github.com/camshaft/etude) workspace. See the
[API documentation on docs.rs](https://docs.rs/etude-span).

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](../../LICENSE)).
