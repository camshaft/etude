// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Wall-clock scoreboard vs serde_json — the time counterpart to `tests/alloc_scoreboard.rs`. The
//! workload is a *streaming digest* (count nodes, sum string lengths, retain nothing): the case the
//! rope-native SAX seam is for. serde_json must build the whole value tree first; the adapter drives a
//! `Visitor` over the token stream, handing escape-free strings as O(1) `RopeStr::Borrowed` slices and
//! keeping only a fixed-size accumulator. Run with `cargo bench -p etude-json-serde`.
//!
//! Current standing (read alongside `tests/alloc_scoreboard.rs`): the adapter wins **allocation**
//! decisively (~2200x fewer allocs on the mixed doc) but currently **trails serde_json ~3x on
//! wall-clock time**. The `raw_tokenize` baseline below *attributes* that gap, and it is **not** the
//! rope scan: raw tokenization is ~59µs on the mixed doc — faster than serde's full 147µs parse. The
//! ~491µs of adapter overhead is the **per-token handoff**, chiefly (a) `StrRope::from_utf8` re-scanning
//! each escape-free string to validate UTF-8 the tokenizer already guaranteed (O(n) per string; wants a
//! checked-elsewhere `from_utf8_unchecked`), and (b) eager `NumberToken` sub-rope slicing (lexeme +
//! three components per number) even when the consumer ignores them. So the latency lever is the
//! *handoff*, not a chunk-cursor scan of the lexer. This bench tracks that; it claims no time win yet.
//! A consumer that materializes every value pays more still — a separate story.
//!
//! The isolated workloads confirm the split (raw_tokenize ≈ or beats serde in every one):
//! - `digest_numbers`: serde ~21µs, raw_tokenize ~20µs, adapter ~130µs — a **6.5x** handoff tax, ~220ns
//!   per number building four eager sub-ropes the digest ignores. The sharpest case for lazy components.
//! - `digest_strings`: serde ~29µs, raw_tokenize ~26µs, adapter ~106µs — the `from_utf8` re-validation.

use bytes::Bytes;
use criterion::{Criterion, criterion_group, criterion_main};
use etude_bytevec::ByteVec;
use etude_json::Tokenizer;
use etude_json_serde::from_rope;
use etude_serde::{Error, MapAccess, NumberToken, RopeBytes, RopeStr, SeqAccess, Visitor};
use serde_json::Value;
use std::hint::black_box;

// Match the host allocator; parsing is allocation-bound for the tree builder, so this keeps the
// numbers production-representative (as in the bigint/bytevec benches).
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// ---- digest ----------------------------------------------------------------------------------

#[derive(Clone, Copy, Default)]
struct Stats {
    scalars: u64,
    containers: u64,
    string_bytes: u64,
}

impl Stats {
    fn merge(self, o: Stats) -> Stats {
        Stats {
            scalars: self.scalars + o.scalars,
            containers: self.containers + o.containers,
            string_bytes: self.string_bytes + o.string_bytes,
        }
    }
    fn scalar(string_bytes: u64) -> Stats {
        Stats {
            scalars: 1,
            containers: 0,
            string_bytes,
        }
    }
}

struct Digest;

impl Visitor for Digest {
    type Value = Stats;

    fn visit_null(self) -> Result<Stats, Error> {
        Ok(Stats::scalar(0))
    }
    fn visit_bool(self, _: bool) -> Result<Stats, Error> {
        Ok(Stats::scalar(0))
    }
    fn visit_str(self, s: RopeStr) -> Result<Stats, Error> {
        Ok(Stats::scalar(s.len() as u64))
    }
    fn visit_bytes(self, _: RopeBytes) -> Result<Stats, Error> {
        Ok(Stats::scalar(0))
    }
    fn visit_number(self, _: NumberToken) -> Result<Stats, Error> {
        Ok(Stats::scalar(0))
    }
    fn visit_seq<A: SeqAccess>(self, mut seq: A) -> Result<Stats, Error> {
        let mut acc = Stats {
            containers: 1,
            ..Stats::default()
        };
        while let Some(s) = seq.next_element(Digest)? {
            acc = acc.merge(s);
        }
        Ok(acc)
    }
    fn visit_map<A: MapAccess>(self, mut map: A) -> Result<Stats, Error> {
        let mut acc = Stats {
            containers: 1,
            ..Stats::default()
        };
        while let Some(k) = map.next_key(Digest)? {
            let v = map.next_value(Digest)?;
            acc = acc.merge(k).merge(v);
        }
        Ok(acc)
    }
}

fn digest_value(v: &Value) -> Stats {
    match v {
        Value::Null | Value::Bool(_) | Value::Number(_) => Stats::scalar(0),
        Value::String(s) => Stats::scalar(s.len() as u64),
        Value::Array(items) => items.iter().fold(
            Stats {
                containers: 1,
                ..Stats::default()
            },
            |a, it| a.merge(digest_value(it)),
        ),
        Value::Object(entries) => entries.iter().fold(
            Stats {
                containers: 1,
                ..Stats::default()
            },
            |a, (k, val)| {
                a.merge(Stats::scalar(k.len() as u64))
                    .merge(digest_value(val))
            },
        ),
    }
}

// ---- documents -------------------------------------------------------------------------------

/// Container/string-heavy: objects with escape-free string fields, an int, and a small nested array.
fn mixed_doc(count: usize) -> String {
    let mut s = String::from("[");
    for i in 0..count {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            r#"{{"name":"item-{i}","kind":"widget","tag":"stable","id":{i},"scores":[{i},{},{},{}]}}"#,
            i + 1,
            i + 2,
            i + 3
        ));
    }
    s.push(']');
    s
}

/// String-heavy: a flat array of escape-free strings — the zero-copy Borrowed win in isolation.
fn string_doc(count: usize) -> String {
    let mut s = String::from("[");
    for i in 0..count {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(r#""a moderately sized string value number {i}""#));
    }
    s.push(']');
    s
}

/// Number-heavy: a flat array of varied numbers (ints, decimals, exponents) — isolates the number
/// handoff (eager lexeme + component sub-rope slicing) from the string path.
fn number_doc(count: usize) -> String {
    let mut s = String::from("[");
    for i in 0..count {
        if i > 0 {
            s.push(',');
        }
        // Rotate through integer / decimal / exponent forms so every NumberToken component arm fires.
        match i % 3 {
            0 => s.push_str(&format!("{}", i as i64 - 250)),
            1 => s.push_str(&format!("{}.{:03}", i, i % 1000)),
            _ => s.push_str(&format!("-{}.{}e{}", i % 100, i % 10, (i % 20) as i64 - 10)),
        }
    }
    s.push(']');
    s
}

fn rope_of(raw: &[u8]) -> ByteVec {
    let mut r = ByteVec::new();
    r.push_back(Bytes::copy_from_slice(raw));
    r
}

// ---- benches ---------------------------------------------------------------------------------

fn bench_workload(c: &mut Criterion, name: &str, doc: String) {
    let raw = doc.into_bytes();
    let rope = rope_of(&raw);

    let mut group = c.benchmark_group(name);
    group.throughput(criterion::Throughput::Bytes(raw.len() as u64));

    group.bench_function("serde_json_value", |b| {
        b.iter(|| {
            let v: Value = serde_json::from_slice(black_box(&raw)).unwrap();
            black_box(digest_value(&v))
        })
    });
    group.bench_function("etude_adapter", |b| {
        b.iter(|| black_box(from_rope(black_box(&rope), Digest).unwrap()))
    });
    // Attribution baseline: the raw tokenizer scan alone (count tokens, touch no content). Isolates
    // the lexer's per-byte rope-scan cost from the adapter's grammar + per-token sub-rope slicing, so
    // the ~3x gap vs serde can be pinned on the scan (byte_at) vs the handoff.
    group.bench_function("raw_tokenize", |b| {
        b.iter(|| {
            let mut n = 0usize;
            for tok in Tokenizer::new(black_box(&rope)) {
                black_box(tok.unwrap());
                n += 1;
            }
            black_box(n)
        })
    });

    group.finish();
}

fn benches(c: &mut Criterion) {
    bench_workload(c, "digest_mixed", mixed_doc(200));
    bench_workload(c, "digest_strings", string_doc(500));
    bench_workload(c, "digest_numbers", number_doc(500));
}

criterion_group!(scoreboard, benches);
criterion_main!(scoreboard);
