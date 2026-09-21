// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use crate::{reader::Chunk, writer::Buffer};
use bytes::{Bytes, BytesMut, buf::UninitSlice};

/// Reports exhausted capacity after the first non-empty write, so a capacity-checking transfer
/// loop performs exactly one transfer round into the storage.
///
/// The gate is advisory by design: the put methods are not hard-gated, so an in-flight multi-put
/// transfer (a reader that checks capacity once and then writes its content as several chunks) may
/// complete its round — only the next capacity check observes the wrapper as full. Callers that
/// bypass capacity checks can therefore still write; wrap the storage in a
/// [`Limit`](crate::writer::Limit) when a hard byte cap is required.
///
/// This can be used for very low latency scenarios where processing the single read is more
/// important than filling the entire storage with as much data as possible.
pub struct WriteOnce<'a, S: Buffer + ?Sized> {
    storage: &'a mut S,
    did_write: bool,
}

impl<'a, S: Buffer + ?Sized> WriteOnce<'a, S> {
    /// Wraps `storage` to accept a single write. Prefer
    /// [`Buffer::write_once`](crate::writer::Buffer::write_once).
    #[inline]
    pub fn new(storage: &'a mut S) -> Self {
        Self {
            storage,
            did_write: false,
        }
    }
}

impl<S: Buffer + ?Sized> Buffer for WriteOnce<'_, S> {
    const SPECIALIZES_BYTES: bool = S::SPECIALIZES_BYTES;
    const SPECIALIZES_BYTES_MUT: bool = S::SPECIALIZES_BYTES_MUT;

    #[inline]
    fn put_slice(&mut self, bytes: &[u8]) {
        let did_write = !bytes.is_empty();
        self.storage.put_slice(bytes);
        self.did_write |= did_write;
    }

    #[inline(always)]
    unsafe fn put_uninit_slice<F, Error>(
        &mut self,
        payload_len: usize,
        f: F,
    ) -> Result<Option<usize>, Error>
    where
        F: FnOnce(&mut UninitSlice) -> Result<usize, Error>,
    {
        // SAFETY: forwards the caller's contract straight through to the underlying storage.
        let committed = unsafe { self.storage.put_uninit_slice(payload_len, f)? };
        self.did_write |= matches!(committed, Some(n) if n > 0);
        Ok(committed)
    }

    #[inline]
    fn remaining_capacity(&self) -> usize {
        ensure!(!self.did_write, 0);
        self.storage.remaining_capacity()
    }

    #[inline]
    fn has_remaining_capacity(&self) -> bool {
        ensure!(!self.did_write, false);
        self.storage.has_remaining_capacity()
    }

    #[inline]
    fn put_bytes(&mut self, bytes: Bytes) {
        let did_write = !bytes.is_empty();
        self.storage.put_bytes(bytes);
        self.did_write |= did_write;
    }

    #[inline]
    fn put_bytes_mut(&mut self, bytes: BytesMut) {
        let did_write = !bytes.is_empty();
        self.storage.put_bytes_mut(bytes);
        self.did_write |= did_write;
    }

    #[inline]
    fn put_chunk(&mut self, chunk: Chunk) {
        let did_write = !chunk.is_empty();
        self.storage.put_chunk(chunk);
        self.did_write |= did_write;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the advisory-by-design boundary (consensus ruling on etude#166): a capacity-checking
    /// transfer loop performs exactly one round (the loop observes exhausted capacity after the
    /// first non-empty write), while a same-pass multi-put transfer completes — the pattern
    /// `copy_into_multi_chunks` blesses. If the operator later redirects to strict single-put
    /// enforcement, this test documents exactly what changes.
    #[test]
    fn advisory_boundary_one_round_for_loops_completion_for_same_pass() {
        // A capacity-checking loop (the shape of a reader's copy_into) stops after one round.
        let mut storage: Vec<u8> = vec![];
        {
            let mut writer = WriteOnce::new(&mut storage);
            let chunks: [&[u8]; 3] = [b"one", b"two", b"three"];
            let mut i = 0;
            while writer.has_remaining_capacity() && i < chunks.len() {
                writer.put_slice(chunks[i]);
                i += 1;
            }
            assert_eq!(i, 1, "loop must observe the gate after the first write");
        }
        assert_eq!(&storage[..], b"one");

        // A real multi-chunk reader drains through its own capacity-checked rounds: only the
        // first round lands.
        let mut storage: Vec<u8> = vec![];
        {
            use crate::reader::{Buffer as _, IoSlice};
            let parts: [&[u8]; 2] = [b"hello", b"world"];
            let mut reader = IoSlice::new(&parts);
            let mut writer = WriteOnce::new(&mut storage);
            reader.copy_into(&mut writer).unwrap();
        }
        assert_eq!(&storage[..], b"hello", "reader copy_into stops at the gate");

        // A same-pass multi-put (no capacity re-check between puts) completes its transfer,
        // exactly as `copy_into_multi_chunks` blesses.
        let mut storage: Vec<u8> = vec![];
        {
            let mut writer = WriteOnce::new(&mut storage);
            writer.put_slice(b"first");
            writer.put_slice(b"second");
        }
        assert_eq!(&storage[..], b"firstsecond");
    }

    #[test]
    fn write_once_test() {
        let mut writer: Vec<u8> = vec![];

        {
            let mut writer = writer.write_once();
            assert!(writer.has_remaining_capacity());
            writer.put_slice(b"hello");
            assert_eq!(writer.remaining_capacity(), 0);
            assert!(!writer.has_remaining_capacity());
        }

        {
            let mut writer = writer.write_once();
            assert!(writer.has_remaining_capacity());
            writer.put_chunk(b"hello"[..].into());
            assert_eq!(writer.remaining_capacity(), 0);
            assert!(!writer.has_remaining_capacity());
        }

        {
            let mut writer = writer.write_once();
            assert!(writer.has_remaining_capacity());
            // SAFETY: the closure initializes exactly the 5 bytes it reports.
            let committed = unsafe {
                writer.put_uninit_slice(5, |slice| {
                    slice.copy_from_slice(b"hello");
                    <Result<usize, core::convert::Infallible>>::Ok(5)
                })
            }
            .unwrap();
            assert_eq!(committed, Some(5));
            assert_eq!(writer.remaining_capacity(), 0);
            assert!(!writer.has_remaining_capacity());
        }

        {
            let mut writer = writer.write_once();
            assert!(writer.has_remaining_capacity());
            writer.put_bytes(Bytes::from_static(b"hello"));
            assert_eq!(writer.remaining_capacity(), 0);
            assert!(!writer.has_remaining_capacity());
        }

        {
            let mut writer = writer.write_once();
            assert!(writer.has_remaining_capacity());
            writer.put_bytes_mut(BytesMut::from(&b"hello"[..]));
            assert_eq!(writer.remaining_capacity(), 0);
            assert!(!writer.has_remaining_capacity());
        }
    }

    // ensures a reader that only reads capacity at the beginning can still write multiple chunks
    #[test]
    fn copy_into_multi_chunks() {
        let mut writer: Vec<u8> = vec![];
        {
            let mut writer = writer.write_once();

            assert!(writer.has_remaining_capacity());
            writer.put_slice(b"hello");
            assert!(!writer.has_remaining_capacity());
            writer.put_slice(b"world");
            assert!(!writer.has_remaining_capacity());
        }

        assert_eq!(&writer[..], b"helloworld");
    }
}
