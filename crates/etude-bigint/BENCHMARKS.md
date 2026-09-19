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
| sub                       | 64b    | 21.5 ns   | 17.2 ns    | 1.25      |
| sub                       | 256b   | 22.7 ns   | 30.2 ns    | **0.75**  |
| sub                       | 1024b  | 39.6 ns   | 43.4 ns    | **0.91**  |
| sub                       | 4096b  | 114 ns    | 95.0 ns    | 1.20      |
| mul                       | 64b    | 21.6 ns   | 19.2 ns    | 1.12      |
| mul                       | 256b   | 42.3 ns   | 52.8 ns    | **0.80**  |
| mul                       | 1024b  | 390 ns    | 389 ns     | 1.00      |
| mul                       | 4096b  | 5.14 µs   | 4.99 µs    | 1.03      |
| divmod                    | 64b    | 48.9 ns   | 111 ns     | **0.44**  |
| divmod                    | 256b   | 207 ns    | 392 ns     | **0.53**  |
| divmod                    | 1024b  | 1.13 µs   | 2.09 µs    | **0.54**  |
| divmod                    | 4096b  | 11.1 µs   | 20.5 µs    | **0.54**  |
| gcd                       | 64b    | 843 ns    | 1.13 µs    | **0.74**  |
| gcd                       | 256b   | 4.09 µs   | 4.71 µs    | **0.87**  |
| gcd                       | 1024b  | 23.5 µs   | 23.1 µs    | 1.02      |
| cmp                       | 64b    | 2.63 ns   | 3.23 ns    | **0.81**  |
| cmp                       | 256b   | 3.66 ns   | 3.94 ns    | **0.93**  |
| cmp                       | 1024b  | 9.06 ns   | 8.93 ns    | 1.01      |
| cmp                       | 4096b  | 27.4 ns   | 27.9 ns    | **0.98**  |
| to_decimal_string         | 64b    | 151 ns    | 73.3 ns    | 2.06      |
| to_decimal_string         | 256b   | 545 ns    | 248 ns     | 2.19      |
| to_decimal_string         | 1024b  | 3.76 µs   | 2.25 µs    | 1.67      |
| to_decimal_string         | 4096b  | 24.1 µs   | 21.1 µs    | 1.14      |
| sign_magnitude_roundtrip  | 64b    | 72.8 ns   | —          | —         |
| sign_magnitude_roundtrip  | 256b   | 135 ns    | —          | —         |
| sign_magnitude_roundtrip  | 1024b  | 250 ns    | —          | —         |
| sign_magnitude_roundtrip  | 4096b  | 524 ns    | —          | —         |

We now **beat num-bigint** on **add** (every tier), **divmod** (every tier), and **cmp** (three of four
tiers), and reach parity-or-better on **mul at 256b/1024b** and **sub at 256b**.

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

## Where the gaps remain (optimization order)

1. **to_decimal_string at the small tiers (64b/256b ~2.5×).** These stay on the linear chunk method
   (below the recursive crossover); the residual is num-bigint's inline small-value handling — a
   small-value fast path (avoiding a heap `Vec` for ≤1-limb values) would help here and elsewhere.
2. **to_decimal_string at 4096b (1.19×), sub/mul at 4096b (1.20× / 1.03×), gcd at 1024b (1.02×), the
   64b tiers (add/sub/mul ~1.1–1.25×).** Largely at parity; num-bigint's edge at the largest tiers is a
   subquadratic (fast) divmod under the recursive base conversion, Toom-3 mul, and a Lehmer gcd.

## Roadmap

Next, in gap order, each landing with its scoreboard delta and the num-bigint differential oracle green:
small-value inline fast path → (later) subquadratic divmod, Toom-3 mul, Lehmer gcd. num-bigint stays
both the correctness oracle and the perf yardstick.

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
