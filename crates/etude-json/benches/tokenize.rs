// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Head-to-head scoreboard: `etude_json`'s copy-avoiding tokenizer vs `serde_json`, across document
//! shapes. Two axes are measured, allocations first-class (the whole point of the copy-avoiding
//! design) and time second:
//!
//! - Allocations: tokenizing allocates nothing — a [`etude_json::Token`] is a value carrying a rope
//!   span, so draining the whole stream touches zero heap. `serde_json` builds a full `Value` tree
//!   (every array a `Vec`, every string a `String`, every object a `Map`). The alloc table below is
//!   the headline: bytes/allocations per parse.
//! - Time: draining the token stream (find + validate every token) vs parsing to a `Value`. This is
//!   not the same work — the tokenizer produces spans, `serde_json` produces an owned tree — so read
//!   the time as "cost to walk the document", with the allocation column carrying the real story.
//!
//! Input is fed through a chunked [`ByteVec`] rope (8 KiB leaves) so the tokenizer's chunk-streaming
//! cursor is exercised as it would be on a rope assembled from network reads; `serde_json` gets the
//! same bytes as one contiguous slice (its only input shape). Rope construction happens outside the
//! measured region, so neither the timing nor the allocation counts include building the input.
//!
//! Run with `cargo bench -p etude-json`. The allocation table prints first, then criterion timings.

use bytes::Bytes;
use criterion::{Criterion, criterion_group, criterion_main};
use etude_bytevec::ByteVec;
use etude_json::{Token, TokenKind, Tokenizer};
use std::alloc::{GlobalAlloc, Layout};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

// ─── allocation-counting global allocator ────────────────────────────────────────────────────────
//
// Wraps jemalloc (the host allocator, matching the other etude benches) and, only while `COUNTING`
// is set, tallies allocation calls + requested bytes. Timing runs with counting off, so the steady
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

/// The string-decode corpus: (name, JSON array-of-strings bytes). Each shape stresses a different
/// path of `Token::decode_string`: unescaped content (a straight copy), sparse escapes, dense unicode
/// escapes (surrogate decoding), and a single large value. The reference is `serde_json` parsing the
/// same bytes into a `Vec<String>` — the fair differential, since that also materializes exactly one
/// owned `String` per element and nothing else.
fn decode_corpus() -> Vec<(&'static str, String)> {
    // Many short strings with no escapes — the common web-payload shape; decode is a plain copy.
    let mut no_escape = String::from("[");
    for i in 0..5_000 {
        if i > 0 {
            no_escape.push(',');
        }
        no_escape.push('"');
        no_escape.push_str(itoa(i).as_str());
        no_escape.push_str("_value_field");
        no_escape.push('"');
    }
    no_escape.push(']');

    // Strings with a few escapes each — decode copies runs, then expands `\n` `\t` `\"`.
    let mut escaped = String::from("[");
    for i in 0..5_000 {
        if i > 0 {
            escaped.push(',');
        }
        escaped.push_str(&format!(r#""line {i}\tcol\ttab\nnext\t\"quoted\"""#));
    }
    escaped.push(']');

    // Strings that are all `\u` escapes — the surrogate/hex-decode path.
    let mut unicode = String::from("[");
    for i in 0..2_000 {
        if i > 0 {
            unicode.push(',');
        }
        // A BMP escape, an accented char, and an astral char via a surrogate pair.
        unicode.push_str(&format!(r#""éA{i}😀""#));
    }
    unicode.push(']');

    // One large unescaped string — decode is a single big copy (isolates per-byte copy throughput).
    let mut big_no_escape = String::from("[\"");
    for _ in 0..100_000 {
        big_no_escape.push('a');
    }
    big_no_escape.push_str("\"]");

    // One large string that is one-third escapes — decode alternates copy runs with expansions.
    let mut big_escaped = String::from("[\"");
    for _ in 0..30_000 {
        big_escaped.push_str("ab\\n");
    }
    big_escaped.push_str("\"]");

    vec![
        ("no_escape_5k", no_escape),
        ("escaped_5k", escaped),
        ("unicode_2k", unicode),
        ("big_no_escape_100k", big_no_escape),
        ("big_escaped_90k", big_escaped),
    ]
}

/// Collect the `String`-kind tokens of `input` (done outside the measured region so a decode
/// benchmark times only `Token::decode_string`, not the tokenize scan).
fn string_tokens(input: &ByteVec) -> Vec<Token> {
    Tokenizer::new(input)
        .map(|t| t.expect("valid json"))
        .filter(|t| t.kind() == TokenKind::String)
        .collect()
}

/// Decode every collected string token's content to an owned `String` — the on-demand
/// unescape/materialize cost of `Token::decode_string`, in isolation.
fn decode_all(tokens: &[Token], input: &ByteVec) {
    for t in tokens {
        black_box(t.decode_string(input).expect("string token decodes"));
    }
}

/// Decode every collected string token to a `StrRope` — the copy-avoiding string value path
/// (`Token::decode_str_rope`): a zero-copy structural-share of the rope when the string has no
/// escapes, a built `StrRope` when it does. The delta over `decode_all` is the allocation avoided.
fn decode_all_rope(tokens: &[Token], input: &ByteVec) {
    for t in tokens {
        black_box(t.decode_str_rope(input).expect("string token decodes"));
    }
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

/// Tokenize and resolve every token's span back to a rope slice — the O(log n)-per-span re-access a
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

/// Allocation scoreboard for string decoding: `Token::decode_string` over every string in the doc vs
/// `serde_json` parsing the same bytes into a `Vec<String>`. Both materialize one owned `String` per
/// element, so this compares the decode path's allocation behavior head to head.
fn print_decode_alloc_scoreboard(corpus: &[(&'static str, String)]) {
    println!(
        "\n=== decode allocation scoreboard (per parse) — Token::decode_string vs serde_json Vec<String> ==="
    );
    println!(
        "{:<20} {:>10} {:>12} {:>10} {:>12} {:>10} {:>12}",
        "shape", "str_allocs", "str_bytes", "rope_allocs", "rope_bytes", "sj_allocs", "sj_bytes"
    );
    for (name, doc) in corpus {
        let bytes = doc.as_bytes();
        let input = rope(bytes);
        let tokens = string_tokens(&input);
        let (str_allocs, str_bytes) = count_allocs(|| decode_all(&tokens, &input));
        let (rope_allocs, rope_bytes) = count_allocs(|| decode_all_rope(&tokens, &input));
        let (sj_allocs, sj_bytes) =
            count_allocs(|| serde_json::from_slice::<Vec<String>>(bytes).unwrap());
        println!(
            "{name:<20} {str_allocs:>10} {str_bytes:>12} {rope_allocs:>10} {rope_bytes:>12} {sj_allocs:>10} {sj_bytes:>12}"
        );
    }
    println!();
}

// ─── criterion timing ────────────────────────────────────────────────────────────────────────────

fn bench(c: &mut Criterion) {
    let corpus = corpus();
    print_alloc_scoreboard(&corpus);
    let dcorpus = decode_corpus();
    print_decode_alloc_scoreboard(&dcorpus);
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

    // Per-function: Token::decode_string (string materialization) vs serde_json Vec<String>. Tokens
    // are collected outside the timed closure so only decode is measured.
    for (name, doc) in &dcorpus {
        let bytes = doc.as_bytes();
        let input = rope(bytes);
        let tokens = string_tokens(&input);
        let mut group = c.benchmark_group(format!("decode/{name}"));
        group.bench_function("etude_json_decode_string", |b| {
            b.iter(|| decode_all(black_box(&tokens), black_box(&input)))
        });
        group.bench_function("etude_json_decode_str_rope", |b| {
            b.iter(|| decode_all_rope(black_box(&tokens), black_box(&input)))
        });
        group.bench_function("serde_json_vec_string", |b| {
            b.iter(|| serde_json::from_slice::<Vec<String>>(black_box(bytes)).unwrap())
        });
        group.finish();
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
