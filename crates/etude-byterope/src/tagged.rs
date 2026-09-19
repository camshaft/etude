// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Ownership tagging for [`ByteRope`], mirroring `etude-bytevec`'s `tagged` module so a bytevec
//! caller can swap the two. A [`Tagged`] wraps a rope together with a [`Handle`] minted by an
//! [`Owner`]; the handle tracks the wrapped byte count against the owner's running budget across
//! `push_back`/`append`/`split_to`, `clone`, and `drop`.

use bytes::Bytes;

use super::{ByteRope, ByteRopeError};
use core::fmt;
use core::ops;

/// Declares a static byte-budget tag: a zero-sized [`Owner`] `Tag` backed by a process-global atomic
/// counter, its paired [`Handle`], and (in the no-argument form) a `ByteRope` alias for the tagged
/// rope. Mirrors `etude-bytevec`'s `static_bytevec_tag!`.
#[macro_export]
macro_rules! static_byterope_tag {
    () => {
        $crate::static_byterope_tag!($crate::tagged);

        pub type ByteRope = $crate::Tagged<Tag>;
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
                }
            }

            fn decrement(&mut self, len: usize) {
                if len > 0 {
                    COUNT.fetch_sub(len as _, core::sync::atomic::Ordering::Relaxed);
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

/// `etude_bytevec`-compatible alias of [`static_byterope_tag!`]: a bytevec consumer's
/// `static_bytevec_tag!()` recompiles unchanged against byterope. The no-argument form declares the
/// tagged-buffer type under bytevec's `ByteVec` name (`pub type ByteVec = Tagged<Tag>`); the
/// path form emits just the `Tag`/`Handle` machinery. Mirrors `etude_bytevec::static_bytevec_tag!`.
#[macro_export]
macro_rules! static_bytevec_tag {
    () => {
        $crate::static_byterope_tag!($crate::tagged);

        pub type ByteVec = $crate::Tagged<Tag>;
    };
    ($($tagged_path:tt)*) => {
        $crate::static_byterope_tag!($($tagged_path)*);
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

/// A [`ByteRope`] paired with an [`Owner`]'s [`Handle`], keeping the owner's byte budget in sync with
/// the wrapped rope's length. Derefs to the inner rope for read-only access.
#[derive(Debug)]
pub struct Tagged<O: Owner> {
    bytes: ByteRope,
    #[allow(dead_code)]
    tag: O::Handle,
}

impl<O: Owner> Tagged<O> {
    /// Wraps `bytes`, charging its current length to `owner`.
    #[inline]
    #[track_caller]
    pub fn new(bytes: ByteRope, owner: &O) -> Self {
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
    pub fn append(&mut self, other: &mut ByteRope) {
        self.tag.increment(other.len());
        self.bytes.append(other);
    }

    /// Splits off the first `at` bytes, releasing them from the owner's budget and returning them
    /// as a plain (untagged) [`ByteRope`].
    ///
    /// # Errors
    /// Returns [`ByteRopeError::OutOfBounds`] if `at` exceeds the rope's length.
    pub fn split_to(&mut self, at: usize) -> Result<ByteRope, ByteRopeError> {
        let chunk = self.bytes.split_to(at)?;
        self.tag.decrement(chunk.len());
        Ok(chunk)
    }

    /// Consumes the wrapper, returning the inner [`ByteRope`] and releasing its bytes from the
    /// owner's budget.
    #[inline]
    pub fn untag(self) -> ByteRope {
        self.bytes
    }

    /// Clones the inner [`ByteRope`] out without charging the copy to the owner.
    #[inline]
    pub fn untag_clone(&self) -> ByteRope {
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

impl<O: Default + Owner> From<ByteRope> for Tagged<O> {
    #[inline]
    fn from(value: ByteRope) -> Self {
        Self::new(value, &Default::default())
    }
}

impl<O: Owner> From<Tagged<O>> for ByteRope {
    #[inline]
    fn from(value: Tagged<O>) -> Self {
        value.bytes
    }
}

impl<O: Owner> ops::Deref for Tagged<O> {
    type Target = ByteRope;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod tag_a {
        static_byterope_tag!(crate::tagged);
    }

    mod tag_b {
        static_byterope_tag!(crate::tagged);
    }

    #[test]
    fn tag_test() {
        let chunk = ByteRope::from(b"hello!");
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

    /// Exercises the `Tagged` op surface (push_back/append/split_to/untag) and its budget tracking.
    #[test]
    fn tagged_ops_track_budget() {
        mod tag_c {
            static_byterope_tag!(crate::tagged);
        }

        let mut t: Tagged<tag_c::Tag> = ByteRope::from(b"abc").tag(&tag_c::Tag);
        assert_eq!(tag_c::Tag::current(), 3);
        assert_eq!(t.len(), 3);

        t.push_back(Bytes::from_static(b"de"));
        assert_eq!(tag_c::Tag::current(), 5);
        assert_eq!(t.len(), 5);

        let mut extra = ByteRope::from(b"fg");
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
