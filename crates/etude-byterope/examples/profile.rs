// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Focused profiling driver: run one workload in a tight loop so `perf` gets dense samples.
//! Usage: `profile <push_back|pop_front|advance>`; build with `--release`.

use bytes::Bytes;
use etude_byterope::ByteRope;
use std::hint::black_box;

#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn chunk(seed: u8) -> Bytes {
    Bytes::from(vec![seed; 1400])
}

fn main() {
    let which = std::env::args().nth(1).unwrap_or_else(|| "push_back".into());
    let n = 1000usize;
    let iters = 200_000usize;
    match which.as_str() {
        "push_back" => {
            // pre-make chunks; clone (Arc bump) in the loop so we measure push_back, not alloc+memset
            let chunks: Vec<Bytes> = (0..n).map(|i| chunk(i as u8)).collect();
            for _ in 0..iters {
                let mut r = ByteRope::new();
                for c in &chunks {
                    r.push_back(c.clone());
                }
                black_box(&r);
            }
        }
        "pop_front" => {
            let src: ByteRope = (0..n).map(|i| chunk(i as u8)).collect();
            for _ in 0..iters {
                let mut r = src.clone();
                while r.pop_front().is_some() {}
                black_box(&r);
            }
        }
        "advance" => {
            // build a FRESH, uniquely-owned rope each iter (clone the chunk handles, not the rope)
            // so FBIP fires — the realistic case where you own the rope you drain
            let chunks: Vec<Bytes> = (0..n).map(|i| chunk(i as u8)).collect();
            for _ in 0..iters {
                let mut r: ByteRope = chunks.iter().cloned().collect();
                while !r.is_empty() {
                    let _ = r.advance(700);
                }
                black_box(&r);
            }
        }
        "iterate" => {
            let src: ByteRope = (0..n).map(|i| chunk(i as u8)).collect();
            let mut acc = 0usize;
            for _ in 0..(iters * 4) {
                acc = acc.wrapping_add(src.chunks().map(|c| c.len()).sum::<usize>());
            }
            black_box(acc);
        }
        _ => panic!("unknown workload"),
    }
}
