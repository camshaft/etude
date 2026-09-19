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
    fn put_uninit_slice<F, Error>(&mut self, payload_len: usize, f: F) -> Result<bool, Error>
    where
        F: FnOnce(&mut UninitSlice) -> Result<(), Error>,
    {
        let chunk = self.buf_mut.chunk_mut();

        // make sure the current chunk is capable of reading the entire slice
        ensure!(chunk.len() >= payload_len, Ok(false));

        // Zero-initialize the exposed region before the safe closure runs: on `Ok(())` we commit
        // `payload_len` bytes via the unsafe `advance_mut`, so a closure that fills fewer bytes (or
        // none) must not leave uninitialized heap to be exposed as content. A short fill now yields
        // zeros, not stale memory.
        // SAFETY: `payload_len <= chunk.len()` (checked above), so the whole written region is in bounds.
        unsafe {
            core::ptr::write_bytes(chunk.as_mut_ptr(), 0, payload_len);
        }

        f(&mut chunk[..payload_len])?;

        unsafe {
            self.buf_mut.advance_mut(payload_len);
        }

        Ok(true)
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
            fn put_uninit_slice<F, Error>(
                &mut self,
                payload_len: usize,
                f: F,
            ) -> Result<bool, Error>
            where
                F: FnOnce(&mut UninitSlice) -> Result<(), Error>,
            {
                use bytes::BufMut;

                // try to reserve additional capacity for the write, if possible
                $(
                    self.$reserve(payload_len);
                )?

                let chunk = self.chunk_mut();
                ensure!(chunk.len() >= payload_len, Ok(false));

                // Zero-initialize the exposed region before the safe closure runs: on `Ok(())` we
                // commit `payload_len` bytes via the unsafe `advance_mut`, so a closure that fills
                // fewer bytes (or none) must not leave uninitialized memory to be exposed as content.
                // A short fill now yields zeros, not stale heap.
                // SAFETY: `payload_len <= chunk.len()` (checked above), so the region is in bounds.
                unsafe {
                    core::ptr::write_bytes(chunk.as_mut_ptr(), 0, payload_len);
                }

                f(&mut chunk[..payload_len])?;

                unsafe {
                    self.advance_mut(payload_len);
                }

                Ok(true)
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

    /// Red (breaker-byterope): `put_uninit_slice` commits `payload_len` bytes on `Ok(())` without
    /// any guarantee the closure initialized them — the trait documents no initialization
    /// requirement, yet the `BufMut` bridge (and the standard-type impls routed through it, like
    /// `Vec<u8>`) run `advance_mut(payload_len)` on trust. A safe no-op closure therefore commits
    /// stale heap as initialized content: an information leak in the same class as the
    /// `for_socket_read` finding (#33), reachable from fully safe code. The buffer is pre-poisoned
    /// so the stale bytes are deterministic rather than possibly-fresh zero pages; `cargo miri`
    /// gives the definitive undefined-behavior verdict on the same sequence. Fix-shape-agnostic: a
    /// conforming implementation may decline the write (`false`), zero-initialize before calling
    /// the closure, or otherwise guarantee initialization — committing stale bytes fails.
    #[test]
    fn put_uninit_slice_must_not_commit_stale_heap_on_noop_closure() {
        // Poison the allocation, then reset len so the spare capacity holds stale 0xAB bytes.
        let mut v: Vec<u8> = vec![0xAB; 64];
        v.clear();
        let did = v
            .put_uninit_slice(32, |_slice| Ok::<(), core::convert::Infallible>(()))
            .unwrap();
        assert!(
            !did || v.iter().all(|&b| b == 0),
            "stale heap committed as initialized content: len={} first_bytes={:?}",
            v.len(),
            &v[..v.len().min(8)]
        );
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
