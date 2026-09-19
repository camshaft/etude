// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Head-to-head rational-arithmetic benchmarks: `etude_rational::Rational` vs the `num-rational`
//! reference (`BigRational`), across component-magnitude tiers. num-rational is BOTH the correctness
//! oracle (see `src/tests.rs`) and the optimization target — the scoreboard measures the gap each
//! optimization is meant to close and guards against regression. The north star is to BEAT it
//! (ratios below 1.00).
//!
//! Every operand is built through the PUBLIC API only — large `Big` components come from
//! `etude_bigint`'s byte parser, never `Rational`'s private fields — so a rebuild measures the same
//! inputs. Tiers are named by the component bit width (`bytes * 8`). Rational arithmetic is dominated
//! by a handful of `Big` multiplies plus a gcd-normalize, so the `normalize` (via `new`) cell is the
//! most load-bearing one. Run with `cargo bench -p etude-rational`.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use etude_bigint::Big;
use etude_rational::Rational;
use num_bigint::BigInt;
use num_rational::BigRational;
use std::hint::black_box;
use std::time::Duration;

// Match the host allocator; allocation of limb `Vec`s dominates bignum work, so this keeps the
// numbers production-representative (as in the bigint/bytevec benches).
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// (label, per-component magnitude byte count). Rational ops cost ~2-3x the underlying bignum op, so the
/// tiers are capped a notch below the bigint bench to keep a single sample sub-millisecond at baseline.
const TIERS: &[(&str, usize)] = &[
    ("64b", 8),
    ("256b", 32),
    ("1024b", 128),
    ("2048b", 256),
    ("4096b", 512),
];

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
    /// A positive, nonzero `i64` of ~48 bits (fits `i64`; products of two fit `i128`).
    fn i64_small(&mut self) -> i64 {
        let mut v = 0i64;
        for _ in 0..6 {
            v = (v << 8) | i64::from(self.byte());
        }
        v | 1 // nonzero
    }
    /// A `Rational` with `nbytes`-wide numerator and denominator (normalized on construction).
    fn rat(&mut self, nbytes: usize) -> Rational {
        let num = self.big(nbytes);
        let den = self.big(nbytes);
        Rational::new(num, den).expect("denominator is nonzero")
    }
    /// Two rationals that share the SAME `nbytes`-wide denominator (to exercise the equal-denominator
    /// add/sub fast path). Numerators are RANDOM and made coprime to `den` at setup time (nudged up until
    /// `gcd == 1`) so `new` leaves the denominator intact — the resulting `num1 + num2` has a
    /// representative (random-cost) reduction, NOT an artificially trivial one.
    fn rat_pair_eqden(&mut self, nbytes: usize) -> (Rational, Rational) {
        let one = Big::from_i64(1);
        let den = self.big(nbytes);
        let coprime_num = |rng: &mut Self| {
            let mut n = rng.big(nbytes);
            while n.gcd(&den) != one {
                n = n.add(&one);
            }
            n
        };
        let num1 = coprime_num(self);
        let num2 = coprime_num(self);
        let a = Rational::new(num1, den.clone()).expect("nonzero denominator");
        let b = Rational::new(num2, den).expect("nonzero denominator");
        assert_eq!(a.denom(), b.denom(), "eqden pair must share a denominator");
        (a, b)
    }
}

/// The `num-bigint` value equal to `b` (via the public two's-complement encoding).
fn to_bigint(b: &Big) -> BigInt {
    BigInt::from_signed_bytes_le(&b.to_le_twos_complement_bytes())
}

/// The `num-rational` value equal to `r` (built from its canonical components; `new` re-reduces, a
/// no-op here since `r` is already in lowest terms).
fn to_ref(r: &Rational) -> BigRational {
    BigRational::new(to_bigint(r.numer()), to_bigint(r.denom()))
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
    ours: impl Fn(&Rational, &Rational) -> Rational,
    theirs: impl Fn(&BigRational, &BigRational) -> BigRational,
) {
    let mut g = group(c, name);
    for &(label, nbytes) in TIERS {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (nbytes as u64));
        let a = rng.rat(nbytes);
        let b = rng.rat(nbytes);
        let (ra, rb) = (to_ref(&a), to_ref(&b));
        g.bench_with_input(BenchmarkId::new("etude", label), &(&a, &b), |be, (a, b)| {
            be.iter(|| black_box(ours(black_box(a), black_box(b))))
        });
        g.bench_with_input(
            BenchmarkId::new("num-rational", label),
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
    binop(c, "div", |a, b| a.div(b).expect("nonzero"), |a, b| a / b);

    // Small (i64-fitting) operands: the common real-world case (e.g. `3/10`), and the one the byte-width
    // tiers UNDER-represent (their top bit is set, so a 64b coefficient exceeds i64). Exercises the native
    // i128 fast path (no Big allocation) vs num-rational, which always uses BigInt.
    {
        let mut g = group(c, "mul_i64");
        let mut rng = Rng(0x1122_3344_5566_7788);
        let a = Rational::from_ratio_i64(rng.i64_small(), rng.i64_small()).expect("nonzero den");
        let b = Rational::from_ratio_i64(rng.i64_small(), rng.i64_small()).expect("nonzero den");
        let (ra, rb) = (to_ref(&a), to_ref(&b));
        g.bench_with_input(BenchmarkId::new("etude", "48b"), &(&a, &b), |be, (a, b)| {
            be.iter(|| black_box(a.mul(black_box(b))))
        });
        g.bench_with_input(
            BenchmarkId::new("num-rational", "48b"),
            &(&ra, &rb),
            |be, (a, b)| be.iter(|| black_box(*a * *b)),
        );
        g.finish();

        let mut g = group(c, "div_i64");
        g.bench_with_input(BenchmarkId::new("etude", "48b"), &(&a, &b), |be, (a, b)| {
            be.iter(|| black_box(a.div(black_box(b)).expect("nonzero")))
        });
        g.bench_with_input(
            BenchmarkId::new("num-rational", "48b"),
            &(&ra, &rb),
            |be, (a, b)| be.iter(|| black_box(*a / *b)),
        );
        g.finish();

        let mut g = group(c, "add_i64");
        g.bench_with_input(BenchmarkId::new("etude", "48b"), &(&a, &b), |be, (a, b)| {
            be.iter(|| black_box(a.add(black_box(b))))
        });
        g.bench_with_input(
            BenchmarkId::new("num-rational", "48b"),
            &(&ra, &rb),
            |be, (a, b)| be.iter(|| black_box(*a + *b)),
        );
        g.finish();

        let mut g = group(c, "sub_i64");
        g.bench_with_input(BenchmarkId::new("etude", "48b"), &(&a, &b), |be, (a, b)| {
            be.iter(|| black_box(a.sub(black_box(b))))
        });
        g.bench_with_input(
            BenchmarkId::new("num-rational", "48b"),
            &(&ra, &rb),
            |be, (a, b)| be.iter(|| black_box(*a - *b)),
        );
        g.finish();

        let mut g = group(c, "cmp_i64");
        g.bench_with_input(BenchmarkId::new("etude", "48b"), &(&a, &b), |be, (a, b)| {
            be.iter(|| black_box(a.cmp(black_box(b))))
        });
        g.bench_with_input(
            BenchmarkId::new("num-rational", "48b"),
            &(&ra, &rb),
            |be, (a, b)| be.iter(|| black_box(a.cmp(black_box(b)))),
        );
        g.finish();

        // Small equal-denominator add: two i64-fitting rationals sharing a denominator. The native i128
        // fast path now covers this (it is tried before the Big equal-denominator path), so it stays
        // allocation-light on the arithmetic. `d` is prime, so any `0 < n < d` is coprime to it and
        // `from_ratio_i64` leaves the denominator intact — a genuine (non-trivial) shared-denominator sum.
        let d: i64 = 1_000_003;
        let ne = rng.i64_small().rem_euclid(d - 1) + 1;
        let nf = rng.i64_small().rem_euclid(d - 1) + 1;
        let e = Rational::from_ratio_i64(ne, d).expect("nonzero den");
        let f = Rational::from_ratio_i64(nf, d).expect("nonzero den");
        assert_eq!(
            e.denom(),
            f.denom(),
            "small eqden pair must share a denominator"
        );
        let (re, rf) = (to_ref(&e), to_ref(&f));
        let mut g = group(c, "add_eqden_i64");
        g.bench_with_input(BenchmarkId::new("etude", "48b"), &(&e, &f), |be, (e, f)| {
            be.iter(|| black_box(e.add(black_box(f))))
        });
        g.bench_with_input(
            BenchmarkId::new("num-rational", "48b"),
            &(&re, &rf),
            |be, (e, f)| be.iter(|| black_box(*e + *f)),
        );
        g.finish();
    }

    // Equal-denominator add/sub (a common real-workload pattern: accumulating fractions over a shared
    // denominator, or integer-valued rationals). Both implementations fast-path this, so it is a fair
    // fast-path-vs-fast-path measurement.
    {
        let mut g = group(c, "add_eqden");
        for &(label, nbytes) in TIERS {
            let mut rng = Rng(0xabcd_1234 ^ (nbytes as u64));
            let (a, b) = rng.rat_pair_eqden(nbytes);
            let (ra, rb) = (to_ref(&a), to_ref(&b));
            g.bench_with_input(BenchmarkId::new("etude", label), &(&a, &b), |be, (a, b)| {
                be.iter(|| black_box(a.add(black_box(b))))
            });
            g.bench_with_input(
                BenchmarkId::new("num-rational", label),
                &(&ra, &rb),
                |be, (a, b)| be.iter(|| black_box(*a + *b)),
            );
        }
        g.finish();
    }

    // Comparison: exact cross-multiplication vs num-rational's cmp.
    {
        let mut g = group(c, "cmp");
        for &(label, nbytes) in TIERS {
            let mut rng = Rng(0x1234_5678 ^ (nbytes as u64));
            let a = rng.rat(nbytes);
            let b = rng.rat(nbytes);
            let (ra, rb) = (to_ref(&a), to_ref(&b));
            g.bench_with_input(BenchmarkId::new("etude", label), &(&a, &b), |be, (a, b)| {
                be.iter(|| black_box(a.cmp(black_box(b))))
            });
            g.bench_with_input(
                BenchmarkId::new("num-rational", label),
                &(&ra, &rb),
                |be, (a, b)| be.iter(|| black_box(a.cmp(black_box(b)))),
            );
        }
        g.finish();
    }

    // Equal-denominator comparison: exercises the direct numerator-compare fast path (both
    // implementations fast-path this — a fair fast-path-vs-fast-path measurement).
    {
        let mut g = group(c, "cmp_eqden");
        for &(label, nbytes) in TIERS {
            let mut rng = Rng(0x5a5a_1357 ^ (nbytes as u64));
            let (a, b) = rng.rat_pair_eqden(nbytes);
            let (ra, rb) = (to_ref(&a), to_ref(&b));
            g.bench_with_input(BenchmarkId::new("etude", label), &(&a, &b), |be, (a, b)| {
                be.iter(|| black_box(a.cmp(black_box(b))))
            });
            g.bench_with_input(
                BenchmarkId::new("num-rational", label),
                &(&ra, &rb),
                |be, (a, b)| be.iter(|| black_box(a.cmp(black_box(b)))),
            );
        }
        g.finish();
    }

    // Reciprocal: den/num renormalized.
    {
        let mut g = group(c, "recip");
        for &(label, nbytes) in TIERS {
            let mut rng = Rng(0xdead_beef ^ (nbytes as u64));
            let a = rng.rat(nbytes);
            let ra = to_ref(&a);
            g.bench_with_input(BenchmarkId::new("etude", label), &a, |be, a| {
                be.iter(|| black_box(a.recip().expect("nonzero")))
            });
            g.bench_with_input(BenchmarkId::new("num-rational", label), &ra, |be, a| {
                be.iter(|| black_box(a.recip()))
            });
        }
        g.finish();
    }

    // Rendering to a decimal string: our alloc-lean Display (one String via Big::write_decimal) vs
    // num-rational's `to_string`. Now a win at small tiers after etude-bigint's single-limb to_decimal
    // fast path (#82); tracked to guard it.
    {
        let mut g = group(c, "to_string");
        for &(label, nbytes) in TIERS {
            let mut rng = Rng(0x0dec_1a15 ^ (nbytes as u64));
            let a = rng.rat(nbytes);
            let ra = to_ref(&a);
            g.bench_with_input(BenchmarkId::new("etude", label), &a, |be, a| {
                be.iter(|| black_box(a.to_decimal_string()))
            });
            g.bench_with_input(BenchmarkId::new("num-rational", label), &ra, |be, a| {
                be.iter(|| black_box(a.to_string()))
            });
        }
        g.finish();
    }

    // Normalize (via construction): the gcd-normalize hot path — the single most load-bearing cell,
    // since every arithmetic result renormalizes. Operands are raw (unreduced) num/den pairs.
    {
        let mut g = group(c, "normalize");
        for &(label, nbytes) in TIERS {
            let mut rng = Rng(0xcafe_f00d ^ (nbytes as u64));
            let num = rng.big(nbytes);
            let den = rng.big(nbytes);
            let (bn, bd) = (to_bigint(&num), to_bigint(&den));
            g.bench_with_input(
                BenchmarkId::new("etude", label),
                &(&num, &den),
                |be, (n, d)| {
                    be.iter(|| {
                        black_box(Rational::new((*n).clone(), (*d).clone()).expect("nonzero"))
                    })
                },
            );
            g.bench_with_input(
                BenchmarkId::new("num-rational", label),
                &(&bn, &bd),
                |be, (n, d)| be.iter(|| black_box(BigRational::new((*n).clone(), (*d).clone()))),
            );
        }
        g.finish();
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
