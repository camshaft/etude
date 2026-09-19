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
use etude_bytevec::{ByteVec, CompactionConfig, Rope, Utf8};
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
    fn clear(&mut self) {
        self.chunks.clear();
        self.len = 0;
    }
    fn split_to_copy(&mut self, at: usize) -> Bytes {
        let at = at.min(self.len);
        let mut out = BytesMut::with_capacity(at);
        let mut remaining = at;
        while remaining > 0 {
            let front = self.chunks.front_mut().expect("bytes remaining");
            let take = front.len().min(remaining);
            out.extend_from_slice(&front[..take]);
            if take < front.len() {
                bytes::Buf::advance(front, take);
            } else {
                self.chunks.pop_front();
            }
            self.len -= take;
            remaining -= take;
        }
        out.freeze()
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
            "split_to_copy_mid",
            n,
            label,
            move |mut r| r.split_to_copy(mid).unwrap(),
            move |mut v| v.split_to_copy(mid),
        );
        pair_mut(
            c,
            "clear",
            n,
            label,
            |mut r| {
                r.clear();
                r
            },
            |mut v| {
                v.clear();
                v
            },
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

/// `starts_with` / `ends_with`: the rope walks its chunks (early-exit; `ends_with` from the back via
/// the double-ended chunk iterator, so it is O(suffix)); the reference is a contiguous `Vec<u8>`
/// slice compare. The literal spans ~2 chunks so the scan crosses a boundary.
fn bench_starts_ends_with(c: &mut Criterion) {
    for &n in &[SHALLOW, DEEP] {
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let rope = rope_of(n);
        let flat: Vec<u8> = rope.chunks().flat_map(|c| c.iter().copied()).collect();
        let plen = 2500.min(flat.len());
        let prefix: Vec<u8> = flat[..plen].to_vec();
        let suffix: Vec<u8> = flat[flat.len() - plen..].to_vec();

        let mut g = group(c, "starts_with");
        g.bench_function(BenchmarkId::new("rope", label), |b| {
            b.iter(|| black_box(rope.starts_with(black_box(&prefix))))
        });
        g.bench_function(BenchmarkId::new("flat_slice", label), |b| {
            b.iter(|| black_box(flat.starts_with(black_box(&prefix[..]))))
        });
        g.finish();

        let mut g = group(c, "ends_with");
        g.bench_function(BenchmarkId::new("rope", label), |b| {
            b.iter(|| black_box(rope.ends_with(black_box(&suffix))))
        });
        g.bench_function(BenchmarkId::new("flat_slice", label), |b| {
            b.iter(|| black_box(flat.ends_with(black_box(&suffix[..]))))
        });
        g.finish();
    }
}

/// `Rope<Utf8>::try_from_bytes` on valid content (the ingest accept path): the rope streams the
/// validation over its chunks with no full-content allocation; the reference validates a contiguous
/// `Vec<u8>` via `core::str::from_utf8`. Content is ascii so it is valid utf-8.
fn bench_validate_utf8(c: &mut Criterion) {
    let ascii_chunk = |i: usize| Bytes::from(vec![b'a' + (i as u8 % 26); 1400]);
    for &n in &[SHALLOW, DEEP] {
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let template: Vec<Bytes> = (0..n).map(ascii_chunk).collect();
        let flat: Vec<u8> = template.iter().flat_map(|c| c.iter().copied()).collect();

        let mut g = group(c, "validate_utf8");
        g.bench_function(BenchmarkId::new("rope", label), |b| {
            b.iter_batched(
                || template.iter().cloned().collect::<ByteVec>(),
                |bv| black_box(Rope::<Utf8>::try_from_bytes(bv).is_ok()),
                BatchSize::SmallInput,
            )
        });
        g.bench_function(BenchmarkId::new("flat_from_utf8", label), |b| {
            b.iter(|| black_box(core::str::from_utf8(black_box(&flat)).is_ok()))
        });
        g.finish();
    }
}

/// `copy_to_bytes_mut` on a single, uniquely-owned chunk reclaims the allocation in place rather
/// than copying, so its time is flat across chunk size (a copy would scale with size). Benched at a
/// small and a large single chunk to show the size-independence, against `BytesMut::from(Vec)` (the
/// ideal reclaim) and a deep multi-chunk rope (which must copy).
fn bench_copy_to_bytes_mut(c: &mut Criterion) {
    let mut g = group(c, "copy_to_bytes_mut");
    for &sz in &[1400usize, 262_144] {
        let label = format!("single_{sz}");
        g.bench_function(BenchmarkId::new("rope_reclaim", &label), |b| {
            b.iter_batched(
                || {
                    let mut r = ByteVec::new();
                    r.push_back(Bytes::from(vec![0u8; sz]));
                    r
                },
                |r| black_box(r.copy_to_bytes_mut()),
                BatchSize::SmallInput,
            )
        });
        g.bench_function(BenchmarkId::new("bytesmut_from_vec", &label), |b| {
            b.iter_batched(
                || Bytes::from(vec![0u8; sz]),
                |by| black_box(BytesMut::from(by)),
                BatchSize::SmallInput,
            )
        });
    }
    g.bench_function(BenchmarkId::new("rope_copy", "deep"), |b| {
        b.iter_batched(
            || rope_of(DEEP),
            |r| black_box(r.copy_to_bytes_mut()),
            BatchSize::SmallInput,
        )
    });
    g.finish();
}

/// Socket-read hot path: `Builder::for_socket_read` routes through `put_uninit_slice`, which
/// zero-inits (memsets) the `payload_len`-byte spare region before the read closure runs (the #173
/// info-leak fix). This measures that memset's cost so the deferred follow-up that would reclaim it
/// (report bytes-written and skip the zero-init) is only pursued if it is material:
/// - `memset_only`: the closure reports 0 bytes read — a pure memset of `n` bytes plus routing, no
///   copy, no growth (isolates the zero-init cost);
/// - `memset_plus_recv`: the full path — zero-init then the closure copies `n` bytes (a recv);
/// - `recv_copy_no_memset`: the same `n`-byte copy into a fresh `BytesMut` with no zero-init — what
///   the reclaim follow-up would leave. The `memset_plus_recv` − `recv_copy_no_memset` delta is the
///   memset's marginal cost on the hot path.
fn bench_socket_read(c: &mut Criterion) {
    let mut g = group(c, "socket_read");
    for &n in &[1500usize, 65536] {
        let label = format!("{n}");
        let src = vec![0xABu8; n];

        g.bench_function(BenchmarkId::new("memset_only", &label), |b| {
            let mut bld = ByteVec::builder(n);
            b.iter(|| {
                bld.for_socket_read(n, |_slice| 0);
                black_box(&bld);
            })
        });
        g.bench_function(BenchmarkId::new("memset_plus_recv", &label), |b| {
            b.iter_batched_ref(
                || ByteVec::builder(n),
                |bld| {
                    bld.for_socket_read(n, |slice| {
                        slice[0..n].copy_from_slice(&src);
                        n
                    });
                    black_box(&*bld);
                },
                BatchSize::SmallInput,
            )
        });
        g.bench_function(BenchmarkId::new("recv_copy_no_memset", &label), |b| {
            b.iter_batched(
                || BytesMut::with_capacity(n),
                |mut bm| {
                    bytes::BufMut::put_slice(&mut bm, &src);
                    black_box(bm)
                },
                BatchSize::SmallInput,
            )
        });
    }
    g.finish();
}

/// `Rope<Utf8>`'s caller-trusted mutators against `String`:
/// - `append_bytes` (backs `StrRope::push_str`) pushes each fragment as its own chunk, so many small
///   appends accumulate chunks — expected to trail `String::push_str`'s amortized contiguous growth;
/// - `insert_bytes` splits at the byte offset and stitches (O(log n)), against `String::insert_str`
///   which shifts the tail (O(n) memmove) — expected to win as the content grows.
fn bench_utf8_mutate(c: &mut Criterion) {
    let frag = "hello ";
    for &n in &[SHALLOW, DEEP] {
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let mut g = group(c, "append_bytes");
        g.bench_function(BenchmarkId::new("rope_utf8", label), |b| {
            b.iter(|| {
                let mut s = Rope::<Utf8>::default();
                for _ in 0..n {
                    s.append_bytes(frag.as_bytes());
                }
                black_box(s)
            })
        });
        g.bench_function(BenchmarkId::new("std_string", label), |b| {
            b.iter(|| {
                let mut s = String::new();
                for _ in 0..n {
                    s.push_str(frag);
                }
                black_box(s)
            })
        });
        g.finish();

        // insert into the middle of pre-built content (ascii, so every byte is a char boundary).
        let content = frag.repeat(n);
        let mut g = group(c, "insert_bytes");
        g.bench_function(BenchmarkId::new("rope_utf8", label), |b| {
            b.iter_batched(
                || Rope::<Utf8>::from(content.clone()),
                |mut s| {
                    let mid = s.len() / 2;
                    s.insert_bytes(mid, b"XYZ");
                    black_box(s)
                },
                BatchSize::SmallInput,
            )
        });
        g.bench_function(BenchmarkId::new("std_string", label), |b| {
            b.iter_batched(
                || content.clone(),
                |mut s| {
                    let mid = s.len() / 2;
                    s.insert_str(mid, "XYZ");
                    black_box(s)
                },
                BatchSize::SmallInput,
            )
        });
        g.finish();
    }
}

/// Building a `ByteVec` through the [`Builder`] write path. `put_slice` copies each write into the
/// builder's coalescing head buffer (128 KiB here, flushed to a chunk when full); `put_bytes` holds
/// each `Bytes` by reference (the default inline threshold of 0). The naive reference builds a
/// `VecDeque<Bytes>` directly: `put_slice` against one `Bytes::copy_from_slice` per write (no
/// coalescing — the loss the head buffer exists to avoid), `put_bytes` against a bare `push_back`.
fn bench_builder(c: &mut Criterion) {
    use etude_buffer::writer::Buffer;
    const HEAD_CAP: usize = 128 * 1024;
    for &n in &[SHALLOW, DEEP] {
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let chunks: Vec<Bytes> = (0..n).map(|i| mtu_chunk(i as u8)).collect();

        let mut g = group(c, "builder_put_slice");
        g.bench_function(BenchmarkId::new("rope", label), |b| {
            b.iter(|| {
                let mut builder = ByteVec::builder(HEAD_CAP);
                for c in &chunks {
                    builder.put_slice(c);
                }
                black_box(builder.finish())
            })
        });
        g.bench_function(BenchmarkId::new("naive_deque", label), |b| {
            b.iter(|| {
                let mut v = NaiveVec::new();
                for c in &chunks {
                    v.push_back(Bytes::copy_from_slice(c));
                }
                black_box(v)
            })
        });
        g.finish();

        let mut g = group(c, "builder_put_bytes");
        g.bench_function(BenchmarkId::new("rope", label), |b| {
            b.iter(|| {
                let mut builder = ByteVec::builder(HEAD_CAP);
                for c in &chunks {
                    builder.put_bytes(c.clone());
                }
                black_box(builder.finish())
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

/// Reading a `ByteVec` non-destructively through [`ByteVec::reader`]: `reader()` takes an O(1)
/// structural-shared clone, then the [`Iterator`] yields each chunk (a cheap `Bytes` handle) via
/// `pop_front` on that clone, leaving the source untouched. The naive equivalent of a non-destructive
/// full read is an O(n) `clone()` of the deque followed by a `pop_front` drain, so the comparison
/// isolates the reader's O(1)-clone setup against the deque's copy-the-whole-spine setup.
fn bench_reader(c: &mut Criterion) {
    // Forking a Reader currently clones the rope state. Measure that clone in isolation across the
    // Small tier (where it copies the inline `VecDeque` + bumps each chunk handle) and the Deep tier
    // (where the tree clone is O(1) structural sharing), to size the "don't clone the small deque" idea.
    {
        let mut g = group(c, "reader_fork");
        for &n in &[1usize, 4, 16, 32, DEEP] {
            let rope = rope_of(n);
            g.bench_function(BenchmarkId::new("clone", n.to_string()), |b| {
                b.iter(|| black_box(rope.reader()))
            });
        }
        g.finish();
    }

    for &n in &[SHALLOW, DEEP] {
        let label = if n == SHALLOW { "shallow" } else { "deep" };
        let rope = rope_of(n);
        let naive = naive_of(n);

        let mut g = group(c, "reader_iterate");
        g.bench_function(BenchmarkId::new("rope", label), |b| {
            b.iter(|| {
                let mut count = 0usize;
                for chunk in rope.reader() {
                    count += chunk.len();
                }
                black_box(count)
            })
        });
        g.bench_function(BenchmarkId::new("naive_deque", label), |b| {
            b.iter(|| {
                let mut v = naive.clone();
                let mut count = 0usize;
                while let Some(chunk) = v.pop_front() {
                    count += chunk.len();
                }
                black_box(count)
            })
        });
        g.finish();
    }
}

/// Compaction: collapsing a fragmented rope into contiguous storage. Each "group" is a run of 8 small
/// (64 B) fragments followed by one large (2 KiB) chunk. Full `compact()` copies every byte into one
/// buffer; `compact_with(skip_above(1024))` coalesces each small run into one buffer but leaves the
/// large chunk in place (no memcpy). Same input for both, so the delta is the copy the config saves.
fn bench_compact(c: &mut Criterion) {
    fn mixed(groups: usize) -> ByteVec {
        let mut chunks: Vec<Bytes> = Vec::with_capacity(groups * 9);
        for i in 0..groups {
            for _ in 0..8 {
                chunks.push(Bytes::from(vec![i as u8; 64]));
            }
            chunks.push(Bytes::from(vec![i as u8; 2 * 1024]));
        }
        chunks.into_iter().collect()
    }
    for &groups in &[SHALLOW, DEEP] {
        let label = if groups == SHALLOW { "shallow" } else { "deep" };

        let mut g = group(c, "compact_full");
        g.bench_function(BenchmarkId::new("rope", label), |b| {
            b.iter_batched(
                || mixed(groups),
                |mut r| {
                    r.compact();
                    black_box(r)
                },
                BatchSize::SmallInput,
            )
        });
        g.finish();

        let mut g = group(c, "compact_skip_large");
        g.bench_function(BenchmarkId::new("rope", label), |b| {
            b.iter_batched(
                || mixed(groups),
                |mut r| {
                    r.compact_with(&CompactionConfig::new().skip_above(1024));
                    black_box(r)
                },
                BatchSize::SmallInput,
            )
        });
        g.finish();
    }

    // Single-chunk cases: a *shared* lone chunk (a small view pinning a large backing something else
    // holds) is copied out to release the backing; a *uniquely-owned* lone chunk is left untouched.
    // Contrasts the release copy against the near-free no-op.
    let backing = Bytes::from(vec![0xABu8; 64 * 1024]);
    let mut g = group(c, "compact_single_chunk");
    g.bench_function(BenchmarkId::new("rope", "shared_released"), |b| {
        b.iter_batched(
            || {
                // hold a second handle so the rope's chunk is shared (not unique)
                let keep = backing.clone();
                let r: ByteVec = [backing.clone()].into_iter().collect();
                (r, keep)
            },
            |(mut r, _keep)| {
                r.compact();
                black_box(r)
            },
            BatchSize::SmallInput,
        )
    });
    g.bench_function(BenchmarkId::new("rope", "unique_noop"), |b| {
        b.iter_batched(
            || {
                [Bytes::from(vec![0xABu8; 64 * 1024])]
                    .into_iter()
                    .collect::<ByteVec>()
            },
            |mut r| {
                r.compact();
                black_box(r)
            },
            BatchSize::SmallInput,
        )
    });
    g.finish();
}

/// Streaming FIFO workload — the canonical byte-rope use (socket / pipe buffering), which the isolated
/// `push_back` and `pop_front` benches do not cover because they never *interleave*. Two shapes:
///
/// - `stream_fifo`: a buffer held at a steady backlog while bytes flow through it — each round pushes
///   one chunk at the back and pops one at the front, so the length stays ~constant. At `deep` this
///   interleaves the push-freeze and pop-adopt tree machinery; at `boundary` it stays in the flat tier
///   without churning; at `shallow` it is a small flat buffer.
/// - `stream_churn`: the worst case for tiering — the backlog oscillates across *both* thresholds
///   (grow past `PROMOTE_AT` = 64, drain below `DEMOTE_AT` = 32, repeat), so every cycle pays a full
///   `promote` (deque → tree) and `demote` (tree → deque). Measures whether the hysteresis band is
///   wide enough to keep a realistically-oscillating buffer from thrashing the representation.
fn bench_stream(c: &mut Criterion) {
    const ROUNDS: usize = 64;
    let feed: Vec<Bytes> = (0..ROUNDS).map(|i| mtu_chunk(i as u8)).collect();

    // Steady-state FIFO: push one, pop one, holding the length at `backlog`.
    for &backlog in &[SHALLOW, 48usize, DEEP] {
        let label = match backlog {
            SHALLOW => "shallow".to_string(),
            DEEP => "deep".to_string(),
            n => format!("boundary_{n}"),
        };
        let mut g = group(c, "stream_fifo");
        g.bench_function(BenchmarkId::new("rope", &label), |b| {
            b.iter_batched_ref(
                || rope_of(backlog),
                |r| {
                    for chunk in &feed {
                        r.push_back(chunk.clone());
                        black_box(r.pop_front());
                    }
                },
                BatchSize::SmallInput,
            )
        });
        g.bench_function(BenchmarkId::new("naive_deque", &label), |b| {
            b.iter_batched_ref(
                || naive_of(backlog),
                |v| {
                    for chunk in &feed {
                        v.push_back(chunk.clone());
                        black_box(v.pop_front());
                    }
                },
                BatchSize::SmallInput,
            )
        });
        g.finish();
    }

    // Boundary-crossing churn: grow 20 -> 80 (promotes at 64), then drain 80 -> 20 (demotes at 32).
    const LO: usize = 20;
    const HI: usize = 80;
    let mut g = group(c, "stream_churn");
    g.bench_function(BenchmarkId::new("rope", "oscillate_20_80"), |b| {
        b.iter_batched_ref(
            || rope_of(LO),
            |r| {
                for chunk in feed.iter().take(HI - LO) {
                    r.push_back(chunk.clone());
                }
                for _ in 0..(HI - LO) {
                    black_box(r.pop_front());
                }
            },
            BatchSize::SmallInput,
        )
    });
    g.bench_function(BenchmarkId::new("naive_deque", "oscillate_20_80"), |b| {
        b.iter_batched_ref(
            || naive_of(LO),
            |v| {
                for chunk in feed.iter().take(HI - LO) {
                    v.push_back(chunk.clone());
                }
                for _ in 0..(HI - LO) {
                    black_box(v.pop_front());
                }
            },
            BatchSize::SmallInput,
        )
    });
    g.finish();
}

criterion_group!(
    benches,
    bench_stream,
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
    bench_from_iter,
    bench_starts_ends_with,
    bench_validate_utf8,
    bench_copy_to_bytes_mut,
    bench_socket_read,
    bench_utf8_mutate,
    bench_builder,
    bench_reader,
    bench_compact
);
criterion_main!(benches);
