// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Byte-movement benchmarks for the `etude-buffer` reader/writer traits.
//!
//! The hot path of these traits is `reader::Buffer::copy_into` — draining a reader into a writer —
//! which underlies every consumer that moves bytes through a buffer (including `etude-bytevec`). The
//! writer's `SPECIALIZES_BYTES` associated const decides whether a `Bytes` source is *copied* or
//! *moved by handle*. This benchmark contrasts the two paths on the same `Bytes` source:
//! - `Vec<u8>` (contiguous, `SPECIALIZES_BYTES = false`): `copy_into` memcpys the payload, so it
//!   scales with length.
//! - `VecDeque<Bytes>` (a chunk queue, `SPECIALIZES_BYTES = true`): `copy_into` moves the `Bytes`
//!   handle in with no copy, so it is flat regardless of length — the crate's copy-avoiding win.
//!
//! Run with `cargo bench -p etude-buffer`.

use bytes::Bytes;
use bytes::buf::UninitSlice;
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use etude_buffer::reader::Buffer as _;
use etude_buffer::reader::{Chain, IoSlice};
use etude_buffer::writer::Buffer as _;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::hint::black_box;
use std::time::Duration;

// Match the host allocator (jemalloc), so allocation-sensitive numbers reflect production behavior.
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn group<'a>(
    c: &'a mut Criterion,
    name: &str,
) -> criterion::BenchmarkGroup<'a, criterion::measurement::WallTime> {
    let mut g = c.benchmark_group(name);
    g.warm_up_time(Duration::from_millis(500));
    g.measurement_time(Duration::from_secs(2));
    g
}

/// `copy_into` from a contiguous `Bytes` source, at three payload sizes (a small header, an MTU
/// frame, and a 64 KiB block), into a copying writer (`Vec<u8>`) vs the zero-copy handle-move writer
/// (`VecDeque<Bytes>`). Each destination is built fresh inside the timed routine so the two paths are
/// measured on equal footing (dest allocation included in both).
fn bench_copy_into(c: &mut Criterion) {
    for &n in &[64usize, 1400, 65536] {
        let label = n.to_string();
        let src = Bytes::from(vec![0xABu8; n]);

        // Bytes -> Vec<u8>: SPECIALIZES_BYTES = false, so copy_into memcpys the payload.
        let mut g = group(c, "copy_into/to_vec_u8");
        g.bench_function(BenchmarkId::new("bytes", &label), |b| {
            b.iter_batched(
                || src.clone(),
                |mut r| {
                    let mut dst: Vec<u8> = Vec::with_capacity(n);
                    r.copy_into(&mut dst).unwrap();
                    black_box(dst)
                },
                BatchSize::SmallInput,
            )
        });
        g.finish();

        // Bytes -> VecDeque<Bytes>: SPECIALIZES_BYTES = true, so copy_into moves the handle (no copy).
        let mut g = group(c, "copy_into/to_bytes_queue");
        g.bench_function(BenchmarkId::new("bytes", &label), |b| {
            b.iter_batched(
                || src.clone(),
                |mut r| {
                    let mut dst: VecDeque<Bytes> = VecDeque::new();
                    r.copy_into(&mut dst).unwrap();
                    black_box(dst)
                },
                BatchSize::SmallInput,
            )
        });
        g.finish();
    }
}

/// `put_uninit_slice` writes directly into the destination's spare capacity via a closure, avoiding a
/// staging copy. It does not zero-initialize the exposed region: the closure reports how many leading
/// bytes it wrote and exactly that prefix is committed (a trusted-count contract — breaker #33). With
/// the redundant memset gone, filling the whole region is a single copy, so this contrasts
/// `put_uninit_slice` (fill + reported commit) against a plain `put_slice` of the same payload (a
/// single copy) at three sizes: the two should now sit on top of each other (the former zero-init
/// delta is gone).
fn bench_put_uninit(c: &mut Criterion) {
    for &n in &[64usize, 1400, 65536] {
        let label = n.to_string();
        let src = vec![0xABu8; n];

        // put_uninit_slice: no zero-init; the closure fills all `n` and reports `n`, so a single
        // write commits the payload — on the `put_slice` floor.
        let mut g = group(c, "put_uninit/uninit_fill");
        g.bench_function(BenchmarkId::new("vec", &label), |b| {
            b.iter_batched(
                || Vec::<u8>::with_capacity(n),
                |mut dst| {
                    // SAFETY: the closure initializes all `n` bytes it reports.
                    unsafe {
                        dst.put_uninit_slice::<_, Infallible>(n, |u: &mut UninitSlice| {
                            u.copy_from_slice(&src);
                            Ok(n)
                        })
                    }
                    .unwrap();
                    black_box(dst)
                },
                BatchSize::SmallInput,
            )
        });
        g.finish();

        // put_slice baseline: a single copy of the same payload, no zero-init.
        let mut g = group(c, "put_uninit/put_slice");
        g.bench_function(BenchmarkId::new("vec", &label), |b| {
            b.iter_batched(
                || Vec::<u8>::with_capacity(n),
                |mut dst| {
                    dst.put_slice(&src);
                    black_box(dst)
                },
                BatchSize::SmallInput,
            )
        });
        g.finish();
    }
}

/// `IoSlice` is the vectored reader: it presents a slice of byte segments as one logical stream, and
/// its `copy_into` drains them segment by segment (`read_chunk` per segment, then `put_slice`). This
/// contrasts draining the same 64 KiB as many small segments against a single contiguous `Bytes`
/// source, both into a fresh `Vec<u8>`, to isolate the per-segment overhead of the vectored path
/// (segment-boundary bookkeeping plus one `put_slice` per segment) on top of the underlying memcpy.
/// `IoSlice::new` scans the segments once to total their length, so its construction is included, as a
/// real vectored read must build the reader. (aarch64, jemalloc, release.)
fn bench_vectored(c: &mut Criterion) {
    const TOTAL: usize = 64 * 1024;

    for &(count, size) in &[(1024usize, 64usize), (256, 256), (16, 4096)] {
        let segments: Vec<Vec<u8>> = (0..count).map(|_| vec![0xABu8; size]).collect();
        let label = format!("{count}x{size}B");

        let mut g = group(c, "vectored/io_slice_to_vec");
        g.bench_function(BenchmarkId::from_parameter(&label), |b| {
            b.iter(|| {
                let mut r = IoSlice::new(&segments);
                let mut dst: Vec<u8> = Vec::with_capacity(TOTAL);
                r.copy_into(&mut dst).unwrap();
                black_box(dst)
            })
        });
        g.finish();
    }

    // Baseline: the same total bytes as one contiguous source — a single memcpy, no segment boundaries.
    let src = Bytes::from(vec![0xABu8; TOTAL]);
    let mut g = group(c, "vectored/contiguous_to_vec");
    g.bench_function(BenchmarkId::from_parameter("1x65536B"), |b| {
        b.iter(|| {
            let mut r = src.clone();
            let mut dst: Vec<u8> = Vec::with_capacity(TOTAL);
            r.copy_into(&mut dst).unwrap();
            black_box(dst)
        })
    });
    g.finish();
}

/// `Chain` drains reader `a` fully, then `b`, presenting them as one stream. This measures the
/// dispatch overhead of the chained drain by draining two 32 KiB `Bytes` halves via `Chain` versus a
/// single contiguous 64 KiB `Bytes`, both `copy_into` a fresh `Vec<u8>` (the same 64 KiB copied either
/// way). Any gap is `Chain`'s per-drain bookkeeping (the `buffer_is_empty` check and the `a`-then-`b`
/// hand-off), not extra copying. (aarch64, jemalloc, release.)
fn bench_chain(c: &mut Criterion) {
    const HALF: usize = 32 * 1024;
    let half = Bytes::from(vec![0xABu8; HALF]);
    let whole = Bytes::from(vec![0xABu8; 2 * HALF]);

    let mut g = group(c, "chain/two_halves_to_vec");
    g.bench_function(BenchmarkId::from_parameter("2x32KiB"), |b| {
        b.iter(|| {
            let mut r = Chain::new(half.clone(), half.clone());
            let mut dst: Vec<u8> = Vec::with_capacity(2 * HALF);
            r.copy_into(&mut dst).unwrap();
            black_box(dst)
        })
    });
    g.finish();

    let mut g = group(c, "chain/one_source_to_vec");
    g.bench_function(BenchmarkId::from_parameter("1x64KiB"), |b| {
        b.iter(|| {
            let mut r = whole.clone();
            let mut dst: Vec<u8> = Vec::with_capacity(2 * HALF);
            r.copy_into(&mut dst).unwrap();
            black_box(dst)
        })
    });
    g.finish();
}

/// `partial_copy_into` copies until the destination is full and *returns* the trailing chunk for the
/// caller to place — it is the zero-copy hand-off point (a chunk-holding sink or a rope can adopt that
/// `Bytes` by reference). `copy_into` instead copies that trailing chunk too. This contrasts the two on
/// a single 64 KiB `Bytes` drained into a `Vec<u8>` (a non-specializing sink): `partial_copy_into`
/// returns the whole chunk via a refcount move (`split_to`, no memcpy), while `copy_into` memcpys it.
/// The destination `Vec` is pre-allocated in both arms, so the delta is exactly the trailing-chunk copy.
fn bench_partial(c: &mut Criterion) {
    const N: usize = 64 * 1024;
    let src = Bytes::from(vec![0xABu8; N]);

    let mut g = group(c, "partial/return_trailing");
    g.bench_function(BenchmarkId::from_parameter("64KiB"), |b| {
        b.iter(|| {
            let mut r = src.clone();
            let mut dst: Vec<u8> = Vec::with_capacity(N);
            let trailing = r.partial_copy_into(&mut dst).unwrap();
            black_box((dst, trailing.len()))
        })
    });
    g.finish();

    let mut g = group(c, "partial/copy_trailing");
    g.bench_function(BenchmarkId::from_parameter("64KiB"), |b| {
        b.iter(|| {
            let mut r = src.clone();
            let mut dst: Vec<u8> = Vec::with_capacity(N);
            r.copy_into(&mut dst).unwrap();
            black_box(dst)
        })
    });
    g.finish();
}

criterion_group!(
    benches,
    bench_copy_into,
    bench_put_uninit,
    bench_vectored,
    bench_chain,
    bench_partial
);
criterion_main!(benches);
