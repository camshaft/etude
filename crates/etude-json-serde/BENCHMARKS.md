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
| digest_containers (500 empty arrays, post-#247) | ~20 µs | **~54 µs** (2.64×) | ~18 µs |
| digest_escaped (500 escaped strings) | ~68 µs | ~169 µs (2.48×) | **~77 µs** |

**Reading it — the copy-avoidance win is allocation, not (yet) wall-clock.** The adapter trails
`serde_json` ~3× on time despite ~2200× fewer allocations. Crucially, `raw_tokenize` is *at or below*
serde on every workload (the tokenizer beats serde's full parse on the mixed doc), so the gap is **not**
the rope scan — it is the per-token **handoff**. Two attributed costs and their status:

- **Numbers** — the eager sub-rope materialization was ~⅓ of the number handoff. Making `NumberToken`
  lazy (one eager `lexeme` slice + on-demand component accessors, replacing four eager slices) cut
  `digest_numbers` **~130 µs → ~86 µs (−34%)**. Done. The residual over `raw_tokenize` is the one
  unavoidable `lexeme` slice (self-containment) + the `Stream` one-token lookahead + per-value dispatch.
- **Strings** — `StrRope::from_utf8` re-validated each escape-free string's UTF-8 that the tokenizer
  already guaranteed (an O(n)/string redundant scan). Fixed: with the tokenizer's `Strictness::Strict`
  mode validating content UTF-8 at lex, the `Borrowed` arm now uses `StrRope::from_utf8_unchecked`
  (sound by construction) — one validation pass instead of two, `digest_strings` **−6.1%**. `Lenient`
  keeps the checked path (no UB). Done.
- **Dispatch / handoff (the Token memcpy)** — the per-token handoff cost was dominated by `Token`
  itself: it is moved through the parser's one-token lookahead several times per value, and it was
  **120 bytes** (it stored an `Option<StringInfo>` *and* an `Option<NumberParts>`, sizes adding).
  Folding the mutually-exclusive payload into one enum shrank it to **96 bytes (−20%)**, which cut
  `digest_containers` **76 µs → 54 µs (−29.7%, clean A/B isolating just the shrink)** and
  `raw_tokenize` −17.4% — so the adapter/`serde_json` ratio on containers fell **3.76× → 2.64×**. The
  time win far exceeds the 20% size cut because the token is copied several times per element. Done
  (`etude-json` #247). Further compaction (relative-`u32` number offsets, ~another halving) is
  **declined**: it truncates on a pathological >4 GB number lexeme — a correctness risk vs the oracle.
- **Escaped strings — the exception where laziness pays a double-scan** (`digest_escaped`, added to
  justify-or-kill an unchecked escaped-decode variant, "#147"): serde ~68 µs, adapter ~169 µs (2.48×),
  and `raw_tokenize` ~77 µs — *the tokenizer alone already exceeds serde's whole parse*, unlike every
  escape-free workload where it beats serde. Two costs, neither fixable by skipping the final
  `from_utf8` (that saves only single-digit µs of the ~100 µs gap): (1) escape lexing is byte-at-a-time
  (each `\` breaks the bulk-skip) and, in Strict, validates content runs; (2) **lazy decode re-scans
  the content a second time** — the tokenizer scans escapes to find the string end, then
  `decode_string` re-walks to unescape, where serde unescapes in its single parse pass. The
  double-scan is the *cost of laziness* and the right trade: escape-free strings (the common case) get
  the zero-copy borrow and win; escaped strings (the minority) pay twice. So **"#147" is declined** —
  a negligible measured win that would add an `unsafe` public API to the lexer.
- **Dispatch — by reference, not by move** — even at 96 bytes the `Token` was moved *out* of the
  lookahead on every value (`Stream::next() -> Token`), yet the deserializer only ever *reads* it
  through `&self` accessors. Replacing `next()` with `peek() -> Option<&Token>` + `bump()` dispatches
  by reference — each arm reads only the small `Copy` metadata it uses (a container reads just the
  1-byte kind) — cutting `digest_containers` **54 µs → 44.5 µs (−17.0%, ratio 2.64× → 2.20×)** and
  `digest_mixed` **−6.3%**, differential-fuzz-validated as behavior-preserving. Spike-side (the adapter
  is frozen behind #184); staged, not landed to main. Combined with #247 the container path is
  **76 µs → 44.5 µs (~42% off)**.

The `raw_tokenize` baseline is what re-attributed this gap: an earlier reading blamed the tokenizer's
`byte_at` scan, and a fresh measurement falsified it — for escape-free input the scan is competitive
(beats serde) and the handoff was the lever. **Ranked lever status on the ~2.2× container path**
(adapter 44.5 µs vs serde 20 µs vs `raw_tokenize` 18 µs, so ~26 µs of SAX machinery sits over the bare
scan): (1) Token memcpy — *addressed*, shrink #247 + by-ref dispatch together removed the whole
move/copy cost; (2) remaining ~26 µs is lookahead fill/take churn + per-element recursive re-entry into
`deserialize_any` + generic-`Visitor` calls — increasingly **inherent** SAX-vs-specialized-builder
overhead (serde's `Value` builder is one tight recursive loop with no per-node trait dispatch). Further
wins here look small and structural (e.g. flattening the `Option<Result<Option<Token>, Error>>`
lookahead slot); none are a clean high-value lever, so the profiling stops here. All post-Token-memcpy
work is spike-side; the adapter stays spike-only per the operator's Decision 0 pending #184 review, so
these levers graduate *with this data* when #184 clears — they are not landed to main now.
