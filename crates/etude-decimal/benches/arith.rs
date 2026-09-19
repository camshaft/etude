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
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use etude_bigint::Big;
use etude_decimal::{Decimal, RoundingMode};
use num_bigint::BigInt;
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

/// Bench a binary op on both implementations across `TIERS`, with same-width operands.
fn binop(
    c: &mut Criterion,
    name: &str,
    ours: impl Fn(&Decimal, &Decimal) -> Decimal,
    theirs: impl Fn(&BigDecimal, &BigDecimal) -> BigDecimal,
) {
    let mut g = group(c, name);
    for &(label, nbytes) in TIERS {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (nbytes as u64));
        let a = rng.dec(nbytes);
        let b = rng.dec(nbytes);
        let (ra, rb) = (to_ref(&a), to_ref(&b));
        g.bench_with_input(BenchmarkId::new("etude", label), &(&a, &b), |be, (a, b)| {
            be.iter(|| black_box(ours(black_box(a), black_box(b))))
        });
        g.bench_with_input(
            BenchmarkId::new("bigdecimal", label),
            &(&ra, &rb),
            |be, (a, b)| be.iter(|| black_box(theirs(black_box(a), black_box(b)))),
        );
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
}

criterion_group!(benches, bench);
criterion_main!(benches);
