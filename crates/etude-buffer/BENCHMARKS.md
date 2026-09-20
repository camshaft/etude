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

## `put_uninit_slice` — the zero-init write path (runnable: `cargo bench -p etude-buffer -- put_uninit`)

`put_uninit_slice` writes directly into a destination's spare capacity through a closure, avoiding a
staging copy. Before the closure runs it zero-initializes the exposed region — a soundness floor so a
closure that under-fills can never expose stale/uninitialized heap. When the closure fills the *whole*
region (the common case: a socket read that fills its buffer), that memset is immediately overwritten,
so it is pure overhead. Contrasting `put_uninit_slice` (zero-init + full fill) against a plain
`put_slice` of the same payload (a single copy), into a fresh `Vec<u8>` (aarch64, jemalloc, release):

| op | 64 B | 1400 B | 64 KiB |
|----|------|--------|--------|
| `put_uninit_slice` (zero-init + fill) | 13.3 ns | 122 ns | 2.00 µs |
| `put_slice` (single copy) | 6.8 ns | 87 ns | 1.22 µs |
| redundant zero-init overhead | ~2.0× | ~1.4× | ~1.65× |

The zero-init is a full extra pass over the region, so it roughly doubles a small write and adds ~65% to
a 64 KiB one — material, not noise. This is a real optimization target: when the caller knows the
closure fills the entire region (or reports how many bytes it wrote), the redundant memset over the
written prefix can be skipped, keeping only the soundness floor over any *unwritten* tail. That is an API
change on a trait `etude-bytevec` depends on (its `Builder::for_socket_read` routes through this), so it
is being pursued as a coordinated follow-up rather than folded in here.

## Next targets

Hot paths still to bench (subsequent passes): the chunked/vectored `IoSlice` reader drain (multi-segment
`put_slice`), `partial_copy_into`'s trailing-chunk hand-off, and the `Chain` reader. The
`slice::vectored_copy` scatter helper is crate-private and currently used only in tests, so it is not on
a benchable production path.
