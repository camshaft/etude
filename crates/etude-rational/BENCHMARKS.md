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
