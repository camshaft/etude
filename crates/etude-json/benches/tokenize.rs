// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Head-to-head scoreboard: `etude_json`'s copy-avoiding tokenizer vs `serde_json`, across document
//! shapes. Two axes are measured, ALLOCATIONS first-class (the whole point of the copy-avoiding
//! design) and time second:
//!
//! - ALLOCATIONS: tokenizing allocates nothing — a [`etude_json::Token`] is a value carrying a rope
//!   span, so draining the whole stream touches zero heap. `serde_json` builds a full `Value` tree
//!   (every array a `Vec`, every string a `String`, every object a `Map`). The alloc table below is
//!   the headline: bytes/allocations per parse.
//! - TIME: draining the token stream (find + validate every token) vs parsing to a `Value`. This is
//!   not the same work — the tokenizer produces spans, `serde_json` produces an owned tree — so read
//!   the time as "cost to walk the document", with the allocation column carrying the real story.
//!
//! Input is fed through a chunked [`ByteVec`] rope (8 KiB leaves) so the tokenizer's chunk-streaming
//! cursor is exercised as it would be on a rope assembled from network reads; `serde_json` gets the
//! same bytes as one contiguous slice (its only input shape). Rope construction happens OUTSIDE the
//! measured region, so neither the timing nor the allocation counts include building the input.
//!
//! Run with `cargo bench -p etude-json`. The allocation table prints first, then criterion timings.

use bytes::Bytes;
use criterion::{Criterion, criterion_group, criterion_main};
use etude_bytevec::ByteVec;
use etude_json::Tokenizer;
use std::alloc::{GlobalAlloc, Layout};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

// ─── allocation-counting global allocator ────────────────────────────────────────────────────────
//
// Wraps jemalloc (the host allocator, matching the other etude benches) and, only while `COUNTING`
// is set, tallies allocation calls + requested bytes. Timing runs with counting OFF, so the steady
// overhead is a single relaxed load per allocation.

static INNER: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static COUNTING: AtomicBool = AtomicBool::new(false);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Relaxed) {
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(layout.size() as u64, Relaxed);
        }
        unsafe { INNER.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { INNER.dealloc(ptr, layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if COUNTING.load(Relaxed) {
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(layout.size() as u64, Relaxed);
        }
        unsafe { INNER.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.load(Relaxed) {
            ALLOCS.fetch_add(1, Relaxed);
            BYTES.fetch_add(new_size as u64, Relaxed);
        }
        unsafe { INNER.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Run `f` with allocation counting enabled, returning (allocation calls, requested bytes).
fn count_allocs<T>(f: impl FnOnce() -> T) -> (u64, u64) {
    ALLOCS.store(0, Relaxed);
    BYTES.store(0, Relaxed);
    COUNTING.store(true, Relaxed);
    let out = f();
    COUNTING.store(false, Relaxed);
    black_box(out);
    (ALLOCS.load(Relaxed), BYTES.load(Relaxed))
}

// ─── document shapes ──────────────────────────────────────────────────────────────────────────────

/// The benchmark corpus: (name, JSON bytes). Depth is kept under `serde_json`'s default 128-frame
/// recursion limit so both parsers accept every document.
fn corpus() -> Vec<(&'static str, String)> {
    let mut array_ints = String::from("[");
    for i in 0..10_000 {
        if i > 0 {
            array_ints.push(',');
        }
        array_ints.push_str(itoa(i).as_str());
    }
    array_ints.push(']');

    let mut array_floats = String::from("[");
    for i in 0..10_000 {
        if i > 0 {
            array_floats.push(',');
        }
        array_floats.push_str(&format!("{}.5", i));
    }
    array_floats.push(']');

    let mut array_strings = String::from("[");
    for i in 0..5_000 {
        if i > 0 {
            array_strings.push(',');
        }
        array_strings.push('"');
        array_strings.push_str(itoa(i).as_str());
        array_strings.push_str("_value");
        array_strings.push('"');
    }
    array_strings.push(']');

    let mut big_string = String::from("\"");
    for _ in 0..100_000 {
        big_string.push('a');
    }
    big_string.push('"');

    let nested = format!("{}{}{}", "[".repeat(100), "1", "]".repeat(100));

    let mut objects = String::from("[");
    for i in 0..1_000 {
        if i > 0 {
            objects.push(',');
        }
        objects.push_str(&format!(
            r#"{{"id":{i},"name":"item {i}","active":true,"score":{i}.25,"tags":["a","b"]}}"#
        ));
    }
    objects.push(']');

    vec![
        ("array_10k_ints", array_ints),
        ("array_10k_floats", array_floats),
        ("array_5k_strings", array_strings),
        ("big_string_100k", big_string),
        ("nested_100", nested),
        ("objects_1k", objects),
    ]
}

/// Minimal `usize`→decimal without allocating through `format!` in the hot corpus loop.
fn itoa(mut n: usize) -> String {
    if n == 0 {
        return String::from("0");
    }
    let mut digits = [0u8; 20];
    let mut i = digits.len();
    while n > 0 {
        i -= 1;
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    String::from_utf8_lossy(&digits[i..]).into_owned()
}

/// Build a chunked rope (8 KiB leaves) from `bytes`, so the tokenizer's chunk cursor is exercised.
fn rope(bytes: &[u8]) -> ByteVec {
    let mut r = ByteVec::new();
    for chunk in bytes.chunks(8 * 1024) {
        r.push_back(Bytes::copy_from_slice(chunk));
    }
    r
}

/// Drain the whole token stream, touching every token. Allocates nothing.
fn tokenize(input: &ByteVec) {
    for tok in Tokenizer::new(input) {
        black_box(tok.expect("valid json"));
    }
}

/// Tokenize AND resolve every token's span back to a rope slice — the O(log n)-per-span re-access a
/// consumer pays to read a token's bytes from an `(offset, len)` span (a tree descent to locate the
/// span's leaf). The delta over [`tokenize`] isolates that span-resolution cost, which is what a
/// chunk-ref-carrying token would drive toward O(1).
fn tokenize_and_read(input: &ByteVec) {
    for tok in Tokenizer::new(input) {
        let t = tok.expect("valid json");
        black_box(input.slice(t.span().range()));
    }
}

// ─── the allocation scoreboard (printed before timings) ──────────────────────────────────────────

fn print_alloc_scoreboard(corpus: &[(&'static str, String)]) {
    println!(
        "\n=== allocation scoreboard (per parse) — etude_json tokenize vs serde_json parse ==="
    );
    println!(
        "{:<20} {:>10} {:>14} {:>10} {:>14} {:>12}",
        "shape", "ej_allocs", "ej_bytes", "sj_allocs", "sj_bytes", "bytes_ratio"
    );
    for (name, doc) in corpus {
        let bytes = doc.as_bytes();
        let input = rope(bytes);
        let (ej_allocs, ej_bytes) = count_allocs(|| tokenize(&input));
        let (sj_allocs, sj_bytes) =
            count_allocs(|| serde_json::from_slice::<serde_json::Value>(bytes).unwrap());
        let ratio = if sj_bytes == 0 {
            0.0
        } else {
            ej_bytes as f64 / sj_bytes as f64
        };
        println!(
            "{name:<20} {ej_allocs:>10} {ej_bytes:>14} {sj_allocs:>10} {sj_bytes:>14} {ratio:>12.4}"
        );
    }
    println!();
}

// ─── criterion timing ────────────────────────────────────────────────────────────────────────────

fn bench(c: &mut Criterion) {
    let corpus = corpus();
    print_alloc_scoreboard(&corpus);
    for (name, doc) in &corpus {
        let bytes = doc.as_bytes();
        let input = rope(bytes);
        let mut group = c.benchmark_group(*name);
        group.bench_function("etude_json_tokenize", |b| {
            b.iter(|| tokenize(black_box(&input)))
        });
        group.bench_function("etude_json_tokenize_and_read", |b| {
            b.iter(|| tokenize_and_read(black_box(&input)))
        });
        group.bench_function("serde_json_parse", |b| {
            b.iter(|| serde_json::from_slice::<serde_json::Value>(black_box(bytes)).unwrap())
        });
        group.finish();
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
