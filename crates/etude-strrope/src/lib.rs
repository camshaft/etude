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
///
/// # Examples
/// ```
/// use etude_strrope::StrRope;
///
/// // Grows like a `String`, but clone is an O(1) structural share rather than a copy.
/// let mut s = StrRope::from("hello");
/// s.push_str(", world");
/// assert_eq!(s, "hello, world");
/// assert_eq!(s.len(), 12); // length in bytes, like `str::len`
///
/// let clone = s.clone();
/// assert_eq!(clone, s); // equality is by content
///
/// // Split at a byte offset (must be a char boundary): `s` keeps `[0, at)`, the tail is returned.
/// let tail = s.split_off(5);
/// assert_eq!(s, "hello");
/// assert_eq!(tail, ", world");
/// ```
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

    /// Wraps `bytes` as a `StrRope` **without validating** — O(1)-structural (a by-move re-wrap, no
    /// scan), unlike the O(n) [`from_utf8`](Self::from_utf8). For a producer that has already validated
    /// the content (a tokenizer, a trusted codec) this skips a redundant UTF-8 scan; the rope structure
    /// moves as-is with no copy.
    ///
    /// # Safety
    /// The concatenated byte content of `bytes` must be valid UTF-8. The invariant is over the
    /// *concatenation*, not per chunk — a codepoint may span a chunk boundary. Passing content that is
    /// not valid UTF-8 is undefined behavior: `StrRope`'s char-boundary operations, and any future
    /// zero-copy `&str` view of the content, rely on this invariant and would act on invalid UTF-8.
    #[must_use]
    pub unsafe fn from_utf8_unchecked(bytes: ByteVec) -> Self {
        // SAFETY: the caller guarantees `bytes`' concatenated content is valid UTF-8, which is exactly
        // the invariant `Rope::<Utf8>::from_bytes_unchecked` requires (it debug-asserts it in debug/test).
        Self(unsafe { Rope::<Utf8>::from_bytes_unchecked(bytes) })
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
        // A boundary is any byte that is not a UTF-8 continuation byte (0b10xx_xxxx, i.e. 0x80..=0xBF).
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
            cur: "".chars(),
            carry: [0u8; 4],
            carry_len: 0,
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

/// Length of the largest prefix of `bytes` that ends on a codepoint boundary — i.e. everything except a
/// trailing codepoint that continues into the next chunk. Given the whole content is valid UTF-8, an
/// incomplete tail is at most 3 bytes and its leading byte lies within the last 4 bytes.
fn valid_prefix_len(bytes: &[u8]) -> usize {
    let n = bytes.len();
    let mut i = n;
    while i > 0 && n - i < 4 {
        i -= 1;
        let b = bytes[i];
        if b < 0x80 {
            return n; // ASCII byte at/near the end: nothing is mid-codepoint
        }
        if b >= 0xC0 {
            // Leading byte at `i`: the tail codepoint is complete iff its full width fits in the chunk.
            let width = utf8_char_width(b);
            return if i + width <= n { n } else { i };
        }
        // continuation byte: keep scanning back for its leading byte
    }
    n
}

/// Feed the content to `emit` as a sequence of valid `&str` pieces, stitching a codepoint that straddles a
/// chunk boundary into a small stack buffer — so the bulk of the content is written in whole-chunk `&str`
/// runs (fast, no allocation), and only the ≤3-byte seams are handled specially. The backbone of
/// [`Display`](core::fmt::Display) / [`Debug`](core::fmt::Debug).
fn for_each_str<F>(rope: &StrRope, mut emit: F) -> core::fmt::Result
where
    F: FnMut(&str) -> core::fmt::Result,
{
    let mut carry = [0u8; 4];
    let mut carry_len = 0usize;
    for chunk in rope.0.chunks() {
        let mut bytes: &[u8] = chunk;
        // 1. Complete a codepoint carried from the previous chunk, using the front of this one.
        if carry_len > 0 {
            let width = utf8_char_width(carry[0]);
            let need = (width - carry_len).min(bytes.len());
            carry[carry_len..carry_len + need].copy_from_slice(&bytes[..need]);
            carry_len += need;
            bytes = &bytes[need..];
            if carry_len < width {
                continue; // still incomplete; wait for the next chunk
            }
            if let Ok(s) = core::str::from_utf8(&carry[..width]) {
                emit(s)?;
            }
            // carry_len is unconditionally reset by step 2's assignment below.
        }
        // 2. Emit the valid prefix in bulk; stash any trailing incomplete codepoint as the new carry.
        let end = valid_prefix_len(bytes);
        if let Ok(s) = core::str::from_utf8(&bytes[..end]) {
            emit(s)?;
        }
        let tail = &bytes[end..];
        carry[..tail.len()].copy_from_slice(tail);
        carry_len = tail.len();
    }
    Ok(())
}

/// Iterator over the [`char`]s of a [`StrRope`] (see [`StrRope::chars`]). Reassembles a codepoint that
/// spans a chunk boundary by pulling bytes across chunks.
pub struct Chars<'a> {
    chunks: etude_bytevec::Chunks<'a>,
    /// Bulk decoder over the valid `&str` prefix of the current chunk — the fast path (std's own
    /// contiguous UTF-8 decoder), so only chunk seams need special handling.
    cur: core::str::Chars<'a>,
    /// Leading bytes of a codepoint that straddles into the next chunk(s). `carry[0]` is always a
    /// leading byte, so its width is known; `carry_len == 0` in the common (no-seam) case.
    carry: [u8; 4],
    carry_len: usize,
}

impl<'a> Chars<'a> {
    /// Point `cur` at the valid prefix of `bytes` (decoded in bulk via `str`), stashing any trailing
    /// incomplete codepoint into `carry` for the next chunk to complete.
    #[inline]
    fn set_cur(&mut self, bytes: &'a [u8]) {
        let end = valid_prefix_len(bytes);
        self.cur = core::str::from_utf8(&bytes[..end])
            .expect("StrRope invariant: content is valid UTF-8")
            .chars();
        let tail = &bytes[end..];
        self.carry[..tail.len()].copy_from_slice(tail);
        self.carry_len = tail.len();
    }
}

impl Iterator for Chars<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        loop {
            // Fast path: decode within the current chunk's contiguous `&str`.
            if let Some(c) = self.cur.next() {
                return Some(c);
            }
            // The current chunk is drained; pull the next non-empty chunk.
            let bytes: &[u8] = loop {
                match self.chunks.next() {
                    None => {
                        debug_assert_eq!(
                            self.carry_len, 0,
                            "StrRope invariant: content is valid UTF-8 (no truncated tail)"
                        );
                        return None;
                    }
                    Some(chunk) if chunk.is_empty() => continue,
                    Some(chunk) => break &chunk[..],
                }
            };
            // A codepoint carried from the previous chunk completes from the front of this one — and,
            // for tiny chunks, may still span further, so consume leading bytes into `carry` until full.
            if self.carry_len > 0 {
                let width = utf8_char_width(self.carry[0]);
                let need = (width - self.carry_len).min(bytes.len());
                self.carry[self.carry_len..self.carry_len + need].copy_from_slice(&bytes[..need]);
                self.carry_len += need;
                if self.carry_len < width {
                    continue; // still incomplete; wait for the next chunk
                }
                let c = core::str::from_utf8(&self.carry[..width])
                    .expect("StrRope invariant: content is valid UTF-8")
                    .chars()
                    .next()
                    .expect("one codepoint");
                self.carry_len = 0;
                // The rest of this chunk becomes `cur` for subsequent calls.
                self.set_cur(&bytes[need..]);
                return Some(c);
            }
            // No carry: this chunk's valid prefix becomes `cur`; loop to pull its first char.
            self.set_cur(bytes);
        }
    }

    /// Counting chars needs no decoding: the number of `char`s is the number of UTF-8 leading bytes
    /// (every byte that is not a `0b10xx_xxxx` continuation byte). Scan the remaining bytes and count
    /// those, across chunks — the same specialization `str::Chars::count` uses — instead of decoding
    /// each codepoint via the default `Iterator::count`. Accounts for mid-iteration state: any chars
    /// left in the current chunk's decoder, plus the single codepoint whose leading bytes are already
    /// held in `carry` (its continuation bytes sit at the front of the upcoming chunks and are skipped
    /// by the continuation-byte test, so it is counted exactly once).
    fn count(self) -> usize {
        let mut n = self.cur.count();
        if self.carry_len > 0 {
            n += 1;
        }
        for chunk in self.chunks {
            n += chunk.iter().filter(|&&b| (b as i8) >= -0x40).count();
        }
        n
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
    /// Wraps a `&str` with a single copy of its bytes and no validation scan (a `&str` is valid UTF-8
    /// by type), via the typed [`Rope<Utf8>`](etude_bytevec::Rope) constructor.
    #[inline]
    fn from(s: &str) -> Self {
        Self(Rope::<Utf8>::from(s))
    }
}

impl From<String> for StrRope {
    /// Wraps a `String` by *moving* its buffer — the allocation is reused, with no copy and no
    /// validation scan (a `String` is valid UTF-8 by type). Prefer this over [`from`](Self::from)`(&str)`
    /// when you own the `String`, to avoid the copy.
    #[inline]
    fn from(s: String) -> Self {
        Self(Rope::<Utf8>::from(s))
    }
}

// Content equality / ordering / hashing — by the concatenated bytes, so a StrRope compares and hashes like
// its text regardless of how it is chunked internally. (Two StrRopes with the same content but different
// chunk boundaries are equal.)
/// Lexicographic byte-content comparison of two ropes, walking both chunk iterators with two cursors and
/// comparing overlapping runs via slice `cmp` (a `memcmp`) — no per-byte iteration, no allocation, and it
/// short-circuits on the first differing run. Byte order == char order for UTF-8, so this matches `str`.
fn cmp_content(a: &StrRope, b: &StrRope) -> core::cmp::Ordering {
    use core::cmp::Ordering;
    let (mut ai, mut bi) = (a.0.chunks(), b.0.chunks());
    let (mut ca, mut cb): (&[u8], &[u8]) = (&[], &[]);
    loop {
        while ca.is_empty() {
            match ai.next() {
                Some(c) => ca = &c[..],
                None => break,
            }
        }
        while cb.is_empty() {
            match bi.next() {
                Some(c) => cb = &c[..],
                None => break,
            }
        }
        match (ca.is_empty(), cb.is_empty()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less, // a is a proper prefix of b
            (false, true) => return Ordering::Greater,
            (false, false) => {
                let n = ca.len().min(cb.len());
                match ca[..n].cmp(&cb[..n]) {
                    Ordering::Equal => {
                        ca = &ca[n..];
                        cb = &cb[n..];
                    }
                    ord => return ord,
                }
            }
        }
    }
}

impl PartialEq for StrRope {
    fn eq(&self, other: &Self) -> bool {
        // O(1) length reject, then a chunk-aligned memcmp (not per-byte).
        self.len() == other.len() && cmp_content(self, other).is_eq()
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
        cmp_content(self, other)
    }
}
impl core::hash::Hash for StrRope {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        // The `Eq`->`Hash` contract requires equal ropes to hash equally, and two ropes are equal by
        // content regardless of internal chunk layout — so the sequence of `Hasher::write` calls must be
        // a pure function of the *content*, not the chunk boundaries. `write` is NOT concatenation-
        // equivalent for boundary-sensitive hashers (aHash/fxhash mix per call), so feeding raw chunks
        // would hash equal ropes differently under those hashers (SipHash happens to be concatenation-
        // equivalent, which masks it). Re-block into fixed-size buffers: the write sequence then depends
        // only on the bytes — full block writes plus a final partial — with no allocation and still bulk
        // (not per-byte). The `0xff` terminator guards composite-key prefix collisions. NB this is not
        // equal to `str`'s hash (str writes all bytes in one call), so `Borrow<str>` stays off the table.
        const BLOCK: usize = 64;
        let mut buf = [0u8; BLOCK];
        let mut len = 0usize;
        for chunk in self.0.chunks() {
            let mut bytes: &[u8] = chunk;
            while !bytes.is_empty() {
                let take = (BLOCK - len).min(bytes.len());
                buf[len..len + take].copy_from_slice(&bytes[..take]);
                len += take;
                bytes = &bytes[take..];
                if len == BLOCK {
                    state.write(&buf);
                    len = 0;
                }
            }
        }
        if len > 0 {
            state.write(&buf[..len]);
        }
        state.write_u8(0xff);
    }
}

// Equality with the primitive string types (so tests + call sites read naturally).
impl PartialEq<str> for StrRope {
    fn eq(&self, other: &str) -> bool {
        if self.len() != other.len() {
            return false;
        }
        // Chunk-aligned memcmp against the contiguous `str` bytes (not per-byte).
        let mut rest = other.as_bytes();
        for chunk in self.0.chunks() {
            let c = &chunk[..];
            if rest.len() < c.len() || rest[..c.len()] != *c {
                return false;
            }
            rest = &rest[c.len()..];
        }
        rest.is_empty()
    }
}
impl PartialEq<&str> for StrRope {
    fn eq(&self, other: &&str) -> bool {
        *self == **other
    }
}

impl core::fmt::Display for StrRope {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Fast path: with no width or precision set, `Formatter::pad` would just write the content
        // verbatim, so stream whole-chunk `&str` runs (stitching only the ≤3-byte codepoint seams) —
        // no whole-content allocation, no per-char overhead.
        if f.width().is_none() && f.precision().is_none() {
            return for_each_str(self, |s| f.write_str(s));
        }
        // Formatted: width, fill, alignment, and precision (char-count truncation) are applied by
        // `Formatter::pad`, which operates on a contiguous `&str` — as `str`'s own Display does — so
        // linearize the content and delegate for exact parity.
        let contiguous = self.0.copy_to_bytes();
        match core::str::from_utf8(&contiguous) {
            Ok(s) => f.pad(s),
            Err(_) => Err(core::fmt::Error), // unreachable given the UTF-8 invariant
        }
    }
}

impl core::fmt::Debug for StrRope {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        use core::fmt::Write as _;
        // Fast path (no width/precision): stream whole-chunk `&str` runs, escaped per `<str as Debug>`
        // (`str::escape_debug` is a bulk-writing Display adapter, so escaping stays out of the per-char
        // path) — no whole-content allocation.
        if f.width().is_none() && f.precision().is_none() {
            f.write_char('"')?;
            for_each_str(self, |s| write!(f, "{}", s.escape_debug()))?;
            return f.write_char('"');
        }
        // Formatted: delegate to `str`'s own Debug over the linearized content for exact flag parity.
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

    /// A conforming `Hasher` whose result depends on write-call boundaries (aHash-style): any
    /// deterministic function of the write sequence is a legal Hasher, so fences using it catch
    /// hash impls that leak internal chunk layout.
    struct BoundarySensitiveHasher(u64);
    impl core::hash::Hasher for BoundarySensitiveHasher {
        fn write(&mut self, bytes: &[u8]) {
            self.0 = self.0.wrapping_mul(31).wrapping_add(bytes.len() as u64);
            for &b in bytes {
                self.0 = self.0.wrapping_mul(131).wrapping_add(u64::from(b));
            }
        }
        fn finish(&self) -> u64 {
            self.0
        }
    }
    fn boundary_sensitive_hash(r: &StrRope) -> u64 {
        use core::hash::{Hash, Hasher};
        let mut h = BoundarySensitiveHasher(0);
        r.hash(&mut h);
        h.finish()
    }
    use etude_bytevec::ByteVec;

    /// Differential oracle vs `str`/`String`: arbitrary raw bytes, built into ropes under several
    /// chunk layouts (so multi-byte codepoints and invalid sequences STRADDLE leaf boundaries), must
    /// agree with `core::str::from_utf8` on accept/reject and `valid_up_to`; on accept, every
    /// str-facing query must match the flat `&str`. This is the fence for any future optimization of
    /// the linearize-then-validate path (e.g. per-chunk validation with boundary stitching).
    #[test]
    #[cfg_attr(miri, ignore)] // miri: skip the bolero fuzz loop (pathologically slow under miri); deterministic tests cover the surface. Operator directive 2026-09-19.
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
                    assert_eq!(
                        boundary_sensitive_hash(&pair[0]),
                        boundary_sensitive_hash(&pair[1]),
                        "Hash across chunkings (write-boundary-sensitive hasher)"
                    );
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
    #[cfg_attr(miri, ignore)] // miri: skip the bolero fuzz loop (pathologically slow under miri); deterministic tests cover the surface. Operator directive 2026-09-19.
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

    /// Red (breaker-byterope): `Display for StrRope` writes via `f.write_str` and ignores the
    /// formatter's width/fill/precision, so `format!("{:>6}", rope)` yields `"ab"` where the same
    /// format over `&str` yields `"    ab"` — a silent divergence from the type StrRope models.
    /// `str`'s own `Display` routes through `Formatter::pad`, which honors width, alignment, fill,
    /// and precision (truncation); the fix should do the same over the linearized content. The
    /// assertions are parity-based, so any conforming implementation passes.
    #[test]
    fn display_honors_format_parameters_like_str() {
        let s = StrRope::from("ab");
        assert_eq!(
            format!("{:>6}", s),
            format!("{:>6}", "ab"),
            "right-align width"
        );
        assert_eq!(
            format!("{:<6}", s),
            format!("{:<6}", "ab"),
            "left-align width"
        );
        assert_eq!(
            format!("{:-^7}", s),
            format!("{:-^7}", "ab"),
            "center with fill"
        );
        let t = StrRope::from("héllo");
        assert_eq!(
            format!("{:.3}", t),
            format!("{:.3}", "héllo"),
            "precision truncates by chars"
        );
        assert_eq!(
            format!("{:>8.2}", t),
            format!("{:>8.2}", "héllo"),
            "width plus precision"
        );
    }

    /// Debug must match `str`'s Debug byte-for-byte under every formatter configuration — width,
    /// fill/alignment, precision, and the alternate flag — including content that Debug escapes
    /// (quotes, backslashes, control bytes, multi-byte characters). Parity-based like the Display
    /// pin from #155, so it locks whatever std does rather than encoding assumptions; requested by
    /// etude-str-migration to guard their alloc-free formatting fast paths.
    #[test]
    fn debug_matches_str_under_format_flags() {
        for content in [
            "",
            "ab",
            "a\"b\\c\nd\te",
            "héllo wörld",
            "q\u{1F600}x",
            "\u{0}ctl",
        ] {
            let rope = StrRope::from(content);
            let s: &str = content;
            assert_eq!(
                format!("{rope:?}"),
                format!("{s:?}"),
                "plain for {content:?}"
            );
            assert_eq!(
                format!("{rope:#?}"),
                format!("{s:#?}"),
                "alternate for {content:?}"
            );
            assert_eq!(
                format!("{rope:>20?}"),
                format!("{s:>20?}"),
                "right width for {content:?}"
            );
            assert_eq!(
                format!("{rope:<20?}"),
                format!("{s:<20?}"),
                "left width for {content:?}"
            );
            assert_eq!(
                format!("{rope:-^25?}"),
                format!("{s:-^25?}"),
                "center fill for {content:?}"
            );
            assert_eq!(
                format!("{rope:.4?}"),
                format!("{s:.4?}"),
                "precision for {content:?}"
            );
            assert_eq!(
                format!("{rope:>14.3?}"),
                format!("{s:>14.3?}"),
                "width+precision for {content:?}"
            );
        }
    }

    /// The Hash contract (equal values yield equal hashes) must hold for every conforming
    /// `Hasher`, including ones whose result depends on `write`-call boundaries (aHash-style —
    /// `Hasher::write` makes no concatenation-equivalence promise). So StrRope's Hash must issue a
    /// chunking-independent write sequence: hashing per internal chunk leaks the layout into the
    /// hash and breaks `HashMap` lookups between equal ropes with different chunkings.
    #[test]
    fn hash_write_sequence_is_chunking_independent() {
        let content = "hello world, héllo wörld";
        let mut single = StrRope::new();
        single.push_str(content);
        let mut bytes = ByteVec::new();
        for piece in content.as_bytes().chunks(3) {
            bytes.push_back(bytes::Bytes::copy_from_slice(piece));
        }
        let chunked = StrRope::from_utf8(bytes).expect("valid");
        assert_eq!(single, chunked, "equal content");
        assert_eq!(
            boundary_sensitive_hash(&single),
            boundary_sensitive_hash(&chunked),
            "Hash must not leak the internal chunk layout"
        );
    }

    /// Ordering between two ropes must match `str` ordering for arbitrary content pairs under
    /// independently chosen chunk layouts. The cross-chunking checks in the construction
    /// differential only compare equal contents (always `Ordering::Equal`), so the less/greater
    /// comparison paths across misaligned chunk boundaries — where a windowed chunk-aligned
    /// compare can misstep — were unfuzzed. Also pins Eq and the `PartialEq<str>` view against the
    /// same oracle, and both operand orders for antisymmetry.
    #[test]
    #[cfg_attr(miri, ignore)] // miri: skip the bolero fuzz loop (pathologically slow under miri); deterministic tests cover the surface. Operator directive 2026-09-19.
    fn ord_between_ropes_matches_str_for_content_pairs() {
        use bolero_generator::TypeGenerator;

        #[derive(Debug, Clone, TypeGenerator)]
        struct Input {
            a: String,
            b: String,
            a_chunk: u8,
            b_chunk: u8,
        }

        fn build(content: &str, chunk: u8) -> StrRope {
            let width = usize::from(chunk % 7) + 1;
            let mut bytes = ByteVec::new();
            for piece in content.as_bytes().chunks(width) {
                bytes.push_back(bytes::Bytes::copy_from_slice(piece));
            }
            StrRope::from_utf8(bytes).expect("source is a valid str")
        }

        bolero::check!()
            .with_type::<Input>()
            .cloned()
            .for_each(|inp| {
                let ra = build(&inp.a, inp.a_chunk);
                let rb = build(&inp.b, inp.b_chunk);
                let want = inp.a.as_str().cmp(inp.b.as_str());
                assert_eq!(
                    ra.cmp(&rb),
                    want,
                    "Ord vs str for {:?} vs {:?}",
                    inp.a,
                    inp.b
                );
                assert_eq!(rb.cmp(&ra), want.reverse(), "Ord antisymmetry");
                assert_eq!(ra == rb, inp.a == inp.b, "Eq vs str");
                assert_eq!(
                    ra == *inp.b.as_str(),
                    inp.a == inp.b,
                    "PartialEq<str> vs str"
                );
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
    fn from_utf8_unchecked_matches_from_utf8_for_valid_content() {
        // For valid UTF-8 the unchecked ctor must produce the same StrRope as the checked one, across
        // chunk layouts (including codepoints straddling boundaries) — it skips only the O(n) scan, not
        // the structure. The bytevec invariant checker (features=["testing"]) validates each built rope.
        let text = "aé🦀z—ß本\u{10FFFF}";
        for size in 1..=6 {
            let mut bv = ByteVec::default();
            for piece in text.as_bytes().chunks(size) {
                bv.push_back(bytes::Bytes::copy_from_slice(piece));
            }
            let checked = StrRope::from_utf8(bv.clone()).unwrap();
            // SAFETY: `bv` holds the bytes of a valid `&str`, so its concatenation is valid UTF-8.
            let unchecked = unsafe { StrRope::from_utf8_unchecked(bv) };
            assert_eq!(checked, unchecked, "content mismatch at chunk size {size}");
            assert_eq!(unchecked, text, "unchecked vs str at chunk size {size}");
        }
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
    fn chars_match_str_across_varied_chunkings() {
        // The bulk chars() decoder emits each chunk's valid `&str` prefix and stitches only the seam
        // codepoint. Exercise that seam at many fixed chunk sizes so a multi-byte codepoint lands at every
        // possible offset within (and straddling) a chunk — std str is the oracle.
        let text = "aé🦀z—ß本d\u{10FFFF}f";
        let bytes = text.as_bytes();
        for size in 1..=7 {
            let mut bv = ByteVec::default();
            for piece in bytes.chunks(size) {
                bv.push_back(bytes::Bytes::copy_from_slice(piece));
            }
            let s = StrRope::from_utf8(bv).unwrap();
            assert_eq!(
                s.chars().collect::<String>(),
                text,
                "chars() mismatch at chunk size {size}"
            );
            assert_eq!(
                s.char_indices().collect::<Vec<_>>(),
                text.char_indices().collect::<Vec<_>>(),
                "char_indices() mismatch at chunk size {size}"
            );
            // The specialized `Chars::count()` (byte scan, no decode) must equal the decoded count,
            // fresh and after partially advancing (so `cur`/`carry` mid-iteration state is exercised).
            assert_eq!(
                s.chars().count(),
                text.chars().count(),
                "count at size {size}"
            );
            let mut it = s.chars();
            for _ in 0..3 {
                it.next();
            }
            assert_eq!(
                it.count(),
                text.chars().count() - 3,
                "mid-iter count at size {size}"
            );
        }
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
    #[cfg_attr(miri, ignore)] // miri: skip the bolero fuzz loop (pathologically slow under miri); deterministic tests cover the surface. Operator directive 2026-09-19.
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

    // Ord/Eq between DIFFERENT content (incl. prefix / empty / multi-byte), built one byte per chunk so
    // the chunk-aligned `cmp_content` walk crosses many leaf boundaries — must match `str`'s ordering.
    #[test]
    fn ord_matches_str_across_chunk_boundaries() {
        fn one_byte_per_chunk(text: &str) -> StrRope {
            let mut bv = ByteVec::new();
            for b in text.as_bytes() {
                bv.push_back(bytes::Bytes::copy_from_slice(&[*b]));
            }
            StrRope::from_utf8(bv).unwrap()
        }
        let words = [
            "", "a", "ab", "abc", "abd", "b", "apple", "applf", "é", "és", "🦀", "🦀s",
        ];
        for x in words {
            for y in words {
                let (rx, ry) = (one_byte_per_chunk(x), one_byte_per_chunk(y));
                assert_eq!(rx.cmp(&ry), x.cmp(y), "cmp {x:?} vs {y:?}");
                assert_eq!(rx == ry, x == y, "eq {x:?} vs {y:?}");
            }
        }
    }

    // Debug escaping must match `<str as Debug>` (quotes, control chars, unicode) — the alloc-free
    // char-stream Debug uses `char::escape_debug`, the same as str.
    #[test]
    fn debug_matches_str() {
        for text in [
            "hi",
            "a\"b",
            "tab\there",
            "new\nline",
            "café 🦀",
            "back\\slash",
            "",
        ] {
            assert_eq!(format!("{:?}", StrRope::from(text)), format!("{text:?}"));
        }
    }
}
