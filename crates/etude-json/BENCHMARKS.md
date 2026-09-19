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

## String decode — `Token::decode_string` per-function bench (differential vs `serde_json`)

`decode_string` is the on-demand path that materializes a string token's content into an owned
`String` (applying JSON escapes). The reference is `serde_json` parsing the same bytes into a
`Vec<String>` — a fair differential, since both allocate exactly one owned `String` per element.
Tokens are collected outside the timed region, so this measures decode alone (aarch64, jemalloc,
release, 1.0 s):

| shape               | decode_string | serde_json Vec<String> | ratio | note |
|---------------------|--------------:|-----------------------:|------:|------|
| big_no_escape_100k  | 48.8 µs       | 23.9 µs                | 2.04  | one large copy |
| big_escaped_90k     | 143 µs        | 379 µs                 | **0.38** | 2.6× faster (dense escapes) |
| escaped_5k          | 705 µs        | 518 µs                 | 1.36  | a few escapes each |
| unicode_2k          | 168 µs        | 83 µs                  | 2.02  | all `\u`/surrogate |
| no_escape_5k        | 438 µs        | 202 µs                 | 2.17  | many short strings |

Allocations are one `String` per element on both sides (etude_json allocates *fewer bytes* — it sizes
each buffer to the content, serde over-reserves): e.g. no_escape_5k etude_json 80 KB vs serde 472 KB.

### Optimization applied — bulk-copy ordinary-byte runs

The decoder copied ordinary content one `push` per byte. It now finds the next escape and copies the
whole run in one `extend_from_slice` (the same lever the tokenizer's string scan uses), so a string
with no escapes is one scan plus one copy. Measured before → after:

- **big_no_escape_100k: 124 µs → 48.8 µs (−60%)** — closes most of the gap to serde_json (5.2× → 2.0×).
- no_escape_5k 476 → 438 µs, escaped_5k 728 → 705 µs (realistic escape density improves).
- big_escaped_90k 124 → 143 µs (+15%): a synthetic worst case (one third of the bytes are `\`-escapes,
  so runs are 2 bytes and the run scan costs more than it saves) — still 2.6× faster than serde_json.
  Regressing a pathological shape that remains far ahead, to win 60% on the common large-payload shape.

### Next levers (continuous improvement)

The small-many-strings shapes (no_escape_5k, unicode_2k) are ~2× serde_json despite comparable
allocation counts; the suspects are per-token `copy_to_bytes` setup and the two linear passes
(scan-for-escape then copy). Candidates: a single-pass copy-until-escape (fuse the scan into the
copy), a no-escape fast path keyed off `string_has_escapes` that skips the escape machinery entirely,
and (once chunk-ref tokens land, PR #124) decoding from the token's leaf slice to drop `copy_to_bytes`.
A win here is the new baseline, not a stopping point.

## Chunk-ref token read fast path (`Token::bytes`) — the O(1) read (`tokenize_and_read_bytes`)

A `Token` now also carries the leaf slice of its full lexeme **when the token lies within a single
rope leaf** (the common case on a rope of realistic leaves — here 8 KiB): `Token::bytes()` returns
that `&[u8]` in O(1), `None` only when the token straddles a leaf boundary (fall back to the span
slice for those). `tokenize_and_read_bytes` reads every token's bytes through this fast path; compare
it against `tokenize_and_read` (the O(log n)-per-token span descent above) — same work, different
read primitive (aarch64, jemalloc, release, 1.0 s):

| shape             | read via span (O(log n)) | read via `bytes()` (O(1)) | serde_json | bytes/span | bytes/serde |
|-------------------|-------------------------:|--------------------------:|-----------:|-----------:|------------:|
| array_10k_ints    | 1063 µs                  | 233 µs                    | 251 µs     | **0.22×**  | 0.93× |
| array_10k_floats  | 1159 µs                  | 271 µs                    | 338 µs     | **0.23×**  | 0.80× |
| array_5k_strings  | 603 µs                   | 152 µs                    | 259 µs     | **0.25×**  | 0.59× |
| nested_100        | 11.74 µs                 | 2.02 µs                   | 5.52 µs    | **0.17×**  | 0.37× |
| objects_1k        | 1729 µs                  | 297 µs                    | 643 µs     | **0.17×**  | 0.46× |
| big_string_100k   | 77.7 µs                  | 77.7 µs                   | 23.9 µs    | 1.00×      | 3.25× |

**Finding:** reading a token's bytes through the O(1) `bytes()` fast path is **4–6× faster than the
O(log n) span descent** on token-dense documents, and — critically — it **restores etude_json to
faster-than-serde_json (0.37–0.93×) even when a consumer reads every token's bytes**, exactly the
workload where span resolution had made it 2–4× slower. The 0-alloc invariant is unchanged (the leaf
slice is a borrow, not a copy). `big_string_100k` is unaffected: it is one 100 KB token that straddles
many 8 KiB leaves, so `bytes()` is `None` and the read falls back to the span slice (still the SIMD
closing-quote-scan gap noted above, not a read-path cost).

So the chunk-ref token turns the earlier read regression into a win on every multi-token shape; the
only residual loss is the single huge string, whose lexeme cannot fit one leaf. A future refinement
for straddling tokens (a cursor bookmark, or copying the few straddling lexemes into a small scratch)
would close that last case, but the measured payoff there is limited to giant single tokens.
