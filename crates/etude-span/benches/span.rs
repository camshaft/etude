// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Scan-throughput benchmarks for the `etude-span` [`Cursor`].
//!
//! The cursor exists so a tokenizer can walk a chunked rope in O(n) amortized time — each leaf is
//! touched once with a local slice index — instead of the O(log n) tree descent a per-byte
//! [`ByteVec::byte_at`] pays on every byte (O(n log n) total, cache-hostile). This benchmark scans
//! the same rope three ways at several leaf layouts:
//! - `cursor/peek_bump`: the byte-at-a-time tokenizer inner loop (`peek` then `bump`).
//! - `cursor/bulk`: the bulk path (`chunk_tail` scans a whole leaf, `skip_in_chunk` jumps past it).
//! - `byte_at/scan`: the naive baseline the cursor replaces — `byte_at(i)` for every offset.
//!
//! Leaf layout is the variable that matters: `refill` fires at every leaf boundary, and `byte_at`
//! gets cheaper per byte as leaves grow, so the three are compared at 64 B, 4 KiB, and single-leaf
//! layouts of the same 64 KiB input.
//!
//! Run with `cargo bench -p etude-span`.

use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use etude_bytevec::ByteVec;
use etude_span::{Cursor, Span};
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

/// Build a rope holding `bytes`, split into fixed-size leaves of at most `chunk` bytes.
fn rope_chunked(bytes: &[u8], chunk: usize) -> ByteVec {
    let mut r = ByteVec::new();
    let chunk = chunk.max(1);
    let mut i = 0;
    while i < bytes.len() {
        let j = (i + chunk).min(bytes.len());
        r.push_back(Bytes::copy_from_slice(&bytes[i..j]));
        i = j;
    }
    r
}

/// Scan a 64 KiB input laid out at three leaf sizes, three ways each. The cursor allocates nothing,
/// so the timed routine is a pure walk; a running byte sum is returned through `black_box` so the
/// scan cannot be elided. `byte_at` is the O(log n)-per-byte descent the cursor is built to avoid.
fn bench_scan(c: &mut Criterion) {
    const N: usize = 64 * 1024;
    let data = vec![0xABu8; N];

    for &chunk in &[64usize, 4096, N] {
        let label = match chunk {
            N => "1_leaf".to_string(),
            4096 => "4KiB_leaves".to_string(),
            other => format!("{other}B_leaves"),
        };
        let rope = rope_chunked(&data, chunk);

        // Byte-at-a-time cursor walk: the tokenizer inner loop (peek, then bump).
        let mut g = group(c, "cursor/peek_bump");
        g.bench_function(BenchmarkId::from_parameter(&label), |b| {
            b.iter(|| {
                let mut cur = Cursor::new(&rope);
                let mut sum = 0u64;
                while let Some(byte) = cur.peek() {
                    sum = sum.wrapping_add(byte as u64);
                    cur.bump();
                }
                black_box(sum)
            })
        });
        g.finish();

        // Bulk cursor walk: scan each leaf via chunk_tail, then skip_in_chunk past it in one step.
        let mut g = group(c, "cursor/bulk");
        g.bench_function(BenchmarkId::from_parameter(&label), |b| {
            b.iter(|| {
                let mut cur = Cursor::new(&rope);
                let mut sum = 0u64;
                loop {
                    let tail = cur.chunk_tail();
                    if tail.is_empty() {
                        break;
                    }
                    for &byte in tail {
                        sum = sum.wrapping_add(byte as u64);
                    }
                    cur.skip_in_chunk(tail.len());
                }
                black_box(sum)
            })
        });
        g.finish();

        // Naive baseline: byte_at(i) for every offset — an O(log n) tree descent per byte.
        let mut g = group(c, "byte_at/scan");
        g.bench_function(BenchmarkId::from_parameter(&label), |b| {
            b.iter(|| {
                let mut sum = 0u64;
                for i in 0..N {
                    sum = sum.wrapping_add(rope.byte_at(i).unwrap() as u64);
                }
                black_box(sum)
            })
        });
        g.finish();
    }
}

/// Build `n` bytes where a delimiter (`0x20`) lands on every `run + 1`-th byte, so the input is a
/// stream of ~`run`-byte tokens separated by single delimiters.
fn delimited(n: usize, run: usize) -> Vec<u8> {
    const DELIM: u8 = b' ';
    let mut v = vec![0xABu8; n];
    let stride = run + 1;
    let mut i = run;
    while i < n {
        v[i] = DELIM;
        i += stride;
    }
    v
}

/// The realistic tokenizer shape: a delimited stream whose tokens straddle 64 B leaves. A tokenizer
/// skips delimiters and stamps a [`Span`] per token. This contrasts the two ways to scan each token's
/// run — the bulk path (find the next delimiter in `chunk_tail` with a slice scan, `skip_in_chunk` to
/// it, continuing across leaf boundaries) versus naive per-byte `peek`/`bump` — at three token
/// lengths. Short runs are delimiter-dense (per-byte cost dominates either way); long runs straddle
/// leaves and reward the bulk scan. Each token stamps a `Span`, as a real tokenizer would.
fn bench_tokenize(c: &mut Criterion) {
    const N: usize = 64 * 1024;
    const LEAF: usize = 64;
    const DELIM: u8 = b' ';

    for &run in &[4usize, 16, 100] {
        let rope = rope_chunked(&delimited(N, run), LEAF);
        let label = format!("run{run}");

        // Bulk: within each leaf, locate the next delimiter with a slice scan and skip to it; a token
        // that reaches the leaf end continues into the next leaf.
        let mut g = group(c, "tokenize/bulk");
        g.bench_function(BenchmarkId::from_parameter(&label), |b| {
            b.iter(|| {
                let mut cur = Cursor::new(&rope);
                let mut token_bytes = 0u64;
                while let Some(byte) = cur.peek() {
                    if byte == DELIM {
                        cur.bump();
                        continue;
                    }
                    let start = cur.offset();
                    loop {
                        let tail = cur.chunk_tail();
                        if tail.is_empty() {
                            break;
                        }
                        match tail.iter().position(|&x| x == DELIM) {
                            Some(k) => {
                                cur.skip_in_chunk(k);
                                break;
                            }
                            None => cur.skip_in_chunk(tail.len()),
                        }
                    }
                    token_bytes += black_box(Span::new(start, cur.offset())).len() as u64;
                }
                black_box(token_bytes)
            })
        });
        g.finish();

        // Naive: the same tokenization done purely with per-byte peek/bump.
        let mut g = group(c, "tokenize/peek_bump");
        g.bench_function(BenchmarkId::from_parameter(&label), |b| {
            b.iter(|| {
                let mut cur = Cursor::new(&rope);
                let mut token_bytes = 0u64;
                while let Some(byte) = cur.peek() {
                    if byte == DELIM {
                        cur.bump();
                        continue;
                    }
                    let start = cur.offset();
                    while let Some(x) = cur.peek() {
                        if x == DELIM {
                            break;
                        }
                        cur.bump();
                    }
                    token_bytes += black_box(Span::new(start, cur.offset())).len() as u64;
                }
                black_box(token_bytes)
            })
        });
        g.finish();
    }
}

criterion_group!(benches, bench_scan, bench_tokenize);
criterion_main!(benches);
