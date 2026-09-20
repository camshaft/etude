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
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use etude_buffer::reader::Buffer as _;
use std::collections::VecDeque;
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

criterion_group!(benches, bench_copy_into);
criterion_main!(benches);
