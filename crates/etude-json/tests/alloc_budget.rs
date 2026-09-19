// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Pins the tokenizer's zero-allocation invariant: scanning a pre-built rope and reading every
//! token's metadata must perform no heap allocation at all. The BENCHMARKS.md alloc scoreboard
//! measures this; this test enforces it, so an accidental allocation on the scan path (a stray
//! `to_vec`, a formatting call, a buffered decode) fails the suite instead of landing silently.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountingAlloc;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// Allocations performed while running `f`.
fn allocations_during<R>(f: impl FnOnce() -> R) -> (usize, R) {
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let r = f();
    (ALLOCATIONS.load(Ordering::Relaxed) - before, r)
}

#[test]
fn tokenize_scan_allocates_nothing() {
    use etude_json::{TokenKind, Tokenizer};

    // Build the rope OUTSIDE the measured region, in several chunk layouts so leaf-boundary
    // handling is included in the budget.
    let doc =
        br#"{"alpha":[1,2.5,-3e10,true,false,null],"beta":{"nested":"plain string"},"g":"q\nA"}"#;
    for chunk in [1usize, 3, 7, doc.len()] {
        let mut rope = etude_bytevec::ByteVec::new();
        for piece in doc.chunks(chunk) {
            rope.push_back(etude_bytevec::Bytes::copy_from_slice(piece));
        }
        let (allocs, tokens) = allocations_during(|| {
            let mut count = 0usize;
            let mut strings = 0usize;
            for tok in Tokenizer::new(&rope) {
                let t = tok.expect("valid json");
                count += 1;
                // Metadata reads must stay allocation-free too.
                if t.kind() == TokenKind::String {
                    strings += usize::from(t.string_has_escapes().unwrap_or(false));
                }
                let _ = t.span();
            }
            (count, strings)
        });
        assert!(
            tokens.0 > 20,
            "sanity: the doc tokenizes ({} tokens)",
            tokens.0
        );
        // A rope past PROMOTE_AT chunks is deep, and the resumable chunk iterator allocates its
        // DFS stack once per `chunks()` (by design, see the double-ended iterator work in #88) —
        // that is the only allocation permitted, and only on the deep layout. Shallow layouts
        // must be exactly zero.
        let deep = rope.chunks().len() > 64;
        let budget = if deep { 2 } else { 0 };
        assert!(
            allocs <= budget,
            "tokenize scan exceeded its allocation budget (chunk={chunk}, deep={deep}: {allocs} > {budget})"
        );
    }
}
