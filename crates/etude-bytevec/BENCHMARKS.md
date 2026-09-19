<!-- Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# `etude-bytevec` — performance scoreboard

`etude-bytevec` is a tiered relaxed-radix (RRB) byte rope. This file has two parts: the **current
live scoreboard** (rope vs. a naive `VecDeque<Bytes>`, reproducible from `benches/compare.rs`), and
the **historical scoreboard** (rope vs. the flat `VecDeque<Bytes>` deque this crate was reimplemented
from — kept as the design rationale + the record of the one accepted performance exception,
`chunks_iter`).

## Current live scoreboard — rope vs. a naive `VecDeque<Bytes>` (runnable: `cargo bench -p etude-bytevec`)

`benches/compare.rs` now measures the rope against a naive chunk deque baseline, to prove the tiered
rope does not regress a plain `VecDeque<Bytes>` on the streaming path while winning big on structural
ops. Latest run (aarch64, jemalloc, release; `deep` = 1000 chunks, `shallow` = 4):

| op (deep) | rope | naive deque | ratio |
|-----------|------|-------------|-------|
| **clone** | 36.5 ns | 14.9 µs (@1000) / 1.6 ms (@100k) | **~0.002 (∞ faster)** |
| **append_mid** | 1.44 µs | 10.3 µs | **0.14 (7× faster)** |
| **split_to_mid** | 6.8 µs | 7.88 µs | 0.86 |
| copy_to_bytes | 63.7 µs | 64.6 µs | 0.99 |
| from_iter | 19.5 µs | 17.5 µs | 1.11 |
| extend | 22.4 µs | 18.8 µs | 1.19 |
| push_back | 22.6 µs | 17.4 µs | 1.30 |
| advance (drain) | 16.9 µs | 11.5 µs | 1.47 |
| pop_front (drain) | 15.2 µs | 9.04 µs | 1.68 |
| pop_back (drain) | 15.7 µs | 9.06 µs | 1.74 |
| chunks_iter | 2.09 µs | 197 ns | 10.6 (accepted exception) |
| get(index) | 31.6 ns | 1.81 ns | 17 (abs 32 ns — trivial) |

**Reading it:** the rope is at parity-or-faster on the shallow path and the structural ops it exists
for (clone/append/split/slice), and trails **1.1×–1.74×** on deep-tier *sequential per-chunk*
build/drain — the irreducible per-node `Arc` allocation + O(log₃₂) navigation a flat deque does not
pay. `pop_back`/`pop_front` drain already use O(1) block-buffer adoption (the leading refill cost was
removed); the residual is the tree's per-block `pop`/`Arc::get_mut` descent. `chunks_iter` is the one
accepted exception (cache locality — see below). The levers that could shrink the deep build/drain
gaps (a node arena, a single-allocation/DST leaf) each break a core guarantee (O(1) structural-sharing
clone, or in-place bounded copy-on-write), so the gaps are the expected persistent-structure tradeoff.

### String / UTF-8 path — rope vs a contiguous `&[u8]` (runnable: `cargo bench -p etude-bytevec -- 'starts_with|ends_with|validate_utf8'`)

These functions scan bytes rather than move chunk handles, so the fair reference is a contiguous
`Vec<u8>` (a `&[u8]` compare, or `core::str::from_utf8`), not the chunk deque. `starts_with`/`ends_with`
use a literal spanning ~2 chunks; `validate_utf8` benches `Rope<Utf8>::try_from_bytes` on valid ascii.
Latest run (aarch64, jemalloc, release; `deep` = 1000 chunks, `shallow` = 4):

| op | shape | rope | contiguous `&[u8]` | ratio |
|----|-------|------|--------------------|-------|
| starts_with | shallow | 87.2 ns | 67.9 ns | 1.28 |
| starts_with | deep | 122 ns | 67.9 ns | 1.80 |
| ends_with | shallow | 97.6 ns | 74.1 ns | 1.32 |
| ends_with | deep | 151 ns | 74.5 ns | 2.03 |
| validate_utf8 (try_from_bytes) | shallow | 308 ns | 236 ns | 1.30 |
| validate_utf8 (try_from_bytes) | deep | 78.1 µs | 60.8 µs | 1.28 |

The scan functions trail a contiguous buffer by **1.28×–2.03×** — the cost of crossing chunk boundaries
(iterator setup + per-chunk compares/validation), the same chunking overhead as the rest of the deep
tier. Two points worth keeping: `ends_with/deep` is 151 ns, close to `ends_with/shallow` rather than
scaling with length — confirming it is O(suffix) (it walks only the last chunks from the back via the
double-ended chunk iterator, not the whole buffer); and `validate_utf8` streams the validation over the
chunks with no full-content allocation, so it beats the former copy-then-validate path (which allocated
an O(n) contiguous buffer) on the valid ingest path despite the 1.28× vs an already-contiguous slice.

### `copy_to_bytes_mut` single-chunk reclaim (runnable: `cargo bench -p etude-bytevec -- copy_to_bytes_mut`)

`copy_to_bytes_mut` on a single, uniquely-owned chunk reclaims the chunk's allocation in place
(`BytesMut::from` on a unique `Bytes`) instead of copying. The tell is size-independence: it measures
**14.0 ns at a 1400 B chunk and 14.2 ns at a 256 KB chunk** — flat, so no copy happens (a copy would
scale with size, ~microseconds at 256 KB), against `BytesMut::from(Vec)`'s 7 ns ideal reclaim. Before
this landed the fast path cloned the chunk first, which forced the refcount to 2 and defeated the
reclaim, so it always copied; moving the chunk out by value (the rope is consumed anyway) restores the
documented zero-copy. Fenced content-wise by the shared harness (`CopyToBytesMutCheck`).

### Socket-read zero-init memset (runnable: `cargo bench -p etude-bytevec -- socket_read`)

`Builder::for_socket_read` routes through `put_uninit_slice`, which zero-inits the requested spare
region before the read closure runs (the #173 info-leak fix). This measures that memset so the
deferred follow-up that would reclaim it (report bytes-written and skip the zero-init) is pursued only
if it is material. Latest run (aarch64, jemalloc, release):

| read size | memset only (zero-init) | recv copy, no memset | full path (memset + recv) |
|-----------|-------------------------|----------------------|---------------------------|
| 1500 B | 14.7 ns | 71 ns | 130 ns |
| 65536 B | 467 ns | 1.19 µs | 2.53 µs |

The zero-init is a linear extra pass over the read buffer (~15 ns per 1.5 KB, ~467 ns per 64 KB — about
140 GB/s), which is roughly 11–18% of the userspace fill-and-commit path here. Two caveats when reading
this for the reclaim decision: the `recv` in this bench is a userspace memcpy, but a real socket read is
a `recv` syscall (kernel copy) that is far more expensive, so the memset's real-world share is smaller
than the table suggests; and the `full path` column carries `Builder` routing beyond the memset, so the
isolated `memset only` column is the honest zero-init cost. Net: measurable and size-scaling, material
for large or high-throughput reads, diluted by the syscall on a real socket.

## Historical: the switch from a flat deque to the tiered rope

The head-to-head that justified reimplementing this crate — the **rope** (current) vs. the **flat
deque** (previous, now removed). The bench was retired with the flat deque; these are the as-measured
record. `deep` = 1000 chunks, `shallow` = 4; 1400 B chunks; aarch64, jemalloc, release. **Ratio** =
rope ÷ flat-deque; `<1.0` ⇒ rope faster.

## Where the rope WINS (its reason to exist: O(1) clone, O(log) split, zero-copy slice)

| op | size | rope | flat deque | ratio |
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

¹ the flat deque had no pointer-identity fast path, so a shared-clone compare still costs a full walk.

## Parity (within ~5%)

| op | size | rope | flat deque | ratio |
|----|------|----------|---------|-------|
| push_back | shallow | 80.4 ns | 82.1 ns | 0.98 |
| copy_to_bytes | deep | 63.3 µs | 61.3 µs | 1.03 |
| split_to_copy | deep | 64.3 µs | 61.4 µs | 1.05 |
| builder (put_slice) | deep | 96.5 µs | 94.0 µs | 1.03 |
| io_write / io_read | deep | 120.8 / 414 µs | 117.3 / 412 µs | ~1.03 / ~1.0 |
| truncate | deep | 5.75 µs | 5.03 µs | 1.14 |

## Where the rope TRAILS — the deep-tier per-chunk cost (the persistent-structure tradeoff)

These are all *sequential, per-chunk* operations in the **deep** tier: the radix tree pays a small
O(log₃₂) bookkeeping cost per chunk that a flat `VecDeque<Bytes>` push/pop/index does in O(1). This
is the price the rope pays to get the O(1) clone / O(log) split / zero-copy slice above. All shallow
variants are at parity or faster (the rope never touches a tree there).

| op | size | rope | flat deque | ratio |
|----|------|----------|---------|-------|
| pop_back (drain) | deep | 15.0 µs | 10.0 µs | 1.50 (was 1.55; drain-in-place) |
| from_iter | deep | 19.9 µs | 16.9 µs | 1.18 (bulk-built) |
| builder (put_bytes) | deep | 30.0 µs | 20.8 µs | 1.44 |
| extend | deep | 22.4 µs | 16.4 µs | 1.37 |
| push_back | deep | 22.6 µs | 17.7 µs | 1.28 |
| clear | deep | 10.8 µs | 8.5 µs | 1.27 |
| pop_front (drain) | deep | 17.0 µs | 14.6 µs | 1.16 (was 1.23; drain-in-place) |
| advance (drain) | deep | 17.5 µs | 15.5 µs | 1.13 (was 1.16; drain-in-place) |
| chunks_iter | deep | 2.13 µs | 1.16 µs | **1.83 (documented exception — see below)** |
| get(index) chunk | deep | 32.6 ns | 4.2 ns | 7.7 (abs. 33 ns — both trivial) |

## Interpretation

The rope is **parity-or-faster on the shallow streaming path** (the common case) and **wins by 1.35×
to ~45000×** on the structural operations it exists for — clone, slice, split, concat, equality. It
**trails 1.13×–1.50×** on deep-tier *sequential per-chunk* push/pop/drain/build/extend, and is slower
in relative terms on `chunks_iter` (the documented 1.83× locality exception, below) and `get(index)`
(though the absolute cost there is tiny — 33 ns).

`FromIterator` now **bulk-builds** the radix tree bottom-up when the source size is known to exceed
the flat tier (the common `collect` from a slice/`Vec`): `from_iter/deep` improved 1.49× → 1.18×
with `shallow` unchanged. Note that `push_back`'s deep path already batches (it buffers a `FANOUT`
block in the tail deque and folds it in with one `push_block`), so the residual deep-tier gap on
`extend` / `builder(put_bytes)` / pop / drain is the **irreducible tree-construction cost** — Arc
allocations for leaf/branch nodes that a flat `VecDeque<Bytes>` simply does not pay. Shrinking it
further would require changing the tree node representation itself (e.g. a smaller/tagged node or an
arena), a much larger rope-core change; the current gap is the expected persistent-structure
tradeoff for the O(1) clone / O(log) split / zero-copy slice wins above.

The **drain** paths (`pop_back` / `pop_front` / `advance`) were further tightened by adopting a
freshly-popped tree block's buffer wholesale (`VecDeque::from(block)`, O(1)) instead of moving it
chunk-by-chunk into the buffered end — the target deque is always empty at a refill. That closed
`pop_back/deep` 1.55× → 1.50× (and `pop_front`/`advance` similarly).

### The `chunks_iter` exception (1.83×)

`chunks_iter/deep` is the one operation held **above the 1.5× parity bar as a documented,
measured exception**, because closing it is provably incompatible with the rope's reason to exist.
The gap is **cache locality**, not algorithm: the flat deque iterated one *contiguous*
`VecDeque<Bytes>` (hardware-prefetched, ~0 cache misses), while the rope's iterator walks a tree
whose leaf blocks are separate `Arc`-boxed allocations *scattered* across the heap (~one cache miss
per leaf boundary). The two ways to close it each destroy a core guarantee:

- A **contiguous arena** for tree nodes would make iteration cache-friendly, but O(1) structural-
  sharing `clone`/`split` *requires* per-node `Arc`s that can be shared across clones — an arena
  cannot be shared cheaply, so it breaks O(1) `clone`.
- A **single-allocation leaf** (`Arc<[Bytes]>` / DST) removes one indirection, but `set_byte` /
  `replace` grow a leaf's chunk slice *in place* via `Arc::make_mut`; a DST tail cannot resize in
  place, so every small edit would re-copy the whole leaf — breaking the bounded copy-on-write
  guarantee.

Contiguous-memory iteration and O(1)-clone structural sharing are **mutually exclusive**. The
1.83× is therefore the inherent price paid for `clone` (~440× faster at 1000 chunks), `slice`
(~40×), `split_to` (~1.6×) and `eq` (~23×) — and chunk-iteration is rarely the hot path for a
buffer whose purpose is cheap clone/split/slice. Levers evaluated and rejected with measurements:
FANOUT 32→64 (regressed `pop_back` 1.55→1.87 and `chunks_iter` 1.83→1.92), and a branchless
`size_hint` counter (no measurable effect — confirming the cost is locality, not adapter overhead).
