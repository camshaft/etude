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
are an indicative short run (aarch64, jemalloc, release, 1.0 s measurement) — rerun for authoritative
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
| nested_100        | 2.26 µs             | 5.61 µs          | **0.40** | 2.5× faster |
| objects_1k        | 306 µs              | 734 µs           | **0.42** | 2.4× faster |
| array_5k_strings  | 128 µs              | 240 µs           | **0.53** | 1.9× faster |
| array_10k_floats  | 248 µs              | 365 µs           | **0.68** | 1.5× faster |
| array_10k_ints    | 239 µs              | 269 µs           | **0.89** | faster |
| big_string_100k   | 78 µs               | 23.9 µs          | 3.26  | slower — see below |

etude_json is faster on 5 of 6 shapes with zero allocations. The one remaining loss is a single
huge string.

## String bulk-scan optimization (applied) + residual gap

The string scan bulk-skips runs of ordinary bytes: within the current rope leaf it finds the next
significant byte (`"`, `\`, or a control byte) in one contiguous-slice pass and advances the cursor
in bulk, refilling only at leaf boundaries — instead of one `peek`/`bump` per byte. Measured effect
vs the per-byte baseline: **`big_string_100k` 231 µs → 78 µs (−66%)**, `array_5k_strings` −46%
(now 1.9× faster than serde_json).

**Residual:** `big_string_100k` is still ~3.3× slower than `serde_json`, which uses a SIMD scan for
the closing quote/escapes. The `position` predicate over the leaf may not fully vectorize (it tests
three byte classes, including a `< 0x20` range). Next lever, if this workload matters: a SIMD /
`memchr`-style leaf scan (e.g. `memchr2` for `"`/`\` plus a vectorized control-byte check). The
allocation column stays zero regardless.
