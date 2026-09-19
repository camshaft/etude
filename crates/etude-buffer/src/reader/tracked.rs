// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use crate::{reader::Buffer, writer};

/// A reader wrapper that counts the bytes read through it.
///
/// Construct via [`Buffer::track_read`](crate::reader::Buffer::track_read). Reads delegate to the
/// wrapped reader; [`consumed_len`](Tracked::consumed_len) reports the running total.
pub struct Tracked<'a, S: Buffer + ?Sized> {
    consumed: usize,
    storage: &'a mut S,
}

impl<'a, S: Buffer + ?Sized> Tracked<'a, S> {
    /// Wraps `storage` to count bytes read through it. Prefer
    /// [`Buffer::track_read`](crate::reader::Buffer::track_read).
    #[inline]
    pub fn new(storage: &'a mut S) -> Self {
        Self {
            consumed: 0,
            storage,
        }
    }

    /// Returns the number of bytes read through this wrapper so far.
    #[inline]
    pub fn consumed_len(&self) -> usize {
        self.consumed
    }

    /// The underlying storage being tracked.
    #[inline]
    pub fn storage(&self) -> &S {
        self.storage
    }
}

impl<S: Buffer + ?Sized> Buffer for Tracked<'_, S> {
    type Error = S::Error;

    #[inline]
    fn buffered_len(&self) -> usize {
        self.storage.buffered_len()
    }

    #[inline]
    fn read_chunk(&mut self, watermark: usize) -> Result<super::Chunk<'_>, Self::Error> {
        let chunk = self.storage.read_chunk(watermark)?;
        self.consumed += chunk.len();
        Ok(chunk)
    }

    #[inline]
    fn partial_copy_into<Dest>(&mut self, dest: &mut Dest) -> Result<super::Chunk<'_>, Self::Error>
    where
        Dest: writer::Buffer + ?Sized,
    {
        let mut dest = dest.track_write();
        let chunk = self.storage.partial_copy_into(&mut dest)?;
        self.consumed += dest.written_len();
        self.consumed += chunk.len();
        Ok(chunk)
    }

    #[inline]
    fn copy_into<Dest>(&mut self, dest: &mut Dest) -> Result<(), Self::Error>
    where
        Dest: writer::Buffer + ?Sized,
    {
        let mut dest = dest.track_write();
        self.storage.copy_into(&mut dest)?;
        self.consumed += dest.written_len();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use writer::Buffer as _;

    #[test]
    fn tracked_test() {
        let mut reader = &b"hello world"[..];
        let mut writer: Vec<u8> = vec![];

        {
            let mut reader = reader.track_read();
            let chunk = reader.read_chunk(1).unwrap();
            assert_eq!(&chunk[..], b"h");
            assert_eq!(reader.consumed_len(), 1);
        }

        {
            let mut reader = reader.track_read();
            let mut writer = writer.with_write_limit(5);

            let chunk = reader.partial_copy_into(&mut writer).unwrap();
            assert_eq!(&chunk[..], b"ello ");
            assert_eq!(reader.consumed_len(), 5);
        }

        assert_eq!(writer.len(), 0);

        {
            let mut reader = reader.track_read();
            reader.copy_into(&mut writer).unwrap();
            assert_eq!(reader.consumed_len(), 5);
            assert_eq!(&writer[..], b"world");
        }

        assert_eq!(reader.len(), 0);
    }
}
