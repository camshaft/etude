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
| split_to_copy | 63.1 µs | 59.8 µs | 1.06 |
| from_iter | 19.5 µs | 17.5 µs | 1.11 |
| extend | 19.3 µs | 18.6 µs | 1.04 |
| clear | 10.3 µs | 7.89 µs | 1.30 |
| push_back | 22.6 µs | 17.4 µs | 1.30 |
| advance (drain) | 16.9 µs | 11.5 µs | 1.47 |
| pop_front (drain) | 14.7 µs | 9.20 µs | 1.60 |
| pop_back (drain) | 15.4 µs | 9.34 µs | 1.65 |
| chunks_iter | 2.09 µs | 197 ns | 10.6 (accepted exception) |
| get(index) | 31.6 ns | 1.81 ns | 17 (abs 32 ns — trivial) |

**Reading it:** the rope is at parity-or-faster on the shallow path and the structural ops it exists
for (clone/append/split/slice), and trails **1.1×–1.74×** on deep-tier *sequential per-chunk*
build/drain — the irreducible per-node `Arc` allocation + O(log₃₂) navigation a flat deque does not
pay. The two _copy_ ops (`copy_to_bytes`, `split_to_copy`) are at parity (~0.99–1.06): both materialise
the same contiguous bytes, so the shared memcpy dominates and the rope's per-chunk drain overhead is in
the noise. `clear` (1.30) sits with the per-chunk build/drain group — it drops the tree's per-node
`Arc`s one level at a time where the deque drops a single contiguous buffer. `extend` into an empty rope
from a known-large source now routes through the same bulk bottom-up build as `from_iter` (folding whole
`FANOUT` blocks into the tree) instead of a `push_back`-per-chunk loop — that took `extend/deep` 1.19 →
1.04 (~15%); the gate matches `from_iter`'s size threshold so the shallow path stays on the plain loop
(within ~1 ns). `pop_back`/`pop_front` drain already use O(1) block-buffer adoption (the leading refill cost was
removed), and the post-pop demote check is now split so the hot path inlines only a couple of cached-count
loads and a branch while the flatten body stays out-of-line behind `#[cold]` — that took `pop_front/deep`
1.68 → 1.60 and `pop_back/deep` 1.74 → 1.65 (~6–7%). The residual is the tree's per-block `pop`/`Arc::get_mut`
descent. `chunks_iter` is the one
accepted exception (cache locality — see below). The levers that could shrink the deep build/drain
gaps (a node arena, a single-allocation/DST leaf) each break a core guarantee (O(1) structural-sharing
clone, or in-place bounded copy-on-write), so the gaps are the expected persistent-structure tradeoff.

### String / UTF-8 path — rope vs a contiguous `&[u8]` (runnable: `cargo bench -p etude-bytevec -- 'starts_with|ends_with|validate_utf8'`)

These functions scan bytes rather than move chunk handles, so the fair reference is a contiguous
`Vec<u8>` (a `&[u8]` compare, or `core::str::from_utf8`), not the chunk deque. `starts_with`/`ends_with`
use a literal spanning ~2 chunks; `validate_utf8` benches `Rope<Utf8>::try_from_bytes` on valid ascii.
Latest run (aarch64, jemalloc, release; `deep` = 1000 chunks, `shallow` = 4):

These functions now scan via the direct early-exit traversals (`try_for_each_chunk` /
`try_for_each_chunk_rev`, a recursive DFS with `ControlFlow` break) rather than the `Chunks` iterator, so
a prefix/suffix scan that stops early never pays to build the iterator's resumable stacks. Latest run
(aarch64, jemalloc, release; `deep` = 1000 chunks, `shallow` = 4; the `was` column is the prior
`chunks()`-iterator implementation on the same box):

| op | shape | rope (traversal) | was (`chunks()`) | contiguous `&[u8]` |
|----|-------|------------------|------------------|--------------------|
| starts_with | shallow | 80.6 ns | 88.4 ns | 67.9 ns |
| starts_with | deep | 89.8 ns | 122 ns | 67.9 ns |
| ends_with | shallow | 85.7 ns | 97.4 ns | 74.1 ns |
| ends_with | deep | 95.1 ns | 152 ns | 74.5 ns |
| validate_utf8 (try_from_bytes) | shallow | 298 ns | 310 ns | 236 ns |
| validate_utf8 (try_from_bytes) | deep | 76.9 µs | 78.4 µs | 60.8 µs |

The early-exit traversal helps most where the scan stops before the end: `starts_with/deep` improved
**122 → 90 ns (~26%)** and `ends_with/deep` **152 → 95 ns (~38%)**, since neither now builds the deep-tier
iterator's tree-descent stacks just to look at the near end (`ends_with` uses the reverse
`try_for_each_chunk_rev`). `validate_utf8` scans the whole content on the valid path (no early exit), so it
gains only the small forward-traversal-vs-iterator margin (~2–4%). The residual gap to a contiguous
`&[u8]` is the inherent cost of crossing chunk boundaries. Note `ends_with/deep` stays close to
`ends_with/shallow` rather than scaling with length — it is O(suffix), touching only the last chunks — and
`validate_utf8` streams validation with no full-content allocation, beating the former copy-then-validate
path on ingest despite trailing an already-contiguous slice.

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

### `Rope<Utf8>` mutation — `append_bytes` / `insert_bytes` vs `std::String` (runnable: `cargo bench -p etude-bytevec -- 'append_bytes|insert_bytes'`)

The typed `Rope<Utf8>` string mutators take pre-validated bytes and stitch chunks; the fair reference
is `String::push_str` / `String::insert_str`, which memcpy/memmove into one contiguous buffer. `append`
repeats a 6-byte fragment `n` times from empty; `insert` builds a `6·n`-byte buffer once and inserts a
3-byte literal at the midpoint. Latest run (aarch64, jemalloc, release; `deep` = 1000, `shallow` = 4):

| op | shape | `Rope<Utf8>` | `std::String` | ratio |
|----|-------|--------------|---------------|-------|
| append_bytes | shallow | 84.2 ns | 87.9 ns | 0.96 |
| append_bytes | deep | 29.0 µs | 2.16 µs | 13.4 |
| insert_bytes | shallow | 111 ns | 43.0 ns | 2.6 |
| insert_bytes | deep | 226 ns | 645 ns | **0.35 (2.9× faster)** |

The split is exactly the persistent-structure story. `append_bytes/deep` is the rope's worst case: each
6-byte fragment becomes its own chunk, so `n` tiny appends build a 1000-node tree while `String`
amortizes into one contiguous push — a 13× gap that is the tiny-fragment amplification of the deep-tier
per-chunk `Arc` cost already documented above (appending _chunk handles_, the rope's actual job, is the
`append`/`extend` rows in the main scoreboard, not this). `insert_bytes/shallow` also trails: for a
few-byte buffer a memmove is cheaper than the split-and-stitch bookkeeping. But `insert_bytes/deep`
inverts it — inserting into a `6 KB` buffer splits the chunk into two zero-copy `Bytes` slices plus the
inserted middle, no payload copy, so the rope is **2.9× faster** than `String`'s O(n) midpoint memmove,
and (like `ends_with`) is near-flat in the buffer length rather than scaling with it.

### Builder write path — `put_slice` / `put_bytes` vs a direct `VecDeque<Bytes>` (runnable: `cargo bench -p etude-bytevec -- builder_put`)

`Builder` accumulates writes into a coalescing head buffer (128 KiB here, the crate default), flushing
it to a chunk when full, then `finish()` seals the tail. `put_slice` copies each write into that buffer;
`put_bytes` holds each `Bytes` by reference (default inline threshold 0). The naive reference builds a
`VecDeque<Bytes>` directly — one `Bytes::copy_from_slice` per `put_slice` (no coalescing), a bare
`push_back` per `put_bytes`. Latest run (aarch64, jemalloc, release; `deep` = 1000, `shallow` = 4):

| op | shape | builder | naive deque | ratio |
|----|-------|---------|-------------|-------|
| put_slice | shallow | 573 ns | 176 ns | 3.25 |
| put_slice | deep | 66.7 µs | 130 µs | **0.51 (2× faster)** |
| put_bytes | shallow | 103 ns | 80.8 ns | 1.28 |
| put_bytes | deep | 27.2 µs | 17.8 µs | 1.53 |

`put_slice` is the builder's reason to exist and it shows at `deep`: coalescing 1000 slices into 128 KiB
buffers makes ~11 chunk allocations where the naive path makes 1000 `copy_from_slice` allocations, so the
builder is **2× faster**. The `shallow` sign-flip is the honest tradeoff — for only 4 small writes the
builder allocates a whole 128 KiB head buffer that never amortizes, dwarfing 4 tiny `copy_from_slice`s; a
caller that writes little should size the builder down with `ByteVec::builder(small_cap)`. `put_bytes`
holds by reference, so it is `push_back` through the builder's flush machinery and sits with the deep-tier
per-chunk build family (1.28–1.53×, the per-chunk `Arc` + flush-check overhead) — the builder adds
coalescing value on copied writes, not on by-reference chunk handoff.

### Reader — non-destructive full read via `ByteVec::reader` (runnable: `cargo bench -p etude-bytevec -- reader_iterate`)

`reader()` reads without disturbing the source, and the `Iterator` yields each chunk (a cheap `Bytes`
handle). How it holds the source depends on the tier: a *Small* source is read through a cursor that
borrows the source chunks directly (no clone), a *Deep* source through an O(1) structural-shared clone
drained in place. The naive equivalent of a non-destructive full read is an O(n) `clone()` of the deque
followed by a `pop_front` drain. Latest run (aarch64, jemalloc, release; `deep` = 1000, `shallow` = 4):

| op | shape | rope reader | naive clone + drain | ratio |
|----|-------|-------------|---------------------|-------|
| reader_iterate | shallow | 78.1 ns | 78.0 ns | 1.00 |
| reader_iterate | deep | 23.4 µs | 16.8 µs | 1.39 |

Shallow now matches the naive deque drain (1.00): the borrowed cursor does the same per-chunk work with
no upfront clone, so there is nothing to trail. Deep still trails 1.39×, for the persistent-structure
reason: the reader's *setup* is O(1) (the structural-shared tree clone is ~37 ns regardless of length),
but the cost is on *iteration* — because the source rope is still alive (the whole point of a
non-destructive reader), the shared clone's spine `Arc`s have a refcount above 1, so each `pop_front`
must copy-on-write the node it mutates instead of mutating in place. So the deep reader pays cheap-setup +
COW-drain where the deque pays copy-everything-once + O(1)-drain, and at these sizes the COW-drain edges
it out. A consumer that does *not* need the source afterward should drain the rope directly (`pop_front` /
`advance`, ~15 µs deep, no COW) rather than take a reader; the reader earns its keep precisely when the
source must stay intact.

**Fork cost** (runnable: `cargo bench -p etude-bytevec -- reader_fork`). Creating the reader is O(1) in
both tiers. The Small cursor borrows the source chunk list rather than cloning it, so the fork is a flat
handful of nanoseconds regardless of chunk count; the Deep fork is the O(1) structural-shared tree clone:

| fork (`reader()`) | 1 chunk | 4 | 16 | 32 (Small max) | 1000 (Deep) |
|-------------------|---------|-----|------|----------------|-------------|
| borrowed cursor / shared clone | 4.7 ns | 4.7 ns | 4.7 ns | **4.7 ns** | **37.5 ns** |

Earlier this crate forked the Small tier by cloning the inline `VecDeque` and bumping every chunk handle,
which was O(n) — a full 32-chunk Small fork cost ~495 ns, ~13× a 1000-chunk Deep fork. Because
`reader(&self) -> Reader<'_>` already borrows the rope for the reader's lifetime, the Small path now reads
through a borrowing cursor with no public-API change, dropping that 495 ns to ~4.7 ns (~105×). It
reproduces the owning drain's read sequence exactly — including `partial_copy_into`'s maximal-run ordering
contract that `Builder` relies on — pinned by a deterministic tier-spanning parity test and a fuzzed
borrowed-vs-owning trace-equality check.

**Fan-out / broadcast** (runnable: `cargo bench -p etude-bytevec -- reader_fanout`). The fork's O(1)-ness
is the whole point in a fan-out: one source buffer, many independent consumable cursors, each peeking a
little. Handing a plain `VecDeque<Bytes>` an *independent, consumable* cursor per reader means cloning the
whole deque each time — you cannot advance N cursors over one deque without N copies — so it is O(readers ×
chunks). The rope forks by borrowing (Small) or an O(1) shared clone (Deep). 64 readers, each reading the
first chunk:

| reader_fanout | source | rope | naive deque (clone-per-reader) | ratio |
|---------------|--------|------|--------------------------------|-------|
| 64 readers | small_max (32) | 1.24 µs | 32.2 µs | **0.038 (~26× faster)** |
| 64 readers | deep (1000) | 76.2 µs | 972 µs | **0.078 (~13× faster)** |

This is the same O(1)-clone advantage the `clone` row shows, applied to the reader: the naive baseline
pays a full deque copy per cursor, the rope pays only the fork plus what each cursor actually touches. The
deep rope's 76 µs is not the fork (that is ~37 ns × 64 ≈ 2.4 µs) but the first `next()` on each cursor
copying-on-write the shared leftmost leaf — the documented reader drain cost — yet it still finishes ~13×
ahead because it never copies the untouched remainder of the buffer.

### Compaction — `compact` / `compact_with` (runnable: `cargo bench -p etude-bytevec -- compact_`)

`compact()` collapses a fragmented rope into one contiguous allocation; `compact_with(skip_above(n))`
coalesces the small fragments but leaves segments over `n` bytes in place (no memcpy). The fragmentation
bench input is a rope of groups — 8 small (64 B) fragments then one large (2 KiB) chunk — so full compact
copies everything into one buffer, while the skip variant coalesces each small run and keeps the large
chunks. The single-chunk bench compacts one 64 KiB chunk that is either shared (a second handle held) or
uniquely owned. Latest run (aarch64, jemalloc, release; `deep` = 1000 groups ≈ 9000 chunks / 2.56 MB,
`shallow` = 4 groups; the sub-µs shallow rows are small and noisy on a shared box — the deep rows are the
stable signal):

| op | shape | time |
|----|-------|------|
| compact_full | shallow | ~2 µs |
| compact_full | deep | 293 µs |
| compact_skip_large | shallow | 1.3 µs |
| compact_skip_large | deep | 300 µs |
| compact_single_chunk | shared_released (64 KiB) | 2.0 µs |
| compact_single_chunk | unique_noop | 19 ns |

Things the numbers pin down. The full `compact()` scans the chunks once to size the coalesce buffer to the
exact total, then fills that one pre-sized buffer (it never reallocates mid-fill) and swaps it in as the
sole chunk — no intermediate segment list and no per-chunk rebuild when the whole rope collapses to one
chunk (the common case). Two levers got it there: pre-sizing the buffer (no grow-as-you-go realloc/recopy),
and running the scan and the fill over the direct `for_each_chunk` traversal — a straight recursive DFS
that skips the `Chunks` iterator's per-chunk save/restore bookkeeping — instead of the iterator. Together
they took `compact()/deep` from 457 µs (naive buffer + iterator) to ~293 µs, roughly a third faster; the
traversal switch alone accounted for about the last ~344 → ~293 µs of that (the same primitive backs
`copy_to_bytes`, which is unchanged at ~62 µs). `skip_above` saves the large-chunk memcpy
(here ~2 MB of the 2.56 MB is left in place) but only edges out full compact at `deep` (300 vs 344 µs),
because leaving the large chunks in place means the result still has ~2000 segments to rebuild, and that
rebuild offsets most of the copy saved — so `skip_above` is most worthwhile when it keeps a *few* genuinely
large segments, not many. The single-chunk rows show the release behavior: a *shared* lone chunk (a small
view pinning a large backing another handle holds) is copied out into a fresh right-sized allocation to
release the backing (one 64 KiB copy, ~2 µs), while a *uniquely-owned* lone chunk — nothing to consolidate
or release — is a ~19 ns no-op.

### Streaming / FIFO workloads (runnable: `cargo bench -p etude-bytevec -- stream_`)

The isolated `push_back` and `pop_front` benches never *interleave*, yet the canonical use of a byte
rope — a socket or pipe buffer — does exactly that: bytes flow in at the back and out at the front while
the buffer sits at some backlog. Two shapes, each 64 push/pop rounds (aarch64, jemalloc, release):

| workload | shape | rope | naive deque | ratio |
|----------|-------|------|-------------|-------|
| stream_fifo | shallow (backlog 4) | 1.86 µs | 1.47 µs | 1.27 |
| stream_fifo | boundary (backlog 48) | 2.82 µs | 2.22 µs | 1.27 |
| stream_fifo | deep (backlog 1000) | 4.47 µs | 2.82 µs | 1.58 |
| stream_churn | oscillate 20↔80 | 3.61 µs | 2.18 µs | 1.66 |

What the numbers confirm — no surprises, and one thing worth pinning:

- **The hysteresis works.** `boundary` holds the backlog at 48, *between* `DEMOTE_AT` (32) and
  `PROMOTE_AT` (64), so the rope stays in the flat tier and never churns its representation — its ratio
  (1.27×) is identical to `shallow`, not elevated. A single-threshold design (promote and demote at the
  same count) would thrash here on every push/pop that crossed the line; the 32-wide band is what keeps a
  buffer parked near the boundary flat.
- **Steady-state deep (1.58×)** is the interleaved per-node tree cost: the rope keeps two buffered ends
  (`head` + `tail` deques) around the tree where the flat deque has one contiguous buffer, plus the
  amortized block freeze/adopt every `FANOUT` ops. Same persistent-structure tradeoff as the isolated
  `push_back`/`pop_front` deep rows, no worse for being interleaved.
- **Churn (1.66×)** is the deliberate worst case: an oscillation whose amplitude (60) exceeds the
  hysteresis band, so each cycle pays one full `promote` (deque → tree) and one `demote` (tree → deque).
  This is inherent to an amplitude that large — a wider band would not help a 20↔80 swing — and `promote`
  already builds the tree by folding whole `FANOUT` blocks (the bulk path), so there is no cheap lever
  here beyond the representation changes ruled out below. It is bounded and predictable, not a blow-up.

The **amplitude sweep** (`cargo bench -p etude-bytevec -- churn_sweep`) pins where churn actually starts:
oscillate the backlog around a center of 48 (midway between `DEMOTE_AT` 32 and `PROMOTE_AT` 64) with a
growing amplitude, and watch the rope-vs-naive ratio step up as the swing clears the band:

| amplitude | swing | rope | naive deque | ratio |
|-----------|-------|------|-------------|-------|
| 16 | 40↔56 | 813 ns | 646 ns | 1.26 |
| 24 | 36↔60 | 1.17 µs | 972 ns | 1.20 |
| 32 | 32↔64 | 1.84 µs | 1.59 µs | 1.16 |
| 40 | 28↔68 | 3.18 µs | 1.83 µs | **1.74** |
| 56 | 20↔76 | 3.44 µs | 1.99 µs | 1.73 |
| 64 | 16↔80 | 3.65 µs | 2.08 µs | 1.75 |

The step is sharp and lands exactly where the theory says it should. Swings up to amplitude 32 stay inside
the band (a 32↔64 swing never promotes — promotion needs 65 chunks — nor demotes below the flat tier), so
the rope just tracks the naive deque's growing op count at the usual ~1.2× flat-tier overhead. At
amplitude 40 the swing finally clears both thresholds and every cycle pays a `promote` + `demote`, so the
ratio jumps to ~1.74× and then holds flat (bigger swings cross the *same* two thresholds, once each). This
is the hysteresis band doing its job: it absorbs oscillations up to its full width before the
representation flips, where a single-threshold design would flip on the very first push/pop across the
line. The band width is a deliberate memory-vs-churn choice, not headroom.

### Build: bulk vs incremental crossover (runnable: `cargo bench -p etude-bytevec -- build_crossover`)

Building the same rope two ways, swept across sizes: bulk (`collect` / `from_iter`, which folds whole
`FANOUT` blocks bottom-up in one pass when the size hint exceeds `PROMOTE_AT`) vs incremental (a
`push_back` loop). Locates where the bulk build's amortization starts to pay (aarch64, jemalloc, release):

| chunks | bulk | incremental | ratio (bulk ÷ incr) |
|--------|------|-------------|---------------------|
| 16 | 378 ns | 362 ns | 1.04 |
| 32 | 711 ns | 674 ns | 1.05 |
| 64 | 1.34 µs | 1.25 µs | 1.07 |
| 128 | 2.34 µs | 3.31 µs | **0.71** |
| 256 | 4.60 µs | 6.03 µs | **0.76** |
| 1024 | 19.1 µs | 23.6 µs | **0.81** |

The crossover sits between 64 and 128 chunks. At and below `PROMOTE_AT` (64) the two are the *same code*
— `from_iter` only takes the bulk tree path when the size hint exceeds `PROMOTE_AT`, so a small `collect`
runs the identical flat `push_back` loop, and the ~4–7% it trails there is just the iterator-adapter
overhead of `Cloned<Iter>` over a direct `for` loop, not a rope cost. Above the threshold the incremental
path takes on the per-chunk tree descent for every chunk past promotion, while the bulk build folds whole
blocks in one bottom-up pass — so bulk pulls ~20–30% ahead and stays there. This confirms the `from_iter`
gate is well-placed: the bulk machinery is spent only once it is solidly the cheaper path, and small
builds stay on the plain loop.

### Access pattern: sequential vs random point `get` (runnable: `cargo bench -p etude-bytevec -- access_pattern`)

Point access (`get(index)`) descends the size table O(log₃₂) from the root on every call. That descent
is cache-order-sensitive in a way a flat deque's O(1) index is not — measured on a tall 100k-chunk tree
(aarch64, jemalloc, release):

| access order | rope `get` | naive deque `get` |
|--------------|------------|-------------------|
| sequential | 54.2 ns | 3.66 ns |
| random | 93.3 ns | 4.96 ns |

Sequential indices reuse almost the same root→branch→leaf path (crossing a leaf boundary only every
`FANOUT`), so the descent stays cache-warm and well-predicted; random indices land on cold nodes and miss
cache on each hop — a **1.72×** penalty for the rope (the deque shows a smaller 1.35× effect, just from
touching `Bytes` handles scattered across the 140 MB of backing). The takeaway is an API one, not a
missing optimization: for *sequential* traversal use `chunks()`, which keeps the descent stacks alive and
yields each chunk in ~2 ns amortized (`chunks_iter`), ~27× cheaper per element than re-descending from the
root with `get`. A cursor cache on `get` could close the sequential gap, but it would need interior
mutability on the `&self` read path (breaking `Sync`) to serve exactly the access pattern `chunks()`
already serves — so `get` stays the clean random-access primitive and `chunks()` the sequential one.

### Mixed read/write interleave (runnable: `cargo bench -p etude-bytevec -- mixed_rw`)

A growing, randomly-queried buffer — an append-only log with random lookups, or a reassembly buffer
inspected as it fills. Each of 64 rounds appends one chunk (write) and reads a byte at a random live
offset (read), starting from a shallow or a deep backlog (aarch64, jemalloc, release):

| start | rope | naive deque | ratio |
|-------|------|-------------|-------|
| from_shallow (4) | 3.28 µs | 2.51 µs | 1.31 |
| from_deep (1000) | 7.79 µs | 29.1 µs | **0.27 (~3.7× faster)** |

The tradeoff flips with backlog size, because the two halves stress opposite structures. The write is
amortized-cheap for both; the read is where they diverge — the rope indexes O(log₃₂) down its size table
regardless of length, while the deque must walk chunks O(n) to reach the offset. On a small buffer
(`from_shallow`) the walk is short, so the read is cheap for both and the rope's per-chunk write overhead
dominates — it trails 1.31× like the other shallow rows. On a large buffer (`from_deep`) each of the 64
random reads walks up to ~1000 chunks in the deque but stays a shallow tree descent in the rope, so the
read half swamps everything and the rope finishes **~3.7× ahead**. This is the point-lookup counterpart to
the `slice`/`clone`/`append` structural wins: the more content a live buffer holds, the more the rope's
O(log) addressing pays over a flat deque's O(n) reach.

### Chunk-size distribution: many-tiny vs few-huge (runnable: `cargo bench -p etude-bytevec -- chunkdist`)

The same ~1 MiB payload delivered at three chunk granularities — 64 B (~16k chunks, a tall deep tree),
1400 B MTU (~750 chunks), and 64 KiB (~16 chunks, the flat `Small` tier). Chunk *count*, not just size,
is what the rope's structure keys on (aarch64, jemalloc, release):

| op | distribution | rope | naive deque | ratio |
|----|--------------|------|-------------|-------|
| flatten (`copy_to_bytes_mut`) | tiny64 (~16k) | 225 µs | 197 µs | 1.14 |
| flatten | mtu1400 (~750) | 44.6 µs | 40.9 µs | 1.09 |
| flatten | huge64k (~16) | 33.0 µs | 32.6 µs | 1.01 |
| random `byte_at` | tiny64 (~16k) | 95.5 ns | 6.34 µs | **0.015 (~66× faster)** |
| random `byte_at` | mtu1400 (~750) | 54.0 ns | 304 ns | **0.18 (~5.6× faster)** |
| random `byte_at` | huge64k (~16) | 22.3 ns | 18.7 ns | 1.19 |

The two ops pull in opposite directions as fragmentation rises, and both behave as the structure
predicts — no new pathology. **Flatten** trails 1.01→1.14× as the chunk count climbs: it is one presized
memcpy per chunk for both, and the rope pays a little extra to walk the tree where the deque iterates a
flat buffer (the same locality cost as `chunks_iter`), so the gap scales with count and is inherent.
**Random `byte_at`** is the mirror image and the headline: the rope addresses any offset in O(log₃₂)
regardless of fragmentation, while the deque walks chunks O(n) — so at 16k tiny chunks the rope is **~66×
faster**, at MTU ~5.6×, and only at ~16 huge chunks does the deque's short walk edge it (~19 ns). The
practical guidance: fragmentation barely dents rope addressing but is quadratic-feeling for a flat deque;
if a producer floods a buffer with tiny chunks and you only ever flatten it, `compact()` first — but if
you index into it, the tiered rope is exactly the structure you want.

### `split_to` cost by cut position (runnable: `cargo bench -p etude-bytevec -- split_position`)

`split_to` on a deep rope always takes the O(log₃₂) tree-split fast path, but the constant is not
position-uniform — splitting a 1000-chunk rope (aarch64, jemalloc, release):

| cut position | time |
|--------------|------|
| middle (`total/2`) | 7.12 µs |
| near the end (`total-700`) | 1.35 µs |

A cut near an edge descends to a shallow boundary and repacks only the handful of nodes along the
right spine, while a mid cut splits deep nodes on both sides and repacks the seam of two substantial
halves — so an edge-adjacent split is **~5× cheaper** than a mid one on the same rope. The takeaway is
that `split_to`/`truncate`/`advance` near either end are cheap regardless of total length (peeling a
frame off the front or trimming a tail is close to free), while the priciest split is the balanced
mid-cut; there is no folding or degeneracy involved — both are the same fast path, just touching
different amounts of the tree.

### Assemble from many fragments (runnable: `cargo bench -p etude-bytevec -- assemble`)

Gathering scattered buffers into one rope — 1024 chunks assembled as 64 fragments of 16. Two rope
strategies for the same result: fold via repeated `append` (each an O(log₃₂) RRB concat) vs collecting
every chunk at once (one bulk bottom-up build). This is the concat/rebalance path the per-op and workload
sweeps do not reach (aarch64, jemalloc, release):

| strategy | time |
|----------|------|
| rope, `append`-fold | 41.4 µs (was 58.4 µs) |
| rope, `from_iter` (bulk) | ~19 µs |
| naive deque, append-fold | ~25 µs |

Two things this surfaced. First a **validation**: a random `byte_at` on the `append`-folded rope (61.9 ns)
is indistinguishable from one on the bulk-built rope (60.6 ns), so repeatedly repacking the concat seam
does *not* leave a degenerate tree — the assembled structure reads exactly as well. Second a **fix**: the
`append`-fold was needlessly building a whole tree for each tiny fragment and concat-repacking the seam.
Appending a *small* `other` (≤ `FANOUT` chunks — which, since a `Deep` rope always holds more than
`DEMOTE_AT` chunks, is always a flat fragment) now pushes its handful of chunks straight onto the back
(batched into blocks) instead, which took the fold **58.4 → 41.4 µs (~29%)** with no change to the
large-`other` case (`append_mid/deep` still shares structurally via `concat`, unchanged at 1.44 µs). Bulk
`from_iter` is still the fastest way to assemble when every fragment is in hand at once — reach for the
`append`-fold only when the fragments arrive incrementally.

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

## Recorded dead ends (measured, do not re-litigate)

The clean per-method wins have been harvested (`compact`, `extend` bulk build, the pop cold-split, and
the reader Small-tier borrow). What remains on the deep tier is the persistent-structure tradeoff above,
not headroom. Experiments that were tried, measured, and rejected:

- **`truncate` deep — pop-first instead of peek-then-pop.** The Deep whole-block-drop path calls
  `back_block_bytes()` (a rightmost O(height) descent that reads a leaf's *cached* `bytes`) and then
  `pop_back_block()` (a second descent). Replacing that with a single `pop_back_block()` followed by
  re-summing the popped block's 32 chunk lengths *regressed* `truncate_half/deep` 5.97 → 6.29 µs (~5%):
  the cached-total peek is a few cheap pointer hops, while re-summing 32 `Bytes` lengths per block costs
  more than the descent it removes. A non-regressing variant would thread the cached `Block.bytes` out of
  the pop itself (dropping both the peek and the re-sum), but that means plumbing the total through the
  shared-tree persistent pop path for a projected sub-2% (and likely unmeasurable) gain — not worth the
  surface on that path. The peek-then-pop form stands.
- The deep sequential build/drain ratios (`push_back` ~1.32×, `truncate` ~1.20×, `clear`/`advance`/`pop`
  1.30–1.74×) and the trivial-absolute shallow gaps (`truncate` shallow ~1.16× at ~30 ns, `append_mid`
  shallow ~1.27× at ~150 ns) are the expected tradeoff or below the noise floor for a per-method win.
