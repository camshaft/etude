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

Subtracts coefficients directly (`Big::sub`) after aligning exponents, rather than adding a negated clone
of the operand — no intermediate negated value is allocated.

| tier | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 64b   | 72.61 ns  | 56.73 ns  | 1.28 |
| 256b  | 88.64 ns  | 69.95 ns  | 1.27 |
| 1024b | 114.02 ns | 95.16 ns  | 1.20 |
| 2048b | 222.58 ns | 122.51 ns | 1.82 |
| 4096b | 438.18 ns | 151.13 ns | 2.90 |

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

### `from_str` — parse a decimal literal — we win at scale

The coefficient is now assembled by grouping the parsed digits into 19-digit base-`10¹⁹` limbs and calling
`etude_bigint::Big::from_base_10_pow_k_limbs` (a wide `u64` multiply-add Horner pass, no per-chunk `Big`
allocated), rather than the old 18-digit `i64`-chunk `mul`/`add` loop. This turned the worst multi-tier
gap into a win from 1024b up.

| tier | etude (before) | etude (now) | bigdecimal | ratio |
|------|---------------:|------------:|-----------:|------:|
| 64b   | 407.81 ns | 190.87 ns | 146.30 ns | 1.30 |
| 256b  | 1.319 µs  | 390.37 ns | 371.97 ns | 1.05 |
| 1024b | 5.480 µs  | 1.065 µs  | 1.112 µs  | **0.96** |
| 2048b | 12.32 µs  | 2.060 µs  | 2.465 µs  | **0.84** |
| 4096b | 31.90 µs  | 4.820 µs  | 6.127 µs  | **0.79** |

### `neg` — negate

| tier | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 64b   | 13.75 ns | 5.72 ns  | 2.40 |
| 256b  | 13.85 ns | 12.99 ns | 1.07 |
| 1024b | 14.90 ns | 14.46 ns | 1.03 |
| 2048b | 17.50 ns | 17.17 ns | 1.02 |
| 4096b | 23.08 ns | 23.08 ns | 1.00 |

### `abs` — absolute value

| tier | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 64b   | 13.60 ns | 6.45 ns  | 2.11 |
| 256b  | 13.71 ns | 13.21 ns | 1.04 |
| 1024b | 14.83 ns | 14.60 ns | 1.02 |
| 2048b | 17.16 ns | 17.04 ns | 1.01 |
| 4096b | 23.23 ns | 24.15 ns | 0.96 |

### `to_f64` — correctly-rounded conversion to `f64` — we win at scale

Operand scaled into `f64` range (value in `(0, 1)`) so the full big-int-ratio path runs rather than
short-circuiting on overflow. Our direct method scales far better than `bigdecimal`'s.

| tier | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 64b   | 643.28 ns | 189.99 ns | 3.39 |
| 256b  | 720.89 ns | 446.43 ns | 1.61 |
| 1024b | 1.250 µs  | 3.680 µs  | **0.34** |
| 2048b | 1.871 µs  | 13.73 µs  | **0.14** |
| 4096b | 4.020 µs  | 53.81 µs  | **0.075** |

Small values (`|coefficient| < 2^53`, `|exp| ≤ 22`) — the common decimal-literal case — take a fast path:
one correctly-rounded IEEE multiply/divide of two exactly-representable `f64`s (the coefficient and an
exact power of ten). The `TIERS` sweep above cannot reach it (those coefficients exceed the `f64`
mantissa), so it has its own cell — `12345678.9012345` (15 significant digits):

| case | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 15 digits | 5.94 ns | 137.70 ns | **0.043** |

### `div_exact` — exact division by a terminating divisor (etude only)

`bigdecimal` has no exact-terminating division — its `/` is precision-bounded (that comparison is the
`div_round` group above). This tracks our exact `div`'s cost across tiers (divisor `2^10`).

| tier | etude |
|------|------:|
| 64b   | 1.213 µs |
| 256b  | 3.252 µs |
| 1024b | 16.04 µs |
| 2048b | 43.79 µs |
| 4096b | 140.2 µs |

### `new` — construction + canonicalization (etude only)

`BigDecimal::new` does not canonicalize (it keeps trailing zeros), so there is no equivalent to compare
against; this tracks the canonicalization cost (coefficient carries ≥6 trailing zeros to exercise the
strip). `iter_batched` clones the input in unmeasured setup so only `new` is timed.

| tier | etude |
|------|------:|
| 64b   | 146.0 ns |
| 256b  | 186.9 ns |
| 1024b | 371.5 ns |
| 2048b | 623.3 ns |
| 4096b | 1.125 µs |

### Coverage

Every real-work public function is benchmarked (above). Simple O(1) getters — `coefficient`, `exponent`,
`is_zero`, `is_negative`, `is_integer` — are intentionally not benchmarked (per the operator directive
that simple getters need no bench). `parse`/`parse_prefix` share their work with `from_str`.

## Reading the board

- **`div_round` is a clean sweep** — 10× faster at small magnitudes, still ahead at 4096b. Our exact
  scale-and-divide reaches the answer in one `divmod` at the chosen precision, where `bigdecimal` divides
  to its own default precision first and then re-rounds.
- **`add`/`sub`/`mul` closed most of their gap** after `normalize` was rewritten onto `etude-bigint`'s
  scalar primitives (`is_even`, `rem_u64`, `divmod_u64` — #133). Canonicalization used to divide the
  coefficient by ten on every result; now an odd result is rejected in `O(1)` and only a result ending in
  zero is divided. `add` now wins at 256b–1024b; the residual at large tiers is the underlying `Big`
  add/mul cost, which is `etude-bigint`'s to shave. `sub` now subtracts coefficients directly (`Big::sub`)
  instead of `add(neg)`, so no negated clone is allocated — it edges below `add` at large tiers because a
  difference of same-magnitude operands can cancel to a shorter result.
- **`cmp`** is a flat ~24–40 ns via the equal-exponent coefficient compare; the residual ~4–7× over
  `bigdecimal`'s cached-length ~6 ns is the two `abs()` clones plus the unequal-exponent path (which still
  renders decimal strings). A magnitude-only `Big` compare and an exact `decimal_digit_count()` (requested
  from `etude-bigint`) would close it.
- **`from_str` now wins from 1024b up** after adopting `etude-bigint`'s `from_base_10_pow_k_limbs`
  (base-`10¹⁹` Horner absorb). **`to_string`** still trails on the reverse conversion; it improves once we
  adopt `Big::write_decimal` for the coefficient digits (a requested follow-up already landed on the
  bigint side).
- **`to_f64` wins at scale and for small values** — from 1024b up the direct big-int-ratio method is
  3×–13× faster than `bigdecimal` (its conversion grows super-linearly), and small decimal literals take a
  single-IEEE-op fast path that is ~23× faster (5.9 ns vs 138 ns). The mid-range large tiers (64b/256b,
  full-width coefficients that miss the fast path but are small enough for bigdecimal's) still trail.
- **`neg`/`abs`** sit at parity from 256b up (both are an `O(limbs)` clone plus a sign flip); only at 64b
  does `bigdecimal`'s small-value representation edge ahead.

## Next optimizations (ranked by scoreboard leverage)

1. **`to_string`** — adopt `Big::write_decimal` for the coefficient digits (drop the intermediate
   `to_decimal_string` allocation) to match `bigdecimal` at the small tiers.
2. **`cmp` residual** — a magnitude-only `Big` compare (no `abs()` clone) and an exact
   `decimal_digit_count()` so the unequal-exponent path drops its decimal-string render.
3. **`from_str` / `to_string` small-value fast paths** — mirror the `to_f64` small-value win for the
   common decimal-literal case (a coefficient that fits a `u64`).
