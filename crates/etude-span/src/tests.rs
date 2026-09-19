// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Tests for [`Cursor`] and [`Span`]. The cursor is checked against a flat byte-slice model: walking
//! it must yield exactly the input's bytes at their absolute offsets, no matter how the rope is
//! chunked (including empty leaves and tokens/runs straddling leaf boundaries).

use super::*;
use bytes::Bytes;
use etude_bytevec::ByteVec;

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

/// Walk the whole cursor via peek/bump, collecting (absolute offset, byte) pairs.
fn walk(input: &ByteVec) -> Vec<(usize, u8)> {
    let mut c = Cursor::new(input);
    let mut out = Vec::new();
    while let Some(b) = c.peek() {
        out.push((c.offset(), b));
        c.bump();
    }
    out
}

#[test]
fn span_accessors() {
    let s = Span::new(3, 10);
    assert_eq!(s.start(), 3);
    assert_eq!(s.end(), 10);
    assert_eq!(s.len(), 7);
    assert!(!s.is_empty());
    assert_eq!(s.range(), 3..10);
    assert!(Span::new(4, 4).is_empty());
}

#[test]
fn empty_input() {
    let r = ByteVec::new();
    let mut c = Cursor::new(&r);
    assert_eq!(c.peek(), None);
    assert_eq!(c.offset(), 0);
    assert!(c.chunk_tail().is_empty());
    c.bump(); // bump past end is a no-op-ish; must not panic and stays exhausted
    assert_eq!(c.peek(), None);
}

#[test]
fn walks_bytes_with_absolute_offsets_across_chunkings() {
    let bytes: Vec<u8> = (0..250u32).map(|i| i as u8).collect();
    let expected: Vec<(usize, u8)> = bytes.iter().copied().enumerate().collect();
    for chunk in [1usize, 2, 3, 7, 64, 1000] {
        let r = rope_chunked(&bytes, chunk);
        assert_eq!(walk(&r), expected, "chunk={chunk}");
        let mut c = Cursor::new(&r);
        while c.peek().is_some() {
            c.bump();
        }
        assert_eq!(c.offset(), bytes.len(), "end offset chunk={chunk}");
    }
}

#[test]
fn empty_leaves_are_skipped() {
    let mut r = ByteVec::new();
    r.push_back(Bytes::new());
    r.push_back(Bytes::copy_from_slice(b"ab"));
    r.push_back(Bytes::new());
    r.push_back(Bytes::copy_from_slice(b"c"));
    assert_eq!(walk(&r), vec![(0, b'a'), (1, b'b'), (2, b'c')]);
}

#[test]
fn chunk_tail_and_skip_in_chunk() {
    let r = rope_chunked(b"hello world", 4); // leaves "hell", "o wo", "rld"
    let mut c = Cursor::new(&r);
    assert_eq!(c.chunk_tail(), b"hell");
    c.skip_in_chunk(2); // skip "he"
    assert_eq!(c.offset(), 2);
    assert_eq!(c.peek(), Some(b'l'));
    // Skip to the end of the current leaf → refill to the next.
    let k = c.chunk_tail().len();
    c.skip_in_chunk(k);
    assert_eq!(c.offset(), 4);
    assert_eq!(c.peek(), Some(b'o'));
}

#[test]
fn differential_against_flat_model() {
    use bolero::check;
    use bolero_generator::TypeGenerator;

    #[derive(Debug, Clone, TypeGenerator)]
    struct Input {
        bytes: Vec<u8>,
        chunk_lens: Vec<u8>,
    }

    check!().with_type::<Input>().cloned().for_each(|inp| {
        let mut r = ByteVec::new();
        if inp.chunk_lens.is_empty() {
            r.push_back(Bytes::copy_from_slice(&inp.bytes));
        } else {
            let mut i = 0;
            let mut li = 0;
            while i < inp.bytes.len() {
                // `.max(1)` guarantees progress; empty-leaf handling is covered by a dedicated test.
                let len = (inp.chunk_lens[li % inp.chunk_lens.len()] as usize).max(1);
                li += 1;
                let j = (i + len).min(inp.bytes.len());
                r.push_back(Bytes::copy_from_slice(&inp.bytes[i..j]));
                i = j;
            }
        }
        let expected: Vec<(usize, u8)> = inp.bytes.iter().copied().enumerate().collect();
        assert_eq!(walk(&r), expected);
    });
}
