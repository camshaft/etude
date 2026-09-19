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
| add        | 64b    | 0.94 µs   | 3.88 µs      | **0.24**  |
| add        | 256b   | 10.3 µs   | 16.7 µs      | **0.62**  |
| add        | 1024b  | 24.1 µs   | 94.0 µs      | **0.26**  |
| sub        | 64b    | 0.95 µs   | 3.98 µs      | **0.24**  |
| sub        | 256b   | 10.5 µs   | 16.5 µs      | **0.63**  |
| sub        | 1024b  | 24.1 µs   | 94.1 µs      | **0.26**  |
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
| cmp        | 64b    | 9.7 ns    | 55.2 ns      | **0.18**  |
| cmp        | 256b   | 62.4 ns   | 146 ns       | **0.43**  |
| cmp        | 1024b  | 80.3 ns   | 180 ns       | **0.45**  |
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

The `64b` tier's win comes from a **native `u128` cross-multiply** (`cmp_small_u128`): its ~1-limb
components have their top magnitude bit set, so they exceed `i64` and miss `cmp_small`, but their
*magnitudes* fit `u64` — so `|a|*d` and `|c|*b` are `u64 * u64` products that fit `u128` with no overflow.
Comparing them natively (with the sign, already resolved, applied — reversed for two negatives) replaces
two `Big` multiplies + a `Big` compare with two `u128` multiplies + one compare, and allocates nothing:
**64b `cmp` 50.9 ns → 9.7 ns (0.93× → 0.18×)**. Covered by the `cmp_native_u64_magnitude` test (all sign
combinations across magnitudes in `(i64::MAX, u64::MAX]`, cross-checked vs num-rational).

## Native i128 fast path — small (i64-fitting) operands

The common real-world case is *small* rationals (`3/10`, `127/5000`, …) whose numerator and denominator
fit `i64`. There, `mul`/`div` take a **native `i128` fast path**: `a*c`/`b*d` (or `a*d`/`b*c`) fit `i128`
with no overflow, reduced by a native `u128` gcd, boxed back to `Big` — **zero bignum arithmetic**.
num-rational always uses `BigInt`, so this is a large win:

| op      | operands | etude    | num-rational | ratio     |
|---------|----------|----------|--------------|-----------|
| mul_i64 | 48-bit   | 0.20 µs  | 4.13 µs      | **0.048** |
| div_i64 | 48-bit   | 0.23 µs  | 4.26 µs      | **0.054** |
| add_i64 | 48-bit   | 0.16 µs  | 3.14 µs      | **0.050** |
| sub_i64 | 48-bit   | 0.16 µs  | 3.18 µs      | **0.049** |
| cmp_i64 | 48-bit   | 8.7 ns   | ~55 ns       | **~0.16** |
| add_eqden_i64 | 48-bit | 0.10 µs | 0.47 µs   | **0.21**  |
| from_ratio_i64 | 48-bit | 0.12 µs | 1.14 µs  | **0.103** |

**~6–9× faster than num-rational on small operands** — all four arithmetic ops AND `cmp` now take the
native path (`add`/`sub` via `(a*d ± c*b)/(b*d)` with a checked `i128` numerator for the overflow edge;
`cmp` via `a*d ? c*b` in `i128`; falling back to `Big`). `cmp_small` also skips the `byte_len` size probe
for the common i64-fitting case. The native `add`/`sub` path is tried BEFORE the Big equal-denominator
path, so small **equal-denominator** add/sub (`add_eqden_i64` — a shared-denominator accumulation, e.g.
tallying `k/1_000_003`) also goes native rather than allocating a Big sum + Big-gcd normalize: **0.26×**
num-rational (~3.8× faster). (The byte-width tiers below UNDER-represent this case: their top magnitude bit is set, so a "64b"
coefficient exceeds `i64` and takes the `Big` path.) The residual ~0.2 µs (after `mul`/`div` cross-reduce on
the i64 originals — slice 26) is dominated by the two result-`Big` allocations (`from_i64`) — both
implementations must allocate the result; only our *arithmetic* went native, and etude-bigint's 1-limb
`Big` allocation is itself ~1.8× num-bigint's (their deferred inline-repr item), so that residual will
shrink further when that lands.

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
| add | **0.26**| **0.26**|

`mul`/`div` **hold** their ~2× lead (cross-reduction halves the gcd work), and `add`/`sub` now hold a
**~4× lead** too (**0.26×** at 1024b/4096b). Earlier these narrowed to ~parity, which was mis-diagnosed as
multiply-bound (awaiting Toom-3) — the real cost was the `2n`-bit reduce gcd over the `(a*d + c*b)/(b*d)`
product. `add`/`sub` now reduce over `gcd(b, d)` on the *denominators* (an n-bit gcd) and, when they are
coprime (the common random case), skip the reduce gcd entirely; when they share a factor `g`, they work
over the lcm and reduce against the small `g` (`gcd(N, lcm) = gcd(N, g)`). So the multiply — not a wide
gcd — is now the floor for add/sub, and it is already competitive with num-bigint's. (The 256b/2048b bench
seeds happen to have `gcd(b, d) > 1`, so those cells sit at the lcm-branch ratio ~0.62/0.80 rather than the
coprime ~0.26; both branches beat num-rational.) The remaining bignum lever is a **Lehmer gcd** in
etude-bigint for the still-gcd-bound `normalize`/`add_eqden`@≥1024b (raw unreduced pairs), which is deferred.

## Rendering (`to_string` / `Display`)

`to_string` is our alloc-lean `Display` (one `String` via `Big::write_decimal`). Current ratios
(`etude / num-rational`):

| tier  | etude    | num-rational | ratio    |
|-------|----------|--------------|----------|
| 64b   | 94 ns    | 285 ns       | **0.33** |
| 256b  | 531 ns   | 675 ns       | **0.79** |
| 1024b | 2.72 µs  | 4.66 µs      | **0.58** |
| 2048b | 7.61 µs  | 18.0 µs      | **0.42** |
| 4096b | 24.2 µs  | 42.3 µs      | **0.57** |

**Render is now a clean sweep — all five tiers WIN.** Ingredients:
(a) `to_decimal_string` pre-sizes the result `String` from the O(1) `byte_len` (decimal digits ≈
`bytes × 2.41`) and writes each component straight into it via `Big::write_decimal`, bypassing the
`write!`/`format_args` machinery and any mid-render reallocation — this alone nearly halved 64b (0.65× →
**0.33×**) and moved every tier; (b) etude-bigint's reciprocal-`÷10^19` peel (#115), qhat-reciprocal Knuth
divmod (#127), skipping the wasted top squaring in the recursive `to_decimal` power stack (#169), and —
closing the last render loss — raising `DECIMAL_RECURSIVE_THRESHOLD` 10 → 64 limbs (#197). The recursive
split's per-node `divmod`+alloc overhead was actually *slower* than the linear reciprocal-peel through ~64
limbs, so the stale threshold was routing 1024b/2048b renders onto the slower path; retuning it crossed
**1024b 1.27× → 0.58×** (the last remaining render loss) and further improved **2048b 0.74× → 0.42×**,
**4096b 0.78× → 0.57×**. The recursive path (with the skip-top-squaring win) is retained for >64 limbs, so
very wide renders are unaffected. Re-bench on each render land.

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
- **slice 31** — native `u128` `cmp` for the `64b` tier (`cmp_small_u128`): ~1-limb components with the top
  magnitude bit set exceed `i64` (missing `cmp_small`) but their magnitudes fit `u64`, so `|a|*d` and `|c|*b`
  are `u64 * u64` products that fit `u128`. Compare them natively — sign already resolved by the caller,
  reversed for two negatives — replacing two `Big` multiplies + a `Big` compare with two `u128` multiplies:
  **64b `cmp` 50.9 ns → 9.7 ns (0.93× → 0.18×)**, the last non-decisive `cmp` cell. `cmp_i64` (i64 path)
  and 256b+ (CF) unchanged. New `cmp_native_u64_magnitude` test covers the path (the oracle's i64 seeds
  can't reach the `(i64::MAX, u64::MAX]` band) across all sign combinations vs num-rational.
- **slice 30** — banked etude-bigint #197 (raised `to_decimal`'s `DECIMAL_RECURSIVE_THRESHOLD` 10 → 64
  limbs): the recursive split's per-node `divmod`+alloc was slower than the linear reciprocal-`÷10^19` peel
  through ~64 limbs, so the stale threshold routed 1024b/2048b renders onto the slower path. Retuning it
  **closed the last render loss — `to_string` 1024b 1.27× → 0.58×** — and further improved **2048b 0.74× →
  0.42×, 4096b 0.78× → 0.57×** (64b 0.37× → 0.33×, 256b 0.72× → 0.79×). Render is now a clean 5/5 sweep.
  Scoreboard refresh only, no local change (render is bignum-`to_decimal`-bound). This retires the "render
  scratch-node" lever I'd flagged — the root cause was thresholding, not per-node allocation.
- **slice 29** — native `mul_small`/`div_small` add a coprime fast path: when both cross-gcds are 1 (the
  common case for random canonical operands), skip the four `x/1` cancellations — each a hardware `sdiv`
  (~12–20 cycles on aarch64) even when the divisor is 1 — and multiply the originals directly (the result
  is already lowest-terms). `mul_i64` 208.7 ns → **197.2 ns (−5.5%)**, `div_i64` 249.6 ns → **231.6 ns
  (−7.2%)**. Refines slice 26 (the gcds themselves stay; only the redundant divisions are elided). Oracle
  (i64 seeds) covers both branches.
- **slice 28** — mixed-magnitude coverage: `add_mixed`/`mul_mixed`/`div_mixed` bench a small (i64-fitting)
  rational against a 1024b one — a real workload (nudging an accumulated big rational by a small correction)
  that the same-width tiers miss. Confirms no hidden loss when the native i128 path can't fire (the wide
  operand forces the `Big` path): **add_mixed 0.41×, mul_mixed 0.63×, div_mixed 0.64×** — the
  gcd-of-denominators add and cross-reduce mul/div handle unbalanced operands well. Bench-only (coverage +
  regression guard); no code change.
- **slice 27** — native `addsub_small` reduces over `gcd(b, d)` on the denominators (a `u64` hardware-divide
  gcd) instead of `gcd_u128` over the ~127-bit `(a*d ± c*b, b*d)` product: coprime denominators (common) ⇒
  already lowest-terms, skip the wide gcd (canonicalizing zero to 0/1); shared factor ⇒ the previous
  `gcd_u128` fallback. `add_i64`/`sub_i64` 0.40 µs → **0.16 µs (0.126× → 0.050×)**, `add_eqden_i64` 0.26× →
  0.21×. The native analog of slice 23; completes the "gcd on the small denominators, not the wide product"
  treatment across mul/div (slice 26) and add/sub.
- **slice 26** — native `mul_small`/`div_small` cross-reduce on the i64 originals (`gcd(a,d)`, `gcd(c,b)`)
  instead of one `gcd_u128` over the ~126-bit products. Since the operands are canonical, the cross-reduced
  result is already lowest-terms (no final gcd), and the two gcds run on `u64` (hardware divide) rather than
  `u128` (an `__umodti3` libcall). `mul_i64` 0.43 µs → **0.21 µs (0.107× → 0.050×)**, `div_i64` → **0.058×**
  (~17–20× num-rational). Corrects the earlier read that the native residual was purely allocation-bound —
  the u128-product gcd was a real chunk. Oracle (i64 seeds) exercises both.
- **slice 25** — banked the render crossing from etude-bigint's "skip the wasted top squaring" in the
  recursive `to_decimal` (#169): `to_string` **4096b 1.02× → 0.78×** (now beats num-rational), 2048b 0.90× →
  0.74×, 1024b 1.43× → 1.27×; 64b/256b unchanged (linear peel). Scoreboard refresh only, no local change
  (render is bignum-render-bound). Corrected the residual attribution (it is the recursive split's
  `divmod`/per-node allocation overhead, not the power-stack squarings — etude-bigint #157/#169).
- **slice 24** — lower `CMP_SMALL_BYTES` 64 → 16: the slice-22 `q ∈ {0,1}` fast path made the
  continued-fraction comparison cheap enough to beat cross-multiply from ~32 bytes up, so route 256b+
  operands to CF (previously cross-multiply). `cmp` 256b **0.65× → 0.43×** (94 ns → 62 ns); 64b (8-byte,
  1-limb) stays on cross-multiply (50 ns, unchanged); 1024b+ already CF. A re-tuned crossover after the CF
  speedup — no new code path, just the threshold.
- **slice 23** — `add`/`sub` reduce over `gcd(b, d)` (denominators, n-bit) instead of the full
  `gcd(a*d ± c*b, b*d)` (2n-bit). Coprime denominators (common) ⇒ the result is already lowest-terms, skip
  the reduce gcd; shared factor ⇒ work over the lcm and reduce against the small `g` (`gcd(N, lcm) =
  gcd(N, g)`). This corrects the earlier "add/sub@large is multiply-bound / needs Toom-3" diagnosis — it was
  the wide reduce gcd. **add/sub 1024b 0.77× → 0.26×, 4096b 0.98× → 0.26×, 64b 0.50× → 0.24×** (≈4×
  num-rational); coprime cells only. Guarded by the oracle + a new `add_sub_large_shared_denominator_factor`
  test covering the Big lcm/`g > 1` branch.
