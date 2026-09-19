// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Rope-shaped complexity scoreboard for [`StrRope`], each op measured at two shapes:
//! - **shallow** (4 chunks) — the common streaming case; and
//! - **deep** (1000 chunks) — past the promote threshold, exercising the underlying radix tree.
//!
//! The structural ops (`clone`, `split_off`, `slice`) should stay ~flat from shallow to deep (the rope
//! shares structure — O(1) clone, O(log n) split/slice), while the content ops (`chars`, `eq`, `hash`)
//! scale with total length. Run with `cargo bench -p etude-strrope`.

use bytes::Bytes;
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use etude_bytevec::ByteVec;
use etude_strrope::StrRope;
use std::hash::{Hash, Hasher};
use std::hint::black_box;

const SHALLOW: usize = 4;
const DEEP: usize = 1000;

/// Build a `StrRope` of `chunks` valid-UTF-8 leaves (varied content so eq/ord/hash do real work).
fn build(chunks: usize) -> StrRope {
    let mut bv = ByteVec::new();
    for i in 0..chunks {
        let s = format!("chunk-{i:05}-payload-abcdefghijklmnopqrstuvwxyz-0123456789");
        bv.push_back(Bytes::from(s.into_bytes()));
    }
    StrRope::from_utf8(bv).expect("valid utf8")
}

fn bench_ops(c: &mut Criterion) {
    let mut g = c.benchmark_group("strrope");
    for &n in &[SHALLOW, DEEP] {
        let rope = build(n);
        // ASCII payload => the midpoint is a char boundary, but resolve defensively.
        let mid = rope.len() / 2;
        let split_at = (0..=mid).rev().find(|&i| rope.is_char_boundary(i)).unwrap();

        g.bench_with_input(BenchmarkId::new("clone", n), &rope, |b, r| {
            b.iter(|| black_box(r.clone()));
        });
        g.bench_with_input(BenchmarkId::new("split_off", n), &rope, |b, r| {
            b.iter_batched(
                || r.clone(),
                |mut s| black_box(s.split_off(split_at)),
                BatchSize::SmallInput,
            );
        });
        g.bench_with_input(BenchmarkId::new("slice", n), &rope, |b, r| {
            b.iter(|| black_box(r.slice(split_at..)));
        });
        g.bench_with_input(BenchmarkId::new("chars_count", n), &rope, |b, r| {
            b.iter(|| black_box(r.chars().count()));
        });
        g.bench_with_input(BenchmarkId::new("eq", n), &rope, |b, r| {
            let other = r.clone();
            b.iter(|| black_box(*r == other));
        });
        g.bench_with_input(BenchmarkId::new("hash", n), &rope, |b, r| {
            b.iter(|| {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                r.hash(&mut h);
                black_box(h.finish())
            });
        });
    }
    g.finish();
}

criterion_group!(benches, bench_ops);
criterion_main!(benches);
