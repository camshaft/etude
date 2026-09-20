<!-- Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# etude-rational benchmarks

Head-to-head against [`num-rational`](https://crates.io/crates/num-rational)'s `BigRational`, the
reference we optimize toward — the north star is to **beat it** (ratios below 1.00). `benches/arith.rs`
measures each operation across five component-magnitude tiers (named by the per-component bit width) with
the jemalloc allocator, deterministic operands built through the public API, and criterion. Run it with:

```
cargo bench -p etude-rational
```

Numbers below are medians from one `aarch64-linux` run and are **indicative, not authoritative** —
absolute times vary by machine; what matters is the **ratio to num-rational** and its movement as
optimizations land. `ratio` is `etude / num-rational`: `<1.00` = we are faster (**bold**), `>1.00` = slower.

The four core binary ops (`add`/`sub`/`mul`/`div`) are timed **per operation, averaged over 8 operand pairs
per tier** (via criterion `Throughput`), so each cell reflects the op's typical cost rather than a single
draw. A single random pair can land on an unrepresentative regime — this is what produced the earlier
non-monotonic cells (`add`/`sub`@2048b, a real gcd bug caught because that draw shared a denominator factor;
`cmp`@2048b, operand-luck) — so the binop operands additionally use **coprime denominators**, the common
`addsub_big` branch (the rarer shared-factor branch is a separate concern). With averaging, `add`/`sub`
scale monotonically (0.11× → 0.21×) instead of the old jagged 0.11/0.31/0.17/0.60/0.20.

## Current board (against etude-bigint's Stein binary gcd)

| op         | tier   | etude     | num-rational | ratio     |
|------------|--------|-----------|--------------|-----------|
| add        | 64b    | 0.48 µs   | 4.43 µs      | **0.11**  |
| add        | 256b   | 2.48 µs   | 16.9 µs      | **0.15**  |
| add        | 1024b  | 17.3 µs   | 94.3 µs      | **0.18**  |
| add        | 2048b  | 53.4 µs   | 275 µs       | **0.19**  |
| add        | 4096b  | 185 µs    | 883 µs       | **0.21**  |
| sub        | 64b    | 0.48 µs   | 4.33 µs      | **0.11**  |
| sub        | 256b   | 2.49 µs   | 17.0 µs      | **0.15**  |
| sub        | 1024b  | 17.3 µs   | 94.4 µs      | **0.18**  |
| sub        | 2048b  | 53.5 µs   | 275 µs       | **0.19**  |
| sub        | 4096b  | 185 µs    | 878 µs       | **0.21**  |
| mul        | 64b    | 0.28 µs   | 5.40 µs      | **0.053** |
| mul        | 256b   | 4.92 µs   | 21.5 µs      | **0.23**  |
| mul        | 1024b  | 34.1 µs   | 116 µs       | **0.29**  |
| mul        | 2048b  | 103 µs    | 333 µs       | **0.31**  |
| mul        | 4096b  | 350 µs    | 1049 µs      | **0.33**  |
| div        | 64b    | 0.29 µs   | 5.67 µs      | **0.052** |
| div        | 256b   | 4.88 µs   | 21.3 µs      | **0.23**  |
| div        | 1024b  | 33.7 µs   | 116 µs       | **0.29**  |
| div        | 2048b  | 102 µs    | 334 µs       | **0.31**  |
| div        | 4096b  | 350 µs    | 1048 µs      | **0.33**  |
| recip      | 64b    | 29.7 ns   | 14.5 ns      | 2.04      |
| recip      | 256b   | 29.3 ns   | 32.2 ns      | **0.91**  |
| recip      | 1024b  | 32.1 ns   | 34.7 ns      | **0.93**  |
| neg        | 64b    | 25.8 ns   | 17.1 ns      | 1.51      |
| neg        | 256b   | 24.0 ns   | 26.7 ns      | **0.90**  |
| neg        | 1024b  | 25.4 ns   | 30.6 ns      | **0.83**  |
| abs        | 64b    | 25.6 ns   | 16.1 ns      | 1.59      |
| abs        | 256b   | 23.8 ns   | 34.8 ns      | **0.69**  |
| abs        | 1024b  | 25.5 ns   | 37.3 ns      | **0.68**  |
| normalize  | 64b    | 0.16 µs   | 1.24 µs      | **0.13**  |
| normalize  | 256b   | 1.91 µs   | 5.02 µs      | **0.38**  |
| normalize  | 1024b  | 15.4 µs   | 24.6 µs      | **0.63**  |
| normalize  | 2048b  | 46.4 µs   | 65.2 µs      | **0.71**  |
| normalize  | 4096b  | 163 µs    | 192 µs       | **0.85**  |
| cmp        | 64b    | 9.7 ns    | 55.2 ns      | **0.18**  |
| cmp        | 256b   | 62.4 ns   | 146 ns       | **0.43**  |
| cmp        | 1024b  | 80.3 ns   | 180 ns       | **0.45**  |
| cmp        | 2048b  | 812 ns    | 1.27 µs      | **0.64**  |
| cmp        | 4096b  | 146 ns    | 561 ns       | **0.26**  |
| add_eqden  | 64b    | 0.34 µs   | 1.31 µs      | **0.26**  |
| add_eqden  | 256b   | 1.90 µs   | 4.87 µs      | **0.39**  |
| add_eqden  | 1024b  | 15.5 µs   | 24.9 µs      | **0.62**  |
| add_eqden  | 2048b  | 46.1 µs   | 64.6 µs      | **0.71**  |
| add_eqden  | 4096b  | 161 µs    | 191 µs       | **0.85**  |
| cmp_eqden  | 64b    | 6.25 ns   | 7.94 ns      | **0.79**  |
| cmp_eqden  | 256b   | 6.72 ns   | 8.88 ns      | **0.76**  |
| cmp_eqden  | 1024b  | 11.5 ns   | 14.2 ns      | **0.81**  |
| cmp_close  | 64b    | 10.1 ns   | 118 ns       | **0.085** |
| cmp_close  | 256b   | 319 ns    | 474 ns       | **0.67**  |
| cmp_close  | 1024b  | 470 ns    | 593 ns       | **0.79**  |
| cmp_close  | 2048b  | 674 ns    | 808 ns       | **0.83**  |
| cmp_close  | 4096b  | 1.07 µs   | 1.19 µs      | **0.90**  |

**We now beat num-rational on add, sub, mul, div, cmp, normalize, and add_eqden at EVERY tier, plus recip
(256b/1024b) and the equal-denominator fast paths** — a decisive across-the-board lead. `cmp` wins every
tier by 1.5–5× (continued-fraction `q ∈ {0,1}` fast path + the native `u128` `64b` tier). `normalize` and
`add_eqden` — which bottom out on a single large gcd and used to sit at parity (1024b) / a slight loss
(4096b) — now win every tier (64b 0.27×/0.26× up to 4096b 0.85×) after etude-bigint made the Stein gcd
subtract in place (#207), turning raw gcd into a full sweep. The only non-wins left:

1. **`recip`/`neg`/`abs` @64b — 1.5–2.0× (clone-bound).** All three are O(limbs) sign/swap ops that only
   clone the components; at 64b the small `Big` clone dominates, and etude-bigint's 1-limb `Big` clone is
   ~2× num-bigint's (its deferred inline-repr item). All three WIN at 256b and above (num-rational's clone
   grows with size while ours stays ~flat). A single etude-bigint small-`Big` inline representation would
   close the borrowing `recip`/`neg`/`abs`@64b together. For callers that own their operand, the consuming
   variants below already eliminate that clone locally.

## Shared-factor add/sub (`add_shared`)

The core `add`/`sub` board uses **coprime** denominators (the common `addsub_big` branch: `den = b*d`, no
reduction). The other branch — **denominators sharing a factor** `g` — is the everyday case (`1/12 + 1/8`,
`1/6 + 1/4`): there `addsub_big` works over the `lcm` and reduces the numerator against the small `g`
(`gcd(num, g)`, using the identity `gcd(N, lcm) = gcd(N, g)`). The `add_shared` group measures it with
denominators `g·ca` / `g·cb` (`ca ⊥ cb`, so `gcd = g`) and `g = 210` (a single-limb factor, so the reduce
is `gcd(wide num, small g)` — exactly etude-bigint's single-limb gcd fast path, #295):

| tier  | etude    | num-rational | ratio    |
|-------|----------|--------------|----------|
| 64b   | 0.79 µs  | 4.41 µs      | **0.18** |
| 256b  | 2.73 µs  | 17.3 µs      | **0.16** |
| 1024b | 18.0 µs  | 95.6 µs      | **0.19** |
| 2048b | 54.6 µs  | 275 µs       | **0.20** |
| 4096b | 189 µs   | 886 µs       | **0.21** |

**The shared-factor branch WINS every tier (0.16–0.21×)**, essentially matching the coprime branch
(0.11–0.21×) — a touch costlier at 64b (0.18× vs 0.11×) for the extra reduce `gcd(num, g)` + two
`div_exact`, converging to the same 0.21× at 4096b (both branches are multiply-bound there). This closes the
add/sub story: whether or not the denominators share a factor, etude-rational is ~5× num-rational. The
single-limb `g` keeps the reduce `O(n)` via #295 — a wide `gcd(num, g)` would otherwise be `O(bit_len·n)`.
Correctness is covered by the differential oracle (random operands routinely hit the shared-factor branch).

## Consuming sign transforms (`into_recip`/`into_neg`/`into_abs`)

The borrowing `recip`/`neg`/`abs` must clone both components to return an owned value, and at 64b that
1-limb `Big` clone is the whole cost. When the caller owns the operand and does not need it afterwards
(`x = x.into_recip()`), the consuming variants reuse those allocations instead. All three are now **fully
zero-allocation**: `into_recip` swaps the two owned fields (negating both in place when the numerator is
negative), and `into_neg`/`into_abs` flip the numerator's sign in place via etude-bigint's `Big::negate` /
`Big::abs_assign` (an `O(1)` sign-bit toggle — no magnitude copy) while moving the denominator unchanged.
Measured with `iter_batched` (the operand is cloned in the untimed setup, so only the transform is timed);
num-rational has no consuming form, so its `recip`/`-`/`abs` is the reference — note our side additionally
carries the `iter_batched` harness overhead, so these ratios are conservative.

| op         | tier   | etude     | num-rational | ratio     |
|------------|--------|-----------|--------------|-----------|
| into_recip | 64b    | 14.0 ns   | 12.2 ns      | 1.15 ⚠    |
| into_recip | 256b   | 14.1 ns   | 28.9 ns      | **0.49**  |
| into_recip | 1024b  | 14.1 ns   | 31.7 ns      | **0.45**  |
| into_neg   | 64b    | 7.05 ns   | 17.5 ns      | **0.40**  |
| into_neg   | 256b   | 7.16 ns   | 29.4 ns      | **0.24**  |
| into_neg   | 1024b  | 7.10 ns   | 32.4 ns      | **0.22**  |
| into_abs   | 64b    | 7.71 ns   | 16.3 ns      | **0.47**  |
| into_abs   | 256b   | 7.70 ns   | 34.7 ns      | **0.22**  |
| into_abs   | 1024b  | 7.82 ns   | 37.4 ns      | **0.21**  |

Wiring the in-place sign flips took **`into_neg` 0.94× → 0.40×** and **`into_abs` 1.01× → 0.47×** at 64b —
`into_abs@64b` flips from a loss to a decisive win, and both are now ~4–5× faster than num-rational from
256b up (the sign toggle is `O(1)` while num-rational's grows with size). This closes the last consuming-
transform gap the borrowing forms left (`neg`/`abs`@64b at 1.5–1.6×). Covered by
`consuming_sign_transforms_match_borrowing` and the differential oracle (both assert the consuming and
borrowing results are value- and canonical-form-identical).

⚠ `into_recip`@64b reads **1.15×** here — a regression on its **unchanged** positive-swap branch (this PR
touched only the negative-numerator branch, making it zero-alloc too). num-rational's `into_recip` is stable
(12.8 → 12.2 ns), so this is not machine load: the etude field-swap path measured ~7.3 ns at #274 and ~14 ns
now, i.e. a **~2× cost increase on a 1-limb `Big` swap/move+drop introduced by an etude-bigint landing
between #274 and now** (the `&mut`-accumulator surface / #275 / #268 / #271). Flagged to etude-bigint to
bisect Big's move/`Drop` cost for single-limb values; `into_recip` still wins decisively at 256b/1024b.

The large-tier `cmp` lead comes from the continued-fraction comparison, sharpened two ways: a
**borrow-first-iteration** (components passed by reference; the first Euclidean step allocates nothing and,
when integer parts differ — the common case — decides with zero operand clones), and a **`q ∈ {0,1}` fast
path** (`divmod_small_q`) that replaces the per-step `divmod` with a `Big` comparison / one subtraction,
since similar-magnitude operands have quotient 0 or 1. The latter took 1024b 0.90× → **0.44×** and 4096b
0.73× → **0.26×**. Only a same-integer-part tie with fractional remainders on both sides clones the two denominators
and recurses. `abs` is now taken only when both operands are negative (denominators are already positive).

cmp's cost is therefore operand-dependent: it decides on the first Euclidean step when the integer parts
differ (the common case — the `cmp` row above), and recurses when they tie. The single random operand pair
the `cmp` row draws per tier hits that recursive tie only by luck — its `2048b` pair happens to (both
operands land in `(0,1)`), which is why that one cell (812 ns) reads slower than `1024b`/`4096b` rather than
scaling monotonically. The `cmp_close` row is the deterministic worst case (both operands proper fractions
in `(0,1)` with different denominators, so the integer parts always tie): it wins every tier and scales
monotonically (0.085× at 64b via the native `u128` path, up to 0.90× at 4096b as recursion depth grows).

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
| mul_i64 | 48-bit   | 0.17 µs  | 4.13 µs      | **0.042** |
| div_i64 | 48-bit   | 0.21 µs  | 4.26 µs      | **0.048** |
| add_i64 | 48-bit   | 0.13 µs  | 3.14 µs      | **0.042** |
| sub_i64 | 48-bit   | 0.13 µs  | 3.18 µs      | **0.041** |
| cmp_i64 | 48-bit   | 8.7 ns   | ~55 ns       | **~0.16** |
| add_eqden_i64 | 48-bit | 0.10 µs | 0.47 µs   | **0.21**  |
| from_ratio_i64 | 48-bit | 0.12 µs | 1.14 µs  | **0.103** |

**~6–9× faster than num-rational on small operands** — all four arithmetic ops AND `cmp` now take the
native path (`add`/`sub` via `(a*d ± c*b)/(b*d)` with a checked `i128` numerator for the overflow edge;
`cmp` via `a*d ? c*b` in `i128`; falling back to `Big`). `cmp_small` also skips the `byte_len` size probe
for the common i64-fitting case. The native `add`/`sub` path is tried BEFORE the Big equal-denominator
path, so small **equal-denominator** add/sub (`add_eqden_i64` — a shared-denominator accumulation, e.g.
tallying `k/1_000_003`) also goes native rather than allocating a Big sum + Big-gcd normalize: **0.26×**
num-rational (~3.8× faster). The residual ~0.2 µs (after `mul`/`div` cross-reduce on
the i64 originals — slice 26) is dominated by the two result-`Big` allocations — both
implementations must allocate the result; only our *arithmetic* went native. Slice 38 boxed those
results via `Big::from_i128` (a direct ≤2-limb build) instead of a sign-magnitude byte round-trip,
cutting −11–15% off every native op. What remains is etude-bigint's 1-limb `Big` allocation itself,
which is ~1.8× num-bigint's (their deferred inline-repr item), so the residual will shrink further
when that lands.

**A second native tier covers the `64b` byte-width band** (`~1-limb` components whose top magnitude bit is
set, so they exceed `i64` and miss the `to_i64` paths, but whose magnitudes still fit `u64`). There `mul`,
`div`, and `cmp` use `u128`: the cross-products `|a|*d`, `|c|*b` are `u64 * u64` (`< 2^128`), so the
arithmetic runs natively — `cmp` compares them directly (`cmp_small_u128`); `mul`/`div` cross-reduce on the
u64 magnitudes and box the (possibly `> i128`) products via a sign + `u128`→`Big` encode (`big_from_u128`).
This took the `64b` binop tier from the `Big` path to native: **`mul` 0.41× → 0.054×** (2.08 µs → 0.27 µs),
**`div` 0.36× → 0.045×** (1.86 µs → 0.24 µs), **`cmp` 0.93× → 0.18×**. `normalize` (small-rational
construction) takes the same band: a `u64` hardware-divide gcd + `big_from_u128` box instead of the `Big`
gcd/`div_exact`, **`normalize` 64b 0.27× → 0.13×** (0.32 µs → 0.16 µs). The `mul_div_native_u64_magnitude`
and `cmp_native_u64_magnitude` tests cover the band (the i64-seeded oracle can't reach it; both build via
`Rational::new`, so they also exercise the `normalize` u64 path).

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
| mul | **0.29**| **0.33**|
| div | **0.29**| **0.33**|
| add | **0.18**| **0.21**|

`mul`/`div` hold a **~3× lead** (cross-reduction halves the gcd work); the lead does not *widen* at the
large tiers — it plateaus at ~0.33×, since both implementations' schoolbook multiplies scale `O(n²)` and the
constant-factor gap is what we keep. `add`/`sub` hold a **~5× lead** (**0.18–0.21×**), scaling monotonically.
Earlier the add/sub large tiers *appeared* to narrow to ~parity, which was mis-diagnosed as multiply-bound
(awaiting Toom-3) — the real cost was the `2n`-bit reduce gcd over the `(a*d + c*b)/(b*d)` product. `add`/`sub`
now reduce over `gcd(b, d)` on the *denominators* (an n-bit gcd) and, when they are coprime (the common
random case), skip the reduce gcd entirely; when they share a factor `g`, they work over the lcm and reduce
against the small `g` (`gcd(N, lcm) = gcd(N, g)`). So the multiply — not a wide gcd — is the floor for
add/sub, and it beats num-bigint's. (The earlier board's jagged add/sub cells — e.g. `256b` 0.31× and a
`2048b` 0.60× spike — were single-draw artifacts: those seeds happened to have `gcd(b, d) > 1` and hit the
shared-factor branch. The binop cells now average over several coprime-denominator pairs, so the board
reflects the common branch and scales monotonically; the shared-factor branch is tracked separately.)
The whole binop board dropped again after etude-bigint's in-place Stein gcd (#207), which rippled through
every reduction; that also closed the last gcd-bound cells (`normalize`/`add_eqden` now win all tiers), so
the previously-deferred **Lehmer/HGCD** gcd is no longer gap-closing and has been dropped.

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

- **slice 44** (bench-only) — added the `add_shared` group: add/sub on denominators sharing a small factor
  (`g = 210`), exercising `addsub_big`'s **shared-factor branch** (`den = lcm`, reduce `gcd(num, g)`) that
  the coprime `rat_pair` board deliberately excludes. This is the everyday case (`1/12 + 1/8`) and the one
  where etude-bigint's single-limb gcd fast path (#295) applies (the reduce is `gcd(wide num, small g)`).
  It **wins every tier (0.16–0.21×)**, essentially matching the coprime branch — closing the add/sub story
  (both branches ~5× num-rational). Confirmed on fresh main that #295 does **not** move the coprime board
  (normalize/add_eqden unchanged at 0.84×): those compute `gcd(wide, wide)`, so the single-limb fast path
  only reaches the shared-factor reduce. `binop` now takes the operand-pair generator as a parameter.
- **slice 43** (etude #299) — wired etude-bigint's in-place `Big::negate` / `Big::abs_assign` (landed as etude
  #275) into the consuming sign transforms. `into_neg`/`into_abs` now flip the numerator's sign in place
  instead of allocating a fresh magnitude via `Big::neg`/`Big::abs`, and `into_recip`'s negative-numerator
  branch negates the swapped fields in place — all three are fully zero-allocation. **`into_neg`@64b 0.94× →
  0.40×**, **`into_abs`@64b 1.01× → 0.47×** (loss → win). Also surfaced an unrelated `into_recip`@64b
  regression (7.3 → 14 ns on the unchanged swap branch; num-rational stable) flagged to etude-bigint as a
  1-limb `Big` move/`Drop` cost increase. (An interim `RationalSum` accumulator, proposed as etude #294, was
  **closed** by operator design-taste ruling — no separate data structure for marginal gains — so this
  in-place-on-the-existing-type work is the landed form of the scratch-reuse lever.)
- **slice 42** (bench-only) — hardened the core binop board against operand luck. Each `add`/`sub`/`mul`/`div`
  cell is now timed per-op, averaged over 8 operand pairs per tier (criterion `Throughput`), with coprime
  denominators so add/sub measure the common `addsub_big` branch. A single random draw could land on an
  unrepresentative regime — the earlier jagged cells (add/sub `256b` 0.31× and a `2048b` 0.60× spike were
  shared-factor draws; that same single-draw trap is how the slice-40 gcd bug and the cmp@2048b operand-luck
  surfaced). `add`/`sub` now scale monotonically **0.11× → 0.21×** (was 0.11/0.31/0.17/0.60/0.20); `mul`/`div`
  confirmed to hold ~0.33× at the large tiers (the lead plateaus, does not widen — both sides schoolbook
  O(n²)). No code change; makes the scoreboard reflect the algorithm regime, not the draw. Self-merged.
- **slice 41** (bench-only) — added the `cmp_close` group: a deterministic continued-fraction worst case
  (both operands proper fractions in `(0,1)` with different denominators, so their integer parts always
  tie and the comparison recurses). The existing random `cmp` row hits that recursive case only by luck —
  its `2048b` draw does, which is why that one cell read 812 ns (slower than 1024b/4096b) rather than
  scaling monotonically. `cmp_close` bounds the worst case honestly: wins every tier, monotonic (64b 0.085×
  via the native `u128` path → 4096b 0.90×). No code change; makes the scoreboard representative rather than
  draw-dependent. (Self-merged bench change.)
- **slice 39** — consuming sign transforms `into_recip`/`into_neg`/`into_abs`, reusing the owned
  components instead of cloning. `into_recip` of a positive value swaps the two fields (zero allocation),
  flat ~7.3 ns at every tier and **0.57× at 64b** — the one cell the borrowing `recip` loses (2.34×).
  `into_neg` moves the unchanged denominator (one fewer alloc): 64b 1.51× → **0.94×**; `into_abs` 1.59× →
  1.01×. Directly closes the operator-named "recip@64b clone elision" lever for owning callers, no
  etude-bigint change needed; an in-place `Big` sign flip would take `into_neg`/`into_abs`@64b to zero
  allocation too (raised to etude-bigint). Value/canonical-form identity vs the borrowing forms is asserted
  by a dedicated test and the differential oracle.
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
- **slice 33** — banked etude-bigint #207 (Stein gcd subtracts in place — `sub_mag_inplace` instead of
  allocating a fresh magnitude each shift-and-subtract step), which turned raw gcd into a full sweep (4096b
  1.10× → 0.85×, its last loss). This is the gcd behind `normalize`/`add_eqden`, so both now **win every
  tier**: `normalize` 1024b 1.01× → **0.63×** (was parity), 4096b ~1.10× → **0.85×** (was a loss), and
  down to 64b 0.74× → **0.27×**; `add_eqden` 1024b 1.00× → **0.62×**, 4096b **0.85×**, 64b 0.69× → **0.26×**
  (added the 2048b/4096b rows). Closes the last gcd-bound parity/loss class — the only remaining non-wins
  are `recip`/`neg`/`abs`@64b (etude-bigint small-`Big` inline-repr). Scoreboard refresh only, no local
  change. etude-bigint has consequently deprioritized Lehmer/HGCD (no longer needed to close a gap).
- **slice 38** — box native-path results via `Big::from_i128` (direct ≤2-limb build) instead of a
  sign-magnitude byte round-trip. `big_from_i128`/`big_from_u128` had encoded the `i128`/`u128` result to a
  17-byte buffer and re-parsed it whenever the value exceeded `i64` — which is the *common* case for the
  native paths (a 48-bit × 48-bit numerator is ~96-bit), so every native op paid an encode+decode. Switching
  to the direct limb constructor sped up every small-operand hot path: **add_i64 0.15 → 0.13 µs (−15%),
  sub_i64 −15%, mul_i64 0.20 → 0.17 µs (−14%), div_i64 0.23 → 0.21 µs (−11%)**, and the u64-band **mul/64b
  0.27 → 0.25 µs, div/64b 0.24 → 0.22 µs (−6%)** (`big_from_u128` uses `from_i128` for magnitudes ≤ `i128::MAX`,
  keeping the byte path only for the `(2^127, 2^128)` products). No behavior change; differential oracle green.
- **slice 37** — banked the rest of etude-bigint #207 (in-place Stein gcd): slice 33 refreshed only
  `normalize`/`add_eqden`, but the **binops** also route their reductions through that gcd — `add`/`sub` via
  `gcd(b, d)` (`addsub_big`) and `mul`/`div` via `gcd(a,d)`+`gcd(c,b)` (`cross_reduce_mul`) — so all four
  improved and were unbanked. Refreshed the full binop board: **add/sub 256b 0.62× → 0.31×, 1024b 0.26× →
  0.17×, 64b 0.24× → 0.11×**; **mul 256b 0.39× → 0.20×, 1024b 0.43× → 0.28×**; **div 256b 0.41× → 0.21×,
  1024b 0.42× → 0.28×**; added the 2048b/4096b rows (mul/div ≈0.30–0.33×, add/sub ≈0.20×). Scoreboard
  refresh only, no local change. NB: the `add`/`sub` **2048b** cell (0.60×) reflects the shared-denominator-
  factor `lcm` branch — the fixed-seed 2048b operands happen to have denominators sharing a factor, so
  `addsub_big` takes the `gcd(b,d) > 1` path (an extra `gcd(num, g)` on the ~2n-bit numerator); still a win,
  and confirms the shared-factor branch beats num-rational too (mul/div at the same tier are ≈0.30×, on the
  coprime path). Localized via mul/div scaling normally (1024→2048→4096 = 32→97→339 µs), ruling out a
  `Big::mul`/`gcd` size cliff.
- **slice 35** — extended the native `u64`-magnitude band to `normalize` (small-rational construction).
  Its native path used `to_i64_checked`, so the `64b` construction tier (magnitudes fit u64 but exceed
  i64) fell to the `Big` gcd + `div_exact`. Added a `to_i128_checked` + `≤ u64::MAX` guard branch: sign
  fixup, one `gcd_u64` (hardware divide), and `big_from_u128` box — no `Big` gcd. **`normalize` 64b 0.32 µs
  → 0.16 µs (0.27× → 0.13×)**, ~1.9×. The i64 path (`from_ratio_i64`, 117 ns) is tried first and unchanged;
  256b+ (magnitudes > u64) still take the `Big` path. Covered by the existing u64-band tests, which build
  via `Rational::new` → `normalize`. Completes the `64b` native band across cmp/mul/div/normalize.
- **slice 32** — native `u128` `mul`/`div` for the `64b` band (`mul_small_u128`/`div_small_u128`),
  generalizing slice 31's `cmp` win to arithmetic: `~1-limb` components with the top magnitude bit set
  exceed `i64` (missing the `to_i64` paths) but their magnitudes fit `u64`, so after cross-reducing on the
  u64 magnitudes the products fit `u128` (`< 2^128`). They can exceed `i128`, so the sign is carried
  separately and boxed via a new `big_from_u128` (sign + 16 LE magnitude bytes). **`mul` 64b 2.08 µs →
  0.27 µs (0.41× → 0.054×)**, **`div` 64b 1.86 µs → 0.24 µs (0.36× → 0.045×)** — ~8× faster, ~20× vs
  num-rational. `mul_i64`/`div_i64` (i64 path) and 256b+ (Big) unchanged. New `mul_div_native_u64_magnitude`
  test covers the band (products near `2^128` also exercise `big_from_u128`) across sign combinations vs
  num-rational.
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
