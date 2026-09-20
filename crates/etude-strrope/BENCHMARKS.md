<!-- Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# etude-strrope benchmarks

`cargo bench -p etude-strrope` runs `benches/ops.rs`, a per-function scoreboard of `StrRope`
against the contiguous reference it replaces: `std::string::String` / `str`. Numbers below are
aarch64, release, `tikv-jemallocator` as the global allocator (matching the host so
allocation-sensitive numbers are production-representative), 500 ms warm-up + 2 s measurement.

Each op runs at two shapes:

- **shallow** — 4 chunks; the streaming common case, rope stays in its flat tier.
- **deep** — 1000 chunks (~56 KiB); past the promote threshold, exercising the radix tree.

The point of the reference is honesty: a contiguous `String` wins the small and byte-dense cases;
`StrRope` earns its place on O(1) clone / owned-construction / `into_bytes` and structural sharing.
The table records both, and flags where the rope pays for its shape.

## Where the rope wins

| op | shape | StrRope | std | note |
|----|-------|---------|-----|------|
| `from(String)` (owned) | deep | 43 ns | 1.28 µs (`from(&str)`) | moves the buffer — one alloc reused, no copy, no scan; flat in length |
| `from(String)` (owned) | shallow | 40 ns | 12 ns (`from(&str)`) | move vs a small copy |
| `clone` | deep | 159 ns | 1.06 µs | O(1) refcount bump vs a full `String` copy |
| `into_bytes` | deep | 4.7 ns | 3.8 ns | drops the kind marker, no copy |

`clone` inverts as depth grows: at 4 chunks std wins (74 ns vs 13 ns — copying 4 short leaves is
cheap), at 1000 chunks the rope's O(1) share pulls far ahead (159 ns vs 1.06 µs).

## Where std wins (and why)

| op | shape | StrRope | std | why |
|----|-------|---------|-----|-----|
| `from(&str)` | deep | 1.28 µs | 1.23 µs | both copy the bytes once; parity |
| `from_utf8` | deep | 7.96 µs | 2.83 µs | streams a validation scan across chunks |
| `push_str` | deep | 7.84 µs | 2.57 µs | appends a leaf; `String` amortizes realloc |
| `split_off` | deep | 8.98 µs | 1.60 µs | structural split; a 56 KiB memcpy is cheaper at this size |
| `slice` | deep | 31.2 µs | 401 ns | same — the rope's structural share only pays off well past 56 KiB |
| `eq` / `cmp` | deep | 8.5 µs | 1.65 µs | two-cursor chunk walk vs one contiguous `memcmp` |
| `display` | deep | 16.6 µs | 1.24 µs | one `write_str` per chunk vs one for the whole string |
| `chars().count()` | deep | 202 µs | 4.39 µs | per-codepoint decode vs `str`'s specialized byte-scan count |
| `char_indices().last()` | deep | 387 µs | 4.7 ns | forward O(n) scan vs `str`'s O(1) reverse iterator |
| `debug` | deep | 315 µs | 47.6 µs | per-chunk `escape_debug` |

At the shallow shape std wins nearly everything — a small contiguous buffer is optimal, and the
rope's per-chunk bookkeeping is pure overhead there. That is expected: `StrRope` is for content
that is already chunked (streamed / sliced / shared), not for building small strings.

## Candidate optimizations (follow-up)

Measured hot paths worth a code-opt pass (own PRs, operator review):

- `chars()` / `char_indices()` — ~46x off std at depth; decode contiguous runs harder / avoid the
  per-codepoint `from_utf8` on the seam-free interior.
- `char_indices().last()` — a `DoubleEndedIterator` for `chars()` would make `.last()` / `.next_back()`
  O(1)-from-the-back instead of a full forward scan. Deferred until a consumer needs reverse iteration.
- `eq` / `cmp` at depth — the chunk-walk carries per-chunk setup; a coarser run comparison could close
  part of the gap to a contiguous `memcmp`.

Note: `slice` / `split_off` / `insert` at depth are dominated by the underlying `etude-bytevec` rope
ops, not the `StrRope` wrapper; any structural-op speedup belongs there (coordinate cross-crate).

## Investigated: forward decode (`chars_collect`)

The `chars_collect` group materializes every char (`chars().collect::<String>()`), so — unlike
`count` / `last` — neither side has a specialization, isolating raw per-codepoint decode:

| op | shape | StrRope | std |
|----|-------|---------|-----|
| `chars().collect::<String>()` | deep | 266 µs | 112 µs |
| `chars().collect::<String>()` | shallow | 1.29 µs | 632 ns |

At ~2.4x off std the decode is already close, and dominated by std's own `str::Chars` decoder plus
the output `String` pushes — not a hot path worth contorting.

Dead end (measured, not pursued): replacing the per-chunk `core::str::from_utf8` re-validation in the
`Chars` decoder with an invariant-justified `from_utf8_unchecked` moved `chars_collect/1000` 266 µs →
277 µs — no win (within noise). On valid UTF-8 the validation is a cheap SIMD scan; the decode
dominates. Not worth introducing `unsafe` in the iterator for it, so the checked path stays.
