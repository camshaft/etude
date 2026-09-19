<!-- Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# etude-rational benchmarks

Head-to-head against [`num-rational`](https://crates.io/crates/num-rational)'s `BigRational`, the
reference we optimize toward — the north star is to **beat it** (ratios below 1.00), not merely reach
parity. `benches/arith.rs` measures each operation at three component-magnitude tiers (named by the
per-component bit width) with the jemalloc allocator, deterministic operands built through the public
API, and criterion. Run it with:

```
cargo bench -p etude-rational
```

Numbers below are medians from one `aarch64-linux` run and are **indicative, not authoritative** —
absolute times vary by machine; what matters is the **ratio to num-rational** and its movement as
optimizations land. `ratio` is `etude / num-rational`: `<1.00` = we are faster (**bold**), `>1.00` = slower.

## Current — equal-denominator add/sub fast path (slice 4)

`add`/`sub` now fast-path a common real-world pattern: when the two denominators are equal, the result is
`(a ± c)/b` — skipping BOTH cross-multiplies and the `b*d` product (three multiplies → one add). The
`den == den` test is an O(limbs) compare that short-circuits when they differ, so the random-operand rows
below are **unchanged** (no regression). Measured (`add_eqden`, random coprime numerators over a shared
denominator — a fair fast-path-vs-fast-path comparison):

| op (eq-den) | tier   | etude    | num-rational | ratio | vs our general add |
|-------------|--------|----------|--------------|-------|--------------------|
| add_eqden   | 64b    | 1.34 µs  | 1.34 µs      | **1.00** | 3.30 µs → 1.34 µs (2.5×) |
| add_eqden   | 256b   | 8.71 µs  | 4.97 µs      | 1.75  | 22.8 µs → 8.71 µs (2.6×) |
| add_eqden   | 1024b  | 60.6 µs  | 25.3 µs      | 2.40  | 158 µs → 60.6 µs (2.6×) |

This is ~2.5–2.6× faster than our own general (cross-multiply) path and reaches parity at 64b. The
residual gap to num-rational at ≥256b is now **entirely gcd-bound**: the fast path leaves only the
`gcd`-normalize, and `etude-bigint`'s Euclidean gcd is ~1.7× slower than `num-integer`'s on random
operands (per etude-bigint's own scoreboard). **`etude-bigint` gcd is the single shared bottleneck for
every normalize-heavy op here (add/sub/mul/div/normalize) — the highest-leverage next win is a faster gcd
in etude-bigint** (coordination item raised to that vertical).

> Investigated but NOT landed — continued-fraction `cmp`: replacing the double cross-multiply with the
> Euclidean/continued-fraction comparison cut `cmp @1024b` from 789 ns to ~196 ns (4.4× → 1.11×) BUT
> regressed the 64b/256b tiers (its per-step overhead loses to two tiny multiplies), and a size-thresholded
> hybrid regressed too because the only size probe available (`to_sign_magnitude_bytes_into`) still copies
> all limbs when the value fits (not O(1)). A clean cmp hybrid needs an O(1) size/bit-length accessor from
> etude-bigint (or an allocation-free continued-fraction). Reverted; coordination item raised.

## Current — gcd-free `recip` (slice 3)

Reciprocal is now an O(limbs) swap+sign instead of a full gcd-normalize (a canonical rational is already
coprime), collapsing the recip row from 60–1651× slower to **parity-or-better** (flat ~30 ns across all
tiers) — it now beats num-rational at 256b/1024b. All other rows are unchanged from the baseline below.

| op         | tier   | etude     | num-rational | ratio     | vs baseline |
|------------|--------|-----------|--------------|-----------|-------------|
| recip      | 64b    | 31.0 ns   | 15.1 ns      | 2.05      | 909 ns → 31 ns (29× faster) |
| recip      | 256b   | 29.7 ns   | 32.4 ns      | **0.92**  | 9.12 µs → 30 ns (307× faster) |
| recip      | 1024b  | 32.6 ns   | 34.4 ns      | **0.95**  | 57.3 µs → 33 ns (1759× faster) |

## Baseline — faithful port (single-gcd normalize, cross-multiply arithmetic)

| op         | tier   | etude     | num-rational | ratio     |
|------------|--------|-----------|--------------|-----------|
| add        | 64b    | 3.28 µs   | 3.61 µs      | **0.91**  |
| add        | 256b   | 22.8 µs   | 16.3 µs      | 1.40      |
| add        | 1024b  | 159 µs    | 93.1 µs      | 1.71      |
| sub        | 64b    | 3.31 µs   | 3.79 µs      | **0.87**  |
| sub        | 256b   | 23.3 µs   | 16.2 µs      | 1.44      |
| sub        | 1024b  | 162 µs    | 92.6 µs      | 1.75      |
| mul        | 64b    | 3.81 µs   | 4.98 µs      | **0.76**  |
| mul        | 256b   | 23.3 µs   | 20.4 µs      | 1.14      |
| mul        | 1024b  | 167 µs    | 116 µs       | 1.44      |
| div        | 64b    | 3.45 µs   | 5.16 µs      | **0.67**  |
| div        | 256b   | 23.7 µs   | 21.4 µs      | 1.11      |
| div        | 1024b  | 160 µs    | 113 µs       | 1.42      |
| cmp        | 64b    | 39.6 ns   | 54.1 ns      | **0.73**  |
| cmp        | 256b   | 84.8 ns   | 141 ns       | **0.60**  |
| cmp        | 1024b  | 789 ns    | 179 ns       | 4.42      |
| recip      | 64b    | 909 ns    | 15.1 ns      | 60.3      |
| recip      | 256b   | 9.12 µs   | 32.3 ns      | 282       |
| recip      | 1024b  | 57.3 µs   | 34.7 ns      | 1651      |
| normalize  | 64b    | 1.01 µs   | 1.30 µs      | **0.77**  |
| normalize  | 256b   | 8.71 µs   | 5.12 µs      | 1.70      |
| normalize  | 1024b  | 59.1 µs   | 24.9 µs      | 2.37      |

## Read of the baseline — where the gaps are (the optimization backlog)

The port already **wins the small (64b) tier across the board** and beats num-rational on cmp at 64b/256b
(its cmp allocates; ours cross-multiplies in place). The measured gaps, in priority order:

1. ~~**`recip` — 60–1651× slower (the glaring one).**~~ **DONE (slice 3):** gcd-free `recip` — a canonical
   rational is already coprime, so `1/(a/b) = b/a` is already in lowest terms (only the sign moves onto the
   new numerator). Now an O(limbs) swap, flat ~30 ns, and beats num-rational at 256b/1024b. Residual: at
   64b we are 2.05× (clone-bound — two small `Vec` clones); a future micro-opt could avoid a clone. The
   same "inputs known coprime ⇒ no gcd" insight guards any future such op.
2. **`cmp` at 1024b — 4.42× slower.** We always do two full-width `num*den` cross-multiplies. num-rational
   compares integer/floor parts first and only falls back to full cross-multiplication when they tie, so
   it is far cheaper when the values differ in magnitude. Fix: a cheaper comparison that avoids the full
   double-width multiply in the common case.
3. **`add`/`sub`/`normalize` at ≥256b — 1.4–2.4× slower.** We form `(a*d ± c*b)/(b*d)` then reduce with
   ONE gcd over the (now large) product. num-rational reduces the denominators FIRST via `g = gcd(b,d)`,
   keeping every intermediate smaller. Fix: the gcd-of-denominators add/sub (smaller multiplies + a
   smaller final gcd). `mul`/`div` similarly benefit from cross-reducing before multiplying.
4. **Underlying bignum.** All of the above are ultimately `etude-bigint`-bound (mul + gcd + divmod). Some
   headroom is a coordination item with the etude-bigint vertical (its gcd/divmod ratios feed directly
   into ours).

Each optimization slice records its before/after delta against this baseline row, re-runs the
num-rational differential oracle, and keeps the canonical-form invariant intact.
