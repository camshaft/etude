// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use crate::{reader::Chunk, writer::Buffer};
use bytes::{Bytes, BytesMut, buf::UninitSlice};

/// An implementation that limits the number of bytes that can be written to the underlying storage
pub struct Limit<'a, S: Buffer + ?Sized> {
    storage: &'a mut S,
    remaining_capacity: usize,
}

impl<'a, S: Buffer + ?Sized> Limit<'a, S> {
    /// Wraps `storage`, capping its writable capacity at `remaining_capacity` (or the storage's
    /// own capacity, whichever is smaller). Prefer
    /// [`Buffer::with_write_limit`](crate::writer::Buffer::with_write_limit).
    #[inline]
    pub fn new(storage: &'a mut S, remaining_capacity: usize) -> Self {
        let remaining_capacity = storage.remaining_capacity().min(remaining_capacity);
        Self {
            storage,
            remaining_capacity,
        }
    }
}

impl<S: Buffer + ?Sized> Buffer for Limit<'_, S> {
    const SPECIALIZES_BYTES: bool = S::SPECIALIZES_BYTES;
    const SPECIALIZES_BYTES_MUT: bool = S::SPECIALIZES_BYTES_MUT;

    #[inline]
    fn put_slice(&mut self, bytes: &[u8]) {
        // Enforce the limit with a REAL check BEFORE forwarding to storage: `bytes.len()` is
        // caller-controlled, so an `assume!` here would (in release) both let the oversized write
        // bypass the cap into storage AND make the capacity subtraction underflow into UB.
        assert!(
            bytes.len() <= self.remaining_capacity,
            "put_slice of {} bytes exceeds the write limit's {} remaining",
            bytes.len(),
            self.remaining_capacity
        );
        self.storage.put_slice(bytes);
        self.remaining_capacity -= bytes.len();
    }

    #[inline(always)]
    fn put_uninit_slice<F, Error>(&mut self, payload_len: usize, f: F) -> Result<bool, Error>
    where
        F: FnOnce(&mut UninitSlice) -> Result<(), Error>,
    {
        assert!(
            payload_len <= self.remaining_capacity,
            "put_uninit_slice of {} bytes exceeds the write limit's {} remaining",
            payload_len,
            self.remaining_capacity
        );
        let did_write = self.storage.put_uninit_slice(payload_len, f)?;
        if did_write {
            self.remaining_capacity -= payload_len;
        }
        Ok(did_write)
    }

    #[inline]
    fn remaining_capacity(&self) -> usize {
        self.storage
            .remaining_capacity()
            .min(self.remaining_capacity)
    }

    #[inline]
    fn has_remaining_capacity(&self) -> bool {
        self.remaining_capacity > 0 && self.storage.has_remaining_capacity()
    }

    #[inline]
    fn put_bytes(&mut self, bytes: Bytes) {
        let len = bytes.len();
        assert!(
            len <= self.remaining_capacity,
            "put_bytes of {len} bytes exceeds the write limit's {} remaining",
            self.remaining_capacity
        );
        self.storage.put_bytes(bytes);
        self.remaining_capacity -= len;
    }

    #[inline]
    fn put_bytes_mut(&mut self, bytes: BytesMut) {
        let len = bytes.len();
        assert!(
            len <= self.remaining_capacity,
            "put_bytes_mut of {len} bytes exceeds the write limit's {} remaining",
            self.remaining_capacity
        );
        self.storage.put_bytes_mut(bytes);
        self.remaining_capacity -= len;
    }

    #[inline]
    fn put_chunk(&mut self, chunk: Chunk) {
        let len = chunk.len();
        assert!(
            len <= self.remaining_capacity,
            "put_chunk of {len} bytes exceeds the write limit's {} remaining",
            self.remaining_capacity
        );
        self.storage.put_chunk(chunk);
        self.remaining_capacity -= len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_test() {
        let mut writer: Vec<u8> = vec![];

        {
            let mut writer = writer.with_write_limit(5);
            assert_eq!(writer.remaining_capacity(), 5);
            writer.put_slice(b"hello");
            assert_eq!(writer.remaining_capacity(), 0);
        }

        {
            let mut writer = writer.with_write_limit(5);
            assert_eq!(writer.remaining_capacity(), 5);
            writer.put_bytes(Bytes::from_static(b"hello"));
            assert_eq!(writer.remaining_capacity(), 0);
        }

        {
            let mut writer = writer.with_write_limit(5);
            assert_eq!(writer.remaining_capacity(), 5);
            writer.put_bytes_mut(BytesMut::from(&b"hello"[..]));
            assert_eq!(writer.remaining_capacity(), 0);
        }

        {
            let writer = writer.with_write_limit(0);
            assert!(!writer.has_remaining_capacity());
        }
    }

    /// A write past the cap must panic (a real limit check), not bypass the limit into storage and
    /// hit UB on the underflowing capacity subtraction as the pre-fix `assume!` allowed in release.
    #[test]
    #[should_panic(expected = "exceeds the write limit")]
    fn put_slice_over_limit_panics() {
        let mut storage: Vec<u8> = vec![];
        let mut writer = storage.with_write_limit(4);
        writer.put_slice(b"toolong"); // 7 bytes past a 4-byte limit -> must panic
    }
}
