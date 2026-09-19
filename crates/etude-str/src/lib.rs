// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! `Str` — a cheaply-clonable, `Bytes`-backed UTF-8 string.
//!
//! Everywhere a value would otherwise be a `String` (or `Arc<str>`) — an id, a name, a reason, a
//! target — [`Str`] is the cheaper choice. Why: such text values are cloned constantly as they thread
//! through routing, dispatch, and results, and a `String` clone is an allocation + copy; a `Str` clone
//! is an O(1) `bytes::Bytes` refcount bump. It also gives text and bytes ONE representation, so a value
//! crosses the text/binary boundary without re-allocating.
//!
//! It is a newtype over `bytes::Bytes`. Invariant: the wrapped `Bytes` is always valid UTF-8 — every
//! constructor establishes it, so [`Str::as_str`] is a zero-cost view.
//!
//! Ported from cadenza's `cdz-str` crate, preserving its representation, invariants, and public API.

use bytes::Bytes;
use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::str::Utf8Error;

/// A `Bytes`-backed UTF-8 string. `Clone` is an O(1) refcount bump. Deref/Borrow/AsRef to `str` so it
/// works anywhere a `&str` does; equality and ordering are by string content.
#[derive(Clone, Default)]
pub struct Str(Bytes);

impl Str {
    /// The empty string.
    #[must_use]
    pub const fn new() -> Self {
        Self(Bytes::new())
    }

    /// A `Str` from a `'static str` — `const`, so it can back a `static`/`const` item. Mirrors
    /// `Bytes::from_static`: no allocation, the string literal's own static bytes back it. Valid UTF-8
    /// holds by construction (the source is a `str`).
    #[must_use]
    pub const fn from_static(s: &'static str) -> Self {
        Self(Bytes::from_static(s.as_bytes()))
    }

    /// Borrow the text. Zero-cost: the invariant guarantees the bytes are valid UTF-8.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // SAFETY: every constructor establishes and preserves the "wrapped Bytes is valid UTF-8"
        // invariant (the fallible ones validate; the `unchecked` one documents the caller's obligation),
        // and `Bytes` is immutable, so the bytes are still valid UTF-8 here.
        unsafe { std::str::from_utf8_unchecked(&self.0) }
    }

    /// The underlying UTF-8 bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Consume into the backing `Bytes` (O(1)) — for crossing to the binary side without a copy.
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.0
    }

    /// Length in bytes (not chars), like `str::len`.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the string is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Wrap `Bytes` as a `Str`, validating UTF-8. The O(1)-clone entry point for text that arrived as
    /// bytes (a payload, a wire field) without re-allocating.
    ///
    /// # Errors
    /// Returns the [`Utf8Error`] if `bytes` is not valid UTF-8.
    pub fn from_utf8(bytes: Bytes) -> Result<Self, Utf8Error> {
        std::str::from_utf8(&bytes)?;
        Ok(Self(bytes))
    }

    /// Wrap `Bytes` as a `Str` WITHOUT validating UTF-8.
    ///
    /// # Safety
    /// The caller must guarantee `bytes` is valid UTF-8; otherwise [`Str::as_str`] is undefined behaviour.
    #[must_use]
    pub const unsafe fn from_utf8_unchecked(bytes: Bytes) -> Self {
        Self(bytes)
    }
}

impl From<&str> for Str {
    fn from(s: &str) -> Self {
        Self(Bytes::copy_from_slice(s.as_bytes()))
    }
}

impl From<String> for Str {
    fn from(s: String) -> Self {
        // reuses the String's allocation as the Bytes buffer — no copy.
        Self(Bytes::from(s.into_bytes()))
    }
}

impl Deref for Str {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

// OPTIONAL serde support (the `serde` feature, off by default): a `Str` (de)serializes as a plain
// string — the natural mapping for the canonical text type. This lets crates derive
// Serialize/Deserialize on Str-bearing structs without a per-field adapter, while the default
// (feature-off) build stays serde-free.
#[cfg(feature = "serde")]
impl serde::Serialize for Str {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for Str {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Deserialize as an owned String, then reuse its allocation as the backing Bytes (From<String>
        // is a no-copy move). Simpler and always-correct vs a borrowed &str (which not every format
        // can supply).
        <String as serde::Deserialize>::deserialize(deserializer).map(Str::from)
    }
}

impl AsRef<str> for Str {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for Str {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for Str {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Debug for Str {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

// Content equality/ordering + hashing — by the string, so a `Str` interns/compares like its text and
// works as a map/set key alongside `&str` lookups (via `Borrow<str>`).
impl PartialEq for Str {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}
impl Eq for Str {}
impl PartialOrd for Str {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Str {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}
impl std::hash::Hash for Str {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // hash as the str would, so `Borrow<str>` keys hash-match a `&str` lookup.
        self.as_str().hash(state);
    }
}

// Convenience equality with the primitive string types (so tests + call sites read naturally).
impl PartialEq<str> for Str {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}
impl PartialEq<&str> for Str {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

// Integration with the `etude-buffer` copy-avoiding reader (the `buffer` feature). `Str` is a readable
// byte source (`impl reader::Buffer for Str`, below), and `Str::from_reader` reads UTF-8 text back out of
// any reader.
#[cfg(feature = "buffer")]
impl Str {
    /// Drain every buffered byte out of `reader` and validate the result as UTF-8, producing a `Str`.
    ///
    /// Fast path — no copy: when the reader yields all of its bytes in a single `Bytes`- or
    /// `BytesMut`-backed chunk (the common case, e.g. a `bytes::Bytes` source), the resulting `Str`
    /// reuses that chunk's allocation directly. A borrowed-slice chunk, or a reader that spans several
    /// chunks (e.g. a `Chain`), is instead collected into one `bytes::BytesMut` (a single copy) so the
    /// bytes are contiguous before validation.
    ///
    /// # Errors
    /// Returns [`FromReaderError::Read`] if `reader` errors while draining, or [`FromReaderError::Utf8`]
    /// if the drained bytes are not valid UTF-8.
    pub fn from_reader<B>(reader: &mut B) -> Result<Self, FromReaderError<B::Error>>
    where
        B: etude_buffer::reader::Buffer,
    {
        use etude_buffer::reader::Chunk;

        // Read the first contiguous chunk (up to the whole reader). Extract OWNED bytes from it so the
        // reader's mutable borrow is released before we inspect the reader again below.
        let first: bytes::Bytes = match reader
            .read_chunk(usize::MAX)
            .map_err(FromReaderError::Read)?
        {
            Chunk::Bytes(b) => b,             // zero-copy: shares the source allocation
            Chunk::BytesMut(b) => b.freeze(), // zero-copy freeze
            Chunk::Slice(s) => bytes::Bytes::copy_from_slice(s), // a borrowed slice must be copied out
        };

        // Common case: that one chunk drained the reader — validate it directly, no accumulation buffer.
        if reader.buffer_is_empty() {
            return Str::from_utf8(first).map_err(FromReaderError::Utf8);
        }

        // Multi-chunk reader: accumulate the first chunk plus the rest into one contiguous buffer.
        let mut out = bytes::BytesMut::with_capacity(first.len() + reader.buffered_len());
        out.extend_from_slice(&first);
        while !reader.buffer_is_empty() {
            let before = reader.buffered_len();
            let chunk = reader
                .read_chunk(usize::MAX)
                .map_err(FromReaderError::Read)?;
            out.extend_from_slice(&chunk);
            drop(chunk); // release the reader borrow before re-inspecting buffered_len below
            // Guard against a non-advancing implementation so we can never spin forever.
            if reader.buffered_len() == before {
                break;
            }
        }
        Str::from_utf8(out.freeze()).map_err(FromReaderError::Utf8)
    }
}

// `Str` is a readable byte source: its bytes flow through the copy-avoiding reader like a `bytes::Bytes`
// (which is exactly what backs it), so `Str` drops straight into anything that consumes a `reader::Buffer`.
// The read delegates to the inner `Bytes`, but clamps each read down to a UTF-8 char boundary so the bytes
// left in `self` remain valid UTF-8 — the wrapped-bytes invariant is never broken, so `as_str` (a
// zero-cost `from_utf8_unchecked`) stays sound even on a partially-drained `Str`. (A bounded destination
// narrower than the next char therefore takes that char on a later read; an unbounded destination — the
// common case — drains it all in one go.)
#[cfg(feature = "buffer")]
impl etude_buffer::reader::Buffer for Str {
    type Error = core::convert::Infallible;

    #[inline]
    fn buffered_len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn read_chunk(
        &mut self,
        watermark: usize,
    ) -> Result<etude_buffer::reader::Chunk<'_>, Self::Error> {
        // Largest char-boundary offset within the watermark, so the remaining `Str` stays valid UTF-8.
        let mut len = self.0.len().min(watermark);
        let s = self.as_str();
        while len > 0 && !s.is_char_boundary(len) {
            len -= 1;
        }
        // Delegate the actual split to the inner `Bytes` reader.
        self.0.read_chunk(len)
    }

    #[inline]
    fn partial_copy_into<Dest>(
        &mut self,
        dest: &mut Dest,
    ) -> Result<etude_buffer::reader::Chunk<'_>, Self::Error>
    where
        Dest: etude_buffer::writer::Buffer + ?Sized,
    {
        self.read_chunk(dest.remaining_capacity())
    }
}

/// The error returned by [`Str::from_reader`]: either the underlying reader errored, or the drained
/// bytes were not valid UTF-8.
#[cfg(feature = "buffer")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromReaderError<E> {
    /// The reader errored while draining.
    Read(E),
    /// The drained bytes were not valid UTF-8.
    Utf8(Utf8Error),
}

#[cfg(feature = "buffer")]
impl<E: fmt::Display> fmt::Display for FromReaderError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(e) => write!(f, "reader error while reading a Str: {e}"),
            Self::Utf8(e) => write!(f, "invalid UTF-8 while reading a Str: {e}"),
        }
    }
}

#[cfg(feature = "buffer")]
impl<E: std::error::Error + 'static> std::error::Error for FromReaderError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(e) => Some(e),
            Self::Utf8(e) => Some(e),
        }
    }
}

// Property/fuzz generation (the `bolero-generator` feature; also available inside this crate's own
// tests). Generate a `String`, then `Str::from` — so the produced `Str` is always valid UTF-8 by
// construction and the wrapped-bytes invariant can never be violated by a generated value.
#[cfg(any(test, feature = "bolero-generator"))]
impl bolero_generator::TypeGenerator for Str {
    #[inline]
    fn generate<D>(driver: &mut D) -> Option<Self>
    where
        D: bolero_generator::Driver,
    {
        let s: String = bolero_generator::TypeGenerator::generate(driver)?;
        Some(Str::from(s))
    }
}

#[cfg(test)]
mod tests {
    use super::Str;
    use bolero::check;
    use bytes::Bytes;
    use std::collections::HashMap;

    #[test]
    fn from_and_views_round_trip() {
        let s = Str::from("cadenza");
        assert_eq!(s.as_str(), "cadenza");
        assert_eq!(s.as_bytes(), b"cadenza");
        assert_eq!(s.len(), 7);
        assert!(!s.is_empty());
        assert!(Str::new().is_empty());
        assert_eq!(Str::default().as_str(), "");
        // From<String> reuses the allocation; content preserved.
        assert_eq!(Str::from(String::from("héllo")).as_str(), "héllo");
        // into_bytes gives back the same bytes.
        assert_eq!(Str::from("x").into_bytes(), Bytes::from_static(b"x"));
    }

    #[test]
    fn from_static_is_const_and_backs_const_and_static_items() {
        // usable in a `const` item (proves it is a genuine const fn)...
        const HI: Str = Str::from_static("hi");
        assert_eq!(HI, "hi");
        // ...and a `static` item.
        static NAME: Str = Str::from_static("cadenza");
        assert_eq!(NAME.as_str(), "cadenza");
        assert!(Str::from_static("").is_empty());
    }

    #[test]
    fn from_utf8_validates() {
        assert_eq!(
            Str::from_utf8(Bytes::from_static(b"ok")).unwrap().as_str(),
            "ok"
        );
        // 0xFF is not valid UTF-8.
        assert!(Str::from_utf8(Bytes::from_static(&[0xFF, 0xFE])).is_err());
        // multi-byte UTF-8 survives.
        let e = "café".to_string().into_bytes();
        assert_eq!(Str::from_utf8(Bytes::from(e)).unwrap().as_str(), "café");
    }

    #[test]
    fn clone_shares_the_buffer() {
        // A clone is a refcount bump on the same allocation, not a copy — assert they alias.
        let a = Str::from("shared buffer, one allocation");
        let b = a.clone();
        assert_eq!(a, b);
        assert_eq!(
            a.as_bytes().as_ptr(),
            b.as_bytes().as_ptr(),
            "clone must share the buffer"
        );
    }

    #[test]
    fn deref_and_equality_with_primitives() {
        let s = Str::from("verb");
        // Deref<str>: str methods work directly.
        assert!(s.starts_with("ve"));
        assert_eq!(s.to_uppercase(), "VERB");
        // equality with &str / str.
        assert_eq!(s, "verb");
        assert_eq!(s, *"verb");
        assert_ne!(s, "other");
    }

    #[test]
    fn borrow_str_lets_a_str_key_look_up_a_str_map() {
        // Str hashes + compares as its text, so a HashMap<Str, _> is queryable by &str (Borrow<str>).
        let mut m: HashMap<Str, i32> = HashMap::new();
        m.insert(Str::from("key"), 7);
        assert_eq!(m.get("key"), Some(&7));
    }

    #[test]
    fn ordering_is_by_content() {
        let mut v = [Str::from("banana"), Str::from("apple"), Str::from("cherry")];
        v.sort();
        assert_eq!(
            v.iter().map(Str::as_str).collect::<Vec<_>>(),
            ["apple", "banana", "cherry"]
        );
    }

    // Property: for any string, `Str::from` preserves the text exactly through every view, a clone is
    // content-equal, and ordering/equality against the source `str` agree with the primitive.
    #[test]
    fn prop_from_str_preserves_content() {
        check!().with_type::<String>().for_each(|s| {
            let str = Str::from(s.as_str());
            assert_eq!(str.as_str(), s.as_str());
            assert_eq!(str.as_bytes(), s.as_bytes());
            assert_eq!(str.len(), s.len());
            assert_eq!(str.is_empty(), s.is_empty());
            assert_eq!(&str, s.as_str());
            assert_eq!(str.clone(), str);
        });
    }

    // Property: `from_utf8` accepts exactly the byte sequences `std::str::from_utf8` accepts, and on
    // success round-trips to the same text.
    #[test]
    fn prop_from_utf8_matches_std() {
        check!().with_type::<Vec<u8>>().for_each(|bytes| {
            let expected = std::str::from_utf8(bytes);
            let got = Str::from_utf8(Bytes::copy_from_slice(bytes));
            assert_eq!(expected.is_ok(), got.is_ok());
            if let (Ok(e), Ok(g)) = (expected, &got) {
                assert_eq!(g.as_str(), e);
            }
        });
    }

    // The `TypeGenerator` impl yields only valid `Str` values: every generated `Str` is valid UTF-8
    // (as_str/as_bytes agree with std), a clone is content-equal, and byte-length matches as_str.
    #[test]
    fn prop_generated_str_is_valid() {
        check!().with_type::<Str>().for_each(|s: &Str| {
            assert_eq!(std::str::from_utf8(s.as_bytes()), Ok(s.as_str()));
            assert_eq!(s.len(), s.as_str().len());
            assert_eq!(s.clone(), *s);
        });
    }
}

#[cfg(all(test, feature = "buffer"))]
mod buffer_tests {
    use super::{FromReaderError, Str};
    use bytes::Bytes;
    use etude_buffer::reader::Buffer as _;

    #[test]
    fn from_reader_reads_and_validates() {
        // A `bytes::Bytes` is a `reader::Buffer` source; from_reader drains it and validates UTF-8.
        let mut r = Bytes::from_static("héllo".as_bytes());
        let s = Str::from_reader(&mut r).unwrap();
        assert_eq!(s, "héllo");
        assert!(r.buffer_is_empty(), "reader must be fully drained");

        // an empty reader yields the empty Str.
        let mut empty = Bytes::new();
        assert_eq!(Str::from_reader(&mut empty).unwrap(), "");
    }

    #[test]
    fn from_reader_rejects_invalid_utf8() {
        let mut r = Bytes::from_static(&[0xFF, 0xFE]);
        assert!(matches!(
            Str::from_reader(&mut r),
            Err(FromReaderError::Utf8(_))
        ));
    }

    #[test]
    fn from_reader_single_bytes_chunk_is_zero_copy() {
        // A single `Bytes`-backed chunk that drains the reader must be REUSED, not copied — the produced
        // Str aliases the source allocation. Locks in the fast path (and the doc's no-copy claim).
        let src = Bytes::from_static("zero copy path".as_bytes());
        let ptr = src.as_ptr();
        let mut r = src;
        let s = Str::from_reader(&mut r).unwrap();
        assert_eq!(s, "zero copy path");
        assert_eq!(
            s.as_bytes().as_ptr(),
            ptr,
            "single Bytes chunk must be reused without a copy"
        );
    }

    #[test]
    fn from_reader_reassembles_multibyte_char_split_across_chunks() {
        // "é" == 0xC3 0xA9, split across two reader chunks. from_reader collects ALL bytes before
        // validating, so the multi-byte char survives the chunk boundary (a per-chunk validation would
        // wrongly reject each half).
        let mut r = Bytes::from_static(&[0xC3]).chain(Bytes::from_static(&[0xA9]));
        assert_eq!(Str::from_reader(&mut r).unwrap(), "é");
    }

    #[test]
    fn str_is_a_reader_source_drained_into_a_writer() {
        // `Str` is a `reader::Buffer`: drain it (into an unbounded Vec<u8> dest) — bytes match and the
        // Str is left empty. Then round-trip through from_reader back to the same Str.
        let mut s = Str::from("héllo world");
        let mut out: Vec<u8> = Vec::new();
        while !s.buffer_is_empty() {
            s.copy_into(&mut out).unwrap();
        }
        assert_eq!(out, "héllo world".as_bytes());
        assert!(s.buffer_is_empty());

        let mut src = Str::from("round trip");
        assert_eq!(Str::from_reader(&mut src).unwrap(), "round trip");
    }

    #[test]
    fn reader_clamps_to_char_boundary_so_remainder_stays_valid_utf8() {
        // "aé" = 'a'(1 byte) + 'é'(0xC3 0xA9, 2 bytes) = 3 bytes. A watermark of 2 would split 'é' at
        // its first byte; the reader clamps DOWN to the 'a' boundary, so the remaining `Str` is the valid
        // "é" — never a partial-UTF-8 Str (as_str on the remainder is sound, no UB).
        let mut s = Str::from("aé");
        let chunk = s.read_chunk(2).unwrap();
        assert_eq!(&chunk[..], b"a");
        assert_eq!(s.as_str(), "é"); // remainder is valid UTF-8
        assert_eq!(s.buffered_len(), 2);

        // Next read takes the whole 'é'.
        let chunk = s.read_chunk(usize::MAX).unwrap();
        assert_eq!(&chunk[..], "é".as_bytes());
        assert!(s.buffer_is_empty());
    }
}
