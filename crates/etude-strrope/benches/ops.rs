// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Per-function benchmark scoreboard for [`StrRope`], differential against the reference contiguous
//! string (`std::string::String` / `str`) — every public function is measured, per the perf-push
//! directive.
//!
//! Each op is run at two shapes:
//! - **shallow** (4 chunks) — the common streaming case; the underlying rope stays in its flat tier, and
//! - **deep** (1000 chunks) — past the promote threshold, exercising the radix tree, where the rope's
//!   structural sharing (O(1) clone, O(log n) split/slice/insert) is expected to pull ahead of the
//!   contiguous `String` (which must shift/copy).
//!
//! The `std` variant is the honest reference: a contiguous `String` wins the small/dense cases; the rope
//! wins clone/split/slice/insert as depth grows. Run with `cargo bench -p etude-strrope`.

use bytes::Bytes;
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use etude_bytevec::ByteVec;
use etude_strrope::StrRope;
use std::hash::{Hash, Hasher};
use std::hint::black_box;
use std::time::Duration;

// Match the host allocator (jemalloc) so allocation-sensitive numbers reflect production behavior.
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const SHALLOW: usize = 4;
const DEEP: usize = 1000;

/// A benchmark group with a bounded warm-up/measurement window, so a full `cargo bench` run stays
/// quick across the many per-op groups below.
fn group<'a>(
    c: &'a mut Criterion,
    name: &str,
) -> criterion::BenchmarkGroup<'a, criterion::measurement::WallTime> {
    let mut g = c.benchmark_group(name);
    g.warm_up_time(Duration::from_millis(500));
    g.measurement_time(Duration::from_secs(2));
    g
}

/// One valid-UTF-8 leaf of varied content (so eq/ord/hash do real work).
fn payload(i: usize) -> String {
    format!("chunk-{i:05}-payload-abcdefghijklmnopqrstuvwxyz-0123456789")
}

/// Build a `StrRope` of `chunks` leaves and the equivalent flat `String`.
fn build(chunks: usize) -> (StrRope, String) {
    let mut bv = ByteVec::new();
    let mut s = String::new();
    for i in 0..chunks {
        let p = payload(i);
        bv.push_back(Bytes::from(p.clone().into_bytes()));
        s.push_str(&p);
    }
    (StrRope::from_utf8(bv).expect("valid utf8"), s)
}

fn hash64<T: Hash>(t: &T) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

fn bench(c: &mut Criterion) {
    for &n in &[SHALLOW, DEEP] {
        let (rope, string) = build(n);
        let mid = rope.len() / 2;
        let at = (0..=mid).rev().find(|&i| rope.is_char_boundary(i)).unwrap();
        let ins = "inserted-text";

        // ---- construction ----
        let mut g = group(c, "from_str");
        g.bench_with_input(BenchmarkId::new("strrope", n), &string, |b, s| {
            b.iter(|| black_box(StrRope::from(s.as_str())));
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s| {
            b.iter(|| black_box(String::from(s.as_str())));
        });
        g.finish();

        // Owned construction: `from(String)` MOVES the buffer (allocation reused, no copy, no scan),
        // where `from(&str)` above must copy. The clone lives in the untimed setup, so this measures the
        // move alone; the reference is a bare identity move of the owned String — the floor for consuming
        // one — so `strrope` should sit close to it and neither should scale with length.
        let mut g = group(c, "from_string_owned");
        g.bench_with_input(BenchmarkId::new("strrope", n), &string, |b, s| {
            b.iter_batched(
                || s.clone(),
                |owned| black_box(StrRope::from(owned)),
                BatchSize::SmallInput,
            );
        });
        g.bench_with_input(BenchmarkId::new("identity_move", n), &string, |b, s| {
            b.iter_batched(|| s.clone(), black_box, BatchSize::SmallInput);
        });
        g.finish();

        let mut g = group(c, "from_utf8");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            b.iter_batched(
                || r.clone().into_bytes(),
                |bv| black_box(StrRope::from_utf8(bv).unwrap()),
                BatchSize::SmallInput,
            );
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s| {
            b.iter_batched(
                || s.clone().into_bytes(),
                |v| black_box(String::from_utf8(v).unwrap()),
                BatchSize::SmallInput,
            );
        });
        g.finish();

        // Intentionally unbenched: `len`/`is_empty` are simple O(1) field reads on the underlying rope,
        // so per the operator ruling (bench real-work fns + ctors, skip simple getters) they carry no
        // bench. `is_char_boundary` stays below because it does real work — an O(log n) `byte_at` lookup
        // into the rope to find the byte at `at`, not a field read.
        let mut g = group(c, "is_char_boundary");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            b.iter(|| black_box(r.is_char_boundary(at)));
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s| {
            b.iter(|| black_box(s.is_char_boundary(at)));
        });
        g.finish();

        // ---- mutation ----
        let mut g = group(c, "push_str");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            b.iter_batched(
                || r.clone(),
                |mut s| {
                    s.push_str(ins);
                    black_box(s)
                },
                BatchSize::SmallInput,
            );
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s0| {
            b.iter_batched(
                || s0.clone(),
                |mut s| {
                    s.push_str(ins);
                    black_box(s)
                },
                BatchSize::SmallInput,
            );
        });
        g.finish();

        let mut g = group(c, "push");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            b.iter_batched(
                || r.clone(),
                |mut s| {
                    s.push('x');
                    black_box(s)
                },
                BatchSize::SmallInput,
            );
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s0| {
            b.iter_batched(
                || s0.clone(),
                |mut s| {
                    s.push('x');
                    black_box(s)
                },
                BatchSize::SmallInput,
            );
        });
        g.finish();

        let mut g = group(c, "insert_str");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            b.iter_batched(
                || r.clone(),
                |mut s| {
                    s.insert_str(at, ins);
                    black_box(s)
                },
                BatchSize::SmallInput,
            );
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s0| {
            b.iter_batched(
                || s0.clone(),
                |mut s| {
                    s.insert_str(at, ins);
                    black_box(s)
                },
                BatchSize::SmallInput,
            );
        });
        g.finish();

        let mut g = group(c, "insert");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            b.iter_batched(
                || r.clone(),
                |mut s| {
                    s.insert(at, 'x');
                    black_box(s)
                },
                BatchSize::SmallInput,
            );
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s0| {
            b.iter_batched(
                || s0.clone(),
                |mut s| {
                    s.insert(at, 'x');
                    black_box(s)
                },
                BatchSize::SmallInput,
            );
        });
        g.finish();

        // ---- structural (where the rope is expected to win at depth) ----
        let mut g = group(c, "split_off");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            b.iter_batched(
                || r.clone(),
                |mut s| black_box(s.split_off(at)),
                BatchSize::SmallInput,
            );
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s0| {
            b.iter_batched(
                || s0.clone(),
                |mut s| black_box(s.split_off(at)),
                BatchSize::SmallInput,
            );
        });
        g.finish();

        let mut g = group(c, "slice");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            b.iter(|| black_box(r.slice(at..)));
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s| {
            // Reference: materialize the substring (a &str slice is free, but the comparable owned op is
            // allocating the tail — what a contiguous string pays where the rope shares structure).
            b.iter(|| black_box(String::from(&s[at..])));
        });
        g.finish();

        let mut g = group(c, "clone");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            b.iter(|| black_box(r.clone()));
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s| {
            b.iter(|| black_box(s.clone()));
        });
        g.finish();

        let mut g = group(c, "into_bytes");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            b.iter_batched(
                || r.clone(),
                |s| black_box(s.into_bytes()),
                BatchSize::SmallInput,
            );
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s0| {
            b.iter_batched(
                || s0.clone(),
                |s| black_box(s.into_bytes()),
                BatchSize::SmallInput,
            );
        });
        g.finish();

        // ---- iteration (content ops; scale with length) ----
        let mut g = group(c, "chars_count");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            b.iter(|| black_box(r.chars().count()));
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s| {
            b.iter(|| black_box(s.chars().count()));
        });
        g.finish();

        let mut g = group(c, "char_indices_last");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            b.iter(|| black_box(r.char_indices().last()));
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s| {
            b.iter(|| black_box(s.char_indices().last()));
        });
        g.finish();

        let mut g = group(c, "bytes_sum");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            b.iter(|| black_box(r.bytes().fold(0u64, |a, x| a + u64::from(x))));
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s| {
            b.iter(|| black_box(s.bytes().fold(0u64, |a, x| a + u64::from(x))));
        });
        g.finish();

        // chunks() has no contiguous-String analogue; measure the rope alone.
        let mut g = group(c, "chunks_count");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            b.iter(|| black_box(r.chunks().count()));
        });
        g.finish();

        // ---- trait ops ----
        let mut g = group(c, "eq");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            let other = r.clone();
            b.iter(|| black_box(*r == other));
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s| {
            let other = s.clone();
            b.iter(|| black_box(*s == other));
        });
        g.finish();

        let mut g = group(c, "eq_str");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            let s = string.clone();
            b.iter(|| black_box(*r == *s.as_str()));
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s0| {
            let s = s0.clone();
            b.iter(|| black_box(s0.as_str() == s.as_str()));
        });
        g.finish();

        let mut g = group(c, "cmp");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            let other = r.clone();
            b.iter(|| black_box(r.cmp(&other)));
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s| {
            let other = s.clone();
            b.iter(|| black_box(s.cmp(&other)));
        });
        g.finish();

        let mut g = group(c, "hash");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            b.iter(|| black_box(hash64(r)));
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s| {
            b.iter(|| black_box(hash64(s)));
        });
        g.finish();

        let mut g = group(c, "display");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            b.iter(|| black_box(r.to_string()));
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s| {
            b.iter(|| black_box(s.to_string()));
        });
        g.finish();

        let mut g = group(c, "debug");
        g.bench_with_input(BenchmarkId::new("strrope", n), &rope, |b, r| {
            b.iter(|| black_box(format!("{r:?}")));
        });
        g.bench_with_input(BenchmarkId::new("std", n), &string, |b, s| {
            b.iter(|| black_box(format!("{s:?}")));
        });
        g.finish();
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
