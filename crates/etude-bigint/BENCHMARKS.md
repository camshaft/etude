<!-- Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved. -->
<!-- SPDX-License-Identifier: Apache-2.0 -->

# etude-bigint benchmarks

Head-to-head against [`num-bigint`](https://crates.io/crates/num-bigint), the reference we optimize
toward. `benches/arith.rs` measures each operation at four magnitude tiers (named by bit width) with
the jemalloc allocator, deterministic operands, and criterion. Run it with:

```
cargo bench -p etude-bigint
```

Numbers below are medians from one `aarch64-linux` run and are **indicative, not authoritative** —
absolute times vary by machine; what matters is the **ratio to num-bigint** and its movement as
optimizations land. `ratio` is `etude / num-bigint`: `<1.00` = we are faster, `>1.00` = slower.

## Baseline — u32 limbs, schoolbook algorithms (etude#32 port + encapsulation)

| op                        | tier   | etude     | num-bigint | ratio |
|---------------------------|--------|-----------|------------|-------|
| add                       | 64b    | 19.8 ns   | 25.3 ns    | 0.78  |
| add                       | 256b   | 24.7 ns   | 64.8 ns    | 0.38  |
| add                       | 1024b  | 61.6 ns   | 81.6 ns    | 0.75  |
| add                       | 4096b  | 208 ns    | 169 ns     | 1.23  |
| sub                       | 64b    | 32.9 ns   | 17.2 ns    | 1.91  |
| sub                       | 256b   | 34.9 ns   | 30.4 ns    | 1.15  |
| sub                       | 1024b  | 66.5 ns   | 43.6 ns    | 1.53  |
| sub                       | 4096b  | 184 ns    | 94.6 ns    | 1.95  |
| mul                       | 64b    | 27.4 ns   | 19.1 ns    | 1.43  |
| mul                       | 256b   | 83.9 ns   | 52.3 ns    | 1.60  |
| mul                       | 1024b  | 1.17 µs   | 389 ns     | 3.01  |
| mul                       | 4096b  | 23.2 µs   | 4.99 µs    | 4.64  |
| divmod                    | 64b    | 825 ns    | 111 ns     | 7.45  |
| divmod                    | 256b   | 6.76 µs   | 391 ns     | 17.3  |
| divmod                    | 1024b  | 48.0 µs   | 2.07 µs    | 23.2  |
| divmod                    | 4096b  | 515 µs    | 20.4 µs    | 25.3  |
| gcd                       | 64b    | 9.16 µs   | 1.14 µs    | 8.04  |
| gcd                       | 256b   | 129 µs    | 4.76 µs    | 27.0  |
| gcd                       | 1024b  | 3.03 ms   | 23.4 µs    | 129.6 |
| cmp                       | 64b    | 3.05 ns   | 2.67 ns    | 1.14  |
| cmp                       | 256b   | 5.47 ns   | 3.52 ns    | 1.55  |
| cmp                       | 1024b  | 15.1 ns   | 8.70 ns    | 1.74  |
| cmp                       | 4096b  | 52.3 ns   | 27.4 ns    | 1.91  |
| sign_magnitude_roundtrip  | 64b    | 73.6 ns   | —          | —     |
| sign_magnitude_roundtrip  | 256b   | 148 ns    | —          | —     |
| sign_magnitude_roundtrip  | 1024b  | 330 ns    | —          | —     |
| sign_magnitude_roundtrip  | 4096b  | 727 ns    | —          | —     |

(divmod's dividend is twice the divisor's width — the `2n / n` shape. gcd and to_decimal_string are
capped at 1024b because their bit-at-a-time cost is steep. sign_magnitude_roundtrip is the canonical
map-key encode+decode; num-bigint has no matching operation.)

## Where the gaps are (optimization order)

1. **gcd — up to 130×, and it grows super-linearly.** gcd is Euclid over `divmod`, so it inherits
   divmod's bit-at-a-time cost multiplied over many steps. Fixing divmod fixes most of this; a
   binary/Lehmer gcd closes the rest.
2. **divmod — 7–25×, the single largest per-op gap.** The current algorithm is bit-at-a-time long
   division (O(bits·limbs)). Knuth Algorithm D (word-at-a-time) is the target.
3. **mul — 1.4–4.6×, gap grows with size.** Schoolbook O(n·m). Karatsuba above a crossover closes the
   large-tier gap; the small tiers are already close.
4. **sub — ~1.5–2×.** `sub` routes through `add(neg())`, allocating an extra magnitude; a direct
   signed subtract removes that.
5. **cmp — ~1.5–1.9×** and **add at 4096b (1.23×).** Both are limb-loop bound; a wider limb halves the
   trip count.

## Roadmap

- **Repr: base-2⁶⁴ u64 limbs (under evaluation).** Halving the limb count with u128 intermediate
  products/accumulators is expected to help every op above at once — this is why num-bigint uses a u64
  `BigDigit` on 64-bit targets. Encapsulation (private fields, stable API) landed first so this switch
  is a non-breaking internal change; the two byte encodings and the canonical-form invariant must stay
  byte-identical, guarded by the differential oracle.
- Then, in gap order: Knuth Algorithm D divmod → Karatsuba mul → direct signed sub → binary/Lehmer gcd.

Each optimization lands with its bench delta recorded here and the num-bigint differential oracle green.
