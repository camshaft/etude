<!-- Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# `etude-buffer` — performance scoreboard

`etude-buffer` defines the copy-avoiding `reader::Buffer` / `writer::Buffer` traits that underlie
`etude-bytevec` and every consumer that moves bytes through a buffer. This file is the runnable
scoreboard for the byte-movement hot paths, reproducible from `benches/buffer.rs`
(`cargo bench -p etude-buffer`). It is being stood up incrementally, one hot path at a time.

## `copy_into` — draining a reader into a writer (runnable: `cargo bench -p etude-buffer -- copy_into`)

`reader::Buffer::copy_into` is the core operation: it drains a reader into a writer. Whether it copies
or moves the payload turns on the writer's `SPECIALIZES_BYTES` associated const. Draining the same
contiguous `Bytes` source into a copying writer (`Vec<u8>`) vs the zero-copy handle-move writer
(`VecDeque<Bytes>`), with each destination built fresh inside the timed routine (aarch64, jemalloc,
release):

| dest | 64 B | 1400 B | 64 KiB |
|------|------|--------|--------|
| `Vec<u8>` (copy, `SPECIALIZES_BYTES = false`) | 23.6 ns | 152 ns | 1.78 µs |
| `VecDeque<Bytes>` (move, `SPECIALIZES_BYTES = true`) | 28.1 ns | 27.8 ns | 27.3 ns |

The copying path scales linearly with the payload (it is a `put_slice` memcpy), while the handle-move
path is **flat at ~28 ns regardless of length** — `copy_into` hands the `Bytes` to `put_bytes`, which a
`SPECIALIZES_BYTES` writer stores by reference rather than copying. At an MTU frame the move is ~5.5×
cheaper, and at 64 KiB ~65× — this is the crate's reason to exist, and the mechanism `etude-bytevec`
relies on to absorb received buffers without a copy. The takeaway for callers: draining into a
chunk-holding sink (a `Bytes` queue, a rope) keeps the payload zero-copy; draining into a contiguous
`Vec`/`BytesMut`/slice pays the memcpy that the contiguous representation requires.

## Next targets

Hot paths still to bench (subsequent passes): the chunked/vectored reader path (`IoSlice` / `Chain`
readers, which exercise the internal `vectored_copy` scatter/gather), `put_uninit_slice` (the
socket-read zero-init write path), and `partial_copy_into`'s trailing-chunk hand-off.
