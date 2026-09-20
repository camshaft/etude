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
| `cursor/bulk` | 16.5 µs | 6.0 µs | 5.6 µs |
| `cursor/peek_bump` | 168 µs | 156 µs | 156 µs |
| `byte_at/scan` | 2.33 ms | 757 µs | 149 µs |

Two things the numbers pin down. First, **`byte_at` scanning collapses as the rope fragments**: 149 µs
for a single leaf (a direct index), 757 µs at 16 leaves, and 2.33 ms at 1024 leaves — the descent
deepens and every byte pays it. On a fragmented rope the bulk cursor is ~140× faster and even the
byte-at-a-time cursor is ~14× faster; this is the cursor's reason to exist. Second, the cursor's own
two modes differ sharply: `cursor/bulk` (5.6–16.5 µs) is **~10–30× faster** than `cursor/peek_bump`
(156–168 µs), because the per-leaf inner loop is a tight contiguous slice scan while `peek`/`bump`
pays an `Option` and a boundary branch on every byte. `peek_bump` is nearly flat across layouts (the
per-byte cost dominates; refills are minor), whereas `bulk`'s cost tracks leaf count (refill
frequency).

## Next targets

The `peek_bump` path is the tokenizer's actual inner loop, and it is ~10–30× slower than a bulk
`chunk_tail` scan — a real optimization target worth investigating (can the per-byte `peek`/`bump`
overhead be tightened, or can more tokenizer scanning route through the bulk path?). That is a
library-code question for a follow-up. Subsequent bench passes: mixed run-length scans (short tokens
straddling leaf boundaries, the realistic tokenizer shape) and the `skip_in_chunk` boundary-refill
cost in isolation.
