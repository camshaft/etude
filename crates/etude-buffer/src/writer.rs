// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! The write side: destinations that accept bytes as slices or owned chunks.
//!
//! [`Buffer`] is the writer trait. A destination accepts bytes up to its remaining capacity, by
//! borrowed slice or by owned [`Bytes`]/[`BytesMut`]. A destination that can adopt an owned chunk
//! without copying advertises that through [`SPECIALIZES_BYTES`](Buffer::SPECIALIZES_BYTES) /
//! [`SPECIALIZES_BYTES_MUT`](Buffer::SPECIALIZES_BYTES_MUT), so a reader can hand it a chunk
//! instead of a slice. The adapters ([`Limit`], [`Tracked`], [`WriteOnce`], [`BufMut`], …) wrap a
//! destination to bound, count, or bridge it to a `bytes::BufMut`.

use crate::reader::Chunk;
use bytes::{Bytes, BytesMut};

mod buf;
mod byte_queue;
mod discard;
mod empty;
mod limit;
mod tracked;
mod uninit_slice;
mod vec_deque;
mod write_once;

pub use buf::BufMut;
pub use bytes::buf::UninitSlice;
pub use discard::Discard;
pub use empty::Empty;
pub use limit::Limit;
pub use tracked::Tracked;
pub use write_once::WriteOnce;

/// A byte destination that accepts writes as slices or owned chunks.
///
/// A writer reports its remaining capacity and accepts bytes into it. Some destinations can adopt
/// an owned [`Bytes`]/[`BytesMut`] chunk without copying (a refcount move); those set
/// [`SPECIALIZES_BYTES`](Buffer::SPECIALIZES_BYTES) /
/// [`SPECIALIZES_BYTES_MUT`](Buffer::SPECIALIZES_BYTES_MUT) so a producer can choose the
/// copy-free path over [`put_slice`](Buffer::put_slice).
pub trait Buffer {
    /// `true` if [`put_bytes`](Buffer::put_bytes) can adopt an owned [`Bytes`] without copying.
    /// When `false`, copying a slice is typically cheaper than handing over a [`Bytes`].
    const SPECIALIZES_BYTES: bool = false;
    /// `true` if [`put_bytes_mut`](Buffer::put_bytes_mut) can adopt an owned [`BytesMut`] without
    /// copying. When `false`, copying a slice is typically cheaper.
    const SPECIALIZES_BYTES_MUT: bool = false;

    /// Writes `bytes` into the destination.
    ///
    /// `bytes.len()` must not exceed [`remaining_capacity`](Buffer::remaining_capacity).
    fn put_slice(&mut self, bytes: &[u8]);

    /// Writes `payload_len` bytes directly into the destination's memory via `f`, avoiding a staging
    /// copy.
    ///
    /// The `payload_len`-byte slice handed to `f` is zero-initialized first, and all `payload_len`
    /// bytes are committed on success — so a closure that fills fewer bytes (or none) yields zeros for
    /// the remainder, never uninitialized/stale memory. `f` is expected to fill the whole slice; the
    /// zero-init is a soundness floor, not a substitute for filling it.
    ///
    /// Returns `true` if the write happened. `false` means the destination cannot serve a slice of
    /// that length (e.g. its next chunk is too small); fall back to a regular `put_*` call.
    ///
    /// # Errors
    /// Returns any error `f` produces while filling the slice.
    #[inline(always)]
    fn put_uninit_slice<F, Error>(&mut self, payload_len: usize, f: F) -> Result<bool, Error>
    where
        F: FnOnce(&mut UninitSlice) -> Result<(), Error>,
    {
        // we can specialize on an empty payload
        ensure!(payload_len == 0, Ok(false));

        f(UninitSlice::new(&mut []))?;

        Ok(true)
    }

    /// Returns the additional number of bytes that can be written to the storage
    fn remaining_capacity(&self) -> usize;

    /// Returns `true` if the storage will accept any additional bytes
    #[inline]
    fn has_remaining_capacity(&self) -> bool {
        self.remaining_capacity() > 0
    }

    /// Writes [`Bytes`] into the storage
    ///
    /// Callers should check `SPECIALIZES_BYTES` before deciding to use this method. Otherwise, it
    /// might be cheaper to copy a slice into the storage and then increment the offset.
    #[inline]
    fn put_bytes(&mut self, bytes: Bytes) {
        self.put_slice(&bytes);
    }

    /// Writes [`BytesMut`] into the storage
    ///
    /// Callers should check `SPECIALIZES_BYTES_MUT` before deciding to use this method. Otherwise, it
    /// might be cheaper to copy a slice into the storage and then increment the offset.
    #[inline]
    fn put_bytes_mut(&mut self, bytes: BytesMut) {
        self.put_slice(&bytes);
    }

    /// Writes a reader [`Chunk`] into the storage
    #[inline]
    fn put_chunk(&mut self, chunk: Chunk) {
        match chunk {
            Chunk::Slice(v) => self.put_slice(v),
            Chunk::Bytes(v) => self.put_bytes(v),
            Chunk::BytesMut(v) => self.put_bytes_mut(v),
        }
    }

    /// Limits the number of bytes that can be written to the storage
    #[inline]
    fn with_write_limit(&mut self, max_len: usize) -> Limit<'_, Self> {
        Limit::new(self, max_len)
    }

    /// Tracks the number of bytes written to the storage
    #[inline]
    fn track_write(&mut self) -> Tracked<'_, Self> {
        Tracked::new(self)
    }

    /// Only allows a single write into the storage. After that, no more writes are allowed.
    ///
    /// This can be used for very low latency scenarios where processing the single read is more
    /// important than filling the entire storage with as much data as possible.
    #[inline]
    fn write_once(&mut self) -> WriteOnce<'_, Self> {
        WriteOnce::new(self)
    }
}
