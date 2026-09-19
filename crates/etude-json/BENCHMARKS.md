<!-- Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# `etude-json` — performance scoreboard

`etude-json` is a copy-avoiding JSON reader over the `etude-bytevec` rope. This scoreboard measures
the current tokenizer against `serde_json` (the reference) on two axes, **allocations first**:

- **Allocations** — the whole point of the copy-avoiding design. A token carries a rope *span*, so
  draining the entire token stream touches zero heap. `serde_json` builds an owned `Value` tree.
- **Time** — draining + validating every token vs parsing to a `Value`. Not the same work (spans vs
  an owned tree), so read time with the allocation column carrying the real story.

Runnable: `cargo bench -p etude-json --bench tokenize`. Input is fed through a chunked rope (8 KiB
leaves), exercising the chunk-streaming cursor; `serde_json` gets the same bytes contiguous. Rope
construction is outside the measured region. Allocation counts are exact/deterministic; timings below
are an indicative short run (aarch64, jemalloc, release, 1.5 s measurement) — rerun for authoritative
numbers.

## Allocation scoreboard (per parse) — headline

| shape             | etude_json allocs | etude_json bytes | serde_json allocs | serde_json bytes |
|-------------------|------------------:|-----------------:|------------------:|-----------------:|
| array_10k_ints    | **0**             | **0**            | 13                | 1,048,448        |
| array_10k_floats  | **0**             | **0**            | 13                | 1,048,448        |
| array_5k_strings  | **0**             | **0**            | 5,012             | 573,050          |
| big_string_100k   | **0**             | **0**            | 1                 | 100,000          |
| nested_100        | **0**             | **0**            | 100               | 12,800           |
| objects_1k        | **0**             | **0**            | 10,009            | 856,298          |

Tokenizing allocates **nothing** on every shape — the copy-avoiding invariant holds. A downstream
parser built on this iterator only pays for the values it actually materializes (escaped strings,
decoded numbers), and skipped values cost zero heap.

## Time scoreboard (ratio = etude_json / serde_json; <1.0 = etude_json faster)

| shape             | etude_json tokenize | serde_json parse | ratio | note |
|-------------------|--------------------:|-----------------:|------:|------|
| nested_100        | 2.53 µs             | 5.49 µs          | **0.46** | 2× faster |
| objects_1k        | 384 µs              | 775 µs           | **0.50** | 2× faster |
| array_10k_floats  | 329 µs              | 344 µs           | 0.95  | ~equal |
| array_5k_strings  | 225 µs              | 224 µs           | 1.00  | equal |
| array_10k_ints    | 286 µs              | 251 µs           | 1.14  | slightly slower |
| big_string_100k   | 232 µs              | 23.6 µs          | **9.8**  | ⚠ see below |

## Known gap / next optimization target

**`big_string_100k` is ~10× slower.** The string scan advances the cursor one byte at a time looking
for `"` / `\` / control bytes; `serde_json` finds the closing quote and escapes with a bulk (SIMD /
`memchr`-style) search. For a large escape-free string this per-byte loop dominates.

Next perf slice: bulk-scan within the current rope leaf — search `chunk[pos..]` for the first
`"`/`\`/`<0x20` byte in one vectorized pass and advance the cursor in bulk, refilling only at leaf
boundaries. Expected to close the `big_string` gap and speed up all string-heavy shapes; the
allocation column stays zero. This scoreboard is the baseline that optimization must beat.
