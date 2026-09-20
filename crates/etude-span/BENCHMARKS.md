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
| `tokenize/bulk` | 98.3 µs | 50.9 µs | 36.2 µs |
| `tokenize/peek_bump` | 178 µs | 172 µs | 171 µs |

Routing each token's run through `chunk_tail`/`skip_in_chunk` beats the naive per-byte tokenizer by
**~1.8× at 4 B tokens, ~3.4× at 16 B, and ~4.7× at 100 B** — the win grows with token length because a
longer run is one slice-scan skip instead of many `bump`s. The naive path is nearly flat (~171–178 µs)
regardless of token length: it peeks every byte, so its cost is the per-byte `peek`/`bump` overhead the
scan section describes, not the tokenization structure. The bulk path's floor at short tokens (98 µs)
is the delimiter density — one `chunk_tail`/`position`/`skip_in_chunk` cycle plus a delimiter `bump`
per token, ~13k tokens over the input. The takeaway for tokenizer authors: scan token runs with the
bulk path; reserve `peek`/`bump` for single-byte decisions (delimiters, one-byte lookahead).

## `refill` — the leaf-boundary cost in isolation (runnable: `cargo bench -p etude-span -- refill`)

Every leaf boundary the cursor crosses costs a `refill` (advance the chunk iterator, credit the leaving
leaf's length to the absolute offset, skip any empty leaves). To isolate it, skip whole leaves —
`skip_in_chunk(chunk_tail().len())` lands exactly at each leaf end and triggers one `refill`, reading no
bytes — over a fixed 64 KiB input at four leaf sizes:

| refill/skip_leaves | 4 KiB × 16 | 1 KiB × 64 | 256 B × 256 | 64 B × 1024 |
|--------------------|------------|------------|-------------|-------------|
| total | 98 ns | 332 ns | 1.46 µs | 5.76 µs |
| per refill | 6.1 ns | 5.2 ns | 5.7 ns | 5.6 ns |

The per-refill cost is **flat at ~5–6 ns regardless of leaf size** — it is fixed per-leaf work, not a
function of leaf content, so streaming a rope of `K` leaves costs ~5–6 ns × `K` before any byte is
touched. This composes the `cursor/bulk` scan numbers: at 64 B leaves refill is ~5.8 µs of bulk's
~16 µs (~36 % — a fragmented rope spends over a third of a bulk scan just crossing boundaries), while at
4 KiB leaves it is 98 ns of ~5.8 µs (~2 %) and the per-leaf byte work dominates. The practical read: the
cursor is cheap to advance, but very small leaves make refill a real fraction of a scan — another reason
a rope's leaves want to be chunk-sized, not byte-sized.

## Next targets

The core scan, tokenize, and refill hot paths are now benched. Further passes as new hot paths
appear — e.g. scanning over a deep, multi-level rope (`Repr::Deep`), where the `chunks()` iterator
itself walks tree internals, versus the flat layouts benched here.
