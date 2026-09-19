// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Head-to-head benchmarks: `ByteRope` (tiered radix rope) vs `ByteVec` (flat `Bytes` deque),
//! covering the shared public API so we can prove the rope does not regress the flat buffer.
//!
//! Every op is measured at two shapes:
//! - **shallow** (4 chunks) — the common streaming case; the rope stays in its flat `Small` tier and
//!   MUST NOT regress, and
//! - **deep** (1000 chunks) — past the promote threshold, exercising the radix tree.
//!
//! Mutating ops use `iter_batched` so the per-iteration clone (setup) is not timed. Run with
//! `cargo bench -p etude-byterope`.

use bytes::Bytes;
use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use etude_bytevec::ByteVec;
use etude_byterope::ByteRope;
use std::hint::black_box;
use std::time::Duration;

// Benchmark against the same allocator the host uses (jemalloc), not the system malloc — allocation
// behavior dominates a chunk/tree buffer, so this keeps the numbers production-representative.
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

const SHALLOW: usize = 4;
const DEEP: usize = 1000;

fn mtu_chunk(seed: u8) -> Bytes {
    Bytes::from(vec![seed; 1400])
}

fn rope_of(n: usize) -> ByteRope {
    (0..n).map(|i| mtu_chunk(i as u8)).collect()
}
fn vec_of(n: usize) -> ByteVec {
    let mut v = ByteVec::new();
    for i in 0..n {
        v.push_back(mtu_chunk(i as u8));
    }
    v
}

fn group<'a>(c: &'a mut Criterion, name: &str) -> criterion::BenchmarkGroup<'a, criterion::measurement::WallTime> {
    let mut g = c.benchmark_group(name);
    g.warm_up_time(Duration::from_millis(500));
    g.measurement_time(Duration::from_secs(2));
    g
}

/// Benchmarks a mutating op on both types at a given size. Each iteration's input is built **fresh
/// and uniquely-owned** in (untimed) setup from a shared chunk template — so `Bytes` handles are
/// cloned (cheap) but the rope's tree is not shared with any retained value. This is the realistic
/// case (you own the rope you mutate) and lets the rope's FBIP in-place path fire; cloning from a
/// retained source instead would share the tree and force the persistent path-copy on every op.
fn pair_mut<R, V>(
    c: &mut Criterion,
    name: &str,
    n: usize,
    label: &str,
    mut rope_op: impl FnMut(ByteRope) -> R,
    mut vec_op: impl FnMut(ByteVec) -> V,
) {
    let mut g = group(c, name);
    let template: Vec<Bytes> = (0..n).map(|i| mtu_chunk(i as u8)).collect();
    g.bench_function(BenchmarkId::new("ByteRope", label), |b| {
        b.iter_batched(
            || template.iter().cloned().collect::<ByteRope>(),
            |r| black_box(rope_op(r)),
            BatchSize::SmallInput,
        )
    });
    g.bench_function(BenchmarkId::new("ByteVec", label), |b| {
        b.iter_batched(
            || template.iter().cloned().collect::<ByteVec>(),
            |v| black_box(vec_op(v)),
            BatchSize::SmallInput,
        )
    });
    g.finish();
}

fn bench_push_back(c: &mut Criterion) {
    for &n in &[SHALLOW, DEEP] {
        let chunks: Vec<Bytes> = (0..n).map(|i| mtu_chunk(i as u8)).collect();
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let mut g = group(c, "push_back");
        g.bench_function(BenchmarkId::new("ByteRope", label), |b| {
            b.iter(|| {
                let mut r = ByteRope::new();
                for c in &chunks {
                    r.push_back(c.clone());
                }
                black_box(r)
            })
        });
        g.bench_function(BenchmarkId::new("ByteVec", label), |b| {
            b.iter(|| {
                let mut v = ByteVec::new();
                for c in &chunks {
                    v.push_back(c.clone());
                }
                black_box(v)
            })
        });
        g.finish();
    }
}

fn bench_push_front(c: &mut Criterion) {
    for &n in &[SHALLOW, DEEP] {
        let chunks: Vec<Bytes> = (0..n).map(|i| mtu_chunk(i as u8)).collect();
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let mut g = group(c, "push_front");
        g.bench_function(BenchmarkId::new("ByteRope", label), |b| {
            b.iter(|| {
                let mut r = ByteRope::new();
                for c in &chunks {
                    r.push_front(c.clone());
                }
                black_box(r)
            })
        });
        g.bench_function(BenchmarkId::new("ByteVec", label), |b| {
            b.iter(|| {
                let mut v = ByteVec::new();
                for c in &chunks {
                    v.push_front(c.clone());
                }
                black_box(v)
            })
        });
        g.finish();
    }
}

fn bench_mutating(c: &mut Criterion) {
    for &n in &[SHALLOW, DEEP] {
        let label = if n == SHALLOW { "shallow" } else { "deep" };

        // pop_front: drain the whole buffer
        pair_mut(
            c,
            "pop_front_drain",
            n,
            label,
            |mut r| while r.pop_front().is_some() {},
            |mut v| while v.pop_front().is_some() {},
        );

        // pop_back: drain the whole buffer
        pair_mut(
            c,
            "pop_back_drain",
            n,
            label,
            |mut r| while r.pop_back().is_some() {},
            |mut v| while v.pop_back().is_some() {},
        );

        // advance: consume the whole buffer in 700-byte bites
        pair_mut(
            c,
            "advance_drain",
            n,
            label,
            |mut r| while !r.is_empty() { let _ = r.advance(700); },
            |mut v| while !v.is_empty() { let _ = v.advance(700); },
        );

        // split_to at the midpoint
        let mid = n * 1400 / 2;
        pair_mut(
            c,
            "split_to_mid",
            n,
            label,
            move |mut r| r.split_to(mid).unwrap(),
            move |mut v| v.split_to(mid).unwrap(),
        );

        // truncate to half
        pair_mut(
            c,
            "truncate_half",
            n,
            label,
            move |mut r| { r.truncate(mid); r },
            move |mut v| { v.truncate(mid); v },
        );

        // copy_to_bytes (flatten)
        pair_mut(
            c,
            "copy_to_bytes",
            n,
            label,
            |r| r.copy_to_bytes(),
            |v| v.copy_to_bytes(),
        );

        // copy_to_bytes_mut (flatten into an owned, mutable buffer)
        pair_mut(
            c,
            "copy_to_bytes_mut",
            n,
            label,
            |r| r.copy_to_bytes_mut(),
            |v| v.copy_to_bytes_mut(),
        );

        // split_to_copy at the midpoint (copies out the front as one contiguous Bytes)
        pair_mut(
            c,
            "split_to_copy_mid",
            n,
            label,
            move |mut r| r.split_to_copy(mid).unwrap(),
            move |mut v| v.split_to_copy(mid).unwrap(),
        );
    }
}

fn bench_append(c: &mut Criterion) {
    for &n in &[SHALLOW, DEEP] {
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let mut g = group(c, "append");
        let ra = rope_of(n);
        let rb = rope_of(n);
        let va = vec_of(n);
        let vb = vec_of(n);
        g.bench_function(BenchmarkId::new("ByteRope", label), |b| {
            b.iter_batched(
                || (ra.clone(), rb.clone()),
                |(mut a, mut b)| { a.append(&mut b); black_box(a) },
                BatchSize::SmallInput,
            )
        });
        g.bench_function(BenchmarkId::new("ByteVec", label), |b| {
            b.iter_batched(
                || (va.clone(), vb.clone()),
                |(mut a, mut b)| { a.append(&mut b); black_box(a) },
                BatchSize::SmallInput,
            )
        });
        g.finish();
    }
}

fn bench_iterate(c: &mut Criterion) {
    for &n in &[SHALLOW, DEEP] {
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let mut g = group(c, "chunks_iter");
        let rope = rope_of(n);
        let vec = vec_of(n);
        g.bench_function(BenchmarkId::new("ByteRope", label), |b| {
            b.iter(|| rope.chunks().map(|c| c.len()).sum::<usize>())
        });
        g.bench_function(BenchmarkId::new("ByteVec", label), |b| {
            b.iter(|| vec.chunks().map(|c| c.len()).sum::<usize>())
        });
        g.finish();
    }
}

fn bench_clone(c: &mut Criterion) {
    let mut g = group(c, "clone");
    for &n in &[SHALLOW, DEEP, 100_000] {
        let label = format!("{n}");
        let rope = rope_of(n);
        let vec = vec_of(n);
        g.bench_function(BenchmarkId::new("ByteRope", &label), |b| {
            b.iter(|| black_box(rope.clone()))
        });
        g.bench_function(BenchmarkId::new("ByteVec", &label), |b| {
            b.iter(|| black_box(vec.clone()))
        });
    }
    g.finish();
}

/// Random byte-offset access — a rope-only capability (the flat buffer has no byte API and must walk
/// chunks). Deep buffer only.
fn bench_random_access(c: &mut Criterion) {
    let rope = rope_of(DEEP * 100);
    let vec = vec_of(DEEP * 100);
    let total = rope.len();
    let mut state = 0x1234_5678u64;
    let mut probe = move || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 33) as usize % total
    };
    let mut g = group(c, "random_byte_access");
    g.bench_function("ByteRope_byte_at", |b| b.iter(|| black_box(rope.byte_at(probe()))));
    g.bench_function("ByteVec_walk_to_byte", |b| {
        b.iter(|| black_box(byte_via_walk(&vec, probe())))
    });
    g.finish();
}

/// Random chunk-index access (`get(i)` / `Index`) — both types index into a deque, so this is a fair
/// head-to-head. Deep buffer only.
fn bench_get(c: &mut Criterion) {
    let rope = rope_of(DEEP);
    let vec = vec_of(DEEP);
    let count = DEEP;
    let mut state = 0x9E37_79B9u64;
    let mut probe = move || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 33) as usize % count
    };
    let mut g = group(c, "get_chunk");
    g.bench_function(BenchmarkId::new("ByteRope", "deep"), |b| {
        b.iter(|| black_box(rope.get(probe()).map(|c| c.len())))
    });
    g.bench_function(BenchmarkId::new("ByteVec", "deep"), |b| {
        b.iter(|| black_box(vec.get(probe()).map(|c| c.len())))
    });
    g.finish();
}

/// `slice(range)` — a rope-only capability that shares the covered subtree in O(log32) rather than
/// materializing. Baseline = the naive "flatten the whole rope, then slice the flat `Bytes`" an
/// API without a shared slice would force (O(n) copy). Both shapes.
fn bench_slice(c: &mut Criterion) {
    for &n in &[SHALLOW, DEEP] {
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let rope = rope_of(n);
        let total = n * 1400;
        let (start, end) = (total / 4, total * 3 / 4); // a mid subrange straddling chunk boundaries
        let mut g = group(c, "slice");
        g.bench_function(BenchmarkId::new("ByteRope_slice", label), |b| {
            b.iter(|| black_box(rope.slice(start..end)))
        });
        g.bench_function(BenchmarkId::new("flatten_then_slice", label), |b| {
            b.iter(|| black_box(rope.copy_to_bytes().slice(start..end)))
        });
        g.finish();
    }
}

/// `set_byte(offset, val)` — in-place single-byte write. Measured both when the rope is **uniquely
/// owned** (FBIP: write in place, no copy) and when a **shared clone is retained** (copy-on-write:
/// the touched chunk is copied — bounded, not the whole buffer — and the deep-tier spine is
/// path-copied). The shared case pins the bounded-COW win: it must stay cheap even for a huge rope.
fn bench_set_byte(c: &mut Criterion) {
    for &n in &[SHALLOW, DEEP] {
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let template: Vec<Bytes> = (0..n).map(|i| mtu_chunk(i as u8)).collect();
        let mid = n * 1400 / 2;
        let mut g = group(c, "set_byte");
        g.bench_function(BenchmarkId::new("unique", label), |b| {
            b.iter_batched(
                || template.iter().cloned().collect::<ByteRope>(),
                |mut r| {
                    r.set_byte(mid, 0xEE).unwrap();
                    black_box(r)
                },
                BatchSize::SmallInput,
            )
        });
        g.bench_function(BenchmarkId::new("shared_cow", label), |b| {
            b.iter_batched(
                || {
                    let r: ByteRope = template.iter().cloned().collect();
                    let guard = r.clone(); // retained ⇒ chunks shared ⇒ set_byte takes the COW path
                    (r, guard)
                },
                |(mut r, guard)| {
                    r.set_byte(mid, 0xEE).unwrap();
                    black_box((r, guard))
                },
                BatchSize::SmallInput,
            )
        });
        g.finish();
    }
}

/// `replace(range, value)` — the length-preserving overwrite (UC1–3, in place) and the
/// length-changing splice (UC4–6). Both are on the runtime's hot path, so pin them at both shapes.
/// Ropes are freshly + uniquely built in (untimed) setup so the in-place FBIP path fires.
fn bench_replace(c: &mut Criterion) {
    for &n in &[SHALLOW, DEEP] {
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let template: Vec<Bytes> = (0..n).map(|i| mtu_chunk(i as u8)).collect();
        let start = n * 1400 / 4;
        let span = 2100; // 1.5 chunks — straddles a chunk boundary in the deep tier
        let overwrite = vec![0xABu8; span]; // equal length ⇒ no structure change
        let insert = [0xCDu8; 100]; // shorter ⇒ shrinking splice (structure change)
        let mut g = group(c, "replace");
        g.bench_function(BenchmarkId::new("overwrite_eqlen", label), |b| {
            b.iter_batched(
                || template.iter().cloned().collect::<ByteRope>(),
                |mut r| {
                    r.replace(start..start + span, &overwrite[..]).unwrap();
                    black_box(r)
                },
                BatchSize::SmallInput,
            )
        });
        g.bench_function(BenchmarkId::new("splice_shrink", label), |b| {
            b.iter_batched(
                || template.iter().cloned().collect::<ByteRope>(),
                |mut r| {
                    r.replace(start..start + span, &insert[..]).unwrap();
                    black_box(r)
                },
                BatchSize::SmallInput,
            )
        });
        g.finish();
    }
}

fn byte_via_walk(v: &ByteVec, mut offset: usize) -> Option<u8> {
    for chunk in v.chunks() {
        if offset < chunk.len() {
            return Some(chunk[offset]);
        }
        offset -= chunk.len();
    }
    None
}

criterion_group!(
    benches,
    bench_push_back,
    bench_push_front,
    bench_mutating,
    bench_append,
    bench_iterate,
    bench_clone,
    bench_random_access,
    bench_get,
    bench_slice,
    bench_set_byte,
    bench_replace
);
criterion_main!(benches);
