<!-- Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# `etude-span` — performance scoreboard

`etude-span` provides the byte-scanning primitives every rope tokenizer builds on: a `Span` (a
byte range, carrying no bytes) and a `Cursor` that streams a rope's leaves. This file is the
runnable scoreboard for the scanning hot path, reproducible from `benches/span.rs`
(`cargo bench -p etude-span`). It is stood up one hot path at a time.

`Span`'s methods (`start`/`end`/`len`/`is_empty`/`range`) are O(1) field reads and are not
benchmarked. The cursor is where the time goes.

## `Cursor` scan — walking a rope once (runnable: `cargo bench -p etude-span`)

A tokenizer walks the input rope front to back. The `Cursor` reads each contiguous leaf once with a
local slice index and refills at leaf boundaries, so a full walk is O(n) amortized. The alternative —
asking the rope for `byte_at(i)` at every offset — is an O(log n) tree descent per byte, O(n log n)
over the input. This scans the same 64 KiB input three ways at three leaf layouts (aarch64, jemalloc,
release):

- `cursor/peek_bump`: the byte-at-a-time tokenizer inner loop (`peek`, then `bump`).
- `cursor/bulk`: the bulk path — `chunk_tail` scans a whole leaf as one slice, `skip_in_chunk` jumps
  past it in a single step.
- `byte_at/scan`: the naive baseline the cursor replaces — `byte_at(i)` for every offset.

| scan | 64 B leaves (1024) | 4 KiB leaves (16) | single leaf |
|------|--------------------|-------------------|-------------|
| `cursor/bulk` | 15.9 µs | 5.8 µs | 5.5 µs |
| `cursor/peek_bump` | 55.5 µs | 49.5 µs | 49.0 µs |
| `byte_at/scan` | 2.37 ms | 668 µs | 152 µs |

Two things the numbers pin down. First, **`byte_at` scanning collapses as the rope fragments**: 152 µs
for a single leaf (a direct index), 668 µs at 16 leaves, and 2.37 ms at 1024 leaves — the descent
deepens and every byte pays it. On a fragmented rope the bulk cursor is ~150× faster and even the
byte-at-a-time cursor is ~40× faster; this is the cursor's reason to exist. Second, `cursor/bulk`
(5.5–15.9 µs) is still **~3–9× faster** than `cursor/peek_bump` (49–55 µs): the per-leaf inner loop
is a tight contiguous slice scan, while `peek`/`bump` still pays an `Option` and a boundary branch on
every byte. `peek_bump` is nearly flat across layouts (the per-byte cost dominates; refills are minor),
whereas `bulk`'s cost tracks leaf count (refill frequency), so prefer `chunk_tail`/`skip_in_chunk` for
scanning a known run.

The `peek_bump` numbers above are **after** marking the cursor hot methods `#[inline]`. When the harness
first measured them (see history) they were 156–168 µs — nearly flat because each per-byte `peek` and
`bump` was a cross-crate function call that the downstream crate could not inline. Adding `#[inline]`
(and `#[cold]` on the leaf-boundary `refill` so the inlined body stays just the increment and the branch)
cut the inner loop by ~3× (−67 to −69%). The residual `bulk`-vs-`peek_bump` gap is the intrinsic
per-byte `Option`/branch, not call overhead.

## `tokenize` — the realistic tokenizer shape (runnable: `cargo bench -p etude-span -- tokenize`)

The scan benches above walk the whole input one way. A real tokenizer instead alternates: skip a
delimiter, then scan a token's run, and stamp a [`Span`] per token. This benches a delimited 64 KiB
stream (single-byte delimiters, tokens straddling 64 B leaves) tokenized two ways at three token
lengths — the bulk path (find the next delimiter in `chunk_tail` with a slice scan, `skip_in_chunk` to
it, continuing across leaf boundaries) vs naive per-byte `peek`/`bump`. Each token stamps a `Span`, as
a real tokenizer would.

| tokenizer | 4 B tokens | 16 B tokens | 100 B tokens |
|-----------|------------|-------------|--------------|
| `tokenize/bulk` | 75.0 µs | 45.8 µs | 35.8 µs |
| `tokenize/peek_bump` | 77.6 µs | 53.3 µs | 47.1 µs |

These are measured with the cursor hot methods `#[inline]` (as they are on `main`). Routing each token's
run through `chunk_tail`/`skip_in_chunk` still wins, but only **~1.03× at 4 B tokens, ~1.16× at 16 B, and
~1.31× at 100 B** — the advantage grows with token length (a longer run is one slice-scan skip instead of
many `bump`s), but for short tokens the two are effectively tied. That is a big change from the numbers
this harness first recorded, when `peek_bump` was 171–178 µs and bulk was 1.8–4.7× faster: back then most
of `peek_bump`'s cost was the cross-crate `peek`/`bump` call overhead, and inlining (the `#[inline]` win)
cut it ~2.3–3.6× here (178 → 77.6, 172 → 53.3, 171 → 47.1 µs). With that overhead gone, the residual bulk
advantage is just slice-scan-vs-per-byte-`Option`/branch, which only matters once tokens are long. The
takeaway for tokenizer authors: bulk `chunk_tail`/`skip_in_chunk` is still the right default for scanning
a run and pulls clearly ahead on longer tokens, but the per-byte `peek`/`bump` path is no longer a cliff —
it is fine for short-token or decision-heavy scanning now that it inlines.

## `refill` — the leaf-boundary cost in isolation (runnable: `cargo bench -p etude-span -- refill`)

Every leaf boundary the cursor crosses costs a `refill` (advance the chunk iterator, credit the leaving
leaf's length to the absolute offset, skip any empty leaves). To isolate it, skip whole leaves —
`skip_in_chunk(chunk_tail().len())` lands exactly at each leaf end and triggers one `refill`, reading no
bytes — over a fixed 64 KiB input at four leaf sizes:

| refill/skip_leaves | 4 KiB × 16 | 1 KiB × 64 | 256 B × 256 | 64 B × 1024 |
|--------------------|------------|------------|-------------|-------------|
| total | 84 ns | 271 ns | 1.19 µs | 4.64 µs |
| per refill | 5.2 ns | 4.2 ns | 4.7 ns | 4.5 ns |

The per-refill cost is **flat at ~4–5 ns regardless of leaf size** — it is fixed per-leaf work, not a
function of leaf content, so streaming a rope of `K` leaves costs ~4–5 ns × `K` before any byte is
touched. (These are measured with the cursor `#[inline]`'d, as on `main`; inlining `skip_in_chunk` shaved
the per-refill cost from the ~5–6 ns this harness first recorded.) This composes the `cursor/bulk` scan
numbers: at 64 B leaves refill is ~4.6 µs of bulk's ~16 µs (~29 % — a fragmented rope spends nearly a
third of a bulk scan just crossing boundaries), while at 4 KiB leaves it is 84 ns of ~5.5 µs (~1.5 %) and
the per-leaf byte work dominates. The practical read: the cursor is cheap to advance, but very small
leaves make refill a real fraction of a scan — another reason a rope's leaves want to be chunk-sized, not
byte-sized.

## Next targets

The core scan, tokenize, and refill hot paths are now benched. Further passes as new hot paths
appear — e.g. scanning over a deep, multi-level rope (`Repr::Deep`), where the `chunks()` iterator
itself walks tree internals, versus the flat layouts benched here.
