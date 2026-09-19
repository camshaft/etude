// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Head-to-head benchmarks: [`ByteVec`] (the tiered relaxed-radix rope) vs a **naive flat chunk
//! deque** (`VecDeque<Bytes>` + a cached length) — the shape `ByteVec` had before it became the rope.
//!
//! The point is to prove the rope does **not regress** a plain chunk deque on the common
//! chunk-granular operations, and to keep a runnable scoreboard alongside the historical record in
//! `BENCHMARKS.md`. Each op is measured at two shapes:
//! - **shallow** (4 chunks) — the common streaming case; the rope stays in its flat `Small` tier and
//!   MUST NOT regress, and
//! - **deep** (1000 chunks) — past the promote threshold, exercising the radix tree, where the rope's
//!   structural sharing (O(1) clone, O(log₃₂) random access / slice) is expected to win big.
//!
//! Rope-only capabilities (`slice`, `byte_at`, `set_byte`, `replace`) have no plain-deque counterpart
//! and are measured against the naive materialize-and-scan a deque-only API would force. Run with
//! `cargo bench -p etude-bytevec`.

use bytes::{Bytes, BytesMut};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use etude_bytevec::ByteVec;
use std::collections::VecDeque;
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

/// A naive flat chunk deque with a cached length — the shape `ByteVec` had before the RRB rope. The
/// baseline the rope must not regress on chunk-granular ops. Only the operations the benches exercise
/// are implemented; correctness is not fuzzed (it is a bench yardstick, not production code).
#[derive(Clone, Default)]
struct NaiveVec {
    chunks: VecDeque<Bytes>,
    len: usize,
}

impl NaiveVec {
    fn new() -> Self {
        Self::default()
    }
    fn is_empty(&self) -> bool {
        self.len == 0
    }
    fn push_back(&mut self, b: Bytes) {
        if !b.is_empty() {
            self.len += b.len();
            self.chunks.push_back(b);
        }
    }
    fn push_front(&mut self, b: Bytes) {
        if !b.is_empty() {
            self.len += b.len();
            self.chunks.push_front(b);
        }
    }
    fn pop_front(&mut self) -> Option<Bytes> {
        let b = self.chunks.pop_front()?;
        self.len -= b.len();
        Some(b)
    }
    fn pop_back(&mut self) -> Option<Bytes> {
        let b = self.chunks.pop_back()?;
        self.len -= b.len();
        Some(b)
    }
    fn advance(&mut self, mut n: usize) {
        n = n.min(self.len);
        self.len -= n;
        while n > 0 {
            let front = self.chunks.front_mut().expect("bytes remaining");
            if front.len() <= n {
                n -= front.len();
                self.chunks.pop_front();
            } else {
                bytes::Buf::advance(front, n);
                n = 0;
            }
        }
    }
    fn split_to(&mut self, at: usize) -> Self {
        let at = at.min(self.len);
        let mut out = NaiveVec::new();
        while out.len < at {
            let need = at - out.len;
            let front = self.chunks.front().expect("bytes remaining");
            if front.len() <= need {
                let c = self.chunks.pop_front().unwrap();
                out.push_back(c);
            } else {
                let mut c = self.chunks.pop_front().unwrap();
                let head = c.split_to(need); // head = first `need`, c keeps the rest
                out.push_back(head);
                self.chunks.push_front(c);
            }
        }
        self.len -= out.len;
        out
    }
    fn truncate(&mut self, len: usize) {
        while self.len > len {
            let over = self.len - len;
            let back = self.chunks.back_mut().expect("bytes remaining");
            if back.len() <= over {
                let c = self.chunks.pop_back().unwrap();
                self.len -= c.len();
            } else {
                let keep = back.len() - over;
                back.truncate(keep);
                self.len = len;
            }
        }
    }
    fn append(&mut self, other: &mut Self) {
        self.len += other.len;
        self.chunks.append(&mut other.chunks);
        other.len = 0;
    }
    fn get(&self, i: usize) -> Option<&Bytes> {
        self.chunks.get(i)
    }
    fn chunks(&self) -> std::collections::vec_deque::Iter<'_, Bytes> {
        self.chunks.iter()
    }
    fn copy_to_bytes(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(self.len);
        for c in &self.chunks {
            buf.extend_from_slice(c);
        }
        buf.freeze()
    }
}

impl FromIterator<Bytes> for NaiveVec {
    fn from_iter<I: IntoIterator<Item = Bytes>>(it: I) -> Self {
        let mut v = NaiveVec::new();
        for b in it {
            v.push_back(b);
        }
        v
    }
}

impl Extend<Bytes> for NaiveVec {
    fn extend<I: IntoIterator<Item = Bytes>>(&mut self, it: I) {
        for b in it {
            self.push_back(b);
        }
    }
}

fn rope_of(n: usize) -> ByteVec {
    (0..n).map(|i| mtu_chunk(i as u8)).collect()
}
fn naive_of(n: usize) -> NaiveVec {
    (0..n).map(|i| mtu_chunk(i as u8)).collect()
}

fn group<'a>(
    c: &'a mut Criterion,
    name: &str,
) -> criterion::BenchmarkGroup<'a, criterion::measurement::WallTime> {
    let mut g = c.benchmark_group(name);
    g.warm_up_time(Duration::from_millis(500));
    g.measurement_time(Duration::from_secs(2));
    g
}

/// Benchmarks a mutating op on both types at a given size. Each iteration's input is built **fresh
/// and uniquely-owned** in (untimed) setup from a shared chunk template — so `Bytes` handles are
/// cloned (cheap) but the rope's tree is not shared with any retained value, letting its FBIP in-place
/// path fire (the realistic case: you own the buffer you mutate).
fn pair_mut<R, V>(
    c: &mut Criterion,
    name: &str,
    n: usize,
    label: &str,
    mut rope_op: impl FnMut(ByteVec) -> R,
    mut naive_op: impl FnMut(NaiveVec) -> V,
) {
    let mut g = group(c, name);
    let template: Vec<Bytes> = (0..n).map(|i| mtu_chunk(i as u8)).collect();
    g.bench_function(BenchmarkId::new("rope", label), |b| {
        b.iter_batched(
            || template.iter().cloned().collect::<ByteVec>(),
            |r| black_box(rope_op(r)),
            BatchSize::SmallInput,
        )
    });
    g.bench_function(BenchmarkId::new("naive_deque", label), |b| {
        b.iter_batched(
            || template.iter().cloned().collect::<NaiveVec>(),
            |v| black_box(naive_op(v)),
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
        g.bench_function(BenchmarkId::new("rope", label), |b| {
            b.iter(|| {
                let mut r = ByteVec::new();
                for c in &chunks {
                    r.push_back(c.clone());
                }
                black_box(r)
            })
        });
        g.bench_function(BenchmarkId::new("naive_deque", label), |b| {
            b.iter(|| {
                let mut v = NaiveVec::new();
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
        g.bench_function(BenchmarkId::new("rope", label), |b| {
            b.iter(|| {
                let mut r = ByteVec::new();
                for c in &chunks {
                    r.push_front(c.clone());
                }
                black_box(r)
            })
        });
        g.bench_function(BenchmarkId::new("naive_deque", label), |b| {
            b.iter(|| {
                let mut v = NaiveVec::new();
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

        pair_mut(
            c,
            "pop_front_drain",
            n,
            label,
            |mut r| while r.pop_front().is_some() {},
            |mut v| while v.pop_front().is_some() {},
        );
        pair_mut(
            c,
            "pop_back_drain",
            n,
            label,
            |mut r| while r.pop_back().is_some() {},
            |mut v| while v.pop_back().is_some() {},
        );
        pair_mut(
            c,
            "advance_drain",
            n,
            label,
            |mut r| {
                while !r.is_empty() {
                    let _ = r.advance(700);
                }
            },
            |mut v| {
                while !v.is_empty() {
                    v.advance(700);
                }
            },
        );

        let mid = n * 1400 / 2;
        pair_mut(
            c,
            "split_to_mid",
            n,
            label,
            move |mut r| r.split_to(mid).unwrap(),
            move |mut v| v.split_to(mid),
        );
        pair_mut(
            c,
            "truncate_half",
            n,
            label,
            move |mut r| {
                r.truncate(mid);
                r
            },
            move |mut v| {
                v.truncate(mid);
                v
            },
        );
        pair_mut(
            c,
            "copy_to_bytes",
            n,
            label,
            |r| r.copy_to_bytes(),
            |v| v.copy_to_bytes(),
        );
        pair_mut(
            c,
            "append_mid",
            n,
            label,
            {
                let tail = rope_of(n);
                move |mut r| {
                    r.append(&mut tail.clone());
                    r
                }
            },
            {
                let tail = naive_of(n);
                move |mut v| {
                    v.append(&mut tail.clone());
                    v
                }
            },
        );
    }
}

fn bench_iterate(c: &mut Criterion) {
    for &n in &[SHALLOW, DEEP] {
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let mut g = group(c, "chunks_iter");
        let rope = rope_of(n);
        let naive = naive_of(n);
        g.bench_function(BenchmarkId::new("rope", label), |b| {
            b.iter(|| rope.chunks().map(|c| c.len()).sum::<usize>())
        });
        g.bench_function(BenchmarkId::new("naive_deque", label), |b| {
            b.iter(|| naive.chunks().map(|c| c.len()).sum::<usize>())
        });
        g.finish();
    }
}

/// The rope's headline win: O(1) structural-sharing clone vs the deque's O(chunks) deep copy.
fn bench_clone(c: &mut Criterion) {
    let mut g = group(c, "clone");
    for &n in &[SHALLOW, DEEP, 100_000] {
        let label = format!("{n}");
        let rope = rope_of(n);
        let naive = naive_of(n);
        g.bench_function(BenchmarkId::new("rope", &label), |b| {
            b.iter(|| black_box(rope.clone()))
        });
        g.bench_function(BenchmarkId::new("naive_deque", &label), |b| {
            b.iter(|| black_box(naive.clone()))
        });
    }
    g.finish();
}

/// Random chunk-index access (`get(i)`) — both index a deque, a fair head-to-head. Deep only.
fn bench_get(c: &mut Criterion) {
    let rope = rope_of(DEEP);
    let naive = naive_of(DEEP);
    let mut state = 0x9E37_79B9u64;
    let mut probe = move || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 33) as usize % DEEP
    };
    let mut g = group(c, "get_chunk");
    g.bench_function(BenchmarkId::new("rope", "deep"), |b| {
        b.iter(|| black_box(rope.get(probe()).map(|c| c.len())))
    });
    g.bench_function(BenchmarkId::new("naive_deque", "deep"), |b| {
        b.iter(|| black_box(naive.get(probe()).map(|c| c.len())))
    });
    g.finish();
}

/// Random byte-offset access — the rope descends its size table in O(log₃₂); a deque must walk chunks.
fn bench_random_byte(c: &mut Criterion) {
    let rope = rope_of(DEEP * 100);
    let naive = naive_of(DEEP * 100);
    let total = rope.len();
    let mut state = 0x1234_5678u64;
    let mut probe = move || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        (state >> 33) as usize % total
    };
    let mut g = group(c, "random_byte_access");
    g.bench_function("rope_byte_at", |b| {
        b.iter(|| black_box(rope.byte_at(probe())))
    });
    g.bench_function("naive_walk_to_byte", |b| {
        b.iter(|| black_box(byte_via_walk(&naive, probe())))
    });
    g.finish();
}

fn byte_via_walk(v: &NaiveVec, mut offset: usize) -> Option<u8> {
    for chunk in v.chunks() {
        if offset < chunk.len() {
            return Some(chunk[offset]);
        }
        offset -= chunk.len();
    }
    None
}

/// `slice(range)` — a rope-only capability that shares the covered subtree in O(log₃₂). Baseline =
/// the naive "flatten the whole buffer, then slice the flat `Bytes`" a deque-only API would force.
fn bench_slice(c: &mut Criterion) {
    for &n in &[SHALLOW, DEEP] {
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let rope = rope_of(n);
        let naive = naive_of(n);
        let total = n * 1400;
        let (start, end) = (total / 4, total * 3 / 4);
        let mut g = group(c, "slice");
        g.bench_function(BenchmarkId::new("rope_slice", label), |b| {
            b.iter(|| black_box(rope.slice(start..end)))
        });
        g.bench_function(BenchmarkId::new("naive_flatten_then_slice", label), |b| {
            b.iter(|| black_box(naive.copy_to_bytes().slice(start..end)))
        });
        g.finish();
    }
}

/// `set_byte(offset, val)` — in-place single-byte write, a rope-only capability. Measured both when
/// the rope is uniquely owned (FBIP in place) and when a shared clone is retained (bounded
/// copy-on-write: one chunk + O(log₃₂) spine, not a whole-buffer copy).
fn bench_set_byte(c: &mut Criterion) {
    for &n in &[SHALLOW, DEEP] {
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let template: Vec<Bytes> = (0..n).map(|i| mtu_chunk(i as u8)).collect();
        let mid = n * 1400 / 2;
        let mut g = group(c, "set_byte");
        g.bench_function(BenchmarkId::new("unique", label), |b| {
            b.iter_batched(
                || template.iter().cloned().collect::<ByteVec>(),
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
                    let r: ByteVec = template.iter().cloned().collect();
                    let guard = r.clone(); // retained ⇒ chunks shared ⇒ COW path
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

/// `replace(range, value)` — length-preserving overwrite (in place) and a length-changing splice.
fn bench_replace(c: &mut Criterion) {
    for &n in &[SHALLOW, DEEP] {
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let template: Vec<Bytes> = (0..n).map(|i| mtu_chunk(i as u8)).collect();
        let start = n * 1400 / 4;
        let span = 2100;
        let overwrite = vec![0xABu8; span];
        let insert = [0xCDu8; 100];
        let mut g = group(c, "replace");
        g.bench_function(BenchmarkId::new("overwrite_eqlen", label), |b| {
            b.iter_batched(
                || template.iter().cloned().collect::<ByteVec>(),
                |mut r| {
                    r.replace(start..start + span, &overwrite[..]).unwrap();
                    black_box(r)
                },
                BatchSize::SmallInput,
            )
        });
        g.bench_function(BenchmarkId::new("splice_shrink", label), |b| {
            b.iter_batched(
                || template.iter().cloned().collect::<ByteVec>(),
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

fn bench_extend(c: &mut Criterion) {
    for &n in &[SHALLOW, DEEP] {
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let chunks: Vec<Bytes> = (0..n).map(|i| mtu_chunk(i as u8)).collect();
        let mut g = group(c, "extend");
        g.bench_function(BenchmarkId::new("rope", label), |b| {
            b.iter(|| {
                let mut r = ByteVec::new();
                r.extend(chunks.iter().cloned());
                black_box(r)
            })
        });
        g.bench_function(BenchmarkId::new("naive_deque", label), |b| {
            b.iter(|| {
                let mut v = NaiveVec::new();
                v.extend(chunks.iter().cloned());
                black_box(v)
            })
        });
        g.finish();
    }
}

fn bench_from_iter(c: &mut Criterion) {
    for &n in &[SHALLOW, DEEP] {
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let chunks: Vec<Bytes> = (0..n).map(|i| mtu_chunk(i as u8)).collect();
        let mut g = group(c, "from_iter");
        g.bench_function(BenchmarkId::new("rope", label), |b| {
            b.iter(|| black_box(chunks.iter().cloned().collect::<ByteVec>()))
        });
        g.bench_function(BenchmarkId::new("naive_deque", label), |b| {
            b.iter(|| black_box(chunks.iter().cloned().collect::<NaiveVec>()))
        });
        g.finish();
    }
}

criterion_group!(
    benches,
    bench_push_back,
    bench_push_front,
    bench_mutating,
    bench_iterate,
    bench_clone,
    bench_get,
    bench_random_byte,
    bench_slice,
    bench_set_byte,
    bench_replace,
    bench_extend,
    bench_from_iter
);
criterion_main!(benches);
