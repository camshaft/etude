<!-- Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# `etude-json-serde` — scoreboard vs. `serde_json`

`etude-json-serde` is the rope-native, copy-avoiding JSON adapter: a grammar-enforcing `Deserializer`
over the `etude-json` tokenizer, driving an `etude-serde` `Visitor`. `serde_json` is the differential
oracle *and* the performance target. Two scoreboards below — **allocation** (`tests/alloc_scoreboard.rs`,
a gate-runnable counting-allocator test) and **wall-clock** (`benches/scoreboard.rs`, criterion). The
workload is a **streaming digest** (count nodes, sum string lengths, retain nothing) — the
extract/scan-without-materializing case the no-`'de` SAX seam exists for. `serde_json` must build the
whole `Value` tree first; the adapter drives a `Visitor` that keeps only a fixed-size accumulator and
hands escape-free strings as O(1) `RopeStr::Borrowed` rope slices.

## Allocation scoreboard (runnable: `cargo test -p etude-json-serde --test alloc_scoreboard -- --nocapture`)

A streaming digest over a ~17 KB container/string-heavy document (200 objects):

| path | allocations | bytes |
|------|-------------|-------|
| `serde_json::from_slice::<Value>` + walk | 2207 | 227146 |
| **etude adapter + `Visitor`** | **1** | **24** |
| ratio | **~2200× fewer** | **~9400× fewer** |

The copy-avoidance thesis, quantified: serde must allocate the entire tree (every `Vec`, `Map`, and
`String`); the adapter allocates essentially nothing — escape-free strings are structural rope shares
(no per-string alloc on a single chunk), no container nodes are retained, and the digest is a `Copy`
accumulator. Both paths compute the same digest (asserted equal), so the comparison is honest.

## Wall-clock scoreboard (runnable: `cargo bench -p etude-json-serde`)

Latest run (aarch64, jemalloc; streaming digest; `raw_tokenize` = drive the tokenizer alone, count
tokens, touch no content — the attribution baseline). Absolute µs vary with machine load; read the
*within-run* relationships:

| workload | `serde_json` | etude adapter | `raw_tokenize` |
|----------|--------------|---------------|----------------|
| digest_mixed (200 objects) | ~150 µs | ~493 µs | ~60–90 µs |
| digest_strings (500 strings) | ~29 µs | ~106 µs | ~26 µs |
| digest_numbers (500 numbers) | ~21 µs | ~86 µs | ~20 µs |

**Reading it — the copy-avoidance win is allocation, not (yet) wall-clock.** The adapter trails
`serde_json` ~3× on time despite ~2200× fewer allocations. Crucially, `raw_tokenize` is *at or below*
serde on every workload (the tokenizer beats serde's full parse on the mixed doc), so the gap is **not**
the rope scan — it is the per-token **handoff**. Two attributed costs and their status:

- **Numbers** — the eager sub-rope materialization was ~⅓ of the number handoff. Making `NumberToken`
  lazy (one eager `lexeme` slice + on-demand component accessors, replacing four eager slices) cut
  `digest_numbers` **~130 µs → ~86 µs (−34%)**. Done. The residual over `raw_tokenize` is the one
  unavoidable `lexeme` slice (self-containment) + the `Stream` one-token lookahead + per-value dispatch.
- **Strings** — `StrRope::from_utf8` re-validates each escape-free string's UTF-8 that the tokenizer
  already guaranteed (an O(n)/string redundant scan). The fix is `StrRope::from_utf8_unchecked`
  (approved by the strrope owner, `etude-str-migration`; PR pending operator review of its first
  `unsafe` surface). Wiring the `Borrowed` arm to it is a one-line change once it lands.

The `raw_tokenize` baseline is what re-attributed this gap: an earlier reading blamed the tokenizer's
`byte_at` scan, and a fresh measurement falsified it — the scan is competitive, the handoff is the lever.
