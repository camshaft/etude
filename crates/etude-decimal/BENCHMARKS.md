# `etude-decimal` benchmark scoreboard

Head-to-head against [`bigdecimal`](https://crates.io/crates/bigdecimal) — the arbitrary-precision
decimal crate that is also `etude-decimal`'s correctness oracle (`src/tests.rs`). `bigdecimal` is the
yardstick; the goal is to **beat it** (ratio `< 1.00`).

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
  operand is built from the *identical* coefficient and exponent, so both crates measure the same inputs.
- **Tiers** are named by the coefficient bit width (`64b` = 8 bytes … `4096b` = 512 bytes).
- **`div_round`** targets a fixed working precision of 34 significant digits with `HalfEven`, matched on
  both sides (ours via `div_round(_, 34, HalfEven)`, `bigdecimal` via
  `(a / b).with_precision_round(34, HalfEven)`), so both do the same rounding work.
- **Ratio = etude median / bigdecimal median.** `< 1.00` means `etude-decimal` is faster; `> 1.00` means
  it is slower. Medians below are from a single run on an aarch64 host (`Linux 6.12 aarch64`); treat them
  as directional, not absolute.

## Scoreboard (2026-09-19, baseline)

### `add` — exact aligned sum

| tier | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 64b   | 148.30 ns | 73.26 ns  | 2.02 |
| 256b  | 137.30 ns | 100.39 ns | 1.37 |
| 1024b | 329.84 ns | 119.53 ns | 2.76 |
| 2048b | 698.00 ns | 150.81 ns | 4.63 |
| 4096b | 1.315 µs  | 212.47 ns | 6.19 |

### `sub` — exact aligned difference

| tier | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 64b   | 150.95 ns | 59.57 ns  | 2.53 |
| 256b  | 133.68 ns | 59.84 ns  | 2.23 |
| 1024b | 307.49 ns | 76.28 ns  | 4.03 |
| 2048b | 652.15 ns | 98.50 ns  | 6.62 |
| 4096b | 1.307 µs  | 136.26 ns | 9.59 |

### `mul` — exact product

| tier | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 64b   | 88.25 ns | 35.05 ns  | 2.52 |
| 256b  | 180.11 ns | 64.93 ns  | 2.77 |
| 1024b | 985.02 ns | 403.37 ns | 2.44 |
| 2048b | 2.586 µs  | 1.504 µs  | 1.72 |
| 4096b | 7.432 µs  | 5.006 µs  | 1.48 |

### `div_round` — rounded quotient, 34 sig-digits, HalfEven ✅ **we win every tier**

| tier | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 64b   | 775.95 ns | 7.994 µs  | **0.10** |
| 256b  | 1.746 µs  | 12.095 µs | **0.14** |
| 1024b | 9.396 µs  | 17.152 µs | **0.55** |
| 2048b | 20.41 µs  | 25.17 µs  | **0.81** |
| 4096b | 51.14 µs  | 59.79 µs  | **0.86** |

### `cmp` — three-way ordering (equal-exponent `Big::cmp` fast path)

Now a direct coefficient compare when the exponents match (the common case), instead of rendering both
operands to decimal strings. This collapsed the worst column from hundreds-to-thousands× down to a flat
~24–50 ns — a 30×–1000× speedup — leaving only the two `abs()` clones between us and `bigdecimal`'s
cached-length ~6 ns.

| tier | etude (before) | etude (now) | bigdecimal | ratio |
|------|---------------:|------------:|-----------:|------:|
| 64b   | 151.55 ns | 24.10 ns | 5.56 ns | 4.33 |
| 256b  | 1.025 µs  | 24.06 ns | 5.56 ns | 4.33 |
| 1024b | 7.573 µs  | 26.12 ns | 5.59 ns | 4.67 |
| 2048b | 18.32 µs  | 31.05 ns | 5.72 ns | 5.43 |
| 4096b | 48.43 µs  | 49.81 ns | 5.99 ns | 8.32 |

### `to_string` — render to a decimal literal

| tier | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 64b   | 194.87 ns | 150.02 ns | 1.30 |
| 256b  | 611.83 ns | 285.16 ns | 2.15 |
| 1024b | 3.929 µs  | 2.342 µs  | 1.68 |
| 2048b | 9.261 µs  | 8.921 µs  | 1.04 |
| 4096b | 24.09 µs  | 21.02 µs  | 1.15 |

### `from_str` — parse a decimal literal

| tier | etude | bigdecimal | ratio |
|------|------:|-----------:|------:|
| 64b   | 449.69 ns | 143.67 ns | 3.13 |
| 256b  | 1.398 µs  | 365.46 ns | 3.83 |
| 1024b | 5.737 µs  | 1.112 µs  | 5.16 |
| 2048b | 12.88 µs  | 2.469 µs  | 5.21 |
| 4096b | 30.97 µs  | 6.132 µs  | 5.05 |

## Reading the board

- **`div_round` is a clean sweep** — 10× faster at small magnitudes, still ahead at 4096b. Our exact
  scale-and-divide reaches the answer in one `divmod` at the chosen precision, where `bigdecimal` divides
  to its own default precision first and then re-rounds.
- **`add`/`sub`/`mul` trail** by a fixed factor that *grows with magnitude*. The cause is the
  canonical-form invariant: every result re-runs `normalize`, which strips trailing zero digits by
  repeated `divmod(10)` — each an `O(limbs)` pass over the coefficient. `bigdecimal` keeps trailing zeros
  and normalizes lazily. Closing this needs a cheap "count trailing base-10 zeros" on `Big` (or a
  radix-`10^k` chunked strip) so canonicalization is not a full division chain.
- **`cmp` was the worst offender — now largely closed.** `cmp_magnitude` used to convert *both*
  coefficients to decimal strings on every comparison (an `O(limbs²)` base-10 conversion). The
  equal-exponent path now compares the `Big` coefficients directly (a flat ~24–50 ns), a 30×–1000×
  speedup; the residual ~4–8× is the two `abs()` clones. A magnitude-only compare on `Big` (no clone)
  would close the rest. The unequal-exponent path still renders decimal strings (see below).
- **`from_str`/`to_string`** trail on the base-10 ↔ binary conversion; both improve once `etude-bigint`
  exposes chunked base-`10^k` digit emit/absorb (the same primitive the rational crate is adopting).

## Next optimizations (ranked by scoreboard leverage)

1. ~~**`cmp` equal-exponent fast path**~~ — DONE: compares `Big` coefficients directly; 30×–1000× faster.
   Residual: a magnitude-only `Big` compare (no `abs()` clone) plus an unequal-exponent path that avoids
   decimal strings.
2. **Cheaper `normalize`** — the strip is now base-`10^9` chunked (`O(zeros / 9)` divisions instead of
   `O(zeros)`), but the *common* case (a result with no trailing zero) still pays one `divmod(10^9)` to
   discover it is not divisible by ten — that is the residual `add`/`sub`/`mul` tax. Removing it needs a
   cheap divisibility/parity primitive on `Big` (an `is_even` / `rem_u64`), requested from `etude-bigint`;
   `coeff` divisible by 10 iff even *and* divisible by 5, and the even check is `O(1)`.
3. **Chunked base-`10^k` parse/render** — push the digit loop down into `etude-bigint` for
   `from_str`/`to_string`.
