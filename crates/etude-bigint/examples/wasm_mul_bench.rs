// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Standalone timing harness for the three schoolbook-multiply strategies, so the wasm question can be
//! measured directly (criterion does not build for wasm — its jemalloc allocator and file I/O do not
//! apply there). Each strategy is implemented inline here so ONE binary times all three on whatever
//! target it is built for:
//!
//! 1. **u64 limbs, `u128` widening** — `u64 * u64 -> u128`. Native on 64-bit; on wasm the widening
//!    lowers to a `__multi3` libcall (emulated 128-bit).
//! 2. **u64 limbs, synthesized widening** — `u64 * u64 -> (hi, lo)` from four native `u32 * u32 -> u64`
//!    partials. All native `i64` ops on wasm (what the crate ships on 32-bit targets).
//! 3. **u32 limbs, `u64` widening** — `u32 * u32 -> u64`, native everywhere but twice the limbs.
//!
//! Run natively with `cargo run --release --example wasm_mul_bench`, and on wasm with
//! `cargo build --release --example wasm_mul_bench --target wasm32-wasip1` then
//! `wasmtime run target/wasm32-wasip1/release/examples/wasm_mul_bench.wasm`.

use std::hint::black_box;
use std::time::Instant;

/// xorshift64 for deterministic operands.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

// ── Strategy 1: u64 limbs, u128 widening ─────────────────────────────────────────────────────────
#[inline]
fn wide_u128(a: u64, b: u64) -> (u64, u64) {
    let p = (a as u128) * (b as u128);
    ((p >> 64) as u64, p as u64)
}

// ── Strategy 2: u64 limbs, synthesized u32-half widening ─────────────────────────────────────────
#[inline]
fn wide_synth(a: u64, b: u64) -> (u64, u64) {
    let (a_lo, a_hi) = (a & 0xffff_ffff, a >> 32);
    let (b_lo, b_hi) = (b & 0xffff_ffff, b >> 32);
    let ll = a_lo * b_lo;
    let lh = a_lo * b_hi;
    let hl = a_hi * b_lo;
    let hh = a_hi * b_hi;
    let mut lo = ll;
    let mut hi = hh;
    let (s, c1) = lo.overflowing_add(lh << 32);
    lo = s;
    hi += (lh >> 32) + c1 as u64;
    let (s, c2) = lo.overflowing_add(hl << 32);
    lo = s;
    hi += (hl >> 32) + c2 as u64;
    (hi, lo)
}

/// Schoolbook multiply over u64 limbs using a supplied widening step.
fn mul_u64(a: &[u64], b: &[u64], wide: impl Fn(u64, u64) -> (u64, u64)) -> Vec<u64> {
    let mut out = vec![0u64; a.len() + b.len()];
    for (i, &av) in a.iter().enumerate() {
        let mut carry = 0u64;
        for (j, &bv) in b.iter().enumerate() {
            let (hi, lo) = wide(av, bv);
            let (s1, c1) = out[i + j].overflowing_add(lo);
            let (s2, c2) = s1.overflowing_add(carry);
            out[i + j] = s2;
            carry = hi + c1 as u64 + c2 as u64;
        }
        let mut k = i + b.len();
        while carry != 0 {
            let (s, c) = out[k].overflowing_add(carry);
            out[k] = s;
            carry = c as u64;
            k += 1;
        }
    }
    out
}

// ── Strategy 3: u32 limbs, native u64 widening ───────────────────────────────────────────────────
fn mul_u32(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = vec![0u32; a.len() + b.len()];
    for (i, &av) in a.iter().enumerate() {
        let mut carry = 0u64;
        for (j, &bv) in b.iter().enumerate() {
            let cur = out[i + j] as u64 + (av as u64) * (bv as u64) + carry;
            out[i + j] = cur as u32;
            carry = cur >> 32;
        }
        let mut k = i + b.len();
        while carry != 0 {
            let cur = out[k] as u64 + carry;
            out[k] = cur as u32;
            carry = cur >> 32;
            k += 1;
        }
    }
    out
}

/// Time `iters` calls of `f`, returning nanoseconds per call.
fn time<T>(iters: u32, mut f: impl FnMut() -> T) -> f64 {
    // Warm up.
    for _ in 0..(iters / 10).max(1) {
        black_box(f());
    }
    let start = Instant::now();
    for _ in 0..iters {
        black_box(f());
    }
    start.elapsed().as_nanos() as f64 / iters as f64
}

fn main() {
    // Limb counts (u64) per tier; the u32 variant uses twice as many limbs for the same magnitude.
    let tiers: &[(&str, usize)] = &[("512b", 8), ("2048b", 32), ("8192b", 128)];
    println!("strategy                      tier       ns/mul");
    for &(label, n64) in tiers {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ n64 as u64);
        let a64: Vec<u64> = (0..n64).map(|_| rng.next()).collect();
        let b64: Vec<u64> = (0..n64).map(|_| rng.next()).collect();
        let a32: Vec<u32> = a64
            .iter()
            .flat_map(|&x| [x as u32, (x >> 32) as u32])
            .collect();
        let b32: Vec<u32> = b64
            .iter()
            .flat_map(|&x| [x as u32, (x >> 32) as u32])
            .collect();

        // Fewer iterations for the larger tiers to keep the run short.
        let iters = (2_000_000 / (n64 * n64)).max(200) as u32;
        let t1 = time(iters, || mul_u64(&a64, &b64, wide_u128));
        let t2 = time(iters, || mul_u64(&a64, &b64, wide_synth));
        let t3 = time(iters, || mul_u32(&a32, &b32));
        println!("u64-limbs u128-widening       {label:6}  {t1:10.1}");
        println!("u64-limbs synth-widening      {label:6}  {t2:10.1}");
        println!("u32-limbs u64-widening        {label:6}  {t3:10.1}");
    }
}
