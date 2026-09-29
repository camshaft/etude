// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! `ByteVec` as a copy-avoiding byte rope: O(1) structural-sharing clone and zero-copy slice.
//!
//! This is the property every consumer adopts `etude-bytevec` for — a value threads through
//! reads, routing, and results getting cloned and sliced constantly, and each of those is a
//! reference-count bump / structural share, not a byte copy.
//!
//! Run with: `cargo run -p etude-bytevec --example structural_sharing`

use bytes::Bytes;
use etude_bytevec::ByteVec;

fn main() {
    // Build a rope from several chunks — as if the bytes arrived from separate reads. The chunks
    // are held by reference; `push_back` copies no bytes.
    let mut rope = ByteVec::from(Bytes::from_static(b"the quick "));
    rope.push_back(Bytes::from_static(b"brown fox "));
    rope.push_back(Bytes::from_static(b"jumps"));
    println!(
        "rope: {} bytes across {} chunk(s)",
        rope.len(),
        rope.chunks().count()
    );

    // O(1) clone: a structural share (refcount bump), not a deep byte copy. The two handles are
    // independent — mutating one does not disturb the other.
    let snapshot = rope.clone();

    // Zero-copy slice: a structural share of a byte range, no copy of the underlying bytes.
    let word = rope.slice(4..9);
    println!("slice 4..9  = {:?}", word.copy_to_bytes());

    // Split the front off — again by structural share, not by copying the chunks.
    let head = rope.split_to(10).expect("offset within bounds");
    println!("split head  = {:?}", head.copy_to_bytes());
    println!("split tail  = {:?}", rope.copy_to_bytes());

    // The clone is untouched by the split: it still holds the whole original value.
    println!(
        "snapshot    = {} bytes (the clone is independent)",
        snapshot.len()
    );
}
