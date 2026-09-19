// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! `StrRope` — a UTF-8 string rope.
//!
//! A cheaply-clonable, copy-avoiding growable string built on the `etude-bytevec` byte rope. Where
//! [`etude_str::Str`](https://docs.rs) is a *flat* `Bytes`-backed string, `StrRope` is its rope-shaped
//! sibling: a newtype over [`etude_bytevec::Rope<Utf8>`](etude_bytevec::Rope), so it gets the tiered
//! rope's O(1) clone and O(log n) split/concat, with a UTF-8 invariant layered on top.
//!
//! ## Invariant
//! The rope's **concatenated** byte content is always valid UTF-8. Individual internal chunks may fall
//! inside a multi-byte codepoint (chunks arrive from arbitrary syscall/network splits), so the invariant
//! is over the logical byte stream, not per chunk. Byte-indexed operations (`insert_str`, `split_off`,
//! `slice`) require their offsets to be char boundaries, which `StrRope` checks before delegating to the
//! raw byte-offset rope ops.
//!
//! ## Interop
//! [`StrRope::into_bytes`] returns the underlying [`ByteVec`] for **free** (only
//! the zero-size kind marker is dropped — no copy, no re-validation), and [`StrRope::from_utf8`] validates
//! a `ByteVec` back into a `StrRope` (O(n)).

use etude_bytevec::{ByteVec, Rope, Utf8};

/// A UTF-8 string rope: a validated-UTF-8 view over the [`etude-bytevec`](etude_bytevec) byte rope.
///
/// `Clone` is O(1) (a rope refcount bump). Equality and ordering are by byte content.
#[derive(Clone, Default)]
pub struct StrRope(Rope<Utf8>);

impl StrRope {
    /// The empty string rope.
    #[must_use]
    pub fn new() -> Self {
        Self(Rope::default())
    }

    /// Validates that `bytes`' concatenated content is valid UTF-8 and wraps it as a `StrRope`.
    ///
    /// **O(n)** — the logical byte stream is scanned once (a codepoint may span a chunk boundary, so
    /// validation is over the concatenation, not per chunk). The `ByteVec`'s allocation is reused — no
    /// copy of the rope structure.
    ///
    /// # Errors
    /// Returns the [`Utf8Error`](core::str::Utf8Error) on the first invalid sequence.
    pub fn from_utf8(bytes: ByteVec) -> Result<Self, core::str::Utf8Error> {
        Rope::<Utf8>::try_from_bytes(bytes).map(Self)
    }

    /// Converts into the underlying [`ByteVec`] — **free** (drops the zero-size kind marker; the rope
    /// representation moves as-is, no copy).
    #[inline]
    #[must_use]
    pub fn into_bytes(self) -> ByteVec {
        self.0.into_bytes()
    }

    /// Length in bytes (not chars), like [`str::len`].
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the string rope is empty.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether `byte_idx` lies on a UTF-8 char boundary (so a split/insert there is valid), like
    /// [`str::is_char_boundary`]. `0` and [`len`](Self::len) are always boundaries; an index past the end
    /// is never one.
    #[must_use]
    pub fn is_char_boundary(&self, byte_idx: usize) -> bool {
        let len = self.len();
        if byte_idx == 0 || byte_idx == len {
            return true;
        }
        // A boundary is any byte that is NOT a UTF-8 continuation byte (0b10xx_xxxx, i.e. 0x80..=0xBF).
        // Matches the stdlib check `(b as i8) >= -0x40`.
        match self.0.byte_at(byte_idx) {
            Some(b) => (b as i8) >= -0x40,
            None => false, // byte_idx > len
        }
    }

    /// Appends a string slice at the end. O(log n)-ish (a rope push); no re-validation (`s` is already
    /// valid UTF-8, and valid content appended after valid content meets on a codepoint boundary).
    #[inline]
    pub fn push_str(&mut self, s: &str) {
        self.0.append_bytes(s.as_bytes());
    }

    /// Appends a single [`char`].
    #[inline]
    pub fn push(&mut self, c: char) {
        let mut buf = [0u8; 4];
        self.0.append_bytes(c.encode_utf8(&mut buf).as_bytes());
    }

    /// Iterates the bytes of the concatenated content, in order (across all chunks).
    #[inline]
    pub fn bytes(&self) -> impl Iterator<Item = u8> + '_ {
        self.0.chunks().flat_map(|chunk| chunk.iter().copied())
    }

    /// Iterates the raw byte chunks of the rope, in order (double-ended). Chunk boundaries are arbitrary
    /// — a codepoint may straddle two chunks — so this is for byte-level plumbing; use
    /// [`chars`](Self::chars) for text.
    #[inline]
    pub fn chunks(&self) -> impl DoubleEndedIterator<Item = bytes::Bytes> + '_ {
        self.0.chunks().cloned()
    }

    /// Iterates the [`char`]s of the content, reassembling any codepoint that spans a chunk boundary.
    #[inline]
    #[must_use]
    pub fn chars(&self) -> Chars<'_> {
        Chars {
            chunks: self.0.chunks(),
            cur: &[],
            pos: 0,
        }
    }

    /// Like [`chars`](Self::chars), but also yields each char's starting byte offset.
    #[inline]
    #[must_use]
    pub fn char_indices(&self) -> CharIndices<'_> {
        CharIndices {
            chars: self.chars(),
            offset: 0,
        }
    }

    /// Inserts a string slice at byte offset `byte_idx`. O(log n).
    ///
    /// # Panics
    /// Panics if `byte_idx` is not a char boundary, or is past the end.
    pub fn insert_str(&mut self, byte_idx: usize, s: &str) {
        assert!(
            self.is_char_boundary(byte_idx),
            "insert_str at a non-char-boundary index {byte_idx}"
        );
        self.0.insert_bytes(byte_idx, s.as_bytes());
    }

    /// Inserts a single [`char`] at byte offset `byte_idx`.
    ///
    /// # Panics
    /// Panics if `byte_idx` is not a char boundary, or is past the end.
    pub fn insert(&mut self, byte_idx: usize, c: char) {
        let mut buf = [0u8; 4];
        self.insert_str(byte_idx, c.encode_utf8(&mut buf));
    }

    /// Splits the rope in two at byte offset `byte_idx`: `self` keeps `[0, byte_idx)` and the returned
    /// `StrRope` holds `[byte_idx, len)` (like [`String::split_off`]). O(log n); shares structure.
    ///
    /// # Panics
    /// Panics if `byte_idx` is not a char boundary, or is past the end.
    #[must_use = "the split-off tail is returned; use it or the split is pointless"]
    pub fn split_off(&mut self, byte_idx: usize) -> StrRope {
        assert!(
            self.is_char_boundary(byte_idx),
            "split_off at a non-char-boundary index {byte_idx}"
        );
        // `split_to(byte_idx)` returns [0, byte_idx) and leaves `self` = [byte_idx, len). Swap so `self`
        // keeps the head and we return the tail — `String::split_off` semantics.
        let head = self
            .0
            .split_to(byte_idx)
            .expect("split_off index past the end of the rope");
        let tail = core::mem::replace(&mut self.0, head);
        StrRope(tail)
    }

    /// Returns the sub-rope over the byte `range`. O(log n); shares structure with `self` (no copy).
    ///
    /// # Panics
    /// Panics if either bound is not a char boundary.
    #[must_use]
    pub fn slice<R: core::ops::RangeBounds<usize>>(&self, range: R) -> StrRope {
        use core::ops::Bound;
        // `saturating_add` so an excluded start / inclusive end of `usize::MAX` cannot wrap the
        // `+ 1` to 0 (which would silently return the wrong slice in release); it saturates to
        // `usize::MAX`, which is past the end and so fails the `is_char_boundary` assert below —
        // the documented out-of-bounds panic.
        let start = match range.start_bound() {
            Bound::Included(&s) => s,
            Bound::Excluded(&s) => s.saturating_add(1),
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(&e) => e.saturating_add(1),
            Bound::Excluded(&e) => e,
            Bound::Unbounded => self.len(),
        };
        assert!(
            self.is_char_boundary(start),
            "slice start {start} is not a char boundary"
        );
        assert!(
            self.is_char_boundary(end),
            "slice end {end} is not a char boundary"
        );
        StrRope(self.0.slice(start..end))
    }
}

/// The width in bytes of the UTF-8 codepoint whose leading byte is `b` (1..=4). For a valid leading byte
/// (guaranteed by the rope's UTF-8 invariant) this is exact; a stray continuation byte falls back to 1.
#[inline]
fn utf8_char_width(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

/// Iterator over the [`char`]s of a [`StrRope`] (see [`StrRope::chars`]). Reassembles a codepoint that
/// spans a chunk boundary by pulling bytes across chunks.
pub struct Chars<'a> {
    chunks: etude_bytevec::Chunks<'a>,
    cur: &'a [u8],
    pos: usize,
}

impl Chars<'_> {
    /// The next byte of the logical stream, advancing across chunks (skipping empty ones).
    #[inline]
    fn next_byte(&mut self) -> Option<u8> {
        loop {
            if self.pos < self.cur.len() {
                let b = self.cur[self.pos];
                self.pos += 1;
                return Some(b);
            }
            self.cur = &self.chunks.next()?[..];
            self.pos = 0;
        }
    }
}

impl Iterator for Chars<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        let b0 = self.next_byte()?;
        let width = utf8_char_width(b0);
        let mut buf = [b0, 0, 0, 0];
        for slot in buf.iter_mut().take(width).skip(1) {
            *slot = self
                .next_byte()
                .expect("StrRope invariant: content is valid UTF-8 (truncated codepoint)");
        }
        core::str::from_utf8(&buf[..width])
            .expect("StrRope invariant: content is valid UTF-8")
            .chars()
            .next()
    }
}

/// Iterator over `(byte_offset, char)` pairs of a [`StrRope`] (see [`StrRope::char_indices`]).
pub struct CharIndices<'a> {
    chars: Chars<'a>,
    offset: usize,
}

impl Iterator for CharIndices<'_> {
    type Item = (usize, char);

    fn next(&mut self) -> Option<(usize, char)> {
        let start = self.offset;
        let c = self.chars.next()?;
        self.offset += c.len_utf8();
        Some((start, c))
    }
}

impl From<&str> for StrRope {
    fn from(s: &str) -> Self {
        let mut r = Self::new();
        r.push_str(s);
        r
    }
}

impl From<String> for StrRope {
    fn from(s: String) -> Self {
        Self::from(s.as_str())
    }
}

// Content equality / ordering / hashing — by the concatenated bytes, so a StrRope compares and hashes like
// its text regardless of how it is chunked internally. (Two StrRopes with the same content but different
// chunk boundaries are equal.)
impl PartialEq for StrRope {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.bytes().eq(other.bytes())
    }
}
impl Eq for StrRope {}
impl PartialOrd for StrRope {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for StrRope {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        // Lexicographic by bytes == lexicographic by chars for UTF-8, matching `str`'s ordering.
        self.bytes().cmp(other.bytes())
    }
}
impl core::hash::Hash for StrRope {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        for b in self.bytes() {
            state.write_u8(b);
        }
    }
}

// Equality with the primitive string types (so tests + call sites read naturally).
impl PartialEq<str> for StrRope {
    fn eq(&self, other: &str) -> bool {
        self.len() == other.len() && self.bytes().eq(other.bytes())
    }
}
impl PartialEq<&str> for StrRope {
    fn eq(&self, other: &&str) -> bool {
        *self == **other
    }
}

impl core::fmt::Display for StrRope {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Linearize to one contiguous buffer, then view as &str (valid by the invariant). Individual
        // chunks can't be written as &str — a codepoint may straddle a chunk boundary.
        let contiguous = self.0.copy_to_bytes();
        // SAFETY-equivalent: the invariant guarantees valid UTF-8, but use the checked path (Display is
        // already O(n) here) to avoid any unsafe.
        match core::str::from_utf8(&contiguous) {
            Ok(s) => f.write_str(s),
            Err(_) => Err(core::fmt::Error), // unreachable given the invariant
        }
    }
}

impl core::fmt::Debug for StrRope {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let contiguous = self.0.copy_to_bytes();
        match core::str::from_utf8(&contiguous) {
            Ok(s) => core::fmt::Debug::fmt(s, f),
            Err(_) => Err(core::fmt::Error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StrRope;
    use etude_bytevec::ByteVec;

    /// Differential oracle vs `str`/`String`: arbitrary raw bytes, built into ropes under several
    /// chunk layouts (so multi-byte codepoints and invalid sequences STRADDLE leaf boundaries), must
    /// agree with `core::str::from_utf8` on accept/reject AND `valid_up_to`; on accept, every
    /// str-facing query must match the flat `&str`. This is the fence for any future optimization of
    /// the linearize-then-validate path (e.g. per-chunk validation with boundary stitching).
    #[test]
    fn differential_against_str_oracle() {
        use core::hash::{Hash, Hasher};
        fn hash_of(s: &StrRope) -> u64 {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            s.hash(&mut h);
            h.finish()
        }
        bolero::check!()
            .with_type::<Vec<u8>>()
            .cloned()
            .for_each(|bytes| {
                let oracle = core::str::from_utf8(&bytes);
                let mut accepted: Vec<StrRope> = Vec::new();
                for chunk in [1usize, 2, 3, 7, bytes.len().max(1)] {
                    let mut rope = ByteVec::new();
                    for piece in bytes.chunks(chunk) {
                        rope.push_back(bytes::Bytes::copy_from_slice(piece));
                    }
                    match (StrRope::from_utf8(rope), oracle) {
                        (Ok(s), Ok(flat)) => {
                            assert_eq!(s.len(), flat.len(), "len at chunk={chunk}");
                            assert_eq!(s, *flat, "content at chunk={chunk}");
                            assert!(
                                s.bytes().eq(flat.bytes()),
                                "byte iterator at chunk={chunk}"
                            );
                            // is_char_boundary parity on every index incl. len and past-end.
                            for i in 0..=flat.len() + 1 {
                                assert_eq!(
                                    s.is_char_boundary(i),
                                    flat.is_char_boundary(i),
                                    "is_char_boundary({i}) at chunk={chunk}"
                                );
                            }
                            assert_eq!(format!("{s}"), flat, "Display at chunk={chunk}");
                            accepted.push(s);
                        }
                        (Err(e), Err(want)) => {
                            assert_eq!(
                                e.valid_up_to(),
                                want.valid_up_to(),
                                "valid_up_to at chunk={chunk}"
                            );
                        }
                        (got, want) => panic!(
                            "accept/reject diverged from str oracle at chunk={chunk}: rope={}, str={}",
                            got.is_ok(),
                            want.is_ok()
                        ),
                    }
                }
                // Chunking must be invisible to Eq/Ord/Hash across every accepted layout.
                for pair in accepted.windows(2) {
                    assert_eq!(pair[0], pair[1], "Eq across chunkings");
                    assert_eq!(
                        pair[0].cmp(&pair[1]),
                        core::cmp::Ordering::Equal,
                        "Ord across chunkings"
                    );
                    assert_eq!(hash_of(&pair[0]), hash_of(&pair[1]), "Hash across chunkings");
                }
            });
    }

    /// RED (breaker-byterope): `slice` resolves its bounds with an UNCHECKED `+ 1`
    /// (`Excluded(s) => s + 1`, `Included(e) => e + 1`). In release the add WRAPS to 0, so an
    /// out-of-bounds bound silently returns the WRONG rope instead of panicking: an Excluded start
    /// of `usize::MAX` yields the WHOLE rope, and an Included end of `usize::MAX` yields the EMPTY
    /// rope. Same bug class as etude#40 (ByteVec resolve_range/slice, fixed with checked/saturating
    /// adds) reintroduced in the StrRope layer. Fix-shape-agnostic: any panic satisfies this test
    /// (debug currently panics via arithmetic overflow; the fix should make release panic per the
    /// documented out-of-bounds contract). Run in RELEASE to observe the failure.
    #[test]
    fn slice_bound_plus_one_must_not_wrap_on_overflow() {
        use core::ops::Bound;
        use core::panic::AssertUnwindSafe;
        let r = StrRope::from("hello");
        let got = std::panic::catch_unwind(AssertUnwindSafe(|| {
            r.slice((Bound::Excluded(usize::MAX), Bound::Unbounded))
        }));
        assert!(
            got.is_err(),
            "slice(Excluded(MAX)..) must panic (start out of bounds), not wrap to the whole rope"
        );
        let got = std::panic::catch_unwind(AssertUnwindSafe(|| r.slice(0..=usize::MAX)));
        assert!(
            got.is_err(),
            "slice(..=MAX) must panic (end out of bounds), not wrap to the empty rope"
        );
    }

    /// Mutation-op differential harness vs a `String` model: every mutating and structural op the
    /// crate exposes, applied in fuzz-chosen sequences (fuzzed indices snapped down to the nearest
    /// char boundary so both sides accept them), must keep the rope byte-identical to the model —
    /// content, length, `chars`/`char_indices`, `Display`, and slicing all agree after every step.
    /// The `Rechunk` op rebuilds the rope from the model under a fuzz-chosen chunk layout
    /// mid-sequence, so later ops run against shifted leaf boundaries.
    #[test]
    fn mutation_differential_against_string_model() {
        use bolero_generator::TypeGenerator;

        #[derive(Debug, Clone, TypeGenerator)]
        enum Op {
            PushStr(String),
            Push(char),
            InsertStr(usize, String),
            Insert(usize, char),
            SplitOffKeepHead(usize),
            SplitOffKeepTail(usize),
            SliceCheck(usize, usize),
            CharsCheck,
            Rechunk(u8),
        }

        fn snap(model: &str, idx: usize) -> usize {
            let mut i = idx % (model.len() + 1);
            while !model.is_char_boundary(i) {
                i -= 1;
            }
            i
        }

        bolero::check!()
            .with_type::<Vec<Op>>()
            .cloned()
            .for_each(|ops| {
                let mut rope = StrRope::new();
                let mut model = String::new();
                for op in ops {
                    match &op {
                        Op::PushStr(s) => {
                            rope.push_str(s);
                            model.push_str(s);
                        }
                        Op::Push(c) => {
                            rope.push(*c);
                            model.push(*c);
                        }
                        Op::InsertStr(idx, s) => {
                            let at = snap(&model, *idx);
                            rope.insert_str(at, s);
                            model.insert_str(at, s);
                        }
                        Op::Insert(idx, c) => {
                            let at = snap(&model, *idx);
                            rope.insert(at, *c);
                            model.insert(at, *c);
                        }
                        Op::SplitOffKeepHead(idx) => {
                            let at = snap(&model, *idx);
                            let tail = rope.split_off(at);
                            let mtail = model.split_off(at);
                            assert_eq!(tail, *mtail.as_str(), "split-off tail");
                        }
                        Op::SplitOffKeepTail(idx) => {
                            let at = snap(&model, *idx);
                            let tail = rope.split_off(at);
                            model = model.split_off(at);
                            rope = tail;
                        }
                        Op::SliceCheck(a, b) => {
                            let (mut a, mut b) = (snap(&model, *a), snap(&model, *b));
                            if a > b {
                                core::mem::swap(&mut a, &mut b);
                            }
                            assert_eq!(rope.slice(a..b), model[a..b], "slice({a}..{b})");
                        }
                        Op::CharsCheck => {
                            assert!(rope.chars().eq(model.chars()), "chars");
                            assert!(rope.char_indices().eq(model.char_indices()), "char_indices");
                        }
                        Op::Rechunk(width) => {
                            let width = usize::from(width % 7) + 1;
                            let mut bytes = ByteVec::new();
                            for piece in model.as_bytes().chunks(width) {
                                bytes.push_back(bytes::Bytes::copy_from_slice(piece));
                            }
                            rope = StrRope::from_utf8(bytes).expect("model is valid UTF-8");
                        }
                    }
                    assert_eq!(rope, *model.as_str(), "content after {op:?}");
                    assert_eq!(rope.len(), model.len(), "len after {op:?}");
                }
            });
    }

    #[test]
    fn from_str_and_basic_queries() {
        let s = StrRope::from("héllo");
        assert_eq!(s.len(), 6); // 'é' is 2 bytes
        assert!(!s.is_empty());
        assert!(StrRope::new().is_empty());
        assert_eq!(StrRope::default().len(), 0);
        assert_eq!(s, "héllo");
        assert_eq!(s.bytes().collect::<Vec<_>>(), "héllo".as_bytes());
    }

    #[test]
    fn from_string_preserves_content() {
        assert_eq!(StrRope::from(String::from("café")), "café");
    }

    #[test]
    fn push_str_and_push() {
        let mut s = StrRope::new();
        s.push_str("ab");
        s.push('é');
        s.push_str("cd");
        assert_eq!(s, "abécd");
        assert_eq!(s.len(), 6);
    }

    #[test]
    fn is_char_boundary_matches_str() {
        let text = "aéb"; // bytes: a(1) é(2) b(1) => len 4
        let s = StrRope::from(text);
        for i in 0..=s.len() + 1 {
            assert_eq!(
                s.is_char_boundary(i),
                text.is_char_boundary(i),
                "mismatch at {i}"
            );
        }
    }

    #[test]
    fn into_bytes_and_from_utf8_round_trip() {
        let s = StrRope::from("round trip ✓");
        let bytes: ByteVec = s.clone().into_bytes();
        let back = StrRope::from_utf8(bytes).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn from_utf8_rejects_invalid() {
        let mut bv = ByteVec::default();
        bv.push_back(bytes::Bytes::from_static(&[0xFF, 0xFE]));
        assert!(StrRope::from_utf8(bv).is_err());
    }

    #[test]
    fn clone_is_content_equal() {
        let a = StrRope::from("shared");
        let b = a.clone();
        assert_eq!(a, b);
    }

    #[test]
    fn ordering_and_display_by_content() {
        let mut v = [
            StrRope::from("banana"),
            StrRope::from("apple"),
            StrRope::from("cherry"),
        ];
        v.sort();
        assert_eq!(
            v.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            ["apple", "banana", "cherry"]
        );
        assert_eq!(format!("{}", StrRope::from("café")), "café");
        assert_eq!(format!("{:?}", StrRope::from("hi")), "\"hi\"");
    }

    #[test]
    fn equal_content_different_chunking_compares_equal() {
        // Build a StrRope whose internal chunks split the multi-byte 'é' across a boundary, then confirm
        // it still equals the flat-constructed one (content equality is chunk-independent).
        let mut bv = ByteVec::default();
        bv.push_back(bytes::Bytes::from_static(b"a"));
        bv.push_back(bytes::Bytes::from_static(&[0xC3])); // first byte of 'é'
        bv.push_back(bytes::Bytes::from_static(&[0xA9])); // second byte of 'é'
        bv.push_back(bytes::Bytes::from_static(b"b"));
        let split = StrRope::from_utf8(bv).unwrap();
        assert_eq!(split, StrRope::from("aéb"));
        assert_eq!(split, "aéb");
        assert_eq!(split.len(), 4);
    }

    #[test]
    fn chars_and_char_indices_match_str() {
        let text = "héllo wörld 🦀!";
        let s = StrRope::from(text);
        assert_eq!(s.chars().collect::<String>(), text);
        assert_eq!(
            s.chars().collect::<Vec<_>>(),
            text.chars().collect::<Vec<_>>()
        );
        assert_eq!(
            s.char_indices().collect::<Vec<_>>(),
            text.char_indices().collect::<Vec<_>>()
        );
    }

    #[test]
    fn chars_reassembles_codepoint_split_across_chunks() {
        // Build a rope where 'é' (2 bytes) and '🦀' (4 bytes) are each split one byte per chunk.
        let mut bv = ByteVec::default();
        bv.push_back(bytes::Bytes::from_static(b"a"));
        for byte in "é🦀".as_bytes() {
            bv.push_back(bytes::Bytes::copy_from_slice(&[*byte]));
        }
        bv.push_back(bytes::Bytes::from_static(b"z"));
        let s = StrRope::from_utf8(bv).unwrap();
        assert_eq!(s.chars().collect::<String>(), "aé🦀z");
        assert_eq!(
            s.char_indices().collect::<Vec<_>>(),
            "aé🦀z".char_indices().collect::<Vec<_>>()
        );
    }

    #[test]
    fn insert_str_and_insert() {
        let mut s = StrRope::from("ad");
        s.insert_str(1, "bc");
        assert_eq!(s, "abcd");
        s.insert(0, 'é');
        assert_eq!(s, "éabcd");
    }

    #[test]
    #[should_panic(expected = "non-char-boundary")]
    fn insert_str_non_char_boundary_panics() {
        let mut s = StrRope::from("é"); // 2 bytes; index 1 is mid-codepoint
        s.insert_str(1, "x");
    }

    #[test]
    fn split_off_matches_string_semantics() {
        let mut s = StrRope::from("hello wörld");
        let tail = s.split_off(6); // char boundary before 'w'
        assert_eq!(s, "hello ");
        assert_eq!(tail, "wörld");
        // mirror std::string::String::split_off
        let mut std_s = String::from("hello wörld");
        let std_tail = std_s.split_off(6);
        assert_eq!(s, std_s.as_str());
        assert_eq!(tail, std_tail.as_str());
        // edge cases
        let mut a = StrRope::from("xy");
        assert_eq!(a.split_off(0), "xy");
        assert!(a.is_empty());
        let mut b = StrRope::from("xy");
        assert!(b.split_off(2).is_empty());
        assert_eq!(b, "xy");
    }

    #[test]
    fn slice_ranges() {
        let s = StrRope::from("héllo");
        assert_eq!(s.slice(0..3), "hé"); // 'h'(1) + 'é'(2)
        assert_eq!(s.slice(3..), "llo");
        assert_eq!(s.slice(..), "héllo");
        assert_eq!(s.slice(..1), "h");
    }

    #[test]
    #[should_panic(expected = "char boundary")]
    fn slice_non_char_boundary_panics() {
        let s = StrRope::from("é");
        let _ = s.slice(0..1);
    }

    // Property: StrRope built from any String reproduces its bytes, chars, char_indices, and length —
    // std String is the oracle.
    #[test]
    fn prop_matches_str_oracle() {
        bolero::check!().with_type::<String>().for_each(|text| {
            let s = StrRope::from(text.as_str());
            assert_eq!(s.len(), text.len());
            assert!(s == text.as_str());
            assert_eq!(s.bytes().collect::<Vec<_>>(), text.as_bytes());
            assert_eq!(s.chars().collect::<String>(), *text);
            assert_eq!(
                s.char_indices().collect::<Vec<_>>(),
                text.char_indices().collect::<Vec<_>>()
            );
        });
    }
}
