// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Pins the reads' allocation bound: a claimed length or count is checked against the reader's
//! buffered bytes before any allocation, so a claim of `u64::MAX` is refused with no allocation
//! at all.

use etude_buffer::codec::{DecodeError, ReadExt as _};
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
fn a_claimed_size_of_u64_max_is_refused_before_allocating() {
    let claim = u64::MAX.to_le_bytes();
    let too_long = DecodeError::LengthExceedsRemaining {
        len: u64::MAX,
        remaining: 0,
    };
    let too_many = DecodeError::CountExceedsRemaining {
        count: u64::MAX,
        min_item_len: 1,
        remaining: 0,
    };

    let (allocs, result) = allocations_during(|| claim.as_slice().read_bytes());
    assert_eq!((allocs, result), (0, Err(too_long)));

    let (allocs, result) = allocations_during(|| claim.as_slice().read_text());
    assert_eq!((allocs, result), (0, Err(too_long)));

    let (allocs, result) = allocations_during(|| claim.as_slice().read_count(1));
    assert_eq!((allocs, result), (0, Err(too_many)));

    // The counter sees the one allocation of a length that fits.
    let mut record = 3u64.to_le_bytes().to_vec();
    record.extend_from_slice(b"abc");
    let (allocs, result) = allocations_during(|| record.as_slice().read_bytes());
    assert_eq!((allocs, result), (1, Ok(b"abc".to_vec())));
}
