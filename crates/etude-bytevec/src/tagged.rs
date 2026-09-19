// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Ownership tagging for [`ByteVec`]. A [`Tagged`] wraps a buffer together with a [`Handle`] minted
//! by an [`Owner`]; the handle tracks the wrapped byte count against the owner's running budget
//! across `push_back`/`append`/`split_to`, `clone`, and `drop`.

use bytes::Bytes;

use super::{ByteVec, ByteVecError};
use core::fmt;
use core::ops;

/// Declares a static byte-budget tag: a zero-sized [`Owner`] `Tag` backed by a process-global atomic
/// counter, its paired [`Handle`], and (in the no-argument form) a `ByteVec` alias for the tagged
/// buffer.
#[macro_export]
macro_rules! static_bytevec_tag {
    () => {
        $crate::static_bytevec_tag!($crate::tagged);

        pub type ByteVec = $crate::Tagged<Tag>;
    };
    ($($tagged_path:tt)*) => {
        pub(crate) static COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

        #[derive(Clone, Copy, Debug, Default)]
        pub struct Tag;

        impl Tag {
            pub fn current() -> u64 {
                COUNT.load(core::sync::atomic::Ordering::Relaxed)
            }
        }

        impl $($tagged_path)*::Owner for Tag {
            type Handle = Handle;

            fn tag(&self, len: usize) -> Self::Handle {
                COUNT.fetch_add(len as _, core::sync::atomic::Ordering::Relaxed);
                Handle(len)
            }
        }

        #[derive(Debug)]
        pub struct Handle(usize);

        impl $($tagged_path)*::Handle for Handle {
            fn increment(&mut self, len: usize) {
                if len > 0 {
                    COUNT.fetch_add(len as _, core::sync::atomic::Ordering::Relaxed);
                    // Track the grow in the handle's own remembered length, or a later Clone/Drop
                    // would charge/release the stale initial length and drift the owner budget.
                    self.0 += len;
                }
            }

            fn decrement(&mut self, len: usize) {
                if len > 0 {
                    debug_assert!(
                        self.0 >= len,
                        "tag handle decrement {len} exceeds its tracked length {}",
                        self.0
                    );
                    COUNT.fetch_sub(len as _, core::sync::atomic::Ordering::Relaxed);
                    self.0 -= len;
                }
            }
        }

        impl Clone for Handle {
            fn clone(&self) -> Self {
                COUNT.fetch_add(self.0 as _, core::sync::atomic::Ordering::Relaxed);
                Self(self.0)
            }
        }

        impl Drop for Handle {
            fn drop(&mut self) {
                COUNT.fetch_sub(self.0 as _, core::sync::atomic::Ordering::Relaxed);
            }
        }
    };
}

/// Mints [`Handle`]s that track a byte budget owned by `Self`.
pub trait Owner: 'static + fmt::Debug {
    /// The per-rope handle this owner hands out; its lifetime tracks the rope's bytes.
    type Handle: Handle;

    /// Records `len` bytes as tagged and returns a handle that keeps the owner's budget in step
    /// as the rope changes and until the handle is dropped.
    fn tag(&self, len: usize) -> Self::Handle;
}

/// A live claim on some number of an [`Owner`]'s bytes; adjusts the budget as the tagged rope grows,
/// shrinks, clones, and drops.
pub trait Handle: 'static + fmt::Debug + Clone + Sized {
    /// Charges an additional `len` bytes to the owner.
    fn increment(&mut self, len: usize);
    /// Releases `len` bytes back to the owner.
    fn decrement(&mut self, len: usize);
}

/// A [`ByteVec`] paired with an [`Owner`]'s [`Handle`], keeping the owner's byte budget in sync with
/// the wrapped rope's length. Derefs to the inner rope for read-only access.
#[derive(Debug)]
pub struct Tagged<O: Owner> {
    bytes: ByteVec,
    #[allow(dead_code)]
    tag: O::Handle,
}

impl<O: Owner> Tagged<O> {
    /// Wraps `bytes`, charging its current length to `owner`.
    #[inline]
    #[track_caller]
    pub fn new(bytes: ByteVec, owner: &O) -> Self {
        let len = bytes.len();
        let tag = owner.tag(len);
        Self { bytes, tag }
    }

    /// Appends a chunk, charging its length to the owner.
    pub fn push_back(&mut self, bytes: Bytes) {
        self.tag.increment(bytes.len());
        self.bytes.push_back(bytes);
    }

    /// Moves all of `other` onto the end of this rope, charging its length to the owner and
    /// leaving `other` empty.
    pub fn append(&mut self, other: &mut ByteVec) {
        self.tag.increment(other.len());
        self.bytes.append(other);
    }

    /// Splits off the first `at` bytes, releasing them from the owner's budget and returning them
    /// as a plain (untagged) [`ByteVec`].
    ///
    /// # Errors
    /// Returns [`ByteVecError::OutOfBounds`] if `at` exceeds the rope's length.
    pub fn split_to(&mut self, at: usize) -> Result<ByteVec, ByteVecError> {
        let chunk = self.bytes.split_to(at)?;
        self.tag.decrement(chunk.len());
        Ok(chunk)
    }

    /// Consumes the wrapper, returning the inner [`ByteVec`] and releasing its bytes from the
    /// owner's budget.
    #[inline]
    pub fn untag(self) -> ByteVec {
        self.bytes
    }

    /// Clones the inner [`ByteVec`] out without charging the copy to the owner.
    #[inline]
    pub fn untag_clone(&self) -> ByteVec {
        self.bytes.clone()
    }
}

impl<O: Owner> Clone for Tagged<O> {
    fn clone(&self) -> Self {
        let bytes = self.bytes.clone();
        let tag = self.tag.clone();
        Self { bytes, tag }
    }
}

impl<O: Default + Owner> Default for Tagged<O> {
    #[inline]
    fn default() -> Self {
        Self::new(Default::default(), &Default::default())
    }
}

impl<O: Owner> PartialEq for Tagged<O> {
    fn eq(&self, other: &Self) -> bool {
        self.bytes.eq(&other.bytes)
    }
}

impl<O: Owner> Eq for Tagged<O> {}

impl<O: Default + Owner> From<ByteVec> for Tagged<O> {
    #[inline]
    fn from(value: ByteVec) -> Self {
        Self::new(value, &Default::default())
    }
}

impl<O: Owner> From<Tagged<O>> for ByteVec {
    #[inline]
    fn from(value: Tagged<O>) -> Self {
        value.bytes
    }
}

impl<O: Owner> ops::Deref for Tagged<O> {
    type Target = ByteVec;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod tag_a {
        static_bytevec_tag!(crate::tagged);
    }

    mod tag_b {
        static_bytevec_tag!(crate::tagged);
    }

    #[test]
    fn tag_test() {
        let chunk = ByteVec::from(b"hello!");
        let a: Tagged<tag_a::Tag> = chunk.clone().tag(&tag_a::Tag);
        let b: Tagged<tag_b::Tag> = chunk.tag(&tag_b::Tag);

        assert_eq!(tag_a::Tag::current(), 6);
        assert_eq!(tag_b::Tag::current(), 6);

        let a_clone = a.clone();

        assert_eq!(tag_a::Tag::current(), 12);

        drop(a_clone);

        assert_eq!(tag_a::Tag::current(), 6);

        drop(a);

        assert_eq!(tag_a::Tag::current(), 0);
        assert_eq!(tag_b::Tag::current(), 6);

        drop(b);

        assert_eq!(tag_a::Tag::current(), 0);
        assert_eq!(tag_b::Tag::current(), 0);
    }

    /// red reproducer (breaker-bytevec): the static-tag macro's `Handle` adjusts the global
    /// `COUNT` in `increment`/`decrement` but never updates its own remembered length (`self.0`),
    /// while `Clone`/`Drop` charge/release that stale initial length. Any `Tagged` that grows or
    /// shrinks after creation corrupts the owner budget on clone/drop/untag: grow-then-drop leaks
    /// the growth forever (observed: budget 2 after dropping a 3-byte rope grown by 2; expected 0).
    /// `etude-bytevec`'s `static_bytevec_tag!` has the identical bug — fix both macros together
    /// (`increment`/`decrement` must also do `self.0 += len` / `self.0 -= len`).
    #[test]
    fn handle_drop_releases_current_len_not_initial() {
        mod tag_d {
            static_bytevec_tag!(crate::tagged);
        }
        let mut t: Tagged<tag_d::Tag> = ByteVec::from(b"abc").tag(&tag_d::Tag);
        t.push_back(Bytes::from_static(b"de"));
        assert_eq!(tag_d::Tag::current(), 5);
        drop(t);
        assert_eq!(
            tag_d::Tag::current(),
            0,
            "grow-then-drop leaked owner budget"
        );
    }

    /// Companion reproducer: a clone of a grown rope charges only the stale initial length, so the
    /// owner budget undercounts live bytes (two 5-byte ropes charged 8, not 10), and shrink-then-
    /// drop over-releases (a split_to below the initial length drives the budget negative/wraps).
    #[test]
    fn handle_clone_charges_current_len_and_shrink_does_not_over_release() {
        mod tag_e {
            static_bytevec_tag!(crate::tagged);
        }
        let mut t: Tagged<tag_e::Tag> = ByteVec::from(b"abc").tag(&tag_e::Tag);
        t.push_back(Bytes::from_static(b"de")); // 5 bytes live
        let c = t.clone(); // must charge the CURRENT 5, not the initial 3
        assert_eq!(tag_e::Tag::current(), 10, "clone undercharged the owner");
        drop(c);
        assert_eq!(tag_e::Tag::current(), 5);

        // shrink below the initial length, then drop: must release exactly the remaining 1 byte
        let _front = t.split_to(4).expect("in bounds"); // 1 byte remains tagged
        assert_eq!(tag_e::Tag::current(), 1);
        drop(t); // releasing the stale initial 3 would wrap the budget below zero
        assert_eq!(
            tag_e::Tag::current(),
            0,
            "shrink-then-drop over-released (budget wrapped)"
        );
    }

    /// Exercises the `Tagged` op surface (push_back/append/split_to/untag) and its budget tracking.
    #[test]
    fn tagged_ops_track_budget() {
        mod tag_c {
            static_bytevec_tag!(crate::tagged);
        }

        let mut t: Tagged<tag_c::Tag> = ByteVec::from(b"abc").tag(&tag_c::Tag);
        assert_eq!(tag_c::Tag::current(), 3);
        assert_eq!(t.len(), 3);

        t.push_back(Bytes::from_static(b"de"));
        assert_eq!(tag_c::Tag::current(), 5);
        assert_eq!(t.len(), 5);

        let mut extra = ByteVec::from(b"fg");
        t.append(&mut extra);
        assert_eq!(tag_c::Tag::current(), 7);
        assert_eq!(t.len(), 7);

        let front = t.split_to(3).expect("split within bounds");
        assert_eq!(front, b"abc");
        assert_eq!(tag_c::Tag::current(), 4);
        assert_eq!(t.len(), 4);

        // `untag` yields the inner rope unchanged; the byte content is preserved end to end.
        let rope = t.untag();
        assert_eq!(rope, b"defg");
    }
}
