// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Byte-scanning primitives for copy-avoiding tokenizers over the etude byte-rope.
//!
//! Two types, shared by every format tokenizer built on [`ByteVec`] (JSON, protobuf, decimal number
//! literals, …):
//!
//! - [`Span`] — a half-open byte range `[start, end)` into a rope. A span carries no bytes; a token
//!   references its lexeme by span and the consumer resolves it against the originating rope
//!   ([`ByteVec::slice`], O(1) structural sharing) only when it wants the bytes.
//! - [`Cursor`] — a forward cursor that streams the rope's leaves. It reads each contiguous leaf once
//!   with a local slice index and refills at leaf boundaries, so advancing is O(1) amortized (O(n)
//!   over the input) rather than the O(log n) tree descent a per-byte [`ByteVec::byte_at`] would cost.
//!   It tracks the absolute byte offset only to stamp span endpoints, never to fetch a byte, and
//!   allocates nothing.

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]

use etude_bytevec::ByteVec;

/// A half-open byte range `[start, end)` into a [`ByteVec`].
///
/// A span carries no bytes; resolve it against the originating rope (e.g. [`ByteVec::slice`]) to read
/// them. Resolving it against a different rope is meaningless.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    start: usize,
    end: usize,
}

impl Span {
    /// A span covering the half-open byte range `[start, end)`.
    #[inline]
    pub fn new(start: usize, end: usize) -> Span {
        Span { start, end }
    }

    /// The inclusive start byte offset.
    #[inline]
    pub fn start(&self) -> usize {
        self.start
    }

    /// The exclusive end byte offset.
    #[inline]
    pub fn end(&self) -> usize {
        self.end
    }

    /// The number of bytes the span covers.
    #[inline]
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Whether the span is empty (`start == end`).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// The span as a `start..end` range, for slicing the input rope.
    #[inline]
    pub fn range(&self) -> core::ops::Range<usize> {
        self.start..self.end
    }
}

/// A forward cursor over a [`ByteVec`]'s chunks.
///
/// It walks each contiguous leaf (`chunks()`) with a local slice index and refills at chunk
/// boundaries, so reading the next byte is O(1) amortized — each leaf is touched once, for O(n) over
/// the whole input. A per-byte [`ByteVec::byte_at`] would instead be an O(log n) tree descent every
/// byte (O(n log n) total, cache-hostile). The absolute byte offset is tracked only to stamp span
/// endpoints, never to fetch a byte.
pub struct Cursor<'a> {
    chunks: etude_bytevec::Chunks<'a>,
    /// The current leaf. Empty exactly when the cursor is exhausted (past the last byte).
    chunk: &'a [u8],
    /// Index of the current byte within `chunk`. Invariant: `pos < chunk.len()` unless exhausted.
    pos: usize,
    /// Absolute byte offset of `chunk[0]` in the input.
    base: usize,
}

impl<'a> Cursor<'a> {
    /// Create a cursor positioned at the first byte of `input`.
    #[inline]
    pub fn new(input: &'a ByteVec) -> Self {
        let mut cursor = Cursor {
            chunks: input.chunks(),
            chunk: &[],
            pos: 0,
            base: 0,
        };
        // Prime with the first non-empty leaf (empty leaves carry no bytes and no offset). `refill`
        // over the empty initial `chunk` adds 0 to `base` and finds the first non-empty leaf.
        cursor.refill();
        cursor
    }

    /// The current byte without advancing, or `None` at end of input.
    ///
    /// `#[inline]` because this is the tokenizer inner loop's per-byte read: called from a downstream
    /// crate it must inline to a slice load, not a cross-crate call (measured ~10-30x on a full scan).
    #[inline]
    pub fn peek(&self) -> Option<u8> {
        self.chunk.get(self.pos).copied()
    }

    /// The absolute byte offset of the current position (equals the input length at end of input).
    #[inline]
    pub fn offset(&self) -> usize {
        self.base + self.pos
    }

    /// The unread bytes of the current leaf (empty when exhausted). Borrows the input, not `self`, so
    /// a caller can scan it and then mutate the cursor (e.g. [`Cursor::skip_in_chunk`]). Use it to
    /// bulk-scan a run within one leaf, then [`Cursor::skip_in_chunk`] past it.
    #[inline]
    pub fn chunk_tail(&self) -> &'a [u8] {
        &self.chunk[self.pos..]
    }

    /// Advance past the current byte. The caller must have observed a byte via [`Cursor::peek`] first
    /// (so `pos < chunk.len()`); at a leaf boundary this refills to the next non-empty leaf.
    ///
    /// `#[inline]` for the same reason as [`Cursor::peek`]: the hot per-byte increment must inline
    /// into the caller. The cold leaf-boundary `refill` stays out of line so the inlined body is just
    /// the increment and the boundary branch.
    #[inline]
    pub fn bump(&mut self) {
        self.pos += 1;
        if self.pos >= self.chunk.len() {
            self.refill();
        }
    }

    /// Advance `k` bytes within the current leaf. The caller guarantees `k <= chunk_tail().len()`
    /// (the skip stays inside the current leaf); refills when it lands exactly at the leaf end. This
    /// is the bulk path: a run of bytes is skipped in one step instead of `k` `bump`s.
    #[inline]
    pub fn skip_in_chunk(&mut self, k: usize) {
        self.pos += k;
        if self.pos >= self.chunk.len() {
            self.refill();
        }
    }

    /// Move to the next non-empty leaf (or the exhausted state), crediting the leaving leaf's whole
    /// length to `base` so [`Cursor::offset`] stays absolute.
    ///
    /// `#[cold]`: this fires only at a leaf boundary, so keeping it out of line lets the inlined
    /// `bump`/`skip_in_chunk` hot path stay just the increment and the boundary branch.
    #[cold]
    fn refill(&mut self) {
        self.base += self.chunk.len();
        self.pos = 0;
        self.chunk = &[];
        loop {
            match self.chunks.next() {
                Some(next) if !next.is_empty() => {
                    self.chunk = &next[..];
                    break;
                }
                Some(_) => {}
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests;
