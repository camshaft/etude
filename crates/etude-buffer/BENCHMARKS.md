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

## `put_uninit_slice` — the trusted-count write path (runnable: `cargo bench -p etude-buffer -- put_uninit`)

`put_uninit_slice` writes directly into a destination's spare capacity through a closure, avoiding a
staging copy. It does _not_ zero-initialize the exposed region: the closure reports how many leading
bytes it wrote and exactly that prefix is committed (clamped to the slice length so a commit can never
run past the exposed region). That makes it an `unsafe` method — the caller owes that the closure
initialized at least the prefix it reports, the soundness contract behind breaker #33 — in exchange for
staying on the single-copy floor of a plain `put_slice`. Contrasting `put_uninit_slice` (fill + reported
commit) against a plain `put_slice` of the same payload (a single copy), into a fresh `Vec<u8>`
(aarch64, jemalloc, release, one run):

| op | 64 B | 1400 B | 64 KiB |
|----|------|--------|--------|
| `put_uninit_slice` (no zero-init, fill) | 7.0 ns | 65 ns | 1.17 µs |
| `put_slice` (single copy) | 5.6 ns | 68 ns | 1.12 µs |

With the redundant memset gone, `put_uninit_slice` lands on the `put_slice` baseline at 1400 B and 64 KiB
(1.17 vs 1.12 µs at 64 KiB, within noise; the two overlap at 1400 B), leaving only a small fixed
per-call overhead at 64 B (7.0 vs 5.6 ns) — confirming the earlier delta was the zero-init pass and
nothing else. An earlier revision zero-initialized the region as a soundness floor for untrusted
closures; that cost a full extra pass (the "zero-init + fill" path measured ~2.00 µs at 64 KiB, ~1.65× the
copy). The floor is now a caller obligation (`unsafe`) plus the impl-side clamp for hard bounds, so no
closure can expose memory past the slice it was handed. `etude-bytevec` exposes the same trade one level
up as `Builder::for_socket_read` (a real socket `recv` returning its syscall byte count is the motivating
honest closure).

## `IoSlice` vectored drain (runnable: `cargo bench -p etude-buffer -- vectored`)

`IoSlice` presents a slice of byte segments as one logical stream; its `copy_into` drains them segment
by segment (`read_chunk` per segment, then `put_slice`). Draining the same 64 KiB as many small
segments versus as a single contiguous `Bytes` source, both into a fresh `Vec<u8>`, isolates the
per-segment overhead the vectored path adds on top of the underlying memcpy (`IoSlice::new` scans the
segments once to total their length; that construction is included, as a real vectored read must build
the reader):

| source | drain into `Vec<u8>` | per-segment overhead |
|--------|----------------------|----------------------|
| 1024 × 64 B segments | 4.98 µs | ~3.5 ns |
| 256 × 256 B segments | 2.40 µs | ~3.8 ns |
| 16 × 4 KiB segments | 1.54 µs | ~6 ns |
| 1 × 64 KiB contiguous | 1.44 µs | — (baseline memcpy) |

The vectored drain **converges to the contiguous memcpy baseline as segments grow** — 16 × 4 KiB (1.54 µs)
is within ~7% of the single-copy 1.44 µs — because with few large segments the per-byte memcpy dominates
and the per-segment bookkeeping is amortized away. The per-segment overhead itself is ~3.5–6 ns (one
`read_chunk` control-flow step plus a `put_slice` call per segment), so it only bites when segments are
tiny: 1024 × 64 B pays ~3.5 µs of it on top of the 1.44 µs copy. The takeaway: `IoSlice` is near-optimal
for MTU-sized-and-larger segments; a producer emitting a great many tiny segments pays measurable
per-segment cost and should coalesce upstream where it can.

## `Chain` reader (runnable: `cargo bench -p etude-buffer -- chain`)

`Chain` drains reader `a` fully, then `b`, presenting the two as one stream. Draining two 32 KiB `Bytes`
halves through a `Chain` versus a single contiguous 64 KiB `Bytes`, both `copy_into` a fresh `Vec<u8>`
(the same 64 KiB copied either way):

| source | copy_into `Vec<u8>` |
|--------|---------------------|
| `Chain` of 2 × 32 KiB | 1.29 µs |
| 1 × 64 KiB contiguous | 1.43 µs |

The two are within noise (~10%), both memcpy-bound on the same 64 KiB — `Chain`'s per-drain bookkeeping
(the `buffer_is_empty` check and the `a`-then-`b` hand-off) is negligible next to the copy. Composing
readers with `Chain` is effectively free; the cost is whatever the underlying readers' drain costs.

## `partial_copy_into` trailing-chunk hand-off (runnable: `cargo bench -p etude-buffer -- partial`)

`partial_copy_into` copies until the destination is full and *returns* the trailing chunk for the caller
to place — the zero-copy hand-off point, where a chunk-holding sink or a rope can adopt that `Bytes` by
reference. `copy_into` instead copies that trailing chunk too. On a single 64 KiB `Bytes` drained into a
`Vec<u8>` (a non-specializing sink), with the destination pre-allocated in both arms so the delta is
exactly the trailing-chunk copy:

| operation | 64 KiB `Bytes` → `Vec<u8>` |
|-----------|----------------------------|
| `partial_copy_into` (returns trailing chunk) | 407 ns |
| `copy_into` (copies trailing chunk) | 1.46 µs |

Returning the trailing chunk is **~3.6× cheaper** — `partial_copy_into` hands it back via a `split_to`
refcount move (no memcpy), while `copy_into` memcpys it into the `Vec`. The ~1.05 µs delta is that
64 KiB copy. This is the mechanism a chunk-holding consumer (a `Bytes` queue, `etude-bytevec`'s builder)
uses to absorb a received buffer without a copy: take the trailing chunk from `partial_copy_into` and
`put_bytes` it by reference rather than letting `copy_into` flatten it.

## Next targets

Coverage of the core reader/writer paths is in place: `copy_into` (copy vs handle-move), `put_uninit_slice`
(trusted-count path vs the `put_slice` floor), the `IoSlice` vectored drain, the `Chain` reader, and the `partial_copy_into`
trailing-chunk hand-off. The `slice::vectored_copy` scatter helper is crate-private and currently used
only in tests, so it is not on a benchable production path; it becomes a target if a production caller
appears.
