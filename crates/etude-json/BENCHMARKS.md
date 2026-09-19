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

## Span-resolution cost — the O(log n)-per-token re-access (`tokenize_and_read`)

Tokenizing is cheap and 0-alloc, but a `Span` is `(offset, len)`: reading a token's bytes later means
`ByteVec::slice(span)`, an O(log n) tree descent to locate the span's leaf. `tokenize_and_read`
resolves every token's span; the delta over `tokenize` is that re-access cost (aarch64, jemalloc,
release, 1.0 s):

| shape             | tokenize | tokenize_and_read | serde_json | read/tokenize | read/serde |
|-------------------|---------:|------------------:|-----------:|--------------:|-----------:|
| array_10k_ints    | 239 µs   | 1098 µs           | 270 µs     | 4.6×          | 4.1× |
| array_10k_floats  | 247 µs   | 1144 µs           | 335 µs     | 4.6×          | 3.4× |
| array_5k_strings  | 128 µs   | 574 µs            | 228 µs     | 4.5×          | 2.5× |
| objects_1k        | 306 µs   | 1562 µs           | 649 µs     | 5.1×          | 2.4× |
| nested_100        | 2.26 µs  | 11.55 µs          | 5.55 µs    | 5.1×          | 2.1× |
| big_string_100k   | 77.3 µs  | 77.5 µs           | 23.9 µs    | 1.0×          | 3.2× |

**Finding:** on token-dense documents, resolving each span costs ~4.5–5× the tokenize time and flips
etude_json from *faster* than serde_json to **2–4× slower**. On `big_string_100k` (one token) it is
negligible. So the 0-alloc tokenize win is real only while a consumer *skips* tokens; a consumer that
*reads* them pays O(log n) per span.

This quantifies the motivation for **chunk-ref-carrying tokens** (a token that also holds the leaf
`&[u8]` + local position, or a cursor bookmark) so reading a token's bytes is O(1) rather than an
O(log n) re-descent — see the open design question on the `etude-span` extraction (PR #101).
