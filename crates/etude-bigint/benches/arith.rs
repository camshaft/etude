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

/// Quotient-only division (`2n / n` shape) — `div_exact` vs num-bigint's `/` (which also returns only
/// the quotient). `div_exact` skips the remainder `Vec` that `divmod` builds, so this also shows the
/// gap it closes vs our own `divmod`.
fn bench_div_exact(c: &mut Criterion) {
    let mut g = group(c, "div_exact");
    let mut rng = Rng(0xd1e0_eac7_0000_0001);
    for &(label, nbytes) in TIERS {
        let a = rng.big(nbytes * 2);
        let b = rng.big(nbytes);
        let (na, nb) = (to_num(&a), to_num(&b));
        g.bench_with_input(BenchmarkId::new("etude", label), &(a, b), |bch, (a, b)| {
            bch.iter(|| black_box(a.div_exact(black_box(b))))
        });
        g.bench_with_input(
            BenchmarkId::new("num-bigint", label),
            &(na, nb),
            |bch, (a, b)| bch.iter(|| black_box(a / b)),
        );
    }
    g.finish();
}

/// Division by a SMALL (single-limb) divisor — the `n / small` shape (a bignum over a scalar), which
/// takes the single-limb `div_rem_limb_inplace` path rather than Knuth. vs num-bigint's `/`.
fn bench_div_small(c: &mut Criterion) {
    let mut g = group(c, "div_small");
    let mut rng = Rng(0x5a11_d10e_0000_0001);
    let divisor = Big::from_i64(1_000_000_007); // a small (sub-limb) divisor, not a power of two
    let ndiv = to_num(&divisor);
    let d_u64 = 1_000_000_007u64;
    for &(label, nbytes) in TIERS {
        let a = rng.big(nbytes);
        let na = to_num(&a);
        // Native-scalar divmod: quotient + `u64` remainder, no `Big` divisor or `Big` remainder allocated.
        g.bench_with_input(BenchmarkId::new("etude-u64", label), &a, |bch, a| {
            bch.iter(|| black_box(a.divmod_u64(black_box(d_u64))))
        });
        let d = divisor.clone();
        g.bench_with_input(BenchmarkId::new("etude", label), &(a, d), |bch, (a, d)| {
            bch.iter(|| black_box(a.divmod(black_box(d))))
        });
        let nd = ndiv.clone();
        g.bench_with_input(
            BenchmarkId::new("num-bigint", label),
            &(na, nd),
            |bch, (a, d)| bch.iter(|| black_box((a / d, a % d))),
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

/// Bench a unary op on both implementations across `tiers`, with same-width operands.
fn unary(
    c: &mut Criterion,
    name: &str,
    tiers: &[(&str, usize)],
    ours: impl Fn(&Big) -> Big,
    theirs: impl Fn(&BigInt) -> BigInt,
) {
    let mut g = group(c, name);
    let mut rng = Rng(0x243f_6a88_85a3_08d3);
    for &(label, nbytes) in tiers {
        let a = rng.big(nbytes);
        let na = to_num(&a);
        g.bench_with_input(BenchmarkId::new("etude", label), &a, |bch, a| {
            bch.iter(|| black_box(ours(black_box(a))))
        });
        g.bench_with_input(BenchmarkId::new("num-bigint", label), &na, |bch, a| {
            bch.iter(|| black_box(theirs(black_box(a))))
        });
    }
    g.finish();
}

/// `neg` — sign flip with a magnitude clone. At 64b it is clone-bound (the one-limb `Vec` allocation),
/// like `clone`/`from_i64`; the inline small-value repr would close it (etude-rational sees the same at
/// its `neg` cell).
fn bench_neg(c: &mut Criterion) {
    unary(c, "neg", TIERS, |a| a.neg(), |a| -a);
}

/// `abs` — like `neg`, a magnitude clone with the sign forced non-negative; 64b is clone-bound.
fn bench_abs(c: &mut Criterion) {
    use num_traits::Signed;
    unary(c, "abs", TIERS, |a| a.abs(), |a| a.abs());
}

/// `bit_len` — the value's bit width (position of the top set bit). O(1): the top limb plus a leading-
/// zero count. num-bigint's counterpart is `BigInt::bits`.
fn bench_bit_len(c: &mut Criterion) {
    let mut g = group(c, "bit_len");
    let mut rng = Rng(0xb7e1_5162_8aed_2a6a);
    for &(label, nbytes) in TIERS {
        let a = rng.big(nbytes);
        let na = to_num(&a);
        g.bench_with_input(BenchmarkId::new("etude", label), &a, |bch, a| {
            bch.iter(|| black_box(black_box(a).bit_len()))
        });
        g.bench_with_input(BenchmarkId::new("num-bigint", label), &na, |bch, a| {
            bch.iter(|| black_box(black_box(a).bits()))
        });
    }
    g.finish();
}

/// `rem_u64` — remainder by a single native-limb divisor (the reciprocal single-limb scan, no `Big`
/// remainder allocated). Differential against num-bigint's `%` with a one-limb `BigInt` divisor.
fn bench_rem_u64(c: &mut Criterion) {
    let mut g = group(c, "rem_u64");
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let d: u64 = 0x8000_0000_0000_002d; // odd, top bit set (exercises the reciprocal path, no normalize shift)
    let nd = BigInt::from(d);
    for &(label, nbytes) in TIERS {
        let a = rng.big(nbytes);
        let na = to_num(&a);
        g.bench_with_input(BenchmarkId::new("etude", label), &a, |bch, a| {
            bch.iter(|| black_box(black_box(a).rem_u64(black_box(d))))
        });
        g.bench_with_input(
            BenchmarkId::new("num-bigint", label),
            &(na, nd.clone()),
            |bch, (a, d)| bch.iter(|| black_box(black_box(a) % black_box(d))),
        );
    }
    g.finish();
}

/// `to_i64_checked` — read a small value back into a native `i64` (or `None` on overflow). Differential
/// against num-bigint's `ToPrimitive::to_i64`. Benched at a value that fits (the read path runs to
/// completion) and at a value that overflows (early reject).
fn bench_to_i64_checked(c: &mut Criterion) {
    use num_traits::ToPrimitive;
    let mut g = group(c, "to_i64_checked");
    let fits = Big::from_i64(-1_234_567_890_123_456_789);
    let nfits = to_num(&fits);
    let mut rng = Rng(0xc0ff_ee00_1234_5678);
    let wide = rng.big(32); // 256b — well past i64, so both reject early
    let nwide = to_num(&wide);
    g.bench_with_input(BenchmarkId::new("etude", "fits"), &fits, |bch, a| {
        bch.iter(|| black_box(black_box(a).to_i64_checked()))
    });
    g.bench_with_input(BenchmarkId::new("num-bigint", "fits"), &nfits, |bch, a| {
        bch.iter(|| black_box(black_box(a).to_i64()))
    });
    g.bench_with_input(BenchmarkId::new("etude", "overflow"), &wide, |bch, a| {
        bch.iter(|| black_box(black_box(a).to_i64_checked()))
    });
    g.bench_with_input(
        BenchmarkId::new("num-bigint", "overflow"),
        &nwide,
        |bch, a| bch.iter(|| black_box(black_box(a).to_i64())),
    );
    g.finish();
}

/// `is_even` — parity from the low bit of the low limb (O(1)). Differential against num-integer's
/// `Integer::is_even`. etude-decimal's normalize leans on this to short-circuit ~half its `divmod(10)`s.
fn bench_is_even(c: &mut Criterion) {
    use num_integer::Integer;
    let mut g = group(c, "is_even");
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    for &(label, nbytes) in TIERS {
        let a = rng.big(nbytes);
        let na = to_num(&a);
        g.bench_with_input(BenchmarkId::new("etude", label), &a, |bch, a| {
            bch.iter(|| black_box(black_box(a).is_even()))
        });
        g.bench_with_input(BenchmarkId::new("num-bigint", label), &na, |bch, a| {
            bch.iter(|| black_box(Integer::is_even(black_box(a))))
        });
    }
    g.finish();
}

/// Two's-complement little-endian byte encode + decode round-trip (the serde/wire path). Differential
/// against num-bigint's `to_signed_bytes_le` + `from_signed_bytes_le`.
fn bench_twos_complement_roundtrip(c: &mut Criterion) {
    let mut g = group(c, "twos_complement_roundtrip");
    let mut rng = Rng(0x1319_8a2e_0370_7344);
    for &(label, nbytes) in TIERS {
        let a = rng.big(nbytes);
        let na = to_num(&a);
        g.bench_with_input(BenchmarkId::new("etude", label), &a, |bch, a| {
            bch.iter(|| {
                black_box(Big::from_le_twos_complement_bytes(
                    &black_box(a).to_le_twos_complement_bytes(),
                ))
            })
        });
        g.bench_with_input(BenchmarkId::new("num-bigint", label), &na, |bch, a| {
            bch.iter(|| {
                black_box(BigInt::from_signed_bytes_le(
                    &black_box(a).to_signed_bytes_le(),
                ))
            })
        });
    }
    g.finish();
}

/// `write_decimal` into a reused, pre-grown sink — isolates the digit-emission cost from the fresh
/// allocation `to_decimal_string` pays each call (the `Display`-into-existing-buffer path). etude-only:
/// num-bigint has no sink-writing decimal emitter (its `Display` allocates, and is benched under
/// `to_decimal_string`).
fn bench_write_decimal(c: &mut Criterion) {
    let mut g = group(c, "write_decimal");
    let mut rng = Rng(0x8979_fb1b_d130_5ba3);
    for &(label, nbytes) in TIERS {
        let a = rng.big(nbytes);
        let mut sink = String::with_capacity(2 * nbytes + 8);
        g.bench_with_input(BenchmarkId::new("etude", label), &a, |bch, a| {
            bch.iter(|| {
                sink.clear();
                black_box(black_box(a).write_decimal(&mut sink)).unwrap();
                black_box(sink.len())
            })
        });
    }
    g.finish();
}

/// The sign-magnitude byte codecs that have no num-bigint counterpart (etude's canonical map-key /
/// small-scalar forms): the byte-slice comparator and the scalar `i64`/`i128` readers and encoder.
/// Absolute timings (no differential column) — these are etude's own wire helpers.
fn bench_sign_magnitude_codecs(c: &mut Criterion) {
    let mut g = group(c, "sign_magnitude_codecs");
    let mut rng = Rng(0x4528_21e6_38d0_1377);

    // cmp_sign_magnitude_bytes across tiers: two values equal except in the low magnitude byte, so the
    // comparator scans the full width (worst case).
    for &(label, nbytes) in TIERS {
        let mut sm = Vec::with_capacity(1 + nbytes);
        sm.push(0);
        sm.extend_from_slice(&rng.mag_bytes(nbytes));
        let a = sm.clone();
        sm[1] ^= 1;
        let b = sm;
        g.bench_with_input(
            BenchmarkId::new("cmp_bytes", label),
            &(a, b),
            |bch, (a, b)| {
                bch.iter(|| black_box(Big::cmp_sign_magnitude_bytes(black_box(a), black_box(b))))
            },
        );
    }

    // Scalar `i64`/`i128` readers + the `i128` encoder, on canonical sign-magnitude bytes.
    let i64_bytes = Big::from_i64(-1_234_567_890_123_456_789).to_sign_magnitude_bytes();
    g.bench_with_input(
        BenchmarkId::new("i64_from_bytes", "scalar"),
        &i64_bytes,
        |bch, b| bch.iter(|| black_box(Big::i64_checked_from_sign_magnitude_bytes(black_box(b)))),
    );
    let i128_v: i128 = -123_456_789_012_345_678_901_234_567_890;
    let mut i128_buf = [0u8; 17];
    let n = Big::i128_to_sign_magnitude_bytes_into(i128_v, &mut i128_buf).unwrap();
    let i128_bytes = i128_buf[..n].to_vec();
    g.bench_with_input(
        BenchmarkId::new("i128_from_bytes", "scalar"),
        &i128_bytes,
        |bch, b| bch.iter(|| black_box(Big::i128_from_sign_magnitude_bytes(black_box(b)))),
    );
    g.bench_function("i128_to_bytes/scalar", |bch| {
        let mut buf = [0u8; 17];
        bch.iter(|| {
            black_box(Big::i128_to_sign_magnitude_bytes_into(
                black_box(i128_v),
                &mut buf,
            ))
        })
    });
    g.finish();
}

/// The alloc-free `to_sign_magnitude_bytes_into` encoder (writes into a caller buffer), across tiers.
/// etude-only — the paired `from_sign_magnitude_bytes` decode is benched in `sign_magnitude_roundtrip`.
fn bench_to_sign_magnitude_into(c: &mut Criterion) {
    let mut g = group(c, "to_sign_magnitude_into");
    let mut rng = Rng(0xbe54_66cf_34e9_0c6c);
    for &(label, nbytes) in TIERS {
        let a = rng.big(nbytes);
        let mut buf = vec![0u8; nbytes + 2];
        g.bench_with_input(BenchmarkId::new("etude", label), &a, |bch, a| {
            bch.iter(|| black_box(black_box(a).to_sign_magnitude_bytes_into(&mut buf)))
        });
    }
    g.finish();
}

/// The O(1) predicates and constructors that have no meaningful reference counterpart to race, benched
/// at a single representative width to confirm they stay constant-time: `zero`, `is_zero`,
/// `is_negative`, `is_odd`, `byte_len`. (`is_even` and `bit_len` get their own differential groups.)
fn bench_o1_accessors(c: &mut Criterion) {
    let mut g = group(c, "o1_accessors");
    let mut rng = Rng(0xc97c_50dd_3f84_d5b5);
    let a = rng.big(128); // 1024b: a wide value, so any O(n) mistake would show
    g.bench_function("zero", |bch| bch.iter(|| black_box(Big::zero())));
    g.bench_with_input(BenchmarkId::new("is_zero", "1024b"), &a, |bch, a| {
        bch.iter(|| black_box(black_box(a).is_zero()))
    });
    g.bench_with_input(BenchmarkId::new("is_negative", "1024b"), &a, |bch, a| {
        bch.iter(|| black_box(black_box(a).is_negative()))
    });
    g.bench_with_input(BenchmarkId::new("is_odd", "1024b"), &a, |bch, a| {
        bch.iter(|| black_box(black_box(a).is_odd()))
    });
    g.bench_with_input(BenchmarkId::new("byte_len", "1024b"), &a, |bch, a| {
        bch.iter(|| black_box(black_box(a).byte_len()))
    });
    g.finish();
}

/// Build a `Big` from base-`10^19` decimal chunks (etude-decimal's `from_str` path — it has already
/// grouped the coefficient digits into 19-digit chunks) vs num-bigint parsing the equivalent decimal
/// string. The etude side consumes the chunks directly, with no re-stringifying; num-bigint's side also
/// scans the string characters, so part of any gap is that scan.
fn bench_from_base_10_pow_k(c: &mut Criterion) {
    use std::str::FromStr;
    const K: u32 = 19;
    let mut g = group(c, "from_base_10_pow_k");
    let mut rng = Rng(0x2b7e_1516_28ae_d2a6);
    for &(label, nbytes) in TIERS {
        let a = rng.big(nbytes);
        let s = to_num(&a).to_string(); // non-negative operand → plain decimal, no sign
        // Chunk the decimal string into base-`10^19` limbs, most-significant first (the leading chunk is
        // shorter than 19 unless the length divides evenly).
        let mut limbs: Vec<u64> = Vec::new();
        let first = s.len() % 19;
        if first != 0 {
            limbs.push(s[..first].parse().unwrap());
        }
        let mut i = first;
        while i < s.len() {
            limbs.push(s[i..i + 19].parse().unwrap());
            i += 19;
        }
        g.bench_with_input(BenchmarkId::new("etude", label), &limbs, |bch, limbs| {
            bch.iter(|| black_box(Big::from_base_10_pow_k_limbs(K, black_box(limbs))))
        });
        g.bench_with_input(BenchmarkId::new("num-bigint", label), &s, |bch, s| {
            bch.iter(|| black_box(BigInt::from_str(black_box(s))))
        });
    }
    g.finish();
}

/// `decimal_digit_count` vs the render-and-measure baseline it replaces (rendering the value to decimal
/// just to read the length — what a decimal's adjusted-exponent compare did before this helper). Both
/// sides on etude, so the comparison is like-for-like: the estimate-and-threshold count vs the full
/// base conversion.
fn bench_decimal_digit_count(c: &mut Criterion) {
    let mut g = group(c, "decimal_digit_count");
    let mut rng = Rng(0x3243_f6a8_885a_308d);
    for &(label, nbytes) in TIERS {
        let a = rng.big(nbytes);
        g.bench_with_input(BenchmarkId::new("etude", label), &a, |bch, a| {
            bch.iter(|| black_box(black_box(a).decimal_digit_count()))
        });
        g.bench_with_input(BenchmarkId::new("via-render", label), &a, |bch, a| {
            bch.iter(|| black_box(black_box(a).to_decimal_string().len()))
        });
    }
    g.finish();
}

/// `from_i128` / `to_i128_checked` — the limb-level i128 pair etude-decimal uses to widen its native
/// small-operand add/sub path to values in `[2^63, 2^64)` and beyond, without a `Big` byte round-trip.
/// A two-limb value (needs both limbs, so past the i64 fast path) vs num-bigint's `From<i128>` /
/// `ToPrimitive::to_i128`.
fn bench_i128(c: &mut Criterion) {
    use num_traits::ToPrimitive;
    const V: i128 = -0x0123_4567_89ab_cdef_7654_3210_fedc_ba98; // two full limbs
    let mut g = group(c, "from_i128");
    g.bench_function("etude", |bch| {
        bch.iter(|| black_box(Big::from_i128(black_box(V))))
    });
    g.bench_function("num-bigint", |bch| {
        bch.iter(|| black_box(BigInt::from(black_box(V))))
    });
    g.finish();

    let b = Big::from_i128(V);
    let nb = BigInt::from(V);
    let mut g = group(c, "to_i128_checked");
    g.bench_with_input(BenchmarkId::new("etude", "2-limb"), &b, |bch, b| {
        bch.iter(|| black_box(black_box(b).to_i128_checked()))
    });
    g.bench_with_input(BenchmarkId::new("num-bigint", "2-limb"), &nb, |bch, b| {
        bch.iter(|| black_box(black_box(b).to_i128()))
    });
    g.finish();
}

/// `last_decimal_digit` (limb-sum mod 10, no per-limb reciprocal) vs `rem_u64(10)` (the reciprocal
/// remainder scan it replaces) — the divisibility-by-10 check a decimal runs when canonicalizing every
/// even coefficient. Both on etude; the limb-sum should win at wide magnitudes.
fn bench_last_decimal_digit(c: &mut Criterion) {
    let mut g = group(c, "last_decimal_digit");
    let mut rng = Rng(0x0a10_0a10_0a10_0a10);
    for &(label, nbytes) in TIERS {
        let a = rng.big(nbytes);
        g.bench_with_input(
            BenchmarkId::new("last_decimal_digit", label),
            &a,
            |bch, a| bch.iter(|| black_box(black_box(a).last_decimal_digit())),
        );
        g.bench_with_input(BenchmarkId::new("rem_u64(10)", label), &a, |bch, a| {
            bch.iter(|| black_box(black_box(a).rem_u64(black_box(10))))
        });
    }
    g.finish();
}

criterion_group!(
    benches,
    bench_add,
    bench_sub,
    bench_mul,
    bench_divmod,
    bench_div_exact,
    bench_div_small,
    bench_gcd,
    bench_cmp,
    bench_to_decimal,
    bench_sign_magnitude_roundtrip,
    bench_clone,
    bench_from_i64,
    bench_neg,
    bench_abs,
    bench_bit_len,
    bench_rem_u64,
    bench_to_i64_checked,
    bench_is_even,
    bench_twos_complement_roundtrip,
    bench_write_decimal,
    bench_sign_magnitude_codecs,
    bench_to_sign_magnitude_into,
    bench_o1_accessors,
    bench_from_base_10_pow_k,
    bench_decimal_digit_count,
    bench_i128,
    bench_last_decimal_digit
);
criterion_main!(benches);
