# `etude-decimal` benchmark scoreboard

Head-to-head against [`bigdecimal`](https://crates.io/crates/bigdecimal) — the arbitrary-precision
decimal crate that is also `etude-decimal`'s correctness oracle (`src/tests.rs`). `bigdecimal` is the
yardstick; the goal is to beat it (ratio `< 1.00`).

Run it yourself:

```sh
cargo bench -p etude-decimal --bench arith
```

## Methodology

- **Harness:** `criterion` (300 ms warm-up, 1500 ms measurement per cell), `tikv-jemalloc` as the global
  allocator so limb-`Vec` allocation is production-representative.
- **Operands:** built through the public API only — an `nbytes`-wide non-negative coefficient from
  `etude_bigint::Big`'s sign-magnitude byte parser (top bit forced set for an exact width), placed at a
  fixed scale (`exp = -12`) so every value carries a fractional part. The value-equal `bigdecimal`
  operand is built from the identical coefficient and exponent, so both crates measure the same inputs.
- **Arithmetic cells rotate over a batch of 16 operand pairs.** Canonicalization cost is parity-sensitive
  (a result not ending in zero normalizes in `O(1)`; one ending in zero divides), so a single pair would
  over- or under-state the true cost depending on that pair's luck. Rotating over a batch whose results
  span the parity mix makes the median representative; the rotation is a `Cell` index bump, identical on
  both sides, so the ratio stays fair.
- **Tiers** are named by the coefficient bit width (`64b` = 8 bytes … `4096b` = 512 bytes).
- **`div_round`** targets a fixed working precision of 34 significant digits with `HalfEven`, matched on
  both sides (ours via `div_round(_, 34, HalfEven)`, `bigdecimal` via
  `(a / b).with_precision_round(34, HalfEven)`), so both do the same rounding work.
- **Ratio = etude median / bigdecimal median.** `< 1.00` means `etude-decimal` is faster. Medians below
  are from a single run on an aarch64 host (`Linux 6.12 aarch64`); treat them as directional.

## Scoreboard (2026-09-19)

### `add` — exact aligned sum

| tier | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 64b   | 77.77 ns  | 70.16 ns  | 1.11 |
| 256b  | 88.71 ns  | 109.08 ns | 0.81 |
| 1024b | 122.12 ns | 133.57 ns | 0.91 |
| 2048b | 241.55 ns | 169.86 ns | 1.42 |
| 4096b | 569.46 ns | 223.72 ns | 2.55 |

### `sub` — exact aligned difference

| tier | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 64b   | 80.98 ns  | 64.87 ns  | 1.25 |
| 256b  | 98.88 ns  | 77.57 ns  | 1.27 |
| 1024b | 131.38 ns | 99.97 ns  | 1.31 |
| 2048b | 243.74 ns | 129.19 ns | 1.89 |
| 4096b | 460.00 ns | 156.54 ns | 2.94 |

### `mul` — exact product

| tier | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 64b   | 63.77 ns  | 36.29 ns  | 1.76 |
| 256b  | 122.23 ns | 65.07 ns  | 1.88 |
| 1024b | 580.61 ns | 402.67 ns | 1.44 |
| 2048b | 1.695 µs  | 1.511 µs  | 1.12 |
| 4096b | 5.743 µs  | 5.010 µs  | 1.15 |

### `div_round` — rounded quotient, 34 sig-digits, HalfEven — we win every tier

| tier | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 64b   | 701.25 ns | 8.095 µs  | **0.09** |
| 256b  | 1.443 µs  | 12.069 µs | **0.12** |
| 1024b | 9.138 µs  | 17.138 µs | **0.53** |
| 2048b | 19.23 µs  | 25.11 µs  | **0.77** |
| 4096b | 47.43 µs  | 59.78 µs  | **0.79** |

### `cmp` — three-way ordering (equal-exponent `Big::cmp` fast path)

| tier | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 64b   | 24.71 ns | 5.63 ns | 4.39 |
| 256b  | 24.18 ns | 5.62 ns | 4.30 |
| 1024b | 25.91 ns | 5.61 ns | 4.62 |
| 2048b | 30.77 ns | 5.67 ns | 5.43 |
| 4096b | 39.43 ns | 5.78 ns | 6.82 |

### `to_string` — render to a decimal literal

| tier | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 64b   | 191.59 ns | 146.06 ns | 1.31 |
| 256b  | 463.31 ns | 281.83 ns | 1.64 |
| 1024b | 3.814 µs  | 2.324 µs  | 1.64 |
| 2048b | 8.707 µs  | 8.976 µs  | 0.97 |
| 4096b | 22.31 µs  | 21.09 µs  | 1.06 |

### `from_str` — parse a decimal literal

| tier | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 64b   | 407.81 ns | 145.64 ns | 2.80 |
| 256b  | 1.319 µs  | 375.84 ns | 3.51 |
| 1024b | 5.480 µs  | 1.111 µs  | 4.93 |
| 2048b | 12.32 µs  | 2.482 µs  | 4.96 |
| 4096b | 31.90 µs  | 6.449 µs  | 4.95 |

## Reading the board

- **`div_round` is a clean sweep** — 10× faster at small magnitudes, still ahead at 4096b. Our exact
  scale-and-divide reaches the answer in one `divmod` at the chosen precision, where `bigdecimal` divides
  to its own default precision first and then re-rounds.
- **`add`/`sub`/`mul` closed most of their gap** after `normalize` was rewritten onto `etude-bigint`'s
  scalar primitives (`is_even`, `rem_u64`, `divmod_u64` — #133). Canonicalization used to divide the
  coefficient by ten on every result; now an odd result is rejected in `O(1)` and only a result ending in
  zero is divided. `add` now wins at 256b–1024b; the residual at large tiers is the underlying `Big`
  add/mul cost, which is `etude-bigint`'s to shave. `sub` trails `add` because `sub` is `add(neg)` and the
  `neg` allocates a negated coefficient clone — a direct `sub` would remove that (next).
- **`cmp`** is a flat ~24–40 ns via the equal-exponent coefficient compare; the residual ~4–7× over
  `bigdecimal`'s cached-length ~6 ns is the two `abs()` clones plus the unequal-exponent path (which still
  renders decimal strings). A magnitude-only `Big` compare and an exact `decimal_digit_count()` (requested
  from `etude-bigint`) would close it.
- **`from_str`/`to_string`** trail on the base-10 ↔ binary conversion; both improve once we adopt
  `etude-bigint`'s chunked base-`10^k` digit emit/absorb (the absorb helper is a requested follow-up).

## Next optimizations (ranked by scoreboard leverage)

1. **Direct `sub`** — subtract coefficients in place instead of `add(neg)`, dropping the `neg` clone that
   makes `sub` trail `add`.
2. **`from_str` via chunked base-`10^k` absorb** — the biggest remaining multi-tier gap (≈5×); consume
   `etude-bigint`'s forthcoming base-`10^k` build helper instead of the 18-digit-chunk `i64` loop.
3. **`cmp` residual** — a magnitude-only `Big` compare (no `abs()` clone) and an exact
   `decimal_digit_count()` so the unequal-exponent path drops its decimal-string render.
4. **`to_string`** — adopt a chunked base-`10^k` digit emitter to match `bigdecimal` at the small tiers.
