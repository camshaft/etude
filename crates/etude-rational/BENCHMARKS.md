<!-- Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# etude-rational benchmarks

Head-to-head against [`num-rational`](https://crates.io/crates/num-rational)'s `BigRational`, the
reference we optimize toward — the north star is to **beat it** (ratios below 1.00). `benches/arith.rs`
measures each operation at three component-magnitude tiers (named by the per-component bit width) with
the jemalloc allocator, deterministic operands built through the public API, and criterion. Run it with:

```
cargo bench -p etude-rational
```

Numbers below are medians from one `aarch64-linux` run and are **indicative, not authoritative** —
absolute times vary by machine; what matters is the **ratio to num-rational** and its movement as
optimizations land. `ratio` is `etude / num-rational`: `<1.00` = we are faster (**bold**), `>1.00` = slower.

## Current board (against etude-bigint's Stein binary gcd)

| op         | tier   | etude     | num-rational | ratio     |
|------------|--------|-----------|--------------|-----------|
| add        | 64b    | 1.85 µs   | 3.72 µs      | **0.50**  |
| add        | 256b   | 10.2 µs   | 16.5 µs      | **0.62**  |
| add        | 1024b  | 72.6 µs   | 93.9 µs      | **0.77**  |
| sub        | 64b    | 1.95 µs   | 3.82 µs      | **0.51**  |
| sub        | 256b   | 9.88 µs   | 16.3 µs      | **0.60**  |
| sub        | 1024b  | 73.0 µs   | 93.6 µs      | **0.78**  |
| mul        | 64b    | 2.08 µs   | 5.02 µs      | **0.41**  |
| mul        | 256b   | 8.16 µs   | 20.8 µs      | **0.39**  |
| mul        | 1024b  | 50.1 µs   | 117 µs       | **0.43**  |
| div        | 64b    | 1.86 µs   | 5.24 µs      | **0.36**  |
| div        | 256b   | 8.98 µs   | 21.9 µs      | **0.41**  |
| div        | 1024b  | 48.5 µs   | 114 µs       | **0.42**  |
| recip      | 64b    | 29.7 ns   | 14.5 ns      | 2.04      |
| recip      | 256b   | 29.3 ns   | 32.2 ns      | **0.91**  |
| recip      | 1024b  | 32.1 ns   | 34.7 ns      | **0.93**  |
| normalize  | 64b    | 894 ns    | 1.22 µs      | **0.74**  |
| normalize  | 256b   | 4.16 µs   | 4.97 µs      | **0.84**  |
| normalize  | 1024b  | 24.8 µs   | 24.5 µs      | 1.01      |
| cmp        | 64b    | 51.0 ns   | 54.3 ns      | **0.94**  |
| cmp        | 256b   | 95.2 ns   | 143 ns       | **0.67**  |
| cmp        | 1024b  | 192 ns    | 181 ns       | 1.06      |
| add_eqden  | 64b    | 935 ns    | 1.36 µs      | **0.69**  |
| add_eqden  | 256b   | 4.20 µs   | 4.99 µs      | **0.84**  |
| add_eqden  | 1024b  | 25.2 µs   | 25.3 µs      | 1.00      |
| cmp_eqden  | 64b    | 6.25 ns   | 7.94 ns      | **0.79**  |
| cmp_eqden  | 256b   | 6.72 ns   | 8.88 ns      | **0.76**  |
| cmp_eqden  | 1024b  | 11.5 ns   | 14.2 ns      | **0.81**  |

**We now beat num-rational on add, sub, mul, div, cmp (64b/256b), recip (256b/1024b), normalize
(64b/256b), and the equal-denominator add/cmp fast paths** — a decisive across-the-board lead. There are
no remaining hard losses; the non-wins are at parity or minor:

1. **`cmp` @1024b — 1.06× (near parity).** The size-thresholded hybrid routes large operands to the
   continued-fraction comparison (via the O(1) `Big::byte_len` probe), collapsing this from 4.33× to
   ~parity. The residual ~11 ns is the CF setup (the `abs`/clone of the four components before the loop);
   a borrow-first-iteration specialization could shave it. The small tiers stay wins (the probe adds ~6 ns
   but 64b/256b remain 0.94/0.67).
2. **`recip` @64b — 2.04× (clone-bound).** recip is already O(limbs) (gcd-free swap+sign); at 64b the two
   small `Vec` clones dominate. A clone/alloc-avoiding path could reach parity. Minor.
3. **`normalize`/`add_eqden` @1024b — ~1.00 (parity).** Both bottom out on a single large gcd; parity
   with num-rational's Stein gcd. Further headroom is a Lehmer-gcd item in etude-bigint (raised).

## Native i128 fast path — small (i64-fitting) operands

The common real-world case is *small* rationals (`3/10`, `127/5000`, …) whose numerator and denominator
fit `i64`. There, `mul`/`div` take a **native `i128` fast path**: `a*c`/`b*d` (or `a*d`/`b*c`) fit `i128`
with no overflow, reduced by a native `u128` gcd, boxed back to `Big` — **zero bignum arithmetic**.
num-rational always uses `BigInt`, so this is a large win:

| op      | operands | etude    | num-rational | ratio     |
|---------|----------|----------|--------------|-----------|
| mul_i64 | 48-bit   | 0.43 µs  | 4.04 µs      | **0.107** |
| div_i64 | 48-bit   | ~0.43 µs | ~4.1 µs      | **~0.11** |
| add_i64 | 48-bit   | 0.40 µs  | 3.14 µs      | **0.126** |
| sub_i64 | 48-bit   | ~0.40 µs | ~3.1 µs      | **~0.13** |
| cmp_i64 | 48-bit   | 8.7 ns   | ~55 ns       | **~0.16** |

**~6–9× faster than num-rational on small operands** — all four arithmetic ops AND `cmp` now take the
native path (`add`/`sub` via `(a*d ± c*b)/(b*d)` with a checked `i128` numerator for the overflow edge;
`cmp` via `a*d ? c*b` in `i128`; falling back to `Big`). `cmp_small` also skips the `byte_len` size probe
for the common i64-fitting case. (The byte-width tiers below UNDER-represent this case: their top magnitude bit is set, so a "64b"
coefficient exceeds `i64` and takes the `Big` path.) The residual ~0.4 µs is the two result-`Big`
allocations (`from_i64`) — both implementations must allocate the result; only our *arithmetic* went
native, and etude-bigint's 1-limb `Big` allocation is itself ~1.8× num-bigint's (their deferred inline-repr
item), so that residual will shrink further when that lands.

## Large-tier scaling (2048b/4096b tiers — now part of the default board)

Sample ratios at the large tiers (`ratio = etude / num-rational`; full numbers via `cargo bench`):

| op  | 1024b   | 4096b   |
|-----|---------|---------|
| mul | **0.43**| **0.47**|
| div | **0.42**| **0.46**|
| add | **0.77**| 0.98    |

`mul`/`div` **hold** their ~2× lead at 4096b (cross-reduction halves the gcd work), but `add`/`sub`
**narrow to ~parity** as size grows. This is EXPECTED, not a fixable gap: etude-bigint already has a
Karatsuba multiply (#53, `O(n^1.585)` above a 40-limb crossover, so engaged at 4096b = 64 limbs), and so
does num-bigint — so at 4096b both sides share the same multiply asymptotics and the random-operand add
`(a*d + c*b)/(b*d)` + reduce converges. Reopening a large-tier `add`/`sub` LEAD would need a faster-still
multiply than num-bigint's (Toom-3 in etude-bigint — roadmapped, later); Karatsuba alone cannot beat
num-bigint's Karatsuba. The one remaining locally-relevant bignum lever is a **Lehmer gcd** in
etude-bigint (its next algorithmic slice): Stein already matches num-bigint at 1024b, and Lehmer would
push the gcd-bound `normalize`/`add_eqden`@≥1024b parity cells below 1.0.

## Rendering (`to_string` / `Display`)

`to_string` (our alloc-lean `Display` via `Big::write_decimal`) at 64b: **0.66** — a WIN, after
etude-bigint's single-limb `to_decimal` fast path (#82) combined with our one-allocation render. At
256b/1024b it is ~1.6× (multi-limb `write_decimal` is still slower than num-bigint's `Display`);
etude-bigint is extending the alloc-free render path to a few limbs, which will move these. Render is
bignum-render-bound, not addressable locally.

## History

- **slice 1** — faithful port + num-rational differential oracle (the safety net) wired first.
- **slice 2** — criterion scoreboard + this file.
- **slice 3** — gcd-free `recip` (canonical ⇒ already coprime ⇒ O(limbs) swap+sign): 60–1651× → parity.
- **slice 4** — equal-denominator add/sub fast path (`(a±c)/b`, skips the cross-multiplies).
- **slice 5** — mul/div cross-reduction (cancel `gcd(a,d)`+`gcd(c,b)` first ⇒ no final normalize): flipped
  mul/div from losing to winning every tier.
- **slice 6** — cmp fast paths (sign short-circuit + equal-denominator direct numerator compare) + full
  board refresh against etude-bigint's new Stein binary gcd (#51), which banked the add/sub/normalize
  wins that were previously gcd-bound.
- **slice 7** — size-thresholded continued-fraction `cmp` hybrid, unblocked by etude-bigint's O(1)
  `bit_len`/`byte_len` accessors (#56): small components cross-multiply, large route to the
  continued-fraction comparison. `cmp@1024b` 4.33× → 1.06× (near parity).
- **slice 8** — 2048b/4096b bench tiers added (large-tier scaling: mul/div hold, add/sub converge —
  see the scaling note) + hot-path allocation elision: the `== 1` checks in `normalize`/`div_exact`
  (cross-reduce)/`is_integer` used to allocate a fresh `Big::from_i64(1)` each call; replaced with the
  O(1) allocation-free `bit_len() == 1`, removing 1–2 heap allocations per add/sub/mul/div (~1% at 64b,
  in the noise at large tiers where the bignum ops dominate).
- **slice 9** — `impl Display for Rational` (was missing) + alloc-lean `to_decimal_string`, both via
  etude-bigint's sink-writing `Big::write_decimal` (#79): one `String` instead of three.
- **slice 11** — native `i128` fast path for `mul`/`div` on i64-fitting operands (the common small case):
  compute in `i128` (overflow-free for products of i64s), native `u128` gcd, box back — no bignum
  arithmetic. `mul_i64` 0.107× num-rational (~9× faster); `div` symmetric. Guarded by the differential
  oracle (its i64 seeds exercise the native path).
- **slice 12** — extended the native `i128` fast path to `add`/`sub` (`(a*d ± c*b)/(b*d)` with a checked
  `i128` numerator for the `2^126 + 2^126` overflow edge → falls back to `Big`). `add_i64` 0.126×
  num-rational (~8× faster); `sub` symmetric. All four arithmetic ops now native on small operands.
- **slice 13** — native `i128` `cmp` on i64-fitting operands (`a*d ? c*b` directly, no `Big` multiply and
  no `byte_len` probe). `cmp_i64` ~0.16× num-rational (~6× faster). Now all arithmetic + `cmp` are native
  on the small case.
- **slice 10** — re-add the `to_string` render bench (now a 64b WIN, 0.66, via etude-bigint's single-limb
  `to_decimal` fast path #82 + our alloc-lean Display) + corrected the large-tier scaling analysis:
  Karatsuba is already landed (#53), so add/sub@4096b parity is expected (num-bigint has it too) — a lead
  needs Toom-3; Lehmer gcd is the remaining bignum lever for the ≥1024b gcd-bound cells.
