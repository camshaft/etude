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
| neg        | 64b    | 25.8 ns   | 17.1 ns      | 1.51      |
| neg        | 256b   | 24.0 ns   | 26.7 ns      | **0.90**  |
| neg        | 1024b  | 25.4 ns   | 30.6 ns      | **0.83**  |
| abs        | 64b    | 25.6 ns   | 16.1 ns      | 1.59      |
| abs        | 256b   | 23.8 ns   | 34.8 ns      | **0.69**  |
| abs        | 1024b  | 25.5 ns   | 37.3 ns      | **0.68**  |
| normalize  | 64b    | 894 ns    | 1.22 µs      | **0.74**  |
| normalize  | 256b   | 4.16 µs   | 4.97 µs      | **0.84**  |
| normalize  | 1024b  | 24.8 µs   | 24.5 µs      | 1.01      |
| cmp        | 64b    | 50.9 ns   | 54.3 ns      | **0.94**  |
| cmp        | 256b   | 94.5 ns   | 145 ns       | **0.65**  |
| cmp        | 1024b  | 78.2 ns   | 179 ns       | **0.44**  |
| cmp        | 2048b  | 812 ns    | 1.27 µs      | **0.64**  |
| cmp        | 4096b  | 146 ns    | 561 ns       | **0.26**  |
| add_eqden  | 64b    | 935 ns    | 1.36 µs      | **0.69**  |
| add_eqden  | 256b   | 4.20 µs   | 4.99 µs      | **0.84**  |
| add_eqden  | 1024b  | 25.2 µs   | 25.3 µs      | 1.00      |
| cmp_eqden  | 64b    | 6.25 ns   | 7.94 ns      | **0.79**  |
| cmp_eqden  | 256b   | 6.72 ns   | 8.88 ns      | **0.76**  |
| cmp_eqden  | 1024b  | 11.5 ns   | 14.2 ns      | **0.81**  |

**We now beat num-rational on add, sub, mul, div, cmp (ALL tiers), recip (256b/1024b), normalize
(64b/256b), and the equal-denominator add/cmp fast paths** — a decisive across-the-board lead. `cmp` is
now a decisive win at every tier (the large tiers by 1.5–3.8×, via the continued-fraction `q ∈ {0,1}`
fast path). The only non-wins left are at parity or minor:

1. **`recip`/`neg`/`abs` @64b — 1.5–2.0× (clone-bound).** All three are O(limbs) sign/swap ops that only
   clone the components; at 64b the small `Big` clone dominates, and etude-bigint's 1-limb `Big` clone is
   ~2× num-bigint's (its deferred inline-repr item). All three WIN at 256b and above (num-rational's clone
   grows with size while ours stays ~flat). A single etude-bigint small-`Big` inline representation would
   close `recip`/`neg`/`abs`@64b together — not locally addressable.
2. **`normalize`/`add_eqden` @1024b — ~1.00 (parity), ~1.10× @4096b.** Both bottom out on a single large
   gcd; parity with num-rational's Stein gcd at 1024b, a slight loss at 4096b. The double-word-Lehmer/HGCD
   headroom in etude-bigint is deferred (a narrow, acceptable gap — see the coordination note in the log).

The large-tier `cmp` lead comes from the continued-fraction comparison, sharpened two ways: a
**borrow-first-iteration** (components passed by reference; the first Euclidean step allocates nothing and,
when integer parts differ — the common case — decides with zero operand clones), and a **`q ∈ {0,1}` fast
path** (`divmod_small_q`) that replaces the per-step `divmod` with a `Big` comparison / one subtraction,
since similar-magnitude operands have quotient 0 or 1. The latter took 1024b 0.90× → **0.44×** and 4096b
0.73× → **0.26×**. Only a same-integer-part tie with fractional remainders on both sides clones the two denominators
and recurses. `abs` is now taken only when both operands are negative (denominators are already positive).

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
| add_eqden_i64 | 48-bit | 0.12 µs | 0.47 µs   | **0.26**  |
| from_ratio_i64 | 48-bit | 0.12 µs | 1.14 µs  | **0.103** |

**~6–9× faster than num-rational on small operands** — all four arithmetic ops AND `cmp` now take the
native path (`add`/`sub` via `(a*d ± c*b)/(b*d)` with a checked `i128` numerator for the overflow edge;
`cmp` via `a*d ? c*b` in `i128`; falling back to `Big`). `cmp_small` also skips the `byte_len` size probe
for the common i64-fitting case. The native `add`/`sub` path is tried BEFORE the Big equal-denominator
path, so small **equal-denominator** add/sub (`add_eqden_i64` — a shared-denominator accumulation, e.g.
tallying `k/1_000_003`) also goes native rather than allocating a Big sum + Big-gcd normalize: **0.26×**
num-rational (~3.8× faster). (The byte-width tiers below UNDER-represent this case: their top magnitude bit is set, so a "64b"
coefficient exceeds `i64` and takes the `Big` path.) The residual ~0.4 µs is the two result-`Big`
allocations (`from_i64`) — both implementations must allocate the result; only our *arithmetic* went
native, and etude-bigint's 1-limb `Big` allocation is itself ~1.8× num-bigint's (their deferred inline-repr
item), so that residual will shrink further when that lands.

`normalize` also takes this native path for i64-fitting components, so **small-rational construction**
(`from_ratio_i64`, `new` on small `Big`s) reduces with a native gcd instead of a `Big` gcd. The native
`gcd_u128` further dispatches to a `u64` gcd when both operands fit `u64` (always so on the `normalize`
path): the `u128` remainder step is an `__umodti3` libcall on aarch64, so the `u64` hardware-divide gcd
avoids a libcall per Euclidean step. Together: `from_ratio_i64` went 0.83 µs → 0.18 µs → **0.12 µs**
(0.73× → **0.103×**, ~9.7× faster than num-rational). The `Big` normalize tiers and the u128-path native
arithmetic (`mul_i64`/`add_i64`, whose products exceed `u64`) are unaffected.

## Construction and accessors

`from_ratio_i64` (the load-bearing small-rational constructor, above) is a **0.16×** win via the native
normalize path. The `n/1` constructors are clone/alloc-bound at the smallest size — `from_i64` is 25 ns vs
6 ns (**4.2×**) and `from_bigint` 1.5–4.1× — because each allocates the `Big` `1` denominator (and, for
`from_bigint`, moves the numerator in without a clone); this is the same etude-bigint 1-limb-`Big`
allocation cost as `recip`/`neg`/`abs`@64b and closes with the same inline-repr item. `from_bigint` narrows
toward parity as the integer grows.

The O(1) accessors — `numer`, `denom`, `is_zero`, `is_negative`, `is_integer` — lower to a field read (or a
single `Big::bit_len`/`is_negative` call) and are **intentionally unbenched**: a criterion cell there would
measure only harness/`black_box` overhead, not the function. (If a no-regression *guard* is wanted — to
catch an accidental non-inline regression — that is a separate ask flagged to the operator.)

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

`to_string` is our alloc-lean `Display` (one `String` via `Big::write_decimal`). Current ratios
(`etude / num-rational`):

| tier  | etude    | num-rational | ratio    |
|-------|----------|--------------|----------|
| 64b   | 104 ns   | 290 ns       | **0.36** |
| 256b  | 489 ns   | 682 ns       | **0.72** |
| 1024b | 6.67 µs  | 4.66 µs      | 1.43     |
| 2048b | 16.2 µs  | 18.1 µs      | **0.90** |
| 4096b | 43.3 µs  | 42.2 µs      | 1.02     |

**64b/256b/2048b are WINS**; 1024b/4096b are the last render losses (1.43×, 1.02×). Two ingredients:
(a) `to_decimal_string` pre-sizes the result `String` from the O(1) `byte_len` (decimal digits ≈
`bytes × 2.41`) and writes each component straight into it via `Big::write_decimal`, bypassing the
`write!`/`format_args` machinery and any mid-render reallocation — this alone nearly halved 64b (0.65× →
**0.36×**) and moved every tier; (b) etude-bigint's reciprocal-`÷10^19` peel (#115) + qhat-reciprocal Knuth
divmod (#127) sped the recursive `to_decimal` (256b/2048b/4096b). The residual 1024b/4096b gap is the
recursive `to_decimal`'s own constant factors (the power-stack `10^k` squarings) in etude-bigint, which
they are attacking next — re-bench on each render land.

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
- **slice 14** — route small equal-denominator `add`/`sub` through the native `i128` path: try
  `addsub_small` BEFORE the Big equal-denominator branch (for i64-fitting operands `(a*b + c*b)/(b*b)`
  reduces natively to `(a+c)/b`, cheaper than a Big sum + Big-gcd normalize). New `add_eqden_i64` cell:
  **0.26×** num-rational (~3.8× faster), no regression on the Big tiers. Also wired etude-bigint's
  quotient-only `Big::div_exact` into `normalize`/`reduce_by` (drops the discarded divmod remainder on the
  shared-factor Big path; no random-bench delta since random gcd=1 hits the allocation-free skip).
- **slice 15** — banked the **256b render crossing** (1.36× → 0.97×) that etude-bigint's reciprocal-`÷10^19`
  `to_decimal` peel (#115) rippled straight into our `to_string` — no local change, scoreboard refresh only.
  Recorded the remaining 1024b/4096b render losses as O(n²)-peel-bound, awaiting a subquadratic
  divide-and-conquer `to_decimal` (coordination steer sent to etude-bigint; render is the higher-value
  target over a ≥1024b gcd crossing, which is only ~parity/1.10×).
- **slice 16** — **`cmp` @1024b crossing** (1.06× → 0.90×): borrow-first-iteration of the continued-fraction
  comparison. `cmp_magnitude` now takes the components by reference; the first Euclidean step allocates
  nothing and, when the integer parts differ (the common case), decides with zero operand clones. Only a
  same-integer-part tie recurses (cloning the two denominators once, via `cmp_magnitude_owned`). `abs` is
  taken only when both operands are negative. `cmp` now wins every tier; no regression (64b 0.93, 256b 0.65).
- **slice 17** — benchmark-coverage backfill (operator directive: *every* function benchmarked) + board
  refresh. Added differential `neg`/`abs` benches — both WIN at 256b+ but are clone-bound losses at 64b
  (neg 1.51×, abs 1.59×), the same etude-bigint small-`Big`-clone root as recip@64b. Refreshed the render
  board after etude-bigint's qhat-reciprocal divmod (#127): render 4096b 1.14× → 1.04×, 2048b crossed below
  1.0. No code change to the library; measured deltas only.
- **slice 18** — constructor benches (`from_ratio_i64`/`from_i64`/`from_bigint`) + **native `i128` normalize
  fast path**. Benching `from_ratio_i64` surfaced that small-rational construction routed through the `Big`
  gcd; adding the native path to `normalize` (i64-fitting components → native sign-fixup + `u128` gcd + box)
  cut `from_ratio_i64` 0.83 µs → **0.18 µs** (0.73× → **0.16×**), no regression on the `Big` normalize tiers
  (they exceed `i64` and fall through). Documented the `n/1` ctors as clone-bound (etude-bigint inline-repr)
  and the O(1) accessors as intentionally unbenched.
- **slice 19** — `gcd_u128` dispatches to a native `u64` gcd when both operands fit `u64` (the `normalize`
  small-construction path always does). The `u128` remainder is an `__umodti3` libcall on aarch64; the
  `u64` hardware-divide gcd avoids a libcall per Euclidean step. `from_ratio_i64` 0.18 µs → **0.12 µs**
  (0.16× → **0.103×**, ~9.7× faster than num-rational); no regression on the u128-path native arithmetic
  (`mul_i64`/`add_i64` products exceed `u64`, so they keep the `u128` gcd).
- **slice 20** — `cross_reduce_mul` (the `mul`/`div` `Big` path) borrows via `Cow` instead of cloning each
  factor when its cross-gcd is 1 (the common coprime case for random operands): removes 4 needless `Big`
  clones per `mul`/`div`, allocating only the two products. Wall-clock is neutral at the gcd/mul-dominated
  large tiers and ~1% better at 64b (where the 1-limb clones are the largest fraction); the win is the
  reduced allocation count / allocator pressure. No regression on any tier.
- **slice 21** — `to_decimal_string` pre-sizes the result `String` (from the O(1) `byte_len`) and writes
  each component straight into it via `Big::write_decimal`, instead of `write!(s, "{self}")` into an empty
  `String`. Removes the `format_args`/`Display` dispatch overhead and mid-render reallocations. `to_string`
  improved at every tier — 64b 0.65× → **0.36×** (~halved), 256b 0.98× → **0.72×**, 1024b 1.53× → 1.43×,
  4096b 1.04× → 1.02×. A local render lever that had been mis-attributed entirely to bignum `to_decimal`.
- **slice 22** — continued-fraction `cmp` `divmod_small_q`: each CF step's operands are within a factor of
  ~2, so `⌊a/b⌋ ∈ {0, 1}` dominates; replace the full `divmod` (schoolbook/reciprocal) with a `Big`
  comparison (`q = 0`) or comparison + one subtraction (`q = 1`), falling back to `divmod` only at `q >= 2`.
  Large-tier `cmp` improved sharply: **1024b 0.90× → 0.44×**, **4096b 0.73× → 0.26×**, 2048b 0.78× → 0.64×
  (operand-dependent CF depth). Small tiers (cross-multiply/native) unchanged. Guarded by the differential
  oracle + the 40-pair `cmp_large_continued_fraction` test.
