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
//! wall-clock time**. The gap is the tokenizer's per-byte rope access (O(log n) leaf descent) vs
//! serde's O(1) contiguous-slice reads — not allocation, which is already near-zero. Closing it is the
//! chunk-cursor scan optimization (iterate leaves, not `byte_at` per byte; cf. etude-json #124's
//! chunk-ref fast path). This bench is the scoreboard that motivates and will track that work; it does
//! not yet claim a time win. A consumer that materializes every value pays more still — separate story.

use bytes::Bytes;
use criterion::{Criterion, criterion_group, criterion_main};
use etude_bytevec::ByteVec;
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

    group.finish();
}

fn benches(c: &mut Criterion) {
    bench_workload(c, "digest_mixed", mixed_doc(200));
    bench_workload(c, "digest_strings", string_doc(500));
}

criterion_group!(scoreboard, benches);
criterion_main!(scoreboard);
