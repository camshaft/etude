// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! The read side: sources that yield their bytes as borrowed chunks.
//!
//! [`Buffer`] is the reader trait. A source hands out its bytes one contiguous [`Chunk`] at a
//! time, so a caller can forward each chunk to a [`writer::Buffer`](crate::writer::Buffer)
//! without an intermediate copy. The adapters ([`Chain`], [`Tracked`], [`FullCopy`], [`Buf`],
//! [`IoSlice`], …) wrap a source to compose, count, or bridge it to a `bytes::Buf`.

mod buf;
mod bytes;
mod chain;
mod chunk;
mod empty;
mod full_copy;
mod infallible;
mod io_slice;
mod slice;
mod tracked;

#[cfg(test)]
mod tests;

pub use buf::Buf;
pub use chain::Chain;
pub use chunk::Chunk;
pub use empty::Empty;
pub use full_copy::FullCopy;
pub use infallible::Infallible;
pub use io_slice::IoSlice;
pub use tracked::Tracked;

/// A byte source that yields its buffered bytes as borrowed chunks.
///
/// A reader exposes how many bytes it has buffered and hands them out one contiguous [`Chunk`]
/// at a time, so a caller can move them into a [`writer::Buffer`](crate::writer::Buffer) without
/// an intermediate copy. The trait is cursor-free: a reader tracks only what it still holds, not
/// its position in a larger stream — layer that on top with [`track_read`](Buffer::track_read).
pub trait Buffer {
    /// The error a read may fail with. Use [`core::convert::Infallible`] for a source that
    /// cannot fail (which unlocks the [`Infallible`] extension methods).
    type Error: 'static;

    /// Returns the number of bytes currently buffered and available to read.
    fn buffered_len(&self) -> usize;

    /// Returns `true` when no bytes remain buffered.
    #[inline]
    fn buffer_is_empty(&self) -> bool {
        self.buffered_len() == 0
    }

    /// Returns the next contiguous chunk, borrowing up to `watermark` bytes and consuming them
    /// from the reader.
    ///
    /// A source spanning several internal chunks returns only the first; call again for the
    /// rest. The returned chunk may be shorter than `watermark` (a chunk boundary) and is empty
    /// once the reader is drained.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying source fails.
    fn read_chunk(&mut self, watermark: usize) -> Result<Chunk<'_>, Self::Error>;

    /// Drains the reader into `dest`, returning a trailing chunk the caller must still place.
    ///
    /// Copies until `dest` is full or the reader is exhausted, but leaves the final contiguous
    /// run as the returned [`Chunk`] instead of copying it — letting the caller forward or defer
    /// it without a copy. The returned chunk fits within `dest`'s remaining capacity. Dropping it
    /// without copying it into `dest` discards those bytes; use [`copy_into`](Buffer::copy_into)
    /// to have the trailing chunk placed for you.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying source fails.
    fn partial_copy_into<Dest>(&mut self, dest: &mut Dest) -> Result<Chunk<'_>, Self::Error>
    where
        Dest: crate::writer::Buffer + ?Sized;

    /// Drains the reader into `dest`, copying the trailing chunk too.
    ///
    /// Copies until `dest` is full or the reader is exhausted. Unlike
    /// [`partial_copy_into`](Buffer::partial_copy_into), nothing is left for the caller to place.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying source fails.
    #[inline]
    fn copy_into<Dest>(&mut self, dest: &mut Dest) -> Result<(), Self::Error>
    where
        Dest: crate::writer::Buffer + ?Sized,
    {
        let mut chunk = self.partial_copy_into(dest)?;
        chunk.infallible_copy_into(dest);
        Ok(())
    }

    /// Wraps the reader so [`partial_copy_into`](Buffer::partial_copy_into) always copies the
    /// trailing chunk rather than returning it — its returned [`Chunk`] is always empty.
    ///
    /// Use when the caller cannot place a deferred chunk itself and wants a full copy either way.
    #[inline]
    fn full_copy(&mut self) -> FullCopy<'_, Self> {
        FullCopy::new(self)
    }

    /// Wraps the reader to count the bytes read through it, queryable via
    /// [`Tracked::consumed_len`].
    #[inline]
    fn track_read(&mut self) -> Tracked<'_, Self> {
        Tracked::new(self)
    }

    /// Chains this reader with another, draining `self` fully before `other`.
    #[inline]
    fn chain<Other>(self, other: Other) -> Chain<Self, Other>
    where
        Self: Sized + Buffer<Error = core::convert::Infallible>,
        Other: Buffer<Error = core::convert::Infallible>,
    {
        Chain::new(self, other)
    }
}
