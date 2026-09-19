<!-- Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# `etude-byterope` vs `etude-bytevec` — head-to-head scoreboard

The delete-`etude-bytevec` gate: byterope must be a drop-in for bytevec (done — see the compat
tests) **and** competitive on performance. This records the `benches/compare.rs` numbers so parity
is measurable. Reproduce with `cargo bench -p etude-byterope`.

- **Measured on:** aarch64 Linux, jemalloc (the bench's `#[global_allocator]`), release.
- **Sizes:** `shallow` = 4 chunks (rope stays in its flat `Small` tier); `deep` = 1000 chunks (past
  the promote threshold — exercises the radix tree). Chunks are 1400 B (MTU-ish).
- **Ratio** = byterope ÷ bytevec median. `<1.0` ⇒ byterope faster. Numbers are medians; ±few % run
  to run.

## Where byterope WINS (its reason to exist: O(1) clone, O(log) split, zero-copy slice)

| op | size | byterope | bytevec | ratio |
|----|------|----------|---------|-------|
| **clone** | 1000 chunks | **166 ns** | 15.1 µs | **0.011 (91× faster)** |
| **clone** | 100000 chunks | **36 ns** | 1.64 ms | **~45000× faster** |
| **slice** | deep | **1.13 µs** | 45.1 µs | **0.025 (40× faster)** |
| **slice** | shallow | 74.6 ns | 158 ns | 0.47 (2.1× faster) |
| **eq (distinct alloc)** | deep | ~47 µs | ~1.5 ms | ~0.03 (~30× faster) |
| **eq (shared clone, ptr fast path)** | deep | 4.4 µs | 58 µs¹ | ~0.08 |
| **split_to** | deep | 7.70 µs | 10.39 µs | 0.74 (1.35× faster) |
| **append** | deep | 3.43 µs | 4.00 µs | 0.86 (1.16× faster) |
| advance (drain) | shallow | 62.5 ns | 80.3 ns | 0.78 |

¹ bytevec has no pointer-identity fast path, so a shared-clone compare still costs a full walk.

## Parity (within ~5%)

| op | size | byterope | bytevec | ratio |
|----|------|----------|---------|-------|
| push_back | shallow | 80.4 ns | 82.1 ns | 0.98 |
| copy_to_bytes | deep | 63.3 µs | 61.3 µs | 1.03 |
| split_to_copy | deep | 64.3 µs | 61.4 µs | 1.05 |
| builder (put_slice) | deep | 96.5 µs | 94.0 µs | 1.03 |
| io_write / io_read | deep | 120.8 / 414 µs | 117.3 / 412 µs | ~1.03 / ~1.0 |
| truncate | deep | 5.75 µs | 5.03 µs | 1.14 |

## Where byterope TRAILS — the deep-tier per-chunk cost (the persistent-structure tradeoff)

These are all *sequential, per-chunk* operations in the **deep** tier: the radix tree pays a small
O(log₃₂) bookkeeping cost per chunk that a flat `VecDeque<Bytes>` push/pop/index does in O(1). This
is the price byterope pays to get the O(1) clone / O(log) split / zero-copy slice above. All shallow
variants are at parity or faster (the rope never touches a tree there).

| op | size | byterope | bytevec | ratio |
|----|------|----------|---------|-------|
| pop_back (drain) | deep | 15.5 µs | 10.0 µs | 1.55 |
| from_iter | deep | 24.7 µs | 16.5 µs | 1.49 |
| builder (put_bytes) | deep | 30.0 µs | 20.8 µs | 1.44 |
| extend | deep | 22.4 µs | 16.4 µs | 1.37 |
| push_back | deep | 22.6 µs | 17.7 µs | 1.28 |
| clear | deep | 10.8 µs | 8.5 µs | 1.27 |
| pop_front (drain) | deep | 18.0 µs | 14.6 µs | 1.23 |
| advance (drain) | deep | 18.0 µs | 15.5 µs | 1.16 |
| chunks_iter | deep | 2.13 µs | 1.16 µs | 1.83 |
| get(index) chunk | deep | 32.6 ns | 4.2 ns | 7.7 (abs. 33 ns — both trivial) |

## Interpretation

byterope is **parity-or-faster on the shallow streaming path** (the common case) and **wins by 1.35×
to ~45000×** on the structural operations it exists for — clone, slice, split, concat, equality. It
**trails 1.16×–1.55×** on deep-tier *sequential per-chunk* push/pop/drain/build/extend, and is slower
in relative terms on `chunks_iter`/`get(index)` (though the absolute costs there are tiny).

`from_iter`, `extend`, and `builder(put_bytes)` all funnel through per-chunk `push_back` into the
promoting tree — a **bulk bottom-up tree build** (construct the radix tree from N chunks at once
rather than N incremental inserts) is the single highest-leverage optimization and would close most
of the deep-tier gap at once. That is the next perf target before proposing the bytevec deletion.
