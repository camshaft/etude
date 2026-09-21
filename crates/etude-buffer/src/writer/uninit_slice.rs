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
    unsafe fn put_uninit_slice<F, Error>(
        &mut self,
        payload_len: usize,
        f: F,
    ) -> Result<Option<usize>, Error>
    where
        F: FnOnce(&mut UninitSlice) -> Result<usize, Error>,
    {
        ensure!(self.len() >= payload_len, Ok(None));

        // Clamp the reported count to the slice length so the cursor never advances past the
        // exposed region; a within-slice over-report is the caller's documented responsibility.
        let reported = f(&mut self[..payload_len])?;
        let committed = reported.min(payload_len);

        let empty = UninitSlice::new(&mut []);
        let next = core::mem::replace(self, empty);
        *self = &mut next[committed..];

        Ok(Some(committed))
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
            // SAFETY: the closure initializes exactly the 5 bytes it reports.
            let committed = unsafe {
                writer.put_uninit_slice(5, |slice| {
                    slice.copy_from_slice(b"hello");
                    <Result<usize, core::convert::Infallible>>::Ok(5)
                })
            }
            .unwrap();
            assert_eq!(committed, Some(5));
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
