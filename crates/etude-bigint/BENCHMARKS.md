<!-- Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# etude-bigint benchmarks

Head-to-head against [`num-bigint`](https://crates.io/crates/num-bigint), the reference we optimize
toward — the north star is to **beat it** (ratios below 1.00), not merely reach parity.
`benches/arith.rs` measures each operation at four magnitude tiers (named by bit width) with the
jemalloc allocator, deterministic operands, and criterion. Run it with:

```
cargo bench -p etude-bigint
```

Numbers below are medians from one `aarch64-linux` run and are **indicative, not authoritative** —
absolute times vary by machine; what matters is the **ratio to num-bigint** and its movement as
optimizations land. `ratio` is `etude / num-bigint`: `<1.00` = we are faster (**bold**), `>1.00` = slower.

## Current — u64 limbs + Knuth Algorithm D divmod

| op                        | tier   | etude     | num-bigint | ratio     |
|---------------------------|--------|-----------|------------|-----------|
| add                       | 64b    | 19.4 ns   | 27.3 ns    | **0.71**  |
| add                       | 256b   | 21.6 ns   | 72.3 ns    | **0.30**  |
| add                       | 1024b  | 41.3 ns   | 87.8 ns    | **0.47**  |
| add                       | 4096b  | 123 ns    | 172 ns     | **0.71**  |
| sub                       | 64b    | 20.1 ns   | 16.4 ns    | 1.22      |
| sub                       | 256b   | 21.8 ns   | 31.0 ns    | **0.70**  |
| sub                       | 1024b  | 37.2 ns   | 43.6 ns    | **0.85**  |
| sub                       | 4096b  | 96.8 ns   | 95.3 ns    | 1.02      |
| mul                       | 64b    | 21.6 ns   | 19.2 ns    | 1.12      |
| mul                       | 256b   | 42.3 ns   | 52.8 ns    | **0.80**  |
| mul                       | 1024b  | 390 ns    | 389 ns     | 1.00      |
| mul                       | 4096b  | 5.14 µs   | 4.99 µs    | 1.03      |
| divmod                    | 64b    | 48.9 ns   | 111 ns     | **0.44**  |
| divmod                    | 256b   | 207 ns    | 392 ns     | **0.53**  |
| divmod                    | 1024b  | 1.13 µs   | 2.09 µs    | **0.54**  |
| divmod                    | 4096b  | 11.1 µs   | 20.5 µs    | **0.54**  |
| div_exact (quotient-only) | 64b    | 35.7 ns   | 53.6 ns    | **0.67**  |
| div_exact (quotient-only) | 256b   | 230 ns    | 232 ns     | **0.99**  |
| div_exact (quotient-only) | 1024b  | 1.09 µs   | 1.02 µs    | 1.06      |
| div_exact (quotient-only) | 4096b  | 11.4 µs   | 10.3 µs    | 1.10      |
| gcd                       | 64b    | 843 ns    | 1.13 µs    | **0.74**  |
| gcd                       | 256b   | 4.09 µs   | 4.71 µs    | **0.87**  |
| gcd                       | 1024b  | 23.5 µs   | 23.1 µs    | 1.02      |
| cmp                       | 64b    | 2.63 ns   | 3.23 ns    | **0.81**  |
| cmp                       | 256b   | 3.66 ns   | 3.94 ns    | **0.93**  |
| cmp                       | 1024b  | 9.06 ns   | 8.93 ns    | 1.01      |
| cmp                       | 4096b  | 27.4 ns   | 27.9 ns    | **0.98**  |
| to_decimal_string         | 64b    | 61.9 ns   | 72.2 ns    | **0.86**  |
| to_decimal_string         | 256b   | 486 ns    | 249 ns     | 1.95      |
| to_decimal_string         | 1024b  | 3.76 µs   | 2.25 µs    | 1.67      |
| to_decimal_string         | 4096b  | 24.1 µs   | 21.1 µs    | 1.14      |
| sign_magnitude_roundtrip  | 64b    | 72.8 ns   | —          | —         |
| sign_magnitude_roundtrip  | 256b   | 135 ns    | —          | —         |
| sign_magnitude_roundtrip  | 1024b  | 250 ns    | —          | —         |
| sign_magnitude_roundtrip  | 4096b  | 524 ns    | —          | —         |
| clone                     | 64b    | 12.9 ns   | 5.87 ns    | 2.20      |
| clone                     | 256b   | 12.7 ns   | 12.7 ns    | 1.00      |
| clone                     | 1024b  | 13.8 ns   | 14.0 ns    | **0.98**  |
| clone                     | 4096b  | 23.1 ns   | 23.1 ns    | 1.00      |
| from_i64                  | —      | 12.2 ns   | 6.86 ns    | 1.77      |

We now **beat num-bigint** on **add** (every tier), **divmod** (every tier), **cmp** (three of four
tiers), and **to_decimal_string at 64b** (0.86×), and reach parity-or-better on **mul at 256b/1024b**
and **sub at 256b**.

`clone` and `from_i64` are the small-value CONSTRUCTION paths: at 64b both trail num-bigint (2.20× /
1.77×) because a small `Big` heap-allocates its one-limb `Vec` where num-bigint has a small-value
fast path; at ≥256b they are at parity (the allocation is amortized). Closing 64b needs an inline
small-value magnitude repr — measured to fix clone/`from_i64` (2.20→~1.0×, 1.77→0.97×) but to REGRESS
add/mul (the `*_mag` kernels build a `Vec` that the inline form then has to copy), so it needs the
kernels to emit inline results directly before it is a net win. Deferred behind that (see roadmap).

(divmod's dividend is twice the divisor's width — the `2n / n` shape. gcd is capped at 1024b because
its Euclid cost is steep. sign_magnitude_roundtrip is the canonical map-key encode+decode; num-bigint
has no matching operation.)

## Landed optimizations

- **base-2⁶⁴ u64 limbs** (u128 intermediates) — halved the limb count; improved every op over the
  original u32 baseline (mul 0.27–0.78, add/cmp 0.53–0.96 of the u32 time).
- **Knuth Algorithm D divmod** (word-at-a-time, replacing bit-at-a-time long division; single-limb
  divisors take a linear `u128`-per-limb fast path). The largest win so far:

  | op / tier      | before (u64 bit-at-a-time) | after (Knuth) | speedup | vs num-bigint |
  |----------------|----------------------------|---------------|---------|---------------|
  | divmod / 256b  | 5.53 µs                    | 207 ns        | 27×     | 0.53          |
  | divmod / 4096b | 412 µs                     | 11.1 µs       | 37×     | 0.54          |

  divmod went from 7–20× *slower* than num-bigint to ~2× *faster*; gcd (Euclid over divmod) fell from
  ~115× to 2.6× as a side effect.
- **u128-free widening multiply on wasm** — `wide_mul` keeps u64 limbs but synthesizes `u64*u64` from
  four native `u32*u32` partials on 32-bit/wasm (no `__multi3`); 64-bit keeps the `u128` path unchanged.
- **Chunked `to_decimal_string`** — divide by `10¹⁹` (largest power of ten in a u64) for 19 digits per
  step instead of one:

  | tier   | before (÷10) | after (÷10¹⁹) | speedup | vs num-bigint |
  |--------|--------------|---------------|---------|---------------|
  | 256b   | 4.34 µs      | 675 ns        | 6.4×    | 2.7           |
  | 1024b  | 51.5 µs      | 4.84 µs       | 10.6×   | 2.2           |
- **Binary (Stein) GCD** — shift-and-subtract, no division per step; replaces Euclid-over-divmod:

  | tier   | before (Euclid+Knuth) | after (Stein) | speedup | vs num-bigint |
  |--------|-----------------------|---------------|---------|---------------|
  | 256b   | 8.23 µs               | 4.09 µs       | 2.0×    | **0.87**      |
  | 1024b  | 59.5 µs               | 23.5 µs       | 2.5×    | 1.02          |
- **Karatsuba multiply** above a 40-limb crossover (three half-size products via
  `z1 = (a0+a1)(b0+b1) − z0 − z2`; recurses through the schoolbook base case): mul/4096b 6.19 µs → 5.14 µs
  (1.24× → 1.03× num-bigint). Smaller tiers stay schoolbook (unchanged).
- **Direct signed subtract** — a shared `add_signed` core takes the second operand's sign as a
  parameter, so `sub` no longer allocates a negated copy of `other`: sub/256b 30.8 → 22.7 ns (**0.75×**),
  sub/1024b 50.3 → 39.6 ns (**0.91×**), sub/4096b 133 → 114 ns (1.20×, was 1.40×).
- **Branchless `sub_mag`** — the borrow chain is two native-`u64` `overflowing_sub`s per limb (limb − b,
  then − incoming borrow; the outgoing borrow is their OR), replacing the per-limb `i128` widen +
  `if d < 0` branch. Improves every tier — sub/256b 22.7 → 21.8 ns (**0.70×**), sub/1024b 39.6 → 37.2 ns
  (**0.85×**), sub/4096b 114 → 96.8 ns (1.20× → **1.02×**, closing the last losing sub tier).
- **In-place divide-by-limb in `to_decimal_string`** — the ÷10¹⁹ chunk loop divides the magnitude in
  place (`div_rem_limb_inplace`), so no per-chunk quotient `Vec` is allocated: 1024b 4.84 → 4.37 µs.
- **Recursive divide-and-conquer `to_decimal_string`** — above a 10-limb crossover, split the magnitude
  by a half-width power of ten (`10^(19·2^i)`, built by repeated squaring) into two ≈equal-width halves,
  each converted recursively; narrow magnitudes keep the linear chunk method. This is the subquadratic
  base conversion num-bigint uses — the linear method's O(n²) chunk scan dominates as the value widens:

  | tier   | before (linear) | after (recursive) | speedup | vs num-bigint |
  |--------|-----------------|-------------------|---------|---------------|
  | 1024b  | 4.37 µs         | 4.03 µs           | 1.08×   | 1.95 → 1.79   |
  | 4096b  | ~77 µs (est.)   | 25.3 µs           | ~3×     | — → 1.19      |
- **Sink-writing `write_decimal`** — the emitter writes into a `core::fmt::Write` sink (one `write_str`
  per 19-digit chunk) instead of pushing bytes one at a time into a `Vec<u8>`; `to_decimal_string` is a
  thin wrapper that writes into a fresh `String`. Fewer sink calls + a single grow per chunk cut every
  tier: 64b 186 → 151 ns (2.58 → 2.06), 256b 623 → 545 ns (2.51 → 2.19), 1024b 4.03 → 3.76 µs
  (1.79 → 1.67), 4096b 25.3 → 24.1 µs (1.19 → 1.14). It also lets a downstream `Display` render a `Big`
  with no intermediate allocation.
- **Single-limb `to_decimal` fast path** — a value that fits one `u64` limb (≤ ~1.8·10¹⁹) writes in a
  single chunk with no magnitude clone and no chunk-index `Vec`, instead of running the peel loop:
  64b 151 → 61.9 ns, crossing from 2.06× to **0.86× — now faster than num-bigint** at the small tier.
- **Stack-scratch linear `to_decimal`** — the `2..=10`-limb peel loop runs over fixed stack buffers (the
  quotient and the chunk list, both bounded by the limb threshold) instead of a heap `Vec` clone + chunk
  `Vec`, so the whole narrow path allocates nothing: 256b 545 → 486 ns (2.19× → 1.95×).
- **Quotient-only `div_exact`** — a `want_rem` flag threaded through the division core (`a<b` / single-limb
  / Knuth) skips building the remainder `Vec` entirely, for callers that divide by a known factor and
  discard the remainder (fraction reduction, cross-reduction). vs num-bigint's `/`: 64b **0.67×** (beats
  it), 256b 0.99×; vs our own `divmod` it removes one allocation per call. (num-bigint's `/` column is
  quotient-only, so it is faster than its `/`+`%` divmod column — hence the tighter ratios at ≥1024b.)

## Where the gaps remain (optimization order)

1. **to_decimal_string at 256b/1024b (1.95× / 1.67×).** The narrow path is now alloc-free; the residual
   is num-bigint's divide-by-`u64` inner loop and its recursive conversion constant factors.
2. **clone / from_i64 at 64b (2.20× / 1.77×).** The small-value construction/clone paths heap-allocate a
   one-limb `Vec`. An inline small-value magnitude repr fixes these (measured 2.20→~1.0× / 1.77→0.97×) but
   REGRESSES add/mul unless the arithmetic kernels emit inline results directly — a larger change (below).
3. **to_decimal_string at 4096b (1.14×), mul at 4096b (1.03×), sub at 4096b (1.02×), gcd at 1024b
   (1.02×), the 64b add/sub/mul tiers (~1.1–1.25×).** Largely at parity; num-bigint's edge at the largest
   tiers is a subquadratic (fast) divmod under the recursive base conversion, Toom-3 mul, and a Lehmer gcd.

## Roadmap

Next, in gap order, each landing with its scoreboard delta and the num-bigint differential oracle green:
Lehmer gcd → subquadratic divmod → Toom-3 mul. The inline small-value magnitude repr is deferred: it must
first grow inline-emitting arithmetic kernels (otherwise it regresses add/mul), then it closes clone /
from_i64 / the 64b arithmetic tiers together. num-bigint stays both the correctness oracle and the
perf yardstick.

## Target notes: wasm / 32-bit

The limb STORAGE is `u64` on every target — `wasm32` has native 64-bit integers, so add/sub/shift/cmp
over u64 limbs are native there and the u64-limb win (half the limbs) holds. The one operation that
would otherwise pay for emulation is the widening multiply: `u64 * u64 -> u128` lowers to a `__multi3`
libcall on wasm. `wide_mul` avoids it — on 64-bit targets it is one native `u128` multiply, and on
wasm32 / 32-bit targets it is synthesized from four native `u32 * u32 -> u64` partials (no 128-bit
intrinsic). Both paths are covered by the differential oracle (the `synth-mul` feature runs the
synthesized path on a 64-bit host).

The three-way comparison, measured via `examples/wasm_mul_bench.rs` (which times all three inline, so
one binary runs on any target — `cargo run --release --example wasm_mul_bench` natively, or on wasm
`--target wasm32-wasip1` under `wasmtime`, wired in `.cargo/config.toml`). ns per schoolbook multiply:

| strategy                       | aarch64 512b | aarch64 2048b | wasm32 512b | wasm32 2048b | wasm32 8192b |
|--------------------------------|--------------|---------------|-------------|--------------|--------------|
| u64 limbs, `u128` widening     | **126**      | **1476**      | 383         | 5473         | 85186        |
| u64 limbs, synthesized widening| 196          | 2472          | **263**     | **3578**     | **56114**    |
| u32 limbs, `u64` widening      | 304          | 5308          | 303         | 5323         | 97021        |

The per-target winner is exactly what the crate ships: **`u128` on 64-bit native** (1.5× faster than
the synthesized path there) and **the synthesized `u32`-half path on wasm** (~1.5× faster than the
emulated `u128`, and faster than dropping to u32 limbs). u64 limbs stay the storage on both. This
confirms the `wide_mul` cfg gate; no change was needed. The divmod inner loop still uses `u128`;
de-emulating it (a 128÷64 step from 64-bit ops) is a follow-up.
