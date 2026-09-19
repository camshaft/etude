// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Head-to-head arithmetic benchmarks: `etude_bigint::Big` vs the `num-bigint` reference, across
//! magnitude tiers. num-bigint is the optimization target — the scoreboard measures the gap each
//! optimization is meant to close, and guards the cheap tiers against regression.
//!
//! Operands are built from a deterministic xorshift RNG (through the public byte-parsing API only, so
//! the bench never touches `Big`'s private internals) — a rebuild measures the same inputs. Tiers are
//! named by bit width (`limbs * 32`). Run with `cargo bench -p etude-bigint`.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use etude_bigint::Big;
use num_bigint::BigInt;
use std::hint::black_box;
use std::time::Duration;

// Match the host allocator; allocation of limb `Vec`s dominates bignum work, so this keeps the
// numbers production-representative (as in the bytevec benches).
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// (label, magnitude byte count). 8 bytes ≈ 64 bits.
const TIERS: &[(&str, usize)] = &[("64b", 8), ("256b", 32), ("1024b", 128), ("4096b", 512)];
/// Tiers for the super-linear ops (gcd, decimal render) whose bit-at-a-time cost is steep — capped so
/// a single sample stays sub-millisecond at the baseline.
const HEAVY_TIERS: &[(&str, usize)] = &[("64b", 8), ("256b", 32), ("1024b", 128)];

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
    /// `nbytes` random magnitude bytes with the top byte forced nonzero (so the width is exact).
    fn mag_bytes(&mut self, nbytes: usize) -> Vec<u8> {
        let mut b: Vec<u8> = (0..nbytes).map(|_| self.byte()).collect();
        if let Some(top) = b.last_mut() {
            *top |= 0x80;
        }
        b
    }
    /// A non-negative `Big` of exactly `nbytes` magnitude bytes, built via the public parser.
    fn big(&mut self, nbytes: usize) -> Big {
        let mag = self.mag_bytes(nbytes);
        let mut sm = Vec::with_capacity(1 + mag.len());
        sm.push(0); // sign byte: non-negative
        sm.extend_from_slice(&mag);
        Big::from_sign_magnitude_bytes(&sm)
    }
}

/// The `num-bigint` value equal to `b` (via the public two's-complement encoding).
fn to_num(b: &Big) -> BigInt {
    BigInt::from_signed_bytes_le(&b.to_le_twos_complement_bytes())
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

/// Bench a binary op on both implementations across `tiers`, with same-width operands.
fn binop(
    c: &mut Criterion,
    name: &str,
    tiers: &[(&str, usize)],
    ours: impl Fn(&Big, &Big) -> Big,
    theirs: impl Fn(&BigInt, &BigInt) -> BigInt,
) {
    let mut g = group(c, name);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for &(label, nbytes) in tiers {
        let (a, b) = (rng.big(nbytes), rng.big(nbytes));
        let (na, nb) = (to_num(&a), to_num(&b));
        g.bench_with_input(BenchmarkId::new("etude", label), &(a, b), |bch, (a, b)| {
            bch.iter(|| black_box(ours(black_box(a), black_box(b))))
        });
        g.bench_with_input(
            BenchmarkId::new("num-bigint", label),
            &(na, nb),
            |bch, (a, b)| bch.iter(|| black_box(theirs(black_box(a), black_box(b)))),
        );
    }
    g.finish();
}

fn bench_add(c: &mut Criterion) {
    binop(c, "add", TIERS, |a, b| a.add(b), |a, b| a + b);
}
fn bench_sub(c: &mut Criterion) {
    binop(c, "sub", TIERS, |a, b| a.sub(b), |a, b| a - b);
}
fn bench_mul(c: &mut Criterion) {
    binop(c, "mul", TIERS, |a, b| a.mul(b), |a, b| a * b);
}

/// divmod: dividend twice the divisor's width (the classic `2n / n` shape).
fn bench_divmod(c: &mut Criterion) {
    let mut g = group(c, "divmod");
    let mut rng = Rng(0xdead_beef_1234_5678);
    for &(label, nbytes) in TIERS {
        let a = rng.big(nbytes * 2);
        let b = rng.big(nbytes);
        let (na, nb) = (to_num(&a), to_num(&b));
        g.bench_with_input(BenchmarkId::new("etude", label), &(a, b), |bch, (a, b)| {
            bch.iter(|| black_box(a.divmod(black_box(b))))
        });
        g.bench_with_input(
            BenchmarkId::new("num-bigint", label),
            &(na, nb),
            |bch, (a, b)| bch.iter(|| black_box((a / b, a % b))),
        );
    }
    g.finish();
}

fn bench_gcd(c: &mut Criterion) {
    use num_integer::Integer;
    binop(c, "gcd", HEAVY_TIERS, |a, b| a.gcd(b), |a, b| a.gcd(b));
}

fn bench_cmp(c: &mut Criterion) {
    let mut g = group(c, "cmp");
    let mut rng = Rng(0x0102_0304_0506_0708);
    for &(label, nbytes) in TIERS {
        // Two values equal except in the low byte, so cmp scans the whole magnitude (worst case).
        let mut sm = Vec::with_capacity(1 + nbytes);
        sm.push(0);
        sm.extend_from_slice(&rng.mag_bytes(nbytes));
        let a = Big::from_sign_magnitude_bytes(&sm);
        sm[1] ^= 1;
        let b = Big::from_sign_magnitude_bytes(&sm);
        let (na, nb) = (to_num(&a), to_num(&b));
        g.bench_with_input(BenchmarkId::new("etude", label), &(a, b), |bch, (a, b)| {
            bch.iter(|| black_box(a.cmp(black_box(b))))
        });
        g.bench_with_input(
            BenchmarkId::new("num-bigint", label),
            &(na, nb),
            |bch, (a, b)| bch.iter(|| black_box(a.cmp(black_box(b)))),
        );
    }
    g.finish();
}

fn bench_to_decimal(c: &mut Criterion) {
    let mut g = group(c, "to_decimal_string");
    let mut rng = Rng(0xcafe_f00d_0011_2233);
    // Full TIERS (through 4096b) — the recursive divide-and-conquer split's advantage over the linear
    // chunk method widens with magnitude, so the largest tier is where it shows most.
    for &(label, nbytes) in TIERS {
        let a = rng.big(nbytes);
        let na = to_num(&a);
        g.bench_with_input(BenchmarkId::new("etude", label), &a, |bch, a| {
            bch.iter(|| black_box(a.to_decimal_string()))
        });
        g.bench_with_input(BenchmarkId::new("num-bigint", label), &na, |bch, a| {
            bch.iter(|| black_box(a.to_string()))
        });
    }
    g.finish();
}

/// Canonical sign-magnitude byte encode + decode round-trip (the map-key path).
fn bench_sign_magnitude_roundtrip(c: &mut Criterion) {
    let mut g = group(c, "sign_magnitude_roundtrip");
    let mut rng = Rng(0x5555_aaaa_3333_cccc);
    for &(label, nbytes) in TIERS {
        let a = rng.big(nbytes);
        g.bench_with_input(BenchmarkId::new("etude", label), &a, |bch, a| {
            bch.iter(|| {
                black_box(Big::from_sign_magnitude_bytes(
                    &black_box(a).to_sign_magnitude_bytes(),
                ))
            })
        });
    }
    g.finish();
}

/// `clone` across tiers — the small-value inline magnitude repr makes a ≤128-bit clone allocation-free
/// (a stack copy), where a `Vec`-backed magnitude heap-allocates. Clone-heavy callers (e.g. rational
/// reciprocal) live or die on this at the small tiers.
fn bench_clone(c: &mut Criterion) {
    let mut g = group(c, "clone");
    let mut rng = Rng(0x0102_0304_0506_0708);
    for &(label, nbytes) in TIERS {
        let a = rng.big(nbytes);
        let na = to_num(&a);
        g.bench_with_input(BenchmarkId::new("etude", label), &a, |bch, a| {
            bch.iter(|| black_box(black_box(a).clone()))
        });
        g.bench_with_input(BenchmarkId::new("num-bigint", label), &na, |bch, a| {
            bch.iter(|| black_box(black_box(a).clone()))
        });
    }
    g.finish();
}

/// `from_i64` — constructing a small value. Inline (no heap allocation) vs num-bigint's `Vec`-backed
/// `BigInt::from`.
fn bench_from_i64(c: &mut Criterion) {
    let mut g = group(c, "from_i64");
    g.bench_function("etude", |bch| {
        bch.iter(|| black_box(Big::from_i64(black_box(-1_234_567_890_123_456_789))))
    });
    g.bench_function("num-bigint", |bch| {
        bch.iter(|| black_box(BigInt::from(black_box(-1_234_567_890_123_456_789i64))))
    });
    g.finish();
}

criterion_group!(
    benches,
    bench_add,
    bench_sub,
    bench_mul,
    bench_divmod,
    bench_gcd,
    bench_cmp,
    bench_to_decimal,
    bench_sign_magnitude_roundtrip,
    bench_clone,
    bench_from_i64
);
criterion_main!(benches);
