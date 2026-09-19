// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use crate::writer::Buffer;
use bytes::buf::UninitSlice;

impl Buffer for &mut UninitSlice {
    #[inline]
    fn put_slice(&mut self, bytes: &[u8]) {
        // `bytes.len()` is caller-controlled, so this is a REAL bounds check, not `assume!`: on a
        // safe trait an `assume!` here would let a release build elide the copy's bounds check and
        // write out of bounds. Panicking on overflow matches `bytes::BufMut::put_slice`.
        assert!(
            self.len() >= bytes.len(),
            "put_slice of {} bytes exceeds the {}-byte remaining capacity",
            bytes.len(),
            self.len()
        );
        self[..bytes.len()].copy_from_slice(bytes);
        let empty = UninitSlice::new(&mut []);
        let next = core::mem::replace(self, empty);
        *self = &mut next[bytes.len()..];
    }

    #[inline]
    fn put_uninit_slice<F, Error>(&mut self, payload_len: usize, f: F) -> Result<bool, Error>
    where
        F: FnOnce(&mut UninitSlice) -> Result<(), Error>,
    {
        ensure!(self.len() >= payload_len, Ok(false));

        f(&mut self[..payload_len])?;

        let empty = UninitSlice::new(&mut []);
        let next = core::mem::replace(self, empty);
        *self = &mut next[payload_len..];

        Ok(true)
    }

    #[inline]
    fn remaining_capacity(&self) -> usize {
        self.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uninit_slice_test() {
        let mut storage = [0; 8];

        {
            let mut writer = UninitSlice::new(&mut storage[..]);
            assert_eq!(writer.remaining_capacity(), 8);
            writer.put_slice(b"hello");
            assert_eq!(writer.remaining_capacity(), 3);
        }

        {
            let mut writer = UninitSlice::new(&mut storage[..]);
            assert_eq!(writer.remaining_capacity(), 8);
            let did_write = writer
                .put_uninit_slice(5, |slice| {
                    slice.copy_from_slice(b"hello");
                    <Result<(), core::convert::Infallible>>::Ok(())
                })
                .unwrap();
            assert!(did_write);
            assert_eq!(writer.remaining_capacity(), 3);
        }
    }

    /// An oversized `put_slice` must panic (a real bounds check), not invoke UB. Before the fix an
    /// `assume!` let a release build elide the copy's bounds check and write out of bounds.
    #[test]
    #[should_panic(expected = "exceeds the")]
    fn put_slice_over_capacity_panics() {
        let mut storage = [0u8; 4];
        let mut writer = UninitSlice::new(&mut storage[..]);
        writer.put_slice(b"toolong"); // 7 bytes into 4 -> must panic, never write OOB
    }
}
