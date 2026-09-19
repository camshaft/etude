// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Accounting wrapper that tracks the bytes held by a [`ByteVec`] against an owner.
//!
//! Wrapping a [`ByteVec`] in [`Tagged`] reports its length to an [`Owner`] and keeps that count in
//! step as the buffer grows and shrinks — useful for bounding or observing total buffered bytes
//! across many buffers. The count follows the bytes: a clone of a `Tagged` adds its length again
//! (the bytes are now referenced twice), and dropping one subtracts it.
//!
//! Implement [`Owner`]/[`Handle`] for a bespoke sink, or use [`crate::static_bytevec_tag`] to
//! generate a zero-sized owner backed by a process-wide atomic counter.

use bytes::Bytes;

use super::{ByteVec, ByteVecError};
use core::fmt;
use std::ops;

/// Generates a zero-sized [`Owner`] (`Tag`) and its [`Handle`] backed by a process-wide
/// [`AtomicU64`](core::sync::atomic::AtomicU64) counter of outstanding tagged bytes.
///
/// The counter rises as bytes are tagged or pushed and falls as tagged buffers shrink or drop;
/// read the current total with `Tag::current()`. Invoke inside a module so the generated `Tag`,
/// `Handle`, and counter are scoped to it. The no-argument form additionally defines
/// `pub type ByteVec = Tagged<Tag>` for that module.
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

/// A sink that accounts for tagged bytes, producing a [`Handle`] per tagged buffer.
pub trait Owner: 'static + fmt::Debug {
    /// The per-buffer handle this owner hands out; its lifetime tracks the buffer's bytes.
    type Handle: Handle;

    /// Records `len` bytes as tagged and returns a handle that will keep the owner's count in
    /// step as the buffer changes and until the handle is dropped.
    fn tag(&self, len: usize) -> Self::Handle;
}

/// The per-buffer accounting handle produced by an [`Owner`].
///
/// It represents `len` bytes currently charged to the owner; the wrapping [`Tagged`] calls
/// [`increment`](Handle::increment) / [`decrement`](Handle::decrement) as its buffer grows and
/// shrinks. A `Clone` charges the same bytes again; a `Drop` releases them.
pub trait Handle: 'static + fmt::Debug + Clone + Sized {
    /// Charges an additional `len` bytes to the owner.
    fn increment(&mut self, len: usize);
    /// Releases `len` bytes back to the owner.
    fn decrement(&mut self, len: usize);
}

/// A [`ByteVec`] whose length is accounted against an [`Owner`].
///
/// Derefs to the inner [`ByteVec`] for read-only access; the mutating methods here keep the
/// owner's count in step. Convert back with [`untag`](Tagged::untag) to stop accounting.
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

    /// Moves all of `other` onto the end of this buffer, charging its length to the owner and
    /// leaving `other` empty.
    pub fn append(&mut self, other: &mut ByteVec) {
        self.tag.increment(other.len());
        self.bytes.append(other);
    }

    /// Splits off the first `at` bytes, releasing them from the owner's count and returning them
    /// as a plain (untagged) [`ByteVec`].
    ///
    /// # Errors
    /// Returns [`ByteVecError::OutOfBounds`] if `at` exceeds the buffer's length.
    pub fn split_to(&mut self, at: usize) -> Result<ByteVec, ByteVecError> {
        let chunk = self.bytes.split_to(at)?;
        self.tag.decrement(chunk.len());
        Ok(chunk)
    }

    /// Consumes the wrapper, returning the inner [`ByteVec`] and releasing its bytes from the
    /// owner's count.
    #[inline]
    pub fn untag(self) -> ByteVec {
        self.bytes
    }

    /// Clones the inner [`ByteVec`] out without accounting for the copy against the owner.
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
}
