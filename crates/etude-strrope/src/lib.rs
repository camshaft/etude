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
}
