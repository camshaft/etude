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
| sub                       | 64b    | 30.4 ns   | 17.3 ns    | 1.76      |
| sub                       | 256b   | 30.8 ns   | 30.9 ns    | **1.00**  |
| sub                       | 1024b  | 50.3 ns   | 44.1 ns    | 1.14      |
| sub                       | 4096b  | 133 ns    | 95.3 ns    | 1.40      |
| mul                       | 64b    | 21.6 ns   | 19.2 ns    | 1.12      |
| mul                       | 256b   | 42.3 ns   | 52.8 ns    | **0.80**  |
| mul                       | 1024b  | 390 ns    | 389 ns     | 1.00      |
| mul                       | 4096b  | 6.19 µs   | 4.99 µs    | 1.24      |
| divmod                    | 64b    | 48.9 ns   | 111 ns     | **0.44**  |
| divmod                    | 256b   | 207 ns    | 392 ns     | **0.53**  |
| divmod                    | 1024b  | 1.13 µs   | 2.09 µs    | **0.54**  |
| divmod                    | 4096b  | 11.1 µs   | 20.5 µs    | **0.54**  |
| gcd                       | 64b    | 1.13 µs   | 1.11 µs    | 1.02      |
| gcd                       | 256b   | 8.23 µs   | 4.66 µs    | 1.77      |
| gcd                       | 1024b  | 59.5 µs   | 23.2 µs    | 2.56      |
| cmp                       | 64b    | 2.63 ns   | 3.23 ns    | **0.81**  |
| cmp                       | 256b   | 3.66 ns   | 3.94 ns    | **0.93**  |
| cmp                       | 1024b  | 9.06 ns   | 8.93 ns    | 1.01      |
| cmp                       | 4096b  | 27.4 ns   | 27.9 ns    | **0.98**  |
| to_decimal_string         | 64b    | 207 ns    | 71.8 ns    | 2.9       |
| to_decimal_string         | 256b   | 675 ns    | 249 ns     | 2.7       |
| to_decimal_string         | 1024b  | 4.84 µs   | 2.25 µs    | 2.2       |
| sign_magnitude_roundtrip  | 64b    | 72.8 ns   | —          | —         |
| sign_magnitude_roundtrip  | 256b   | 135 ns    | —          | —         |
| sign_magnitude_roundtrip  | 1024b  | 250 ns    | —          | —         |
| sign_magnitude_roundtrip  | 4096b  | 524 ns    | —          | —         |

We now **beat num-bigint** on **add** (every tier), **divmod** (every tier), and **cmp** (three of four
tiers), and reach parity-or-better on **mul at 256b/1024b** and **sub at 256b**.

(divmod's dividend is twice the divisor's width — the `2n / n` shape. gcd and to_decimal_string are
capped at 1024b because their Euclid / per-digit cost is steep. sign_magnitude_roundtrip is the
canonical map-key encode+decode; num-bigint has no matching operation.)

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

## Where the gaps remain (optimization order)

1. **to_decimal_string — ~2.2–2.9×.** Now chunked; the residual is num-bigint's recursive/divide-and-
   conquer base conversion. A recursive split (halve by a power of ten) would close more.
2. **gcd — up to 2.6×.** Now divmod-fast but still plain Euclid; a binary or Lehmer gcd closes the rest.
3. **mul at 4096b (1.24×).** Schoolbook O(n·m); Karatsuba above a crossover for the large tier.
4. **sub (1.0–1.76×).** Routes through `add(neg())`, allocating an extra magnitude; a direct signed
   subtract removes that.

## Roadmap

Next, in gap order, each landing with its scoreboard delta and the num-bigint differential oracle green:
binary/Lehmer gcd → Karatsuba mul → direct signed sub → recursive to_decimal_string. num-bigint stays both
the correctness oracle and the perf yardstick.

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
