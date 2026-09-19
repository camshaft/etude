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

## Current — base-2⁶⁴ u64 limbs, schoolbook algorithms

| op                        | tier   | etude     | num-bigint | ratio     |
|---------------------------|--------|-----------|------------|-----------|
| add                       | 64b    | 19.1 ns   | 25.4 ns    | **0.75**  |
| add                       | 256b   | 21.0 ns   | 72.1 ns    | **0.29**  |
| add                       | 1024b  | 40.3 ns   | 87.2 ns    | **0.46**  |
| add                       | 4096b  | 122 ns    | 171 ns     | **0.71**  |
| sub                       | 64b    | 31.1 ns   | 17.2 ns    | 1.81      |
| sub                       | 256b   | 31.0 ns   | 30.3 ns    | 1.02      |
| sub                       | 1024b  | 49.9 ns   | 43.5 ns    | 1.15      |
| sub                       | 4096b  | 134 ns    | 94.4 ns    | 1.42      |
| mul                       | 64b    | 21.4 ns   | 18.9 ns    | 1.13      |
| mul                       | 256b   | 42.4 ns   | 52.0 ns    | **0.82**  |
| mul                       | 1024b  | 390 ns    | 389 ns     | 1.00      |
| mul                       | 4096b  | 6.19 µs   | 4.99 µs    | 1.24      |
| divmod                    | 64b    | 759 ns    | 111 ns     | 6.86      |
| divmod                    | 256b   | 5.53 µs   | 388 ns     | 14.2      |
| divmod                    | 1024b  | 38.4 µs   | 2.07 µs    | 18.6      |
| divmod                    | 4096b  | 412 µs    | 20.4 µs    | 20.3      |
| gcd                       | 64b    | 11.2 µs   | 1.15 µs    | 9.81      |
| gcd                       | 256b   | 120 µs    | 4.77 µs    | 25.2      |
| gcd                       | 1024b  | 2.71 ms   | 23.5 µs    | 115.3     |
| cmp                       | 64b    | 2.63 ns   | 2.73 ns    | **0.96**  |
| cmp                       | 256b   | 3.66 ns   | 3.55 ns    | 1.03      |
| cmp                       | 1024b  | 8.91 ns   | 8.72 ns    | 1.02      |
| cmp                       | 4096b  | 27.5 ns   | 27.5 ns    | 1.00      |
| to_decimal_string         | 64b    | 9.01 µs   | 71.8 ns    | 125       |
| to_decimal_string         | 256b   | 139 µs    | 248 ns     | 562       |
| to_decimal_string         | 1024b  | 2.15 ms   | 2.25 µs    | 958       |
| sign_magnitude_roundtrip  | 64b    | 72.8 ns   | —          | —         |
| sign_magnitude_roundtrip  | 256b   | 135 ns    | —          | —         |
| sign_magnitude_roundtrip  | 1024b  | 250 ns    | —          | —         |
| sign_magnitude_roundtrip  | 4096b  | 524 ns    | —          | —         |

(divmod's dividend is twice the divisor's width — the `2n / n` shape. gcd and to_decimal_string are
capped at 1024b because their per-digit / Euclid cost is steep. sign_magnitude_roundtrip is the
canonical map-key encode+decode; num-bigint has no matching operation.)

## Adopted: base-2⁶⁴ u64 limbs (vs the u32 baseline)

Switching the internal limb from `u32` to `u64` (with `u128` intermediate products and accumulators)
halves the limb count and doubles the work per native instruction. It improved **every** operation and
regressed none, so it is adopted. Speedup vs the u32 baseline (u64 / u32, lower is better):

| op     | 64b  | 256b | 1024b | 4096b |
|--------|------|------|-------|-------|
| add    | 0.96 | 0.85 | 0.65  | 0.59  |
| sub    | 0.94 | 0.89 | 0.75  | 0.73  |
| mul    | 0.78 | 0.51 | 0.33  | 0.27  |
| divmod | 0.92 | 0.82 | 0.80  | 0.80  |
| cmp    | 0.86 | 0.67 | 0.59  | 0.53  |

First crossings **below 1.00 vs num-bigint** with u64: **add beats it at every tier**, **mul at
256b (0.82) and parity at 1024b**, **cmp at parity across the board**. Encapsulation kept this a
non-breaking change; the differential oracle, canonical form, and byte-identical encodings stayed green.

## Where the gaps remain (optimization order)

1. **to_decimal_string — 125–960×.** The single largest gap, newly visible. It divides by 10 one digit
   at a time through the general (bit-at-a-time) divmod. Chunking (divide by 10¹⁹, the largest power of
   ten in a u64, for 19 digits per step) plus a fast divmod collapses this.
2. **gcd — up to 115×.** Euclid over `divmod`, so it inherits divmod's cost. Fixing divmod fixes most;
   a binary/Lehmer gcd closes the rest.
3. **divmod — 7–20×.** Still bit-at-a-time long division (O(bits·limbs)). Knuth Algorithm D
   (word-at-a-time) is the target, and it unblocks gcd and to_decimal_string too.
4. **mul at 4096b (1.24×) and sub (1.0–1.8×).** Karatsuba above a crossover for the large mul; a direct
   signed subtract (dropping the `add(neg())` allocation) for sub.

## Roadmap

Next, in gap order, each landing with its scoreboard delta and the num-bigint differential oracle green:
Knuth Algorithm D divmod → chunked to_decimal_string → Karatsuba mul → direct signed sub →
binary/Lehmer gcd. num-bigint stays both the correctness oracle and the perf yardstick.
