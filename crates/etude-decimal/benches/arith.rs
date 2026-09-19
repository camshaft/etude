// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Head-to-head decimal-arithmetic benchmarks: `etude_decimal::Decimal` vs the `bigdecimal`
//! reference (`BigDecimal`), across coefficient-magnitude tiers. `bigdecimal` is both the
//! correctness oracle (see `src/tests.rs`) and the optimization target — the scoreboard measures the
//! gap each optimization is meant to close and guards against regression. The north star is to beat it
//! (ratios below 1.00).
//!
//! Every operand is built through the public API only — a large `Big` coefficient comes from
//! `etude_bigint`'s byte parser, then `Decimal::new` canonicalizes it — never a private field, so a
//! rebuild measures the same inputs. The equivalent `BigDecimal` is built from the identical
//! coefficient and exponent (value-equal by construction; the oracle proves it). Tiers are named by
//! the coefficient bit width (`bytes * 8`). Run with `cargo bench -p etude-decimal`.

use bigdecimal::{BigDecimal, RoundingMode as RefRound};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use etude_bigint::Big;
use etude_decimal::{Decimal, RoundingMode};
use num_bigint::BigInt;
use num_traits::ToPrimitive; // bigdecimal's to_f64, for the differential to_f64 bench
use std::hint::black_box;
use std::num::NonZeroU64;
use std::str::FromStr;
use std::time::Duration;

// Match the host allocator; allocation of limb `Vec`s dominates bignum work, so this keeps the
// numbers production-representative (as in the bigint/rational benches).
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// (label, coefficient magnitude byte count). Capped a notch below the bigint bench so a single sample
/// stays sub-millisecond at baseline.
const TIERS: &[(&str, usize)] = &[
    ("64b", 8),
    ("256b", 32),
    ("1024b", 128),
    ("2048b", 256),
    ("4096b", 512),
];

/// Significant digits that `div_round` (and the reference's `with_precision_round`) target — a fixed
/// working precision so both implementations do the same amount of rounding work per divide.
const DIV_PRECISION: u32 = 34;
/// The scale (base-10 point shift) applied to every operand so the values carry a fractional part
/// (exercising the exponent-align paths), without letting `mul`'s `e1 + e2` grow unbounded.
const SCALE_EXP: i64 = -12;

struct Rng(u64);
impl Rng {
    fn byte(&mut self) -> u8 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 24) as u8
    }
    /// A non-negative `Big` of exactly `nbytes` magnitude bytes (top bit forced set for an exact width),
    /// built via the public sign-magnitude parser.
    fn big(&mut self, nbytes: usize) -> Big {
        let mut sm: Vec<u8> = Vec::with_capacity(1 + nbytes);
        sm.push(0); // sign byte: non-negative
        for _ in 0..nbytes {
            sm.push(self.byte());
        }
        if let Some(top) = sm.last_mut() {
            *top |= 0x80;
        }
        Big::from_sign_magnitude_bytes(&sm)
    }
    /// A `Decimal` with an `nbytes`-wide coefficient at `SCALE_EXP` (canonicalized on construction).
    fn dec(&mut self, nbytes: usize) -> Decimal {
        Decimal::new(self.big(nbytes), SCALE_EXP)
    }
}

/// The `num-bigint` value equal to `b` (via the public two's-complement encoding).
fn to_bigint(b: &Big) -> BigInt {
    BigInt::from_signed_bytes_le(&b.to_le_twos_complement_bytes())
}

/// The `bigdecimal` value equal to `d`: `coefficient * 10^exponent`. `BigDecimal::new(bigint, scale)`
/// is `bigint * 10^-scale`, so `scale = -exponent`.
fn to_ref(d: &Decimal) -> BigDecimal {
    BigDecimal::new(to_bigint(d.coefficient()), -d.exponent())
}

fn group<'a>(
    c: &'a mut Criterion,
    name: &str,
) -> criterion::BenchmarkGroup<'a, criterion::measurement::WallTime> {
    let mut g = c.benchmark_group(name);
    g.warm_up_time(Duration::from_millis(300));
    g.measurement_time(Duration::from_millis(1500));
    g
}

/// How many operand pairs each arithmetic cell rotates through per measurement. Canonicalization cost is
/// now parity-sensitive (an odd result normalizes in `O(1)`; one ending in zero divides), so a single
/// pair would over- or under-state the true cost depending on that pair's luck. Rotating over a batch
/// whose results span the parity mix makes the median representative. The rotation is a `Cell` index
/// bump — identical overhead on both implementations, so the etude/bigdecimal ratio stays fair.
const BATCH: usize = 16;

/// Bench a binary op on both implementations across `TIERS`, with same-width operands, rotating over a
/// `BATCH` of pairs so the measured median reflects the result-parity mix (see [`BATCH`]).
fn binop(
    c: &mut Criterion,
    name: &str,
    ours: impl Fn(&Decimal, &Decimal) -> Decimal,
    theirs: impl Fn(&BigDecimal, &BigDecimal) -> BigDecimal,
) {
    use std::cell::Cell;
    let mut g = group(c, name);
    for &(label, nbytes) in TIERS {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (nbytes as u64));
        let ours_pairs: Vec<(Decimal, Decimal)> = (0..BATCH)
            .map(|_| (rng.dec(nbytes), rng.dec(nbytes)))
            .collect();
        let ref_pairs: Vec<(BigDecimal, BigDecimal)> = ours_pairs
            .iter()
            .map(|(a, b)| (to_ref(a), to_ref(b)))
            .collect();

        let oi = Cell::new(0usize);
        g.bench_function(BenchmarkId::new("etude", label), |be| {
            be.iter(|| {
                let k = oi.get();
                oi.set((k + 1) % BATCH);
                let (a, b) = &ours_pairs[k];
                black_box(ours(black_box(a), black_box(b)))
            })
        });
        let ri = Cell::new(0usize);
        g.bench_function(BenchmarkId::new("bigdecimal", label), |be| {
            be.iter(|| {
                let k = ri.get();
                ri.set((k + 1) % BATCH);
                let (a, b) = &ref_pairs[k];
                black_box(theirs(black_box(a), black_box(b)))
            })
        });
    }
    g.finish();
}

/// Bench a unary op on both implementations across `TIERS`, one operand per tier. `make` builds the
/// operand (so a bench can shape it — e.g. `to_f64` needs a value in `f64` range, `abs` a negative one).
fn unop<O, T>(
    c: &mut Criterion,
    name: &str,
    seed: u64,
    make: impl Fn(&mut Rng, usize) -> Decimal,
    ours: impl Fn(&Decimal) -> O,
    theirs: impl Fn(&BigDecimal) -> T,
) {
    let mut g = group(c, name);
    for &(label, nbytes) in TIERS {
        let mut rng = Rng(seed ^ (nbytes as u64));
        let a = make(&mut rng, nbytes);
        let ra = to_ref(&a);
        g.bench_with_input(BenchmarkId::new("etude", label), &a, |be, a| {
            be.iter(|| black_box(ours(black_box(a))))
        });
        g.bench_with_input(BenchmarkId::new("bigdecimal", label), &ra, |be, a| {
            be.iter(|| black_box(theirs(black_box(a))))
        });
    }
    g.finish();
}

fn bench(c: &mut Criterion) {
    binop(c, "add", |a, b| a.add(b), |a, b| a + b);
    binop(c, "sub", |a, b| a.sub(b), |a, b| a - b);
    binop(c, "mul", |a, b| a.mul(b), |a, b| a * b);

    // Rounded division to a fixed working precision (exact `div` returns None for non-terminating
    // quotients, so the fair head-to-head is the rounded operation both must support).
    {
        let prec = NonZeroU64::new(DIV_PRECISION as u64).expect("nonzero precision");
        let mut g = group(c, "div_round");
        for &(label, nbytes) in TIERS {
            let mut rng = Rng(0xd117_1de0 ^ (nbytes as u64));
            let a = rng.dec(nbytes);
            let b = rng.dec(nbytes);
            let (ra, rb) = (to_ref(&a), to_ref(&b));
            g.bench_with_input(BenchmarkId::new("etude", label), &(&a, &b), |be, (a, b)| {
                be.iter(|| {
                    black_box(
                        a.div_round(black_box(b), DIV_PRECISION, RoundingMode::HalfEven)
                            .expect("nonzero divisor"),
                    )
                })
            });
            g.bench_with_input(
                BenchmarkId::new("bigdecimal", label),
                &(&ra, &rb),
                |be, (a, b)| {
                    be.iter(|| black_box((*a / *b).with_precision_round(prec, RefRound::HalfEven)))
                },
            );
        }
        g.finish();
    }

    // Exact comparison (equal-exponent coefficient-compare fast path; unequal exponents fall back to an
    // aligned digit-string compare) vs bigdecimal's cmp.
    {
        let mut g = group(c, "cmp");
        for &(label, nbytes) in TIERS {
            let mut rng = Rng(0x1234_5678 ^ (nbytes as u64));
            let a = rng.dec(nbytes);
            let b = rng.dec(nbytes);
            let (ra, rb) = (to_ref(&a), to_ref(&b));
            g.bench_with_input(BenchmarkId::new("etude", label), &(&a, &b), |be, (a, b)| {
                be.iter(|| black_box(a.cmp(black_box(b))))
            });
            g.bench_with_input(
                BenchmarkId::new("bigdecimal", label),
                &(&ra, &rb),
                |be, (a, b)| be.iter(|| black_box(a.cmp(black_box(b)))),
            );
        }
        g.finish();
    }

    // Render to a decimal string: our alloc-lean Display sink vs bigdecimal's `to_string`.
    {
        let mut g = group(c, "to_string");
        for &(label, nbytes) in TIERS {
            let mut rng = Rng(0x0dec_1a15 ^ (nbytes as u64));
            let a = rng.dec(nbytes);
            let ra = to_ref(&a);
            g.bench_with_input(BenchmarkId::new("etude", label), &a, |be, a| {
                be.iter(|| black_box(a.to_string()))
            });
            g.bench_with_input(BenchmarkId::new("bigdecimal", label), &ra, |be, a| {
                be.iter(|| black_box(a.to_string()))
            });
        }
        g.finish();
    }

    // Parse a decimal literal from its canonical string: our streaming parser vs bigdecimal's FromStr.
    {
        let mut g = group(c, "from_str");
        for &(label, nbytes) in TIERS {
            let mut rng = Rng(0x9a12_b34c ^ (nbytes as u64));
            let s = rng.dec(nbytes).to_string();
            g.bench_with_input(BenchmarkId::new("etude", label), &s, |be, s| {
                be.iter(|| black_box(Decimal::from_str(black_box(s)).expect("valid literal")))
            });
            g.bench_with_input(BenchmarkId::new("bigdecimal", label), &s, |be, s| {
                be.iter(|| black_box(BigDecimal::from_str(black_box(s)).expect("valid literal")))
            });
        }
        g.finish();
    }

    // Negate: flip the coefficient's sign (an O(limbs) clone + sign bit). Both return an owned value, so
    // this is a fair clone-cost comparison.
    unop(c, "neg", 0x0f0f_a5a5, |r, n| r.dec(n), |a| a.neg(), |a| -a);

    // Absolute value: drop the sign (O(limbs) clone). Operand is negative so abs does its sign work.
    unop(
        c,
        "abs",
        0x5c5c_3210,
        |r, n| r.dec(n).neg(),
        |a| a.abs(),
        |a| a.abs(),
    );

    // Decimal → f64, correctly rounded. The operand is scaled into f64 range (value in (0, 1)) so the
    // conversion runs its full big-int-ratio path rather than short-circuiting to ±inf on overflow.
    unop(
        c,
        "to_f64",
        0xf64f_64f6,
        |r, n| {
            let coeff = r.big(n);
            let digits = coeff.to_decimal_string().len() as i64;
            Decimal::new(coeff, -digits)
        },
        |a| a.to_f64(),
        |a| a.to_f64(),
    );

    // Exact division by a terminating divisor (2^10, so the quotient always terminates). No bigdecimal
    // cell: bigdecimal has no exact-terminating division — its `/` is precision-bounded (that comparison
    // is the `div_round` group). This tracks our exact `div`'s cost across tiers for regression.
    {
        let mut g = group(c, "div_exact");
        let divisor = Decimal::from_i64(1024); // 2^10
        for &(label, nbytes) in TIERS {
            let mut rng = Rng(0x0d17_ec00 ^ (nbytes as u64));
            let a = rng.dec(nbytes);
            g.bench_with_input(BenchmarkId::new("etude", label), &a, |be, a| {
                be.iter(|| black_box(a.div(black_box(&divisor)).expect("terminates")))
            });
        }
        g.finish();
    }

    // Construction + canonicalization via `new`. No bigdecimal cell: `BigDecimal::new` does not
    // canonicalize (it keeps trailing zeros), so there is no equivalent to compare against; this tracks
    // the canonicalization cost. The coefficient carries ≥6 trailing zeros so `new` exercises the strip,
    // not just the odd-reject. `iter_batched` clones the input in (unmeasured) setup so only `new` is timed.
    {
        let mut g = group(c, "new");
        for &(label, nbytes) in TIERS {
            let mut rng = Rng(0x0c70_0000 ^ (nbytes as u64));
            let coeff = rng.big(nbytes).mul(&Big::from_i64(1_000_000));
            g.bench_with_input(BenchmarkId::new("etude", label), &coeff, |be, coeff| {
                be.iter_batched(
                    || coeff.clone(),
                    |c| black_box(Decimal::new(c, 0)),
                    BatchSize::SmallInput,
                )
            });
        }
        g.finish();
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
