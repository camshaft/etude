// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! A tiered byte rope that degrades to a flat [`bytes::Bytes`] deque when small.
//!
//! [`ByteRope`] is a drop-in for a chunked byte buffer (a `VecDeque<Bytes>` with a cached length):
//! `push_back`/`push_front`/`pop_front`/`pop_back` are chunk-granular and O(1); `advance` consumes
//! bytes from the front zero-copy at chunk boundaries. It matches the flat buffer's cheapest
//! properties exactly:
//!
//! - a **single chunk holds no tracking state** — it lives in `head` with an unallocated deque, and
//! - the **shallow streaming path never touches a tree**.
//!
//! Only once a rope accumulates many chunks does it promote to a *deep* representation — a
//! relaxed-radix (RRB-style) tree of chunk blocks with buffered head/tail deques — where `clone` is
//! O(1) (structural sharing) and `split`/`concat` are O(log₃₂ n) instead of O(chunks). It demotes
//! back to flat when it drains, so a buffer that spikes and drains returns to the cheap shape.
//!
//! The tree carries a cumulative byte-size table per node, so it is *relaxed*: chunks (and blocks)
//! have varying byte lengths, and front-consumption does not require a strict left-full invariant.

#![cfg_attr(not(any(test, feature = "std")), no_std)]

extern crate alloc;

use alloc::{boxed::Box, collections::VecDeque, vec::Vec};
// Re-exported (`pub`) to mirror `etude_bytevec`, which re-exports `Bytes`/`BytesMut` at its crate root.
pub use bytes::{Bytes, BytesMut};

mod tree;
use tree::Tree;

pub mod builder;
pub use builder::Builder;

pub mod tagged;
pub use tagged::Tagged;

// ---- bytevec compat aliases -------------------------------------------------------------------
// `etude-byterope` is a SUPERSET drop-in for `etude-bytevec` (operator ruling 2026-09-19): a consumer
// that swaps ONLY the Cargo dependency recompiles with zero source changes. These aliases expose the
// byterope types under the `etude_bytevec` names. (`Builder`, `Tagged`, `Bytes`, `BytesMut` above
// already match by name; `Owner`/`Handle` live in the `tagged` module as in bytevec.)

/// Alias of [`ByteRope`] under `etude_bytevec`'s `ByteVec` name, for a zero-source-change dep swap.
pub type ByteVec = ByteRope;
/// Alias of [`ByteRopeError`] under `etude_bytevec`'s `ByteVecError` name.
pub type ByteVecError = ByteRopeError;
/// Alias of [`Chunks`] under `etude_bytevec`'s `ChunkIter` iterator name.
pub type ChunkIter<'a> = Chunks<'a>;
/// Alias of [`IntoChunks`] under `etude_bytevec`'s `DrainIter` draining-iterator name.
pub type DrainIter = IntoChunks;

/// Radix width: children per interior node / chunks per leaf block.
const BITS: u32 = 5;
const FANOUT: usize = 1 << BITS;

/// Chunk count at which a `Small` rope promotes to `Deep`.
const PROMOTE_AT: usize = 2 * FANOUT;
/// Chunk count at which a draining `Deep` rope demotes back to `Small` (hysteresis vs `PROMOTE_AT`).
const DEMOTE_AT: usize = FANOUT;

/// A structural [`ByteRope::replace`] whose resulting chunk would be no larger than this is collapsed
/// into a single allocation (kept boundary bytes + the replacement) rather than fragmenting into
/// separate — possibly tiny — chunks. Bounds fragmentation from small edits. Tunable.
const COALESCE_MAX: usize = 256;

/// Error returned by the byte-granular operations ([`ByteRope::advance`], [`ByteRope::split_to`]).
/// Mirrors `etude_bytevec::ByteVecError` (same variants) so callers can swap the two.
///
/// # Examples
///
/// ```
/// use etude_byterope::ByteRopeError;
///
/// assert_eq!(ByteRopeError::OutOfBounds(3).to_string(), "index out of bounds: 3");
/// assert_eq!(ByteRopeError::OutOfBoundsRange(2, 5).to_string(), "range out of bounds: 2..5");
/// // Both convert to an UnexpectedEof io::Error, like ByteVecError.
/// let e: std::io::Error = ByteRopeError::OutOfBoundsRange(2, 5).into();
/// assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof);
/// ```
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ByteRopeError {
    /// An index was past the end of the rope.
    OutOfBounds(usize),
    /// A `start..end` range was past the end of the rope. Present for `etude_bytevec` parity (the
    /// byte-granular ops currently report [`ByteRopeError::OutOfBounds`]).
    OutOfBoundsRange(usize, usize),
}

impl core::fmt::Display for ByteRopeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::OutOfBounds(at) => write!(f, "index out of bounds: {at}"),
            Self::OutOfBoundsRange(start, end) => write!(f, "range out of bounds: {start}..{end}"),
        }
    }
}

impl core::error::Error for ByteRopeError {}

#[cfg(feature = "std")]
impl From<ByteRopeError> for std::io::Error {
    #[inline]
    fn from(error: ByteRopeError) -> Self {
        match error {
            ByteRopeError::OutOfBounds(_) | ByteRopeError::OutOfBoundsRange(_, _) => {
                Self::new(std::io::ErrorKind::UnexpectedEof, error)
            }
        }
    }
}

/// A tiered byte rope. See the crate docs.
#[derive(Clone, Default)]
pub struct ByteRope {
    len: usize,
    repr: Repr,
}

#[derive(Clone)]
enum Repr {
    /// The flat tier: identical in shape to a chunked byte buffer. A single chunk lives in `head`
    /// with an empty (unallocated) `additional`, so it carries no tracking state. Invariant: if
    /// `head` is empty then `additional` is empty (chunks fill front-first).
    Small {
        head: Bytes,
        additional: VecDeque<Bytes>,
    },
    /// The deep tier: buffered ends around a relaxed-radix tree of chunk blocks. Boxed so this rare,
    /// large variant does not inflate every (usually `Small`) rope — it keeps `ByteRope` the same
    /// size as a flat chunk buffer, paying one pointer indirection only in the deep tier.
    Deep(Box<Deep>),
}

/// The deep tier's fields (see [`Repr::Deep`]).
#[derive(Clone)]
struct Deep {
    head: VecDeque<Bytes>,
    tree: Tree,
    tail: VecDeque<Bytes>,
}

impl Default for Repr {
    #[inline]
    fn default() -> Self {
        Repr::Small {
            head: Bytes::new(),
            additional: VecDeque::new(),
        }
    }
}

impl ByteRope {
    /// Creates an empty rope. Allocation-free. `const` to match the flat buffer's `const fn new`, so
    /// a rope can initialize a `const`/`static`.
    ///
    /// # Examples
    ///
    /// ```
    /// use etude_byterope::ByteRope;
    ///
    /// const EMPTY: ByteRope = ByteRope::new();
    /// assert!(EMPTY.is_empty());
    /// ```
    #[inline]
    pub const fn new() -> Self {
        ByteRope {
            len: 0,
            repr: Repr::Small {
                head: Bytes::new(),
                additional: VecDeque::new(),
            },
        }
    }

    /// Creates a [`Builder`] for efficiently constructing a rope by buffering writes into a head
    /// buffer of the given chunk capacity. Mirrors the flat buffer's `builder` so callers can swap
    /// the two.
    #[inline]
    pub fn builder(chunk_capacity: usize) -> builder::Builder {
        builder::Builder::new(chunk_capacity)
    }

    /// Total number of bytes in the rope.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the rope holds no bytes.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The number of `Bytes` chunks in the rope.
    fn chunk_count(&self) -> usize {
        match &self.repr {
            Repr::Small { head, additional } => {
                usize::from(!head.is_empty()) + additional.len()
            }
            Repr::Deep(d) => d.head.len() + d.tree.chunk_count() + d.tail.len(),
        }
    }

    /// Validates every structural invariant. Called after each mutation in test builds (and inlined
    /// away to nothing otherwise), so any operation that corrupts the rope is caught at its source.
    ///
    /// Invariants: `len` equals the actual byte content; no chunk is empty; the `Small` tier upholds
    /// "empty head ⇒ empty additional"; and the deep tier's buffered ends are non-empty with a tree
    /// whose cached sizes/counts match its nodes.
    #[cfg(test)]
    fn check_invariants(&self) {
        let actual: usize = self.chunks().map(|c| c.len()).sum();
        assert_eq!(actual, self.len, "len {} != content {actual}", self.len);
        assert!(self.chunks().all(|c| !c.is_empty()), "empty chunk present");
        match &self.repr {
            Repr::Small { head, additional } => {
                assert!(
                    !head.is_empty() || additional.is_empty(),
                    "empty head with non-empty additional"
                );
            }
            Repr::Deep(d) => {
                assert!(
                    d.head.iter().all(|c| !c.is_empty()) && d.tail.iter().all(|c| !c.is_empty()),
                    "empty chunk in a deep buffered end"
                );
                d.tree.check_invariants();
            }
        }
    }

    /// No-op outside test builds — the invariant checks impose zero cost on downstream crates.
    #[cfg(not(test))]
    #[inline(always)]
    fn check_invariants(&self) {}

    /// Appends a chunk at the back. Empty chunks are ignored. Amortized O(1).
    #[inline]
    pub fn push_back(&mut self, chunk: Bytes) {
        if chunk.is_empty() {
            return;
        }
        self.len += chunk.len();
        match &mut self.repr {
            Repr::Small { head, additional } => {
                if head.is_empty() {
                    *head = chunk;
                } else {
                    additional.push_back(chunk);
                    if additional.len() >= PROMOTE_AT {
                        self.promote();
                    }
                }
            }
            Repr::Deep(d) => {
                d.tail.push_back(chunk);
                // Freeze a full block off the front of the tail into the tree.
                if d.tail.len() >= FANOUT {
                    let block: Vec<Bytes> = d.tail.drain(..FANOUT).collect();
                    d.tree.push_block(block);
                }
            }
        }
        self.check_invariants();
    }

    /// Prepends a chunk at the front. Empty chunks are ignored. Amortized O(1).
    #[inline]
    pub fn push_front(&mut self, chunk: Bytes) {
        if chunk.is_empty() {
            return;
        }
        self.len += chunk.len();
        match &mut self.repr {
            Repr::Small { head, additional } => {
                if head.is_empty() {
                    *head = chunk;
                } else {
                    let prev = core::mem::replace(head, chunk);
                    additional.push_front(prev);
                    if additional.len() >= PROMOTE_AT {
                        self.promote();
                    }
                }
            }
            Repr::Deep(d) => {
                d.head.push_front(chunk);
            }
        }
        self.check_invariants();
    }

    /// Removes and returns the front chunk, or `None` if empty. O(1) amortized.
    #[inline]
    pub fn pop_front(&mut self) -> Option<Bytes> {
        let chunk = match &mut self.repr {
            Repr::Small { head, additional } => {
                if head.is_empty() {
                    return None;
                }
                let out = core::mem::take(head);
                if let Some(next) = additional.pop_front() {
                    *head = next;
                }
                out
            }
            Repr::Deep(d) => {
                if let Some(c) = d.head.pop_front() {
                    c
                } else if let Some(block) = d.tree.pop_front_block() {
                    // refill the head buffer from the tree's leftmost block
                    d.head.extend(block);
                    d.head.pop_front().expect("a block is never empty")
                } else {
                    d.tail.pop_front()?
                }
            }
        };
        self.len -= chunk.len();
        self.maybe_demote();
        self.check_invariants();
        Some(chunk)
    }

    /// Removes and returns the back chunk, or `None` if empty. O(1) amortized.
    #[inline]
    pub fn pop_back(&mut self) -> Option<Bytes> {
        let chunk = match &mut self.repr {
            Repr::Small { head, additional } => {
                if let Some(c) = additional.pop_back() {
                    c
                } else if head.is_empty() {
                    return None;
                } else {
                    core::mem::take(head)
                }
            }
            Repr::Deep(d) => {
                if let Some(c) = d.tail.pop_back() {
                    c
                } else if let Some(block) = d.tree.pop_back_block() {
                    d.tail.extend(block);
                    d.tail.pop_back().expect("a block is never empty")
                } else {
                    d.head.pop_back()?
                }
            }
        };
        self.len -= chunk.len();
        self.maybe_demote();
        self.check_invariants();
        Some(chunk)
    }

    /// Advances past the first `len` bytes, dropping them from the front.
    ///
    /// Zero-copy at chunk boundaries (the boundary chunk is sliced in place). Returns
    /// [`ByteRopeError::OutOfBounds`] if `len` exceeds the rope, matching the flat buffer.
    #[inline]
    pub fn advance(&mut self, len: usize) -> Result<(), ByteRopeError> {
        if len > self.len {
            return Err(ByteRopeError::OutOfBounds(len));
        }
        self.consume_front(len);
        self.check_invariants();
        Ok(())
    }

    /// Drops the first `n` bytes from the front, zero-copy at chunk boundaries (the boundary chunk
    /// is sliced in place via `Buf::advance` — no refcount traffic). Saturates past the end.
    #[inline]
    fn consume_front(&mut self, n: usize) {
        let mut n = n.min(self.len);
        if n == 0 {
            return;
        }
        self.len -= n;
        match &mut self.repr {
            // Tight single-match loop over `head + additional`, mirroring the flat buffer so the
            // shallow streaming hot path pays no more than the flat buffer does.
            Repr::Small { head, additional } => {
                while n > 0 {
                    if head.len() <= n {
                        n -= head.len();
                        *head = additional.pop_front().unwrap_or_default();
                        if head.is_empty() {
                            break; // fully drained
                        }
                    } else {
                        bytes::Buf::advance(head, n); // in-place cursor bump, no refcount traffic
                        break;
                    }
                }
            }
            // Consume the buffered head; refill it a whole block at a time from the tree, then the
            // tail. Each chunk is either dropped whole or sliced in place — never re-descends the
            // tree per byte.
            Repr::Deep(d) => {
                while n > 0 {
                    if d.head.is_empty() {
                        if let Some(block) = d.tree.pop_front_block() {
                            d.head.extend(block);
                        } else if !d.tail.is_empty() {
                            core::mem::swap(&mut d.head, &mut d.tail);
                        } else {
                            break;
                        }
                    }
                    let front = d.head.front_mut().expect("head non-empty");
                    let flen = front.len();
                    if flen <= n {
                        n -= flen;
                        d.head.pop_front();
                    } else {
                        bytes::Buf::advance(front, n);
                        break;
                    }
                }
            }
        }
        self.maybe_demote();
    }

    /// Returns the byte at `offset`, or `None` if out of bounds.
    ///
    /// In the deep tier this is O(log₃₂ n) via the tree's size tables — random byte-offset access
    /// that a flat chunk buffer can only answer by walking chunks. In the shallow tier it walks the
    /// (few) chunks directly. Beyond the `ByteVec` API (like [`ByteRope::slice`]) because the tree
    /// makes it cheap; the Cadenza runtime needs random byte access.
    pub fn byte_at(&self, offset: usize) -> Option<u8> {
        if offset >= self.len {
            return None;
        }
        match &self.repr {
            Repr::Small { head, additional } => {
                byte_in_chunks(core::iter::once(head).chain(additional.iter()), offset)
            }
            Repr::Deep(d) => {
                let head_bytes: usize = d.head.iter().map(|c| c.len()).sum();
                if offset < head_bytes {
                    byte_in_chunks(d.head.iter(), offset)
                } else if offset < head_bytes + d.tree.byte_len() {
                    Some(d.tree.byte_at(offset - head_bytes))
                } else {
                    byte_in_chunks(d.tail.iter(), offset - head_bytes - d.tree.byte_len())
                }
            }
        }
    }

    /// Sets the byte at `offset`; `Err(OutOfBounds)` if `offset >= len`.
    ///
    /// Mutates in place when the containing chunk is uniquely owned (`Bytes::try_into_mut`), copying
    /// only that one chunk on a cache miss; in the deep tier the tree spine to that chunk is likewise
    /// mutated in place when unique and path-copied only where shared (FBIP). So the blast radius of a
    /// write on a shared rope is one chunk plus an O(log₃₂) spine path — never the whole buffer.
    /// Beyond the `ByteVec` API (like [`ByteRope::byte_at`]); the Cadenza runtime needs byte writes.
    pub fn set_byte(&mut self, offset: usize, value: u8) -> Result<(), ByteRopeError> {
        if offset >= self.len {
            return Err(ByteRopeError::OutOfBounds(offset));
        }
        match &mut self.repr {
            Repr::Small { head, additional } => {
                if offset < head.len() {
                    let edit = cow_edit(core::mem::take(head), offset, 1, |s| s[0] = value);
                    splice_cow_head(head, additional, edit);
                } else {
                    set_byte_in_deque(additional, offset - head.len(), value);
                }
            }
            Repr::Deep(d) => {
                let head_bytes: usize = d.head.iter().map(|c| c.len()).sum();
                if offset < head_bytes {
                    set_byte_in_deque(&mut d.head, offset, value);
                } else if offset < head_bytes + d.tree.byte_len() {
                    d.tree.set_byte(offset - head_bytes, value);
                } else {
                    set_byte_in_deque(&mut d.tail, offset - head_bytes - d.tree.byte_len(), value);
                }
            }
        }
        self.check_invariants();
        Ok(())
    }

    /// Iterates over every chunk in order.
    pub fn chunks(&self) -> Chunks<'_> {
        let remaining = self.chunk_count();
        match &self.repr {
            Repr::Small { head, additional } => Chunks {
                remaining,
                inner: ChunksInner::Small {
                    head: if head.is_empty() { None } else { Some(head) },
                    rest: additional.iter(),
                },
            },
            Repr::Deep(d) => Chunks {
                remaining,
                inner: ChunksInner::Deep {
                    head: d.head.iter(),
                    tree: d.tree.chunks(),
                    tail: d.tail.iter(),
                    phase: 0,
                },
            },
        }
    }

    /// Creates an empty rope, pre-reserving space for `cap` chunks.
    #[inline]
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            len: 0,
            repr: Repr::Small {
                head: Bytes::new(),
                additional: VecDeque::with_capacity(cap.saturating_sub(1)),
            },
        }
    }

    /// Removes all chunks, resetting to the empty (flat) state.
    #[inline]
    pub fn clear(&mut self) {
        self.len = 0;
        self.repr = Repr::default();
        self.check_invariants();
    }

    /// Returns the chunk at chunk-`index` (index 0 is the front chunk), or `None`.
    ///
    /// O(1) in the shallow tier (matching the flat buffer); O(log₃₂) in the deep tier via a chunk-count
    /// descent (buffered head/tail are O(1) deque indexing).
    pub fn get(&self, index: usize) -> Option<&Bytes> {
        match &self.repr {
            Repr::Small { head, additional } => {
                if index == 0 {
                    (!head.is_empty()).then_some(head)
                } else {
                    additional.get(index - 1)
                }
            }
            Repr::Deep(d) => {
                if index < d.head.len() {
                    d.head.get(index)
                } else {
                    let i = index - d.head.len();
                    let tree_chunks = d.tree.chunk_count();
                    if i < tree_chunks {
                        d.tree.get_chunk(i)
                    } else {
                        d.tail.get(i - tree_chunks)
                    }
                }
            }
        }
    }

    /// Moves all chunks out of `other` into the back of `self`, leaving `other` empty.
    ///
    /// Fast paths: O(1) when `self` is empty (swap); a cheap flat move when the combined size stays
    /// shallow; otherwise the two ropes' trees are merged with an O(log₃₂) [`Tree::concat`] that
    /// shares all subtrees away from the seam — no per-chunk copying.
    pub fn append(&mut self, other: &mut Self) {
        if other.is_empty() {
            return;
        }
        if self.is_empty() {
            core::mem::swap(self, other);
            return;
        }
        let taken = core::mem::take(other);

        // Shallow combined: keep it flat, just move the few chunks (updates len via push_back).
        if matches!(self.repr, Repr::Small { .. })
            && self.chunk_count() + taken.chunk_count() < PROMOTE_AT
        {
            self.push_all(taken);
            return;
        }

        // Otherwise fold both to trees and concat (O(log), subtree-sharing).
        let left = core::mem::take(self).into_tree();
        let right = taken.into_tree();
        *self = Self::from_tree(Tree::concat(left, right));
        self.check_invariants();
    }

    /// Drains every chunk of `other` into the back of `self` via `push_back` (the slow, chunk-wise
    /// path — used only for the shallow flat case).
    fn push_all(&mut self, other: Self) {
        match other.repr {
            Repr::Small { head, additional } => {
                self.push_back(head);
                for c in additional {
                    self.push_back(c);
                }
            }
            Repr::Deep(d) => {
                let Deep {
                    head,
                    mut tree,
                    tail,
                } = *d;
                for c in head {
                    self.push_back(c);
                }
                while let Some(block) = tree.pop_front_block() {
                    for c in block {
                        self.push_back(c);
                    }
                }
                for c in tail {
                    self.push_back(c);
                }
            }
        }
    }

    /// Splits off the first `at` bytes into a new rope; `self` keeps `[at, len)`.
    ///
    /// Zero-copy at chunk boundaries (whole chunks move by reference; the boundary chunk is sliced).
    /// Returns [`ByteRopeError::OutOfBounds`] if `at` exceeds the rope, matching the flat buffer.
    #[must_use = "consider ByteRope::advance if you don't need the split-off half"]
    pub fn split_to(&mut self, at: usize) -> Result<Self, ByteRopeError> {
        if at > self.len {
            return Err(ByteRopeError::OutOfBounds(at));
        }
        if at == 0 {
            return Ok(Self::new());
        }
        if at == self.len {
            return Ok(core::mem::take(self));
        }
        // Deep: fold to a tree and split it in O(log₃₂), sharing subtrees on both sides.
        if matches!(self.repr, Repr::Deep(_)) {
            let (left, right) = core::mem::take(self).into_tree().split(at);
            *self = Self::from_tree(right);
            self.check_invariants();
            let front = Self::from_tree(left);
            front.check_invariants();
            return Ok(front);
        }
        // Small: move the (few) chunks, slicing the boundary chunk.
        let mut out = Self::new();
        let mut remaining = at;
        while remaining > 0 {
            let front_len = self.front_chunk_len().expect("bytes remaining");
            if front_len <= remaining {
                let chunk = self.pop_front().expect("front chunk present");
                remaining -= chunk.len();
                out.push_back(chunk);
            } else {
                let mut chunk = self.pop_front().expect("front chunk present");
                let front = chunk.split_to(remaining);
                out.push_back(front);
                self.push_front(chunk);
                remaining = 0;
            }
        }
        Ok(out)
    }

    /// Returns a new rope over the byte range `range` (e.g. `rope.slice(10..20)`), sharing structure
    /// with `self` — no bytes are copied in the deep tier. O(log₃₂) via two splits on a cheap
    /// (O(1)-shared) clone.
    ///
    /// # Panics
    ///
    /// Panics if the range is out of bounds or `start > end`.
    pub fn slice(&self, range: impl core::ops::RangeBounds<usize>) -> Self {
        use core::ops::Bound;
        let start = match range.start_bound() {
            Bound::Included(&s) => s,
            Bound::Excluded(&s) => s + 1,
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&e) => e + 1,
            Bound::Excluded(&e) => e,
            Bound::Unbounded => self.len,
        };
        assert!(start <= end && end <= self.len, "slice {start}..{end} out of bounds (len {})", self.len);
        if start == 0 && end == self.len {
            return self.clone();
        }
        if start == end {
            return Self::new();
        }
        match &self.repr {
            // Shallow: copy the (few) chunk handles overlapping [start, end), slicing the boundary
            // chunks (`Bytes::slice` is O(1) shared). No tree.
            Repr::Small { .. } => {
                let mut out = Self::new();
                let mut pos = 0;
                for chunk in self.chunks() {
                    let cs = pos;
                    let ce = pos + chunk.len();
                    pos = ce;
                    if ce <= start {
                        continue;
                    }
                    if cs >= end {
                        break;
                    }
                    let lo = start.saturating_sub(cs);
                    let hi = end.min(ce) - cs;
                    out.push_back(chunk.slice(lo..hi));
                }
                out
            }
            // Deep: slice the tree body in ONE descent (`Tree::subrange`, sharing every interior
            // subtree) and re-attach only the boundary bytes that fall in the buffered head/tail. No
            // whole-rope clone, no head/tail fold: a slice landing entirely in the tree body (the
            // common case for a large rope) is a single subrange with no boundary pushes at all.
            Repr::Deep(d) => {
                let head_bytes: usize = d.head.iter().map(|c| c.len()).sum();
                let tree_end = head_bytes + d.tree.byte_len();

                // Middle: the portion of `[start, end)` that lands in the tree, clamped to it.
                let ts = start.saturating_sub(head_bytes).min(d.tree.byte_len());
                let te = end.saturating_sub(head_bytes).min(d.tree.byte_len());
                let mut out = Self::from_tree(d.tree.subrange(ts, te));

                // Prepend the sliced head chunks (in original order via reversed push_front) and
                // append the sliced tail chunks — each bounded by the buffered-end capacity.
                if start < head_bytes {
                    for chunk in slice_deque_chunks(&d.head, 0, start, end).into_iter().rev() {
                        out.push_front(chunk);
                    }
                }
                if end > tree_end {
                    for chunk in slice_deque_chunks(&d.tail, tree_end, start, end) {
                        out.push_back(chunk);
                    }
                }
                out
            }
        }
    }

    /// Replaces the bytes in `range` with `value`, any infallible byte source — a `&[u8]`, `Bytes`,
    /// another `ByteRope`, a [`Chain`](etude_buffer::reader::Chain), a network buffer, etc. Owned
    /// chunks (`Bytes`/`BytesMut`) from the source splice in zero-copy; only a borrowed slice copies.
    /// A single byte is `replace(i..=i, &[b][..])` — no allocation.
    ///
    /// `Err(OutOfBounds)` if the range falls outside the rope. Beyond the `ByteVec` API (like
    /// [`ByteRope::slice`]); the Cadenza runtime builds byte edits from splices.
    pub fn replace<R>(
        &mut self,
        range: impl core::ops::RangeBounds<usize>,
        mut value: R,
    ) -> Result<(), ByteRopeError>
    where
        R: etude_buffer::reader::Buffer<Error = core::convert::Infallible>,
    {
        let (start, end) = resolve_range(&range, self.len)?;
        let removed = end - start;
        let vlen = value.buffered_len(); // known up front, no read — lets each case pick its strategy
        if removed == 0 && vlen == 0 {
            return Ok(()); // nothing removed, nothing inserted
        }

        // UC1–3: an equal-length overwrite changes no structure — write in place, in either tier.
        if removed == vlen {
            self.overwrite_range(start, vlen, &mut value);
            return Ok(());
        }

        // UC4: a small structural edit within one flat chunk collapses into that single chunk.
        if self.try_coalesce_in_chunk(start, end, removed, vlen, &mut value) {
            return Ok(());
        }

        // UC5: any other flat-tier structural edit (large single-chunk, or a multi-chunk span).
        if self.try_splice_flat(start, end, removed, vlen, &mut value) {
            return Ok(());
        }

        // UC6: deep-tier structural edit — splice at the tree level.
        self.splice_deep(start, end, &mut value);
        Ok(())
    }

    /// UC1–3: overwrites `count` bytes at `start` in place (no structure change), dispatching to the
    /// tier. Each covered chunk is written in place when uniquely owned, else copied once.
    fn overwrite_range<R>(&mut self, start: usize, count: usize, value: &mut R)
    where
        R: etude_buffer::reader::Buffer<Error = core::convert::Infallible>,
    {
        match &mut self.repr {
            Repr::Small { head, additional } => overwrite_flat(head, additional, start, count, value),
            Repr::Deep(d) => overwrite_deep(d, start, count, value),
        }
        self.check_invariants();
    }

    /// UC4: if `[start, end)` is a small structural edit within a single flat-tier chunk, collapses
    /// that chunk's kept bytes + the value into ONE new chunk (one allocation, no fragmentation) and
    /// returns `true`. Otherwise leaves `self` untouched and returns `false`.
    fn try_coalesce_in_chunk<R>(
        &mut self,
        start: usize,
        end: usize,
        removed: usize,
        vlen: usize,
        value: &mut R,
    ) -> bool
    where
        R: etude_buffer::reader::Buffer<Error = core::convert::Infallible>,
    {
        let located = match &self.repr {
            Repr::Small { head, additional } => locate_within_chunk(head, additional, start, end)
                .filter(|&(si, ..)| {
                    let clen = if si == 0 { head.len() } else { additional[si - 1].len() };
                    clen - removed + vlen <= COALESCE_MAX
                }),
            Repr::Deep(_) => None,
        };
        let Some((si, so, eo)) = located else {
            return false;
        };
        if let Repr::Small { head, additional } = &mut self.repr {
            splice_one_chunk(head, additional, si, so, eo, value);
        }
        self.len = self.len - removed + vlen;
        self.check_invariants();
        true
    }

    /// UC5: if `self` is in the flat tier, splices `[start, end)` → `value` directly on the chunk
    /// deque (O(1) boundary slices + one `split_off`/rejoin, owned value chunks zero-copy) and returns
    /// `true`. Otherwise leaves `self` untouched and returns `false`.
    fn try_splice_flat<R>(
        &mut self,
        start: usize,
        end: usize,
        removed: usize,
        vlen: usize,
        value: &mut R,
    ) -> bool
    where
        R: etude_buffer::reader::Buffer<Error = core::convert::Infallible>,
    {
        let (si, so, ei, eo) = match &self.repr {
            Repr::Small { head, additional } => locate_span(head, additional, start, end),
            Repr::Deep(_) => return false,
        };
        if let Repr::Small { head, additional } = &mut self.repr {
            splice_flat(head, additional, si, so, ei, eo, value);
        }
        self.len = self.len - removed + vlen;
        if self.chunk_count() >= PROMOTE_AT {
            self.promote();
        }
        self.check_invariants();
        true
    }

    /// UC6: deep-tier structural splice — `self[..start] ++ value ++ self[end..]`. Folds the buffered
    /// ends into one tree ONCE, splits out `[start, end)` and stitches the pieces back with O(log₃₂)
    /// `Tree::split`/`concat` (subtree-sharing, no per-chunk copy, no intermediate ropes beyond the
    /// value). Owned value chunks stay zero-copy.
    fn splice_deep<R>(&mut self, start: usize, end: usize, value: &mut R)
    where
        R: etude_buffer::reader::Buffer<Error = core::convert::Infallible>,
    {
        use etude_buffer::reader::Infallible as _;
        // Fast path: the spliced range lies entirely within the TREE body, so we can splice the tree
        // directly and leave the buffered head/tail untouched — no fold-into-tree / re-derive
        // round-trip (which the general path below pays via `into_tree`/`from_tree`: two extra
        // seam concats plus rebuilding the buffered ends). Same O(log₃₂) split×2 + concat×2 on the
        // tree, minus the fold. This is the common case (a splice in the middle of a large rope).
        if let Repr::Deep(d) = &mut self.repr {
            let head_bytes: usize = d.head.iter().map(|c| c.len()).sum();
            let tree_bytes = d.tree.byte_len();
            if start >= head_bytes && end <= head_bytes + tree_bytes {
                let ts = start - head_bytes;
                let te = end - head_bytes;
                let (left, rest) = d.tree.split(ts); // left = tree[0, ts)
                let (_dropped, right) = rest.split(te - ts); // right = tree[te, tree_len)
                let mut midrope = ByteRope::new();
                value.infallible_copy_into(&mut midrope); // value chunks (owned zero-copy)
                let inserted = midrope.len();
                d.tree = Tree::concat(Tree::concat(left, midrope.into_tree()), right);
                self.len = self.len - (end - start) + inserted;
                self.maybe_demote();
                self.check_invariants();
                return;
            }
        }
        // General path: the range touches a buffered end, so fold everything into one tree, split
        // out the range, splice, and re-derive the rope (buffered ends start empty).
        let whole = core::mem::take(self).into_tree(); // fold head/tail once
        let (left, rest) = whole.split(start); // left = [0, start)
        let (_dropped, right) = rest.split(end - start); // right = [end, len)
        let mut mid = ByteRope::new();
        value.infallible_copy_into(&mut mid); // value chunks (owned zero-copy)
        let spliced = Tree::concat(Tree::concat(left, mid.into_tree()), right);
        *self = Self::from_tree(spliced);
        self.check_invariants();
    }

    /// Splits off the first `at` bytes, copying them into a single contiguous [`Bytes`].
    ///
    /// Prefer [`ByteRope::split_to`] when you can keep the chunked form — this forces a copy.
    #[must_use = "consider ByteRope::advance if you don't need the split-off half"]
    pub fn split_to_copy(&mut self, at: usize) -> Result<Bytes, ByteRopeError> {
        if at > self.len {
            return Err(ByteRopeError::OutOfBounds(at));
        }
        let mut out = bytes::BytesMut::with_capacity(at);
        let mut remaining = at;
        while remaining > 0 {
            let front_len = self.front_chunk_len().expect("bytes remaining");
            let take = front_len.min(remaining);
            let mut chunk = self.pop_front().expect("front chunk present");
            out.extend_from_slice(&chunk[..take]);
            if take < front_len {
                bytes::Buf::advance(&mut chunk, take);
                self.push_front(chunk);
            }
            remaining -= take;
        }
        Ok(out.freeze())
    }

    /// Shortens the rope to `len` bytes, dropping the rest from the back. No-op if already shorter.
    pub fn truncate(&mut self, len: usize) {
        if len == 0 {
            self.clear();
            return;
        }
        if self.len > len {
            self.consume_back(self.len - len);
        }
        self.check_invariants();
    }

    /// Drops the last `n` bytes from the back, zero-copy at chunk boundaries (the boundary chunk is
    /// truncated in place). Mirror of [`consume_front`]. Saturates past the start.
    #[inline]
    fn consume_back(&mut self, n: usize) {
        let mut n = n.min(self.len);
        if n == 0 {
            return;
        }
        self.len -= n;
        match &mut self.repr {
            Repr::Small { head, additional } => {
                while n > 0 {
                    if let Some(back) = additional.back_mut() {
                        if back.len() <= n {
                            n -= back.len();
                            additional.pop_back();
                        } else {
                            back.truncate(back.len() - n);
                            break;
                        }
                    } else if head.len() <= n {
                        *head = Bytes::new();
                        break; // consumed the whole (only) chunk
                    } else {
                        head.truncate(head.len() - n);
                        break;
                    }
                }
            }
            Repr::Deep(d) => {
                while n > 0 {
                    if let Some(back) = d.tail.back_mut() {
                        // consume the buffered tail chunk by chunk
                        if back.len() <= n {
                            n -= back.len();
                            d.tail.pop_back();
                        } else {
                            back.truncate(back.len() - n);
                            break;
                        }
                    } else if let Some(bb) = d.tree.back_block_bytes() {
                        if bb <= n {
                            // whole rightmost block is dropped — release it in one op, no per-chunk work
                            d.tree.pop_back_block();
                            n -= bb;
                        } else {
                            // the drop boundary is inside this block: buffer it, then trim above
                            let block = d.tree.pop_back_block().expect("block present");
                            d.tail.extend(block);
                        }
                    } else if !d.head.is_empty() {
                        core::mem::swap(&mut d.head, &mut d.tail);
                    } else {
                        break;
                    }
                }
            }
        }
        self.maybe_demote();
    }

    /// Flattens the rope into one contiguous [`Bytes`]. Zero-copy when there is a single chunk;
    /// otherwise copies. Prefer [`ByteRope::chunks`] when you only need to read.
    pub fn copy_to_bytes(&self) -> Bytes {
        if self.len == 0 {
            return Bytes::new();
        }
        // single-chunk fast path: hand back the chunk with no copy (matches the flat buffer)
        if let Repr::Small { head, additional } = &self.repr {
            if additional.is_empty() {
                return head.clone();
            }
        } else if self.chunk_count() == 1 {
            return self.chunks().next().expect("one chunk").clone();
        }
        let mut out = bytes::BytesMut::with_capacity(self.len);
        self.extend_into(&mut out);
        out.freeze()
    }

    /// Wraps this rope in a [`Tagged`] owned by `owner`, tracking its bytes against the owner's
    /// running budget. Mirrors the flat buffer's `tag` so callers can swap the two.
    #[inline]
    pub fn tag<O: tagged::Owner>(self, owner: &O) -> tagged::Tagged<O> {
        tagged::Tagged::new(self, owner)
    }

    // --- internal helpers -------------------------------------------------

    /// Appends every byte of the rope to `out`, in order, via a direct traversal of the underlying
    /// storage — bypassing the [`Chunks`] iterator's per-chunk bookkeeping (and, in the deep tier, its
    /// resumable tree walk). The hot path behind flatten (`copy_to_bytes`/`copy_to_bytes_mut`).
    fn extend_into(&self, out: &mut bytes::BytesMut) {
        match &self.repr {
            Repr::Small { head, additional } => {
                if !head.is_empty() {
                    out.extend_from_slice(head);
                }
                for c in additional {
                    out.extend_from_slice(c);
                }
            }
            Repr::Deep(d) => {
                for c in &d.head {
                    out.extend_from_slice(c);
                }
                d.tree.for_each_chunk(&mut |c| out.extend_from_slice(c));
                for c in &d.tail {
                    out.extend_from_slice(c);
                }
            }
        }
    }

    #[inline]
    fn front_chunk_len(&self) -> Option<usize> {
        match &self.repr {
            Repr::Small { head, .. } => (!head.is_empty()).then(|| head.len()),
            Repr::Deep(d) => d
                .head
                .front()
                .map(|c| c.len())
                .or_else(|| d.tree.front_chunk_len())
                .or_else(|| d.tail.front().map(|c| c.len())),
        }
    }

    /// Promotes a `Small` rope to `Deep`: drains its chunks into a fresh tree, leaving buffered
    /// head/tail empty. Called when `additional` crosses `PROMOTE_AT`.
    fn promote(&mut self) {
        let Repr::Small { head, additional } = &mut self.repr else {
            return;
        };
        let mut chunks: VecDeque<Bytes> = core::mem::take(additional);
        if !head.is_empty() {
            chunks.push_front(core::mem::take(head));
        }
        let mut tree = Tree::new();
        // fold whole blocks into the tree; keep the remainder (< FANOUT) in the tail buffer
        let mut block: Vec<Bytes> = Vec::with_capacity(FANOUT);
        let tail: VecDeque<Bytes> = {
            let mut it = chunks.into_iter();
            let mut tail = VecDeque::new();
            for chunk in it.by_ref() {
                block.push(chunk);
                if block.len() == FANOUT {
                    tree.push_block(core::mem::take(&mut block));
                    block = Vec::with_capacity(FANOUT);
                }
            }
            tail.extend(block);
            tail
        };
        self.repr = Repr::Deep(Box::new(Deep {
            head: VecDeque::new(),
            tree,
            tail,
        }));
    }

    /// Folds the whole rope (buffered head + tree + tail) into a single tree. Buffered ends are
    /// small, so this is O(log) via `concat`; a pure-tree `Deep` rope returns its tree directly.
    fn into_tree(self) -> Tree {
        match self.repr {
            Repr::Small { head, additional } => {
                let mut t = Tree::new();
                extend_blocks(
                    &mut t,
                    core::iter::once(head)
                        .filter(|b| !b.is_empty())
                        .chain(additional),
                );
                t
            }
            Repr::Deep(d) => {
                let Deep { head, tree, tail } = *d;
                let mut t = tree;
                if !head.is_empty() {
                    let mut ht = Tree::new();
                    extend_blocks(&mut ht, head.into_iter());
                    t = Tree::concat(ht, t);
                }
                if !tail.is_empty() {
                    let mut tt = Tree::new();
                    extend_blocks(&mut tt, tail.into_iter());
                    t = Tree::concat(t, tt);
                }
                t
            }
        }
    }

    /// Wraps a tree as a `Deep` rope (empty buffered ends), demoting to `Small` if it is small.
    fn from_tree(tree: Tree) -> Self {
        let mut rope = ByteRope {
            len: tree.byte_len(),
            repr: Repr::Deep(Box::new(Deep {
                head: VecDeque::new(),
                tree,
                tail: VecDeque::new(),
            })),
        };
        rope.maybe_demote();
        rope
    }

    /// Demotes a `Deep` rope back to `Small` once it drains below `DEMOTE_AT` chunks.
    #[inline]
    fn maybe_demote(&mut self) {
        let should = matches!(&self.repr, Repr::Deep(_)) && self.chunk_count() <= DEMOTE_AT;
        if !should {
            return;
        }
        // flatten everything back into a head + additional deque
        let mut additional: VecDeque<Bytes> = self.chunks().cloned().collect();
        let head = additional.pop_front().unwrap_or_default();
        self.repr = Repr::Small { head, additional };
    }
}

enum ChunksInner<'a> {
    Small {
        head: Option<&'a Bytes>,
        rest: alloc::collections::vec_deque::Iter<'a, Bytes>,
    },
    Deep {
        head: alloc::collections::vec_deque::Iter<'a, Bytes>,
        tree: tree::Chunks<'a>,
        tail: alloc::collections::vec_deque::Iter<'a, Bytes>,
        /// Which section is being drained: 0 = head, 1 = tree, 2 = tail. Advances once per section
        /// (rather than re-polling an exhausted iterator on every chunk).
        phase: u8,
    },
}

/// Iterator over a rope's `Bytes` chunks, in order. See [`ByteRope::chunks`].
///
/// Reports its exact chunk count via [`ExactSizeIterator::len`] (like `ByteVec`'s chunk iterator),
/// so `rope.chunks().len()` is the way to get the chunk count.
pub struct Chunks<'a> {
    /// Chunks not yet yielded; drives `size_hint`/`len` and stays exact as `next` drains.
    remaining: usize,
    inner: ChunksInner<'a>,
}

impl<'a> Chunks<'a> {
    #[inline]
    fn next_chunk(&mut self) -> Option<&'a Bytes> {
        match &mut self.inner {
            ChunksInner::Small { head, rest } => {
                core::mem::replace(head, rest.next())
            }
            ChunksInner::Deep {
                head,
                tree,
                tail,
                phase,
            } => loop {
                match phase {
                    0 => match head.next() {
                        some @ Some(_) => return some,
                        None => *phase = 1,
                    },
                    1 => match tree.next() {
                        some @ Some(_) => return some,
                        None => *phase = 2,
                    },
                    _ => return tail.next(),
                }
            },
        }
    }
}

impl<'a> Iterator for Chunks<'a> {
    type Item = &'a Bytes;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let item = self.next_chunk();
        if item.is_some() {
            self.remaining -= 1;
        }
        item
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for Chunks<'_> {}

/// Batches `chunks` into blocks of up to `FANOUT` and pushes each onto `tree`.
fn extend_blocks(tree: &mut Tree, chunks: impl Iterator<Item = Bytes>) {
    let mut block = Vec::with_capacity(FANOUT);
    for c in chunks {
        if c.is_empty() {
            continue;
        }
        block.push(c);
        if block.len() == FANOUT {
            tree.push_block(core::mem::replace(&mut block, Vec::with_capacity(FANOUT)));
        }
    }
    if !block.is_empty() {
        tree.push_block(block);
    }
}

/// Walks `chunks` and returns the byte at `offset` within their concatenation, or `None`.
#[inline]
fn byte_in_chunks<'a>(chunks: impl Iterator<Item = &'a Bytes>, mut offset: usize) -> Option<u8> {
    for chunk in chunks {
        if offset < chunk.len() {
            return Some(chunk[offset]);
        }
        offset -= chunk.len();
    }
    None
}

/// Boundary-slices the chunks of a buffered end (`chunks`, whose first byte is at global offset
/// `region_start`) that overlap the byte range `[start, end)`, returning them in order. `Bytes::slice`
/// is an O(1) shared view; every returned chunk is non-empty (`[start, end)` is a non-empty range and
/// only overlapping chunks are kept).
fn slice_deque_chunks(
    chunks: &VecDeque<Bytes>,
    region_start: usize,
    start: usize,
    end: usize,
) -> Vec<Bytes> {
    let mut out = Vec::new();
    let mut acc = region_start;
    for chunk in chunks {
        let (cs, ce) = (acc, acc + chunk.len());
        acc = ce;
        if ce <= start {
            continue;
        }
        if cs >= end {
            break;
        }
        let lo = start.saturating_sub(cs);
        let hi = end.min(ce) - cs;
        out.push(chunk.slice(lo..hi));
    }
    out
}

/// Resolves a `RangeBounds` against `len` into a validated `[start, end)`, erroring if it is
/// inverted or runs past the end.
fn resolve_range(
    range: &impl core::ops::RangeBounds<usize>,
    len: usize,
) -> Result<(usize, usize), ByteRopeError> {
    use core::ops::Bound;
    let start = match range.start_bound() {
        Bound::Included(&s) => s,
        Bound::Excluded(&s) => s + 1,
        Bound::Unbounded => 0,
    };
    let end = match range.end_bound() {
        Bound::Included(&e) => e + 1,
        Bound::Excluded(&e) => e,
        Bound::Unbounded => len,
    };
    if start > end || end > len {
        return Err(ByteRopeError::OutOfBounds(start.max(end)));
    }
    Ok((start, end))
}

/// Locates the chunks holding `start` (`si`,`so`) and `end` (`ei`,`eo`) in the flat tier (index 0 =
/// `head`, `k` = `additional[k-1]`). An offset at the very end lands past all chunks (index == count,
/// intra-offset 0). One scan, stops once both are found.
fn locate_span(
    head: &Bytes,
    additional: &VecDeque<Bytes>,
    start: usize,
    end: usize,
) -> (usize, usize, usize, usize) {
    let n = if head.is_empty() { 0 } else { 1 + additional.len() };
    let (mut si, mut so, mut ei, mut eo) = (n, 0, n, 0);
    let mut pos = 0;
    for i in 0..n {
        let clen = if i == 0 { head.len() } else { additional[i - 1].len() };
        let ce = pos + clen;
        if si == n && start < ce {
            si = i;
            so = start - pos;
        }
        if ei == n && end < ce {
            ei = i;
            eo = end - pos;
            break; // `end >= start`, so `si` is already set
        }
        pos = ce;
    }
    (si, so, ei, eo)
}

/// Splices the flat tier: replaces the chunks covering `[si.so, ei.eo)` with `prefix ++ value ++
/// suffix`, where prefix/suffix are O(1) `Bytes::slice` views of the boundary chunks and `value`'s
/// owned chunks stream in zero-copy. Restructures the deque with one `split_off`/rejoin — no
/// per-element shift, no whole-list copy. Handles a large single chunk and multi-chunk spans alike.
fn splice_flat<R>(
    head: &mut Bytes,
    additional: &mut VecDeque<Bytes>,
    si: usize,
    so: usize,
    ei: usize,
    eo: usize,
    value: &mut R,
) where
    R: etude_buffer::reader::Buffer<Error = core::convert::Infallible>,
{
    use etude_buffer::reader::Infallible as _;
    // Unify `head` into the deque so indices are contiguous (O(1); reuses `additional`).
    if !head.is_empty() {
        additional.push_front(core::mem::take(head));
    }
    let n = additional.len();
    let prefix = (so > 0).then(|| additional[si].slice(0..so));
    let suffix = (ei < n && eo < additional[ei].len()).then(|| additional[ei].slice(eo..));
    let end_idx = if ei < n { ei + 1 } else { n };

    let mut tail = additional.split_off(end_idx); // tail = [end_idx..]
    additional.truncate(si); // keep [0..si]
    additional.extend(prefix);
    if value.buffered_len() > 0 {
        // Append value chunks (owned stay zero-copy). Skip when empty: the deque writer's
        // `put_bytes`/`put_slice` do not filter empties, so an empty value would push a spurious
        // empty chunk and break the "no empty chunks" invariant.
        value.infallible_copy_into(additional);
    }
    additional.extend(suffix);
    additional.append(&mut tail);
    *head = additional.pop_front().unwrap_or_default();
}

/// If `[start, end)` lies within a single flat-tier chunk, returns `(index, start_offset, end_offset)`
/// (index 0 = `head`, `k` = `additional[k-1]`; offsets are within that chunk). Partial scan.
fn locate_within_chunk(
    head: &Bytes,
    additional: &VecDeque<Bytes>,
    start: usize,
    end: usize,
) -> Option<(usize, usize, usize)> {
    let n = if head.is_empty() { 0 } else { 1 + additional.len() };
    let mut pos = 0;
    for i in 0..n {
        let clen = if i == 0 { head.len() } else { additional[i - 1].len() };
        if start < pos + clen {
            return (end <= pos + clen).then_some((i, start - pos, end - pos));
        }
        pos += clen;
    }
    None // `start` is at the very end — an append, not a within-chunk edit
}

/// Replaces chunk `si` (0 = `head`, `k` = `additional[k-1]`) — keeping `[..so]` and `[eo..]` of it —
/// with a single chunk holding `[..so] ++ value ++ [eo..]`. One allocation; removes the element if
/// the result is empty. The byte source streams into the buffer with no allocation of its own.
fn splice_one_chunk<R>(
    head: &mut Bytes,
    additional: &mut VecDeque<Bytes>,
    si: usize,
    so: usize,
    eo: usize,
    value: &mut R,
) where
    R: etude_buffer::reader::Buffer<Error = core::convert::Infallible>,
{
    use etude_buffer::reader::Infallible as _;
    let old = if si == 0 {
        core::mem::take(head)
    } else {
        core::mem::take(&mut additional[si - 1])
    };
    let mut buf = BytesMut::with_capacity(so + value.buffered_len() + (old.len() - eo));
    buf.extend_from_slice(&old[..so]);
    value.infallible_copy_into(&mut buf); // grows the buffer — drains the whole source
    buf.extend_from_slice(&old[eo..]);
    let new_chunk = buf.freeze();
    if new_chunk.is_empty() {
        if si == 0 {
            *head = additional.pop_front().unwrap_or_default();
        } else {
            additional.remove(si - 1);
        }
    } else if si == 0 {
        *head = new_chunk;
    } else {
        additional[si - 1] = new_chunk;
    }
}

/// Overwrites `count` bytes starting at flat-tier offset `start` with bytes streamed from `value`
/// (an equal-length edit: `start + count` is in bounds). Walks only the covered chunks — locates the
/// first with a partial scan, then writes each once via bounded [`cow_edit`]. No list alloc, no rescan.
fn overwrite_flat<R>(
    head: &mut Bytes,
    additional: &mut VecDeque<Bytes>,
    start: usize,
    count: usize,
    value: &mut R,
) where
    R: etude_buffer::reader::Buffer<Error = core::convert::Infallible>,
{
    use etude_buffer::reader::Infallible as _;
    // Advance to the chunk holding `start` (partial scan). Slot 0 = `head`, slot k = `additional[k-1]`.
    let mut i = 0;
    let mut pos = 0;
    loop {
        let clen = if i == 0 { head.len() } else { additional[i - 1].len() };
        if start < pos + clen {
            break;
        }
        pos += clen;
        i += 1;
    }
    // Walk forward, writing each covered chunk's slice of `value` once, with a bounded copy-on-write.
    let mut written = 0;
    let mut local = start - pos;
    while written < count {
        let clen = if i == 0 { head.len() } else { additional[i - 1].len() };
        let here = (clen - local).min(count - written);
        let taken = if i == 0 {
            core::mem::take(head)
        } else {
            core::mem::take(&mut additional[i - 1])
        };
        let edit = cow_edit(taken, local, here, |s| {
            let mut dst: &mut [u8] = s;
            value.infallible_copy_into(&mut dst);
        });
        // For slot 0 the split spills into `additional`'s front, so the next original chunk lands at
        // `1 + added`; for a deque slot the pieces are inserted in place.
        let added = if i == 0 {
            splice_cow_head(head, additional, edit)
        } else {
            splice_cow_deque(additional, i - 1, edit)
        };
        written += here;
        i += 1 + added;
        local = 0;
    }
}

/// Overwrites `count` bytes at offset `start` within a chunk deque (the deep tier's head/tail),
/// walking only the covered chunks. Bounded copy-on-write: interior chunks are fully rewritten (copy
/// == write), and only a large *shared* boundary chunk with a small covered span is split so its
/// untouched edge is shared, not copied (see [`cow_edit`]).
fn overwrite_deque<R>(chunks: &mut VecDeque<Bytes>, start: usize, count: usize, value: &mut R)
where
    R: etude_buffer::reader::Buffer<Error = core::convert::Infallible>,
{
    use etude_buffer::reader::Infallible as _;
    let mut i = 0;
    let mut pos = 0;
    while i < chunks.len() {
        let clen = chunks[i].len();
        if start < pos + clen {
            break;
        }
        pos += clen;
        i += 1;
    }
    let mut written = 0;
    let mut local = start - pos;
    while written < count {
        let here = (chunks[i].len() - local).min(count - written);
        let edit = cow_edit(core::mem::take(&mut chunks[i]), local, here, |s| {
            let mut dst: &mut [u8] = s;
            value.infallible_copy_into(&mut dst);
        });
        let added = splice_cow_deque(chunks, i, edit);
        written += here;
        i += 1 + added; // skip past any spliced-in prefix/suffix to the next original chunk
        local = 0;
    }
}

/// Overwrites `count` bytes at offset `start` of a deep-tier rope with bytes from `value`, dispatching
/// the range across the buffered head, the tree (O(log₃₂) FBIP descent), and the buffered tail.
fn overwrite_deep<R>(d: &mut Deep, start: usize, count: usize, value: &mut R)
where
    R: etude_buffer::reader::Buffer<Error = core::convert::Infallible>,
{
    let head_bytes: usize = d.head.iter().map(|c| c.len()).sum();
    let tree_bytes = d.tree.byte_len();
    let mut off = start;
    let mut remaining = count;
    if off < head_bytes {
        let here = (head_bytes - off).min(remaining);
        overwrite_deque(&mut d.head, off, here, value);
        remaining -= here;
        off += here;
    }
    if remaining > 0 && off < head_bytes + tree_bytes {
        let here = (head_bytes + tree_bytes - off).min(remaining);
        d.tree.overwrite(off - head_bytes, here, value);
        remaining -= here;
        off += here;
    }
    if remaining > 0 {
        overwrite_deque(&mut d.tail, off - head_bytes - tree_bytes, remaining, value);
    }
}

/// The largest *shared* chunk we copy whole on a copy-on-write edit. Above this we split the chunk,
/// sharing the untouched prefix/suffix via O(1) [`Bytes::slice`] so the copy is bounded by the edited
/// span rather than the whole chunk — editing one byte of a shared 1 GiB chunk copies one byte, not a
/// gigabyte. Below it, a whole-chunk copy is cheap and avoids fragmenting the buffer.
pub(crate) const COW_SPLIT_ABOVE: usize = 4096;

/// The outcome of a bounded copy-on-write edit of a chunk (see [`cow_edit`]).
pub(crate) enum CowEdit {
    /// One chunk replaces the original in place: edited in its own unique allocation, or a bounded
    /// whole-chunk copy (unique, or shared but `<= COW_SPLIT_ABOVE`, or the edit spanned the whole chunk).
    InPlace(Bytes),
    /// A large *shared* chunk was split so the copy stayed bounded: the original chunk is replaced, in
    /// order, by the untouched shared `prefix` (if any), the freshly-written `mid`, and the untouched
    /// shared `suffix` (if any). At least one of `prefix`/`suffix` is present.
    Split {
        prefix: Option<Bytes>,
        mid: Bytes,
        suffix: Option<Bytes>,
    },
}

/// Edits `[at, at + span)` of `chunk` (`span >= 1`, in bounds) by invoking `write` on exactly that
/// span. Mutates the allocation in place when the chunk uniquely owns it; when the chunk is shared,
/// copies only a bounded region — the whole chunk if small, else just the edited span with the
/// untouched prefix/suffix shared as O(1) slices. Never copies the untouched bytes of a large chunk.
pub(crate) fn cow_edit(chunk: Bytes, at: usize, span: usize, write: impl FnOnce(&mut [u8])) -> CowEdit {
    debug_assert!(span >= 1 && at + span <= chunk.len());
    match chunk.try_into_mut() {
        Ok(mut buf) => {
            write(&mut buf[at..at + span]);
            CowEdit::InPlace(buf.freeze())
        }
        Err(shared) if shared.len() <= COW_SPLIT_ABOVE => {
            let mut buf = BytesMut::from(&shared[..]);
            write(&mut buf[at..at + span]);
            CowEdit::InPlace(buf.freeze())
        }
        Err(shared) => {
            let prefix = (at > 0).then(|| shared.slice(0..at));
            let suffix = (at + span < shared.len()).then(|| shared.slice(at + span..));
            let mut mid = BytesMut::from(&shared[at..at + span]);
            write(&mut mid);
            let mid = mid.freeze();
            match (prefix, suffix) {
                // Whole chunk rewritten anyway (copy == write): keep it a single chunk.
                (None, None) => CowEdit::InPlace(mid),
                (prefix, suffix) => CowEdit::Split { prefix, mid, suffix },
            }
        }
    }
}

/// Applies a [`CowEdit`] to chunk `i` of a deque, splicing in the extra pieces of a split. Returns the
/// number of chunks added (0, 1, or 2). The deque slot `i` must already have been consumed (taken).
fn splice_cow_deque(deque: &mut VecDeque<Bytes>, i: usize, edit: CowEdit) -> usize {
    match edit {
        CowEdit::InPlace(c) => {
            deque[i] = c;
            0
        }
        CowEdit::Split { prefix, mid, suffix } => {
            deque[i] = mid;
            let mut added = 0;
            if let Some(s) = suffix {
                deque.insert(i + 1, s);
                added += 1;
            }
            if let Some(p) = prefix {
                deque.insert(i, p); // pushes `mid` (and any suffix) right by one
                added += 1;
            }
            added
        }
    }
}

/// Applies a [`CowEdit`] to a flat-tier `head` chunk: `head` takes the first piece and any remainder
/// of a split goes to the front of `additional`, preserving order. Returns the number of pieces pushed
/// into `additional` (0, 1, or 2) so a caller walking `head` then `additional` can skip past them.
fn splice_cow_head(head: &mut Bytes, additional: &mut VecDeque<Bytes>, edit: CowEdit) -> usize {
    match edit {
        CowEdit::InPlace(c) => {
            *head = c;
            0
        }
        CowEdit::Split { prefix, mid, suffix } => {
            let mut added = 0;
            if let Some(s) = suffix {
                additional.push_front(s);
                added += 1;
            }
            match prefix {
                Some(p) => {
                    additional.push_front(mid); // front order becomes [mid, suffix, ..]
                    added += 1;
                    *head = p;
                }
                None => *head = mid,
            }
            added
        }
    }
}

/// Sets the byte at `offset` within a chunk deque, with a bounded copy-on-write (see [`cow_edit`]).
fn set_byte_in_deque(deque: &mut VecDeque<Bytes>, mut offset: usize, value: u8) {
    for i in 0..deque.len() {
        let clen = deque[i].len();
        if offset < clen {
            let edit = cow_edit(core::mem::take(&mut deque[i]), offset, 1, |s| s[0] = value);
            splice_cow_deque(deque, i, edit);
            return;
        }
        offset -= clen;
    }
    unreachable!("offset past end of chunk deque");
}

impl ByteRope {
    /// Content equality against a contiguous byte slice (chunking-independent). O(n) via per-chunk
    /// `memcmp`.
    fn bytes_eq(&self, mut other: &[u8]) -> bool {
        if self.len != other.len() {
            return false;
        }
        for chunk in self.chunks() {
            let (a, rest) = other.split_at(chunk.len());
            if chunk.as_ref() != a {
                return false;
            }
            other = rest;
        }
        true
    }
}

/// Chunk-aware content equality between two byte-chunk sequences: advances two cursors and compares
/// each overlapping run as a slice (a `memcmp`), instead of comparing one byte at a time through
/// flattened iterator adapters. Empty chunks are skipped; the sequences are equal iff they yield the
/// same bytes in order and both end together. Callers should length-check first for a cheap reject.
fn chunks_content_eq<'a, 'b>(
    mut left: impl Iterator<Item = &'a [u8]>,
    mut right: impl Iterator<Item = &'b [u8]>,
) -> bool {
    let mut a: &[u8] = &[];
    let mut b: &[u8] = &[];
    loop {
        while a.is_empty() {
            match left.next() {
                Some(c) => a = c,
                None => break,
            }
        }
        while b.is_empty() {
            match right.next() {
                Some(c) => b = c,
                None => break,
            }
        }
        if a.is_empty() || b.is_empty() {
            // one side is exhausted — equal iff the other is too
            return a.is_empty() && b.is_empty();
        }
        let n = a.len().min(b.len());
        if a[..n] != b[..n] {
            return false;
        }
        a = &a[n..];
        b = &b[n..];
    }
}

impl PartialEq for ByteRope {
    /// Two ropes are equal iff their byte content is equal, regardless of how it is chunked.
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len
            && chunks_content_eq(
                self.chunks().map(|c| &c[..]),
                other.chunks().map(|c| &c[..]),
            )
    }
}

impl Eq for ByteRope {}

macro_rules! impl_bytes_eq {
    ($($ty:ty => |$s:ident| $bytes:expr),* $(,)?) => {
        $(
            impl PartialEq<$ty> for ByteRope {
                #[inline]
                fn eq(&self, other: &$ty) -> bool {
                    let $s = other;
                    self.bytes_eq($bytes)
                }
            }
        )*
    };
}

impl_bytes_eq! {
    [u8] => |o| o,
    &[u8] => |o| o,
    Vec<u8> => |o| o,
    str => |o| o.as_bytes(),
    &str => |o| o.as_bytes(),
    Bytes => |o| o,
}

impl<const N: usize> PartialEq<[u8; N]> for ByteRope {
    #[inline]
    fn eq(&self, other: &[u8; N]) -> bool {
        self.bytes_eq(other)
    }
}

impl<const N: usize> PartialEq<&[u8; N]> for ByteRope {
    #[inline]
    fn eq(&self, other: &&[u8; N]) -> bool {
        self.bytes_eq(*other)
    }
}

// Equality against a slice/array of `Bytes` chunks (chunking-independent — compares byte content),
// mirroring `etude_bytevec::ByteVec` for a drop-in swap. Chunk-aware (`memcmp` per overlapping run),
// not a byte-at-a-time flatten compare.
impl PartialEq<[Bytes]> for ByteRope {
    #[inline]
    fn eq(&self, other: &[Bytes]) -> bool {
        let other_len: usize = other.iter().map(Bytes::len).sum();
        self.len == other_len
            && chunks_content_eq(self.chunks().map(|c| &c[..]), other.iter().map(|c| &c[..]))
    }
}

impl PartialEq<&[Bytes]> for ByteRope {
    #[inline]
    fn eq(&self, other: &&[Bytes]) -> bool {
        self.eq(*other)
    }
}

impl<const LEN: usize> PartialEq<[Bytes; LEN]> for ByteRope {
    #[inline]
    fn eq(&self, other: &[Bytes; LEN]) -> bool {
        self.eq(&other[..])
    }
}

impl<const LEN: usize> PartialEq<&[Bytes; LEN]> for ByteRope {
    #[inline]
    fn eq(&self, other: &&[Bytes; LEN]) -> bool {
        self.eq(&other[..])
    }
}

impl From<Bytes> for ByteRope {
    fn from(bytes: Bytes) -> Self {
        let mut rope = Self::new();
        rope.push_back(bytes);
        rope
    }
}

impl FromIterator<Bytes> for ByteRope {
    fn from_iter<I: IntoIterator<Item = Bytes>>(iter: I) -> Self {
        let mut rope = Self::new();
        for chunk in iter {
            rope.push_back(chunk);
        }
        rope
    }
}

// ---- byte-buffer trait plumbing (mirrors etude_bytevec::ByteVec for a drop-in swap) ----

use etude_buffer::{reader, writer};

impl ByteRope {
    /// Reads up to `watermark` bytes off the front as one `Bytes` (a zero-copy slice of the boundary
    /// chunk), advancing past them. Empty if the rope is empty or `watermark == 0`.
    #[inline]
    fn read_chunk_bytes(&mut self, watermark: usize) -> Bytes {
        if watermark == 0 {
            return Bytes::new();
        }
        let Some(front_len) = self.front_chunk_len() else {
            return Bytes::new();
        };
        if front_len <= watermark {
            self.pop_front().unwrap_or_default()
        } else {
            let mut chunk = self.pop_front().expect("front chunk present");
            let head = chunk.split_to(watermark);
            self.push_front(chunk);
            head
        }
    }

    /// Copies the front chunk into `dest` (up to `dest`'s remaining capacity), advancing; pushes back
    /// any remainder that did not fit.
    #[inline]
    fn copy_front<Dest: writer::Buffer + ?Sized>(&mut self, dest: &mut Dest) {
        let Some(mut chunk) = self.pop_front() else {
            return;
        };
        let cap = dest.remaining_capacity();
        if chunk.len() <= cap {
            dest.put_bytes(chunk);
        } else {
            let head = chunk.split_to(cap);
            dest.put_bytes(head);
            self.push_front(chunk);
        }
    }

    /// The front chunk as a contiguous slice (for [`bytes::Buf::chunk`]), or `&[]` if empty.
    #[inline]
    fn front_slice(&self) -> &[u8] {
        match &self.repr {
            Repr::Small { head, .. } => head,
            Repr::Deep(d) => d
                .head
                .front()
                .or_else(|| d.tree.front_chunk())
                .or_else(|| d.tail.front())
                .map_or(&[], |b| &b[..]),
        }
    }
}

impl reader::Buffer for ByteRope {
    type Error = core::convert::Infallible;

    #[inline]
    fn buffered_len(&self) -> usize {
        self.len
    }

    #[inline]
    fn read_chunk(&mut self, watermark: usize) -> Result<reader::Chunk<'_>, Self::Error> {
        Ok(self.read_chunk_bytes(watermark).into())
    }

    #[inline]
    fn partial_copy_into<Dest>(&mut self, dest: &mut Dest) -> Result<reader::Chunk<'_>, Self::Error>
    where
        Dest: writer::Buffer + ?Sized,
    {
        loop {
            let front_len = match self.front_chunk_len() {
                Some(l) if l > 0 => l,
                _ => return Ok(reader::Chunk::empty()),
            };
            let cap = dest.remaining_capacity();
            // if the front chunk fills (or overfills) the destination, hand it back for the caller
            if front_len >= cap {
                return Ok(self.read_chunk_bytes(cap).into());
            }
            self.copy_front(dest);
        }
    }
}

impl writer::Buffer for ByteRope {
    const SPECIALIZES_BYTES: bool = true;

    #[inline]
    fn put_slice(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.push_back(Bytes::copy_from_slice(bytes));
    }

    #[inline]
    fn put_bytes(&mut self, bytes: Bytes) {
        self.push_back(bytes);
    }

    #[inline]
    fn remaining_capacity(&self) -> usize {
        usize::MAX
    }
}

impl bytes::Buf for ByteRope {
    #[inline]
    fn remaining(&self) -> usize {
        self.len
    }

    #[inline]
    fn chunk(&self) -> &[u8] {
        self.front_slice()
    }

    #[inline]
    fn advance(&mut self, cnt: usize) {
        assert!(cnt <= self.len, "advance {cnt} past end (len {})", self.len);
        self.consume_front(cnt);
    }

    #[inline]
    fn copy_to_bytes(&mut self, len: usize) -> Bytes {
        assert!(len <= self.len, "copy_to_bytes {len} past end (len {})", self.len);
        // fast path: the whole run is within the front chunk (zero-copy slice)
        if self.front_chunk_len().is_some_and(|fl| fl >= len) {
            return self.read_chunk_bytes(len);
        }
        let mut out = bytes::BytesMut::with_capacity(len);
        let mut remaining = len;
        while remaining > 0 {
            let chunk = self.read_chunk_bytes(remaining);
            out.extend_from_slice(&chunk);
            remaining -= chunk.len();
        }
        out.freeze()
    }
}

impl core::fmt::Debug for ByteRope {
    #[inline]
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.chunks()).finish()
    }
}

impl From<bytes::BytesMut> for ByteRope {
    #[inline]
    fn from(value: bytes::BytesMut) -> Self {
        value.freeze().into()
    }
}

impl From<Vec<u8>> for ByteRope {
    #[inline]
    fn from(value: Vec<u8>) -> Self {
        Bytes::from(value).into()
    }
}

impl From<alloc::string::String> for ByteRope {
    #[inline]
    fn from(value: alloc::string::String) -> Self {
        value.into_bytes().into()
    }
}

impl From<&'static [u8]> for ByteRope {
    #[inline]
    fn from(value: &'static [u8]) -> Self {
        Bytes::from_static(value).into()
    }
}

impl<const LEN: usize> From<&'static [u8; LEN]> for ByteRope {
    #[inline]
    fn from(value: &'static [u8; LEN]) -> Self {
        Bytes::from_static(value).into()
    }
}

impl From<&'static str> for ByteRope {
    #[inline]
    fn from(value: &'static str) -> Self {
        Bytes::from_static(value.as_bytes()).into()
    }
}

impl From<Vec<Bytes>> for ByteRope {
    #[inline]
    fn from(value: Vec<Bytes>) -> Self {
        value.into_iter().collect()
    }
}

impl From<ByteRope> for Vec<Bytes> {
    fn from(rope: ByteRope) -> Self {
        rope.into_iter().collect()
    }
}

impl From<ByteRope> for VecDeque<Bytes> {
    fn from(rope: ByteRope) -> Self {
        rope.into_iter().collect()
    }
}

impl core::ops::Index<usize> for ByteRope {
    type Output = Bytes;

    #[inline]
    fn index(&self, index: usize) -> &Self::Output {
        self.get(index).expect("chunk index out of bounds")
    }
}

impl Extend<Bytes> for ByteRope {
    #[inline]
    fn extend<I: IntoIterator<Item = Bytes>>(&mut self, iter: I) {
        for chunk in iter {
            self.push_back(chunk);
        }
    }
}

impl Extend<ByteRope> for ByteRope {
    #[inline]
    fn extend<I: IntoIterator<Item = ByteRope>>(&mut self, iter: I) {
        for mut rope in iter {
            self.append(&mut rope);
        }
    }
}

impl Extend<Vec<u8>> for ByteRope {
    #[inline]
    fn extend<I: IntoIterator<Item = Vec<u8>>>(&mut self, iter: I) {
        for bytes in iter {
            self.push_back(bytes.into());
        }
    }
}

/// Draining iterator over a rope's `Bytes` chunks (front to back). See [`ByteRope::into_iter`].
pub struct IntoChunks {
    rope: ByteRope,
}

impl Iterator for IntoChunks {
    type Item = Bytes;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.rope.pop_front()
    }
}

impl IntoIterator for ByteRope {
    type Item = Bytes;
    type IntoIter = IntoChunks;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        IntoChunks { rope: self }
    }
}

#[cfg(any(test, feature = "bolero-generator"))]
impl bolero_generator::TypeGenerator for ByteRope {
    #[inline]
    fn generate<D>(driver: &mut D) -> Option<Self>
    where
        D: bolero_generator::Driver,
    {
        use bolero_generator::ValueGenerator as _;

        let count = (1..4).generate(driver)?;
        let mut out = ByteRope::with_capacity(count);
        for _ in 0..count {
            let bytes: Vec<u8> = bolero_generator::TypeGenerator::generate(driver)?;
            out.push_back(bytes.into());
        }
        Some(out)
    }
}

#[cfg(feature = "std")]
impl std::io::Read for ByteRope {
    #[inline]
    fn read(&mut self, mut buf: &mut [u8]) -> std::io::Result<usize> {
        use etude_buffer::reader::Infallible as _;
        use etude_buffer::writer::Buffer as _;
        let mut dest = buf.track_write();
        self.infallible_copy_into(&mut dest);
        Ok(dest.written_len())
    }
}

#[cfg(feature = "std")]
impl std::io::Write for ByteRope {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        writer::Buffer::put_slice(self, buf);
        Ok(buf.len())
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[std::io::IoSlice<'_>]) -> std::io::Result<usize> {
        use etude_buffer::reader::{Buffer as _, Infallible as _};
        let mut bufs = reader::IoSlice::new(bufs);
        let len = bufs.buffered_len();
        let mut bytes = bytes::BytesMut::with_capacity(len);
        bufs.infallible_copy_into(&mut bytes);
        self.push_back(bytes.freeze());
        Ok(len)
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        writer::Buffer::put_slice(self, buf);
        Ok(())
    }

    #[inline]
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl ByteRope {
    /// Flattens the rope into a single [`bytes::BytesMut`], consuming it. Zero-copy when there is a
    /// single chunk; otherwise copies. Prefer [`ByteRope::chunks`] when you only need to read.
    pub fn copy_to_bytes_mut(self) -> bytes::BytesMut {
        if self.len == 0 {
            return bytes::BytesMut::new();
        }
        if let Repr::Small { head, additional } = &self.repr
            && additional.is_empty()
        {
            return bytes::BytesMut::from(head.clone());
        }
        let mut out = bytes::BytesMut::with_capacity(self.len);
        self.extend_into(&mut out);
        out
    }

    /// A non-consuming reader over the rope's bytes. Cheap: it holds an O(1)-shared clone, so reading
    /// through it leaves `self` untouched.
    #[inline]
    pub fn reader(&self) -> Reader {
        Reader {
            inner: self.clone(),
        }
    }
}

/// A non-consuming [`reader::Buffer`] over a [`ByteRope`]. See [`ByteRope::reader`].
pub struct Reader {
    inner: ByteRope,
}

impl Reader {
    #[inline]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl reader::Buffer for Reader {
    type Error = core::convert::Infallible;

    #[inline]
    fn buffered_len(&self) -> usize {
        self.inner.buffered_len()
    }

    #[inline]
    fn read_chunk(&mut self, watermark: usize) -> Result<reader::Chunk<'_>, Self::Error> {
        self.inner.read_chunk(watermark)
    }

    #[inline]
    fn partial_copy_into<Dest>(&mut self, dest: &mut Dest) -> Result<reader::Chunk<'_>, Self::Error>
    where
        Dest: writer::Buffer + ?Sized,
    {
        self.inner.partial_copy_into(dest)
    }
}

impl Iterator for Reader {
    type Item = Bytes;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.pop_front()
    }
}

#[cfg(test)]
mod tests;
