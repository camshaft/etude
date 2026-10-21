// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Allocation scoreboard: the copy-avoiding thesis, measured. A *streaming digest* over a document
//! (count nodes, sum string lengths — process and discard, never retain a tree) is the workload where
//! the rope-native SAX adapter should win: `serde_json::from_slice::<Value>` must allocate the entire
//! value tree (every `Vec`, every `Map`, every `String`), while the adapter drives a `Visitor` that
//! keeps only a fixed-size accumulator and hands escape-free strings as O(1) `RopeStr::Borrowed`
//! slices.
//!
//! This is an integration test (its own binary) so the counting `#[global_allocator]` is isolated to
//! it and does not perturb the unit-test binary. It runs the two parses sequentially on one thread and
//! asserts the adapter allocates strictly fewer times AND fewer bytes, printing the ratios (run with
//! `--nocapture` to see the scoreboard).

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use bytes::Bytes;
use etude_bytevec::ByteVec;
use etude_json_serde::from_rope;
use etude_serde::{Error, MapAccess, NumberToken, RopeBytes, RopeStr, SeqAccess, Visitor};
use serde_json::Value;

// ---- counting allocator ----------------------------------------------------------------------

struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        BYTES.fetch_add(layout.size(), Relaxed);
        // SAFETY: forwarding an unchanged layout to the system allocator.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarding the same (ptr, layout) System returned.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Allocation count + bytes charged while `f` runs. Build inputs OUTSIDE the closure so only the
/// parse+digest is measured.
fn measure<T>(f: impl FnOnce() -> T) -> (usize, usize, T) {
    let (a0, b0) = (ALLOCS.load(Relaxed), BYTES.load(Relaxed));
    let out = f();
    (ALLOCS.load(Relaxed) - a0, BYTES.load(Relaxed) - b0, out)
}

// ---- the digest ------------------------------------------------------------------------------

/// A fixed-size fold over a document: what a streaming consumer keeps instead of a tree.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
struct Stats {
    scalars: u64,
    containers: u64,
    string_bytes: u64,
}

impl Stats {
    fn merge(self, other: Stats) -> Stats {
        Stats {
            scalars: self.scalars + other.scalars,
            containers: self.containers + other.containers,
            string_bytes: self.string_bytes + other.string_bytes,
        }
    }
}

/// Digest `Visitor`: reads scalars and string *lengths*, recurses containers, retains nothing.
struct Digest;

impl Visitor for Digest {
    type Value = Stats;

    fn visit_null(self) -> Result<Stats, Error> {
        Ok(Stats {
            scalars: 1,
            ..Stats::default()
        })
    }
    fn visit_bool(self, _: bool) -> Result<Stats, Error> {
        Ok(Stats {
            scalars: 1,
            ..Stats::default()
        })
    }
    fn visit_str(self, s: RopeStr) -> Result<Stats, Error> {
        // len() is a length query — no decode, no copy for the Borrowed arm.
        Ok(Stats {
            scalars: 1,
            string_bytes: s.len() as u64,
            ..Stats::default()
        })
    }
    fn visit_bytes(self, _: RopeBytes) -> Result<Stats, Error> {
        Ok(Stats {
            scalars: 1,
            ..Stats::default()
        })
    }
    fn visit_number(self, _: NumberToken) -> Result<Stats, Error> {
        // Count only — no decode. (The value ctor is a separate, opt-in cost.)
        Ok(Stats {
            scalars: 1,
            ..Stats::default()
        })
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

/// The same digest over a materialized `serde_json::Value`, so both sides count identically. (Keys
/// count as string scalars, mirroring the adapter, which visits each key as a string.)
fn digest_value(v: &Value) -> Stats {
    match v {
        Value::Null | Value::Bool(_) | Value::Number(_) => Stats {
            scalars: 1,
            ..Stats::default()
        },
        Value::String(s) => Stats {
            scalars: 1,
            string_bytes: s.len() as u64,
            ..Stats::default()
        },
        Value::Array(items) => items.iter().fold(
            Stats {
                containers: 1,
                ..Stats::default()
            },
            |acc, it| acc.merge(digest_value(it)),
        ),
        Value::Object(entries) => entries.iter().fold(
            Stats {
                containers: 1,
                ..Stats::default()
            },
            |acc, (k, val)| {
                acc.merge(Stats {
                    scalars: 1,
                    string_bytes: k.len() as u64,
                    ..Stats::default()
                })
                .merge(digest_value(val))
            },
        ),
    }
}

/// A container-heavy, string-heavy document — the shape where a tree parse pays the most structural
/// allocation. `count` objects, each with three escape-free string fields, an integer, and a nested
/// array of integers.
fn make_doc(count: usize) -> String {
    let mut s = String::from("[");
    for i in 0..count {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&format!(
            r#"{{"name":"item-{i}","kind":"widget","tag":"stable","id":{i},"scores":[{i},{},{},{},{}]}}"#,
            i + 1,
            i + 2,
            i + 3,
            i + 4
        ));
    }
    s.push(']');
    s
}

#[test]
fn adapter_digest_allocates_less_than_serde_value() {
    let doc = make_doc(200);
    let raw = doc.as_bytes().to_vec();

    // Build the rope input up front (single chunk) so it is not charged to the adapter's measurement.
    let rope = {
        let mut r = ByteVec::new();
        r.push_back(Bytes::copy_from_slice(&raw));
        r
    };

    let (serde_allocs, serde_bytes, serde_stats) =
        measure(|| digest_value(&serde_json::from_slice::<Value>(&raw).unwrap()));

    let (adapter_allocs, adapter_bytes, adapter_stats) =
        measure(|| from_rope(&rope, Digest).unwrap());

    // Same document ⇒ identical digest ⇒ both walked the whole tree, so the comparison is honest.
    assert_eq!(
        adapter_stats, serde_stats,
        "digests differ — the two paths did not process the same values"
    );

    println!("alloc scoreboard over {} bytes of JSON:", raw.len());
    println!("  serde_json::Value : {serde_allocs:>6} allocs, {serde_bytes:>8} bytes");
    println!("  etude adapter     : {adapter_allocs:>6} allocs, {adapter_bytes:>8} bytes");
    println!(
        "  ratio             : {:.2}x fewer allocs, {:.2}x fewer bytes",
        serde_allocs as f64 / adapter_allocs.max(1) as f64,
        serde_bytes as f64 / adapter_bytes.max(1) as f64,
    );

    // The copy-avoiding thesis: the streaming digest allocates strictly less than materializing the
    // tree — fewer times and fewer bytes.
    assert!(
        adapter_allocs < serde_allocs,
        "adapter should allocate fewer times ({adapter_allocs} vs {serde_allocs})"
    );
    assert!(
        adapter_bytes < serde_bytes,
        "adapter should allocate fewer bytes ({adapter_bytes} vs {serde_bytes})"
    );
}
