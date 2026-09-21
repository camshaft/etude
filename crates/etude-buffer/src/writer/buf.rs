// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use crate::writer::{Buffer, UninitSlice};

/// Delegates storage operations into a [`bytes::BufMut`] implementation.
pub struct BufMut<'a, T: bytes::BufMut> {
    buf_mut: &'a mut T,
}

impl<'a, T: bytes::BufMut> BufMut<'a, T> {
    /// Wraps a mutable [`bytes::BufMut`] as a writer [`Buffer`]. Writes are forwarded to
    /// `buf_mut`.
    #[inline]
    pub fn new(buf_mut: &'a mut T) -> Self {
        Self { buf_mut }
    }
}

impl<T: bytes::BufMut> Buffer for BufMut<'_, T> {
    #[inline]
    fn put_slice(&mut self, bytes: &[u8]) {
        self.buf_mut.put_slice(bytes);
    }

    #[inline]
    fn remaining_capacity(&self) -> usize {
        self.buf_mut.remaining_mut()
    }

    #[inline]
    unsafe fn put_uninit_slice<F, Error>(
        &mut self,
        payload_len: usize,
        f: F,
    ) -> Result<Option<usize>, Error>
    where
        F: FnOnce(&mut UninitSlice) -> Result<usize, Error>,
    {
        let chunk = self.buf_mut.chunk_mut();

        // make sure the current chunk is capable of serving the whole slice
        ensure!(chunk.len() >= payload_len, Ok(None));

        // No zero-init: per this method's safety contract the closure initializes the prefix it
        // reports. Clamp the reported count to the slice length so the unchecked `advance_mut` can
        // never commit past the exposed region (a hard bounds violation), independent of caller
        // trust; a within-slice over-report is the caller's documented responsibility.
        let reported = f(&mut chunk[..payload_len])?;
        let committed = reported.min(payload_len);

        // SAFETY: `committed <= payload_len <= chunk.len()`, and the caller upholds that the closure
        // initialized the reported prefix, so the committed region is initialized and in bounds.
        unsafe {
            self.buf_mut.advance_mut(committed);
        }

        Ok(Some(committed))
    }
}

/// Delegates standard types to their BufMut implementations
macro_rules! impl_buf_mut {
    ($ty:ty $(, $reserve:ident)?) => {
        impl Buffer for $ty {
            #[inline]
            fn put_slice(&mut self, bytes: &[u8]) {
                bytes::BufMut::put_slice(self, bytes);
            }

            #[inline]
            fn remaining_capacity(&self) -> usize {
                bytes::BufMut::remaining_mut(self)
            }

            #[inline]
            unsafe fn put_uninit_slice<F, Error>(
                &mut self,
                payload_len: usize,
                f: F,
            ) -> Result<Option<usize>, Error>
            where
                F: FnOnce(&mut UninitSlice) -> Result<usize, Error>,
            {
                use bytes::BufMut;

                // try to reserve additional capacity for the write, if possible
                $(
                    self.$reserve(payload_len);
                )?

                let chunk = self.chunk_mut();
                ensure!(chunk.len() >= payload_len, Ok(None));

                // No zero-init: per this method's safety contract the closure initializes the
                // prefix it reports. Clamp the reported count to the slice length so the unchecked
                // `advance_mut` can never commit past the exposed region (a hard bounds violation);
                // a within-slice over-report is the caller's documented responsibility.
                let reported = f(&mut chunk[..payload_len])?;
                let committed = reported.min(payload_len);

                // SAFETY: `committed <= payload_len <= chunk.len()`, and the caller upholds that the
                // closure initialized the reported prefix, so the region is initialized and in bounds.
                unsafe {
                    self.advance_mut(committed);
                }

                Ok(Some(committed))
            }
        }
    };
}

impl_buf_mut!(bytes::BytesMut, reserve);
impl_buf_mut!(alloc::vec::Vec<u8>, reserve);
impl_buf_mut!(&mut [u8]);
impl_buf_mut!(&mut [core::mem::MaybeUninit<u8>]);

#[cfg(test)]
mod tests {
    use crate::{reader::Buffer as _, writer::Buffer as _};

    /// `put_uninit_slice` commits exactly the count the closure reports (a partial fill), leaving
    /// the untouched tail uncommitted — no zero-init, no over-commit. A closure that reports 0
    /// commits nothing, so it can never expose stale heap: the trusted-count contract's answer to
    /// the #33 info-leak class (an honest closure reports only what it wrote; the impl commits only
    /// that). The buffer is pre-poisoned so any accidental over-commit would surface as 0xAB rather
    /// than possibly-fresh zero pages.
    #[test]
    fn put_uninit_slice_commits_exactly_the_reported_prefix() {
        // Poison the allocation, then reset len so the spare capacity holds stale 0xAB bytes.
        let mut v: Vec<u8> = vec![0xAB; 64];
        v.clear();
        // SAFETY: the closure initializes exactly the 3 bytes it reports.
        let committed = unsafe {
            v.put_uninit_slice::<_, core::convert::Infallible>(32, |slice| {
                slice[0..3].copy_from_slice(b"abc");
                Ok(3)
            })
        }
        .unwrap();
        assert_eq!(committed, Some(3));
        assert_eq!(&v[..], b"abc");

        // A closure that reports 0 commits nothing — no stale heap is ever exposed.
        let mut v: Vec<u8> = vec![0xAB; 64];
        v.clear();
        // SAFETY: reporting 0 initializes nothing and commits nothing.
        let committed =
            unsafe { v.put_uninit_slice::<_, core::convert::Infallible>(32, |_slice| Ok(0)) }
                .unwrap();
        assert_eq!(committed, Some(0));
        assert!(
            v.is_empty(),
            "a 0-report committed stale heap: {:?}",
            &v[..]
        );
    }

    /// A within-slice over-report must be clamped to the slice length, so the unchecked advance can
    /// never commit past the region the closure was handed (a hard bounds violation, independent of
    /// caller trust). Here the closure fills the whole 8-byte slice but claims 100; the commit must
    /// be exactly 8.
    #[test]
    fn put_uninit_slice_clamps_over_report_to_the_slice() {
        let mut v: Vec<u8> = Vec::with_capacity(16);
        // SAFETY: the closure initializes all 8 bytes; the over-report is clamped by the impl.
        let committed = unsafe {
            v.put_uninit_slice::<_, core::convert::Infallible>(8, |slice| {
                slice.copy_from_slice(b"01234567");
                Ok(100)
            })
        }
        .unwrap();
        assert_eq!(committed, Some(8));
        assert_eq!(v.len(), 8);
        assert_eq!(&v[..], b"01234567");
    }

    #[test]
    fn vec_test() {
        let mut buffer: Vec<u8> = vec![];
        let expected = vec![42; 1000];
        let expected = &expected[..];

        {
            assert_eq!(buffer.remaining_capacity(), isize::MAX as usize);

            let mut source = expected;

            source.copy_into(&mut buffer).unwrap();
        }

        assert_eq!(&buffer, expected);
    }

    #[test]
    fn vec_buf_test() {
        let mut buffer: Vec<u8> = vec![];
        let expected = vec![42; 1000];
        let expected = &expected[..];

        {
            let mut buffer = super::BufMut::new(&mut buffer);
            assert_eq!(buffer.remaining_capacity(), isize::MAX as usize);

            let mut source = expected;

            source.copy_into(&mut buffer).unwrap();
        }

        assert_eq!(&buffer, expected);
    }
}
