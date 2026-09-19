// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! A [`Builder`] for efficiently constructing a [`ByteVec`] by buffering writes into a head buffer
//! and folding completed chunks into the rope.

use super::{ByteVec, ByteVecError};
use bytes::{Bytes, BytesMut};
use etude_buffer::writer::Buffer as _;
use etude_buffer::{reader, writer};

const DEFAULT_CAPACITY: usize = 1 << 17;

/// A builder for efficiently constructing a [`ByteVec`] by buffering writes.
///
/// The builder maintains a head buffer for direct writes and a rope of completed chunks. This allows
/// for efficient buffering of writes while preserving the chunked, structurally-shared nature of
/// [`ByteVec`].
///
/// # Examples
///
/// ```
/// use etude_bytevec::ByteVec;
/// use bytes::Bytes;
/// use etude_buffer::writer::Buffer;
///
/// let mut builder = ByteVec::builder(1024);
/// builder.put_slice(b"hello");
/// builder.put_slice(b" world");
///
/// let byte_rope = builder.finish();
/// assert_eq!(byte_rope, b"hello world");
/// ```
#[derive(Debug)]
pub struct Builder {
    chunks: ByteVec,
    head: BytesMut,
    capacity: usize,
    /// Chunks with `len <= inline_threshold` are copied into the contiguous head buffer;
    /// larger chunks are held by reference (zero-copy). `0` means never copy — reference
    /// everything.
    inline_threshold: usize,
}

impl Default for Builder {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl Builder {
    /// The head-buffer capacity used by [`Builder::default`] (128 KiB).
    pub const DEFAULT_CAPACITY: usize = DEFAULT_CAPACITY;

    /// Creates a new [`Builder`] with the specified capacity for the head buffer.
    ///
    /// The capacity determines the size of the internal buffer used for direct writes.
    /// When this buffer is full, it will be flushed to the rope of chunks.
    ///
    /// # Examples
    ///
    /// ```
    /// use etude_bytevec::ByteVec;
    ///
    /// let builder = ByteVec::builder(1024);
    /// ```
    pub fn new(capacity: usize) -> Self {
        Builder {
            chunks: ByteVec::new(),
            head: BytesMut::new(),
            capacity,
            inline_threshold: 0,
        }
    }

    /// Sets the inline threshold: chunks with `len <= threshold` handed to `put_bytes`
    /// are copied into the contiguous head buffer, while larger chunks are held by
    /// reference (zero-copy).
    ///
    /// The default is `0` — every chunk is held by reference. Raising it trades a small
    /// `memcpy` for fewer reference-counted chunks (less per-chunk overhead and
    /// fragmentation) when a lot of tiny `Bytes` are written.
    ///
    /// # Examples
    ///
    /// ```
    /// use etude_bytevec::ByteVec;
    ///
    /// // Copy any chunk of 16 bytes or fewer into the contiguous buffer.
    /// let builder = ByteVec::builder(1024).with_inline_threshold(16);
    /// ```
    pub fn with_inline_threshold(mut self, threshold: usize) -> Self {
        self.inline_threshold = threshold;
        self
    }

    /// Returns the current inline threshold. See [`Builder::with_inline_threshold`].
    pub fn inline_threshold(&self) -> usize {
        self.inline_threshold
    }

    /// Returns the total number of bytes in the builder.
    ///
    /// This includes both the bytes in the completed chunks and the head buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// use etude_bytevec::ByteVec;
    /// use etude_buffer::writer::Buffer;
    ///
    /// let mut builder = ByteVec::builder(1024);
    /// builder.put_slice(b"hello");
    /// assert_eq!(builder.len(), 5);
    ///
    /// builder.put_slice(b" world");
    /// assert_eq!(builder.len(), 11);
    /// ```
    pub fn len(&self) -> usize {
        self.chunks.len() + self.head.len()
    }

    /// Returns `true` if the builder contains no bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use etude_bytevec::ByteVec;
    /// use etude_buffer::writer::Buffer;
    ///
    /// let mut builder = ByteVec::builder(1024);
    /// assert!(builder.is_empty());
    ///
    /// builder.put_slice(b"hello");
    /// assert!(!builder.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.head.is_empty() && self.chunks.is_empty()
    }

    /// Appends the contents of another [`ByteVec`] to this builder.
    ///
    /// This operation flushes the current head buffer before appending the new bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use etude_bytevec::ByteVec;
    /// use etude_buffer::writer::Buffer;
    ///
    /// let mut builder = ByteVec::builder(1024);
    /// builder.put_slice(b"hello");
    ///
    /// let mut other = ByteVec::from(b" world");
    /// builder.append(&mut other);
    ///
    /// let result = builder.finish();
    /// assert_eq!(result, b"hello world");
    /// ```
    pub fn append(&mut self, bytes: &mut ByteVec) {
        if bytes.is_empty() {
            return;
        }
        self.flush();
        self.chunks.append(bytes);
    }

    /// Appends the contents of another [`ByteVec`] to this builder, leaving the source untouched.
    ///
    /// This operation flushes the current head buffer before appending the new bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use etude_bytevec::ByteVec;
    /// use etude_buffer::writer::Buffer;
    ///
    /// let mut builder = ByteVec::builder(1024);
    /// builder.put_slice(b"hello");
    ///
    /// let other = ByteVec::from(b" world");
    /// builder.extend(&other);
    ///
    /// let result = builder.finish();
    /// assert_eq!(result, b"hello world");
    /// ```
    pub fn extend(&mut self, bytes: &ByteVec) {
        if bytes.is_empty() {
            return;
        }
        self.flush();
        self.chunks.extend(bytes.chunks().cloned());
    }

    /// Splits the builder, taking all bytes and leaving it empty.
    ///
    /// This operation flushes the current head buffer before splitting.
    ///
    /// # Examples
    ///
    /// ```
    /// use etude_bytevec::ByteVec;
    /// use etude_buffer::writer::Buffer;
    ///
    /// let mut builder = ByteVec::builder(1024);
    /// builder.put_slice(b"hello world");
    ///
    /// let bytes = builder.split();
    /// assert_eq!(bytes, b"hello world");
    /// assert!(builder.is_empty());
    /// ```
    pub fn split(&mut self) -> ByteVec {
        self.flush();
        core::mem::take(&mut self.chunks)
    }

    /// Splits the bytes into two at the given index.
    ///
    /// After this operation, `self` contains elements `[at, len)`, and the returned [`ByteVec`]
    /// contains elements `[0, at)`.
    ///
    /// # Errors
    ///
    /// Returns [`ByteVecError::OutOfBounds`] if `at` is greater than the builder's length.
    ///
    /// # Examples
    ///
    /// ```
    /// use etude_bytevec::ByteVec;
    /// use etude_buffer::writer::Buffer;
    ///
    /// let mut builder = ByteVec::builder(1024);
    /// builder.put_slice(b"hello world");
    ///
    /// let hello = builder.split_to(5).unwrap();
    /// assert_eq!(hello, b"hello");
    ///
    /// let result = builder.finish();
    /// assert_eq!(result, b" world");
    /// ```
    pub fn split_to(&mut self, at: usize) -> Result<ByteVec, ByteVecError> {
        let len = self.len();

        if len < at {
            return Err(ByteVecError::OutOfBounds(at));
        }

        if len == at {
            return Ok(self.split());
        }

        // check if we need to move some of the head into the chunks
        if let Some(remaining) = at.checked_sub(self.chunks.len()).filter(|v| *v > 0) {
            self.chunks
                .push_back(self.head.split_to(remaining).freeze());
        }

        self.chunks.split_to(at)
    }

    /// Finishes building and returns the constructed [`ByteVec`].
    ///
    /// This operation consumes the builder and returns the final [`ByteVec`] containing all written
    /// bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use etude_bytevec::ByteVec;
    /// use etude_buffer::writer::Buffer;
    ///
    /// let mut builder = ByteVec::builder(1024);
    /// builder.put_slice(b"hello");
    /// builder.put_slice(b" world");
    ///
    /// let result = builder.finish();
    /// assert_eq!(result, b"hello world");
    /// ```
    pub fn finish(self) -> ByteVec {
        let mut chunks = self.chunks;
        if !self.head.is_empty() {
            chunks.push_back(self.head.freeze());
        }
        chunks
    }

    /// Calls the provided function and prefixes the written data with a `u64` big-endian length.
    pub fn write_with_len_prefix<F: FnOnce(&mut Self)>(&mut self, f: F) {
        // flush any data we have buffered
        self.flush();

        // record the starting byte length — everything already in `chunks` precedes the caller write
        let before_len = self.len();

        // have the caller write into the buffer
        f(self);

        // flush any data we have buffered from the caller write
        self.flush();

        // compute the amount of data written by the caller
        let written_len = (self.len() - before_len) as u64;

        // build the 8-byte big-endian length as its own chunk
        let len_chunk = Bytes::copy_from_slice(&written_len.to_be_bytes());
        // make sure the length chunk is not torn
        debug_assert_eq!(len_chunk.len(), 8);

        // splice the length chunk in immediately before the caller's write:
        //   chunks = [0, before_len) ++ len_chunk ++ [before_len, end)
        // (`before_len` is a chunk boundary because the pre-write flush emptied `head`).
        let mut prefix = self
            .chunks
            .split_to(before_len)
            .expect("before_len <= chunks.len()");
        prefix.push_back(len_chunk);
        prefix.append(&mut self.chunks);
        self.chunks = prefix;
    }

    /// Reserves buffer space for reading from a socket.
    pub fn for_socket_read<F: FnOnce(&mut bytes::buf::UninitSlice) -> usize>(
        &mut self,
        preferred_read_size: usize,
        f: F,
    ) {
        if preferred_read_size > self.head.spare_capacity_mut().len() {
            self.flush_and_reserve(preferred_read_size);
        }

        let reported = self
            .head
            .put_uninit_slice(preferred_read_size, |slice| {
                let len = f(slice);
                Err(len)
            })
            .unwrap_err();

        // The callback was handed a slice of exactly `preferred_read_size` uninitialized bytes, so it
        // can only have initialized bytes WITHIN that slice. A reported length beyond it (a buggy or
        // hostile socket read claiming more than the buffer it was given) must NOT reach the unsafe
        // `advance_mut`, or uninitialized heap memory past the slice would be committed as rope content
        // (a safe-code info-leak). Clamp to the slice length so the commit is always sound; debug builds
        // additionally assert the contract to surface caller misuse early.
        debug_assert!(
            reported <= preferred_read_size,
            "for_socket_read callback reported {reported} bytes for a {preferred_read_size}-byte slice"
        );
        let len = reported.min(preferred_read_size);

        unsafe {
            use bytes::BufMut;
            self.head.advance_mut(len);
        }
    }

    // flushes and reserves at least the specified `min_len`
    fn flush_and_reserve(&mut self, min_len: usize) {
        let capacity = self.capacity.max(min_len);
        let head = core::mem::replace(&mut self.head, BytesMut::with_capacity(capacity));
        if !head.is_empty() {
            self.chunks.push_back(head.freeze());
        }
    }

    // flushes the current buffer, if non-empty
    fn flush(&mut self) {
        if !self.head.is_empty() {
            self.chunks.push_back(self.head.split().freeze());
        }
    }
}

impl From<ByteVec> for Builder {
    fn from(chunks: ByteVec) -> Self {
        Builder {
            chunks,
            head: BytesMut::new(),
            capacity: DEFAULT_CAPACITY,
            inline_threshold: 0,
        }
    }
}

impl From<Builder> for ByteVec {
    fn from(writer: Builder) -> Self {
        writer.finish()
    }
}

impl writer::Buffer for Builder {
    // Always accept `Bytes`/`BytesMut`; the `inline_threshold` decides at runtime whether a
    // given chunk is copied into the head buffer or held by reference.
    const SPECIALIZES_BYTES: bool = true;
    const SPECIALIZES_BYTES_MUT: bool = true;

    fn put_slice(&mut self, bytes: &[u8]) {
        let remaining_capacity = self.head.spare_capacity_mut().len();
        let len = bytes.len().min(remaining_capacity);
        let (head, tail) = bytes.split_at(len);

        // append the head if it has capacity
        if !head.is_empty() {
            self.head.put_slice(head);
        }

        // if tail is non-empty then we need to allocate a new chunk
        if !tail.is_empty() {
            self.flush_and_reserve(tail.len());
            self.head.put_slice(tail);
        }
    }

    fn remaining_capacity(&self) -> usize {
        usize::MAX
    }

    fn put_uninit_slice<F, Error>(&mut self, payload_len: usize, f: F) -> Result<bool, Error>
    where
        F: FnOnce(&mut bytes::buf::UninitSlice) -> Result<(), Error>,
    {
        if payload_len > self.head.spare_capacity_mut().len() {
            self.flush_and_reserve(payload_len);
        }

        self.head.put_uninit_slice(payload_len, f)
    }

    fn has_remaining_capacity(&self) -> bool {
        true
    }

    fn put_bytes(&mut self, bytes: Bytes) {
        if bytes.is_empty() {
            return;
        }
        // small chunks are cheaper to copy into the contiguous buffer than to hold as a
        // separate reference-counted chunk
        if bytes.len() <= self.inline_threshold {
            self.put_slice(&bytes);
            return;
        }
        self.flush();
        self.chunks.push_back(bytes);
    }

    fn put_bytes_mut(&mut self, bytes: BytesMut) {
        if bytes.is_empty() {
            return;
        }
        if bytes.len() <= self.inline_threshold {
            self.put_slice(&bytes);
            return;
        }
        self.flush();
        self.chunks.push_back(bytes.freeze());
    }
}

impl reader::Buffer for Builder {
    type Error = core::convert::Infallible;

    fn buffered_len(&self) -> usize {
        self.head.buffered_len() + self.chunks.buffered_len()
    }

    fn buffer_is_empty(&self) -> bool {
        self.head.is_empty() && self.chunks.is_empty()
    }

    fn read_chunk(&mut self, watermark: usize) -> Result<reader::Chunk<'_>, Self::Error> {
        if self.chunks.is_empty() {
            self.head.read_chunk(watermark)
        } else {
            self.chunks.read_chunk(watermark)
        }
    }

    fn partial_copy_into<Dest>(&mut self, dest: &mut Dest) -> Result<reader::Chunk<'_>, Self::Error>
    where
        Dest: writer::Buffer + ?Sized,
    {
        // First drain chunks into dest
        if !self.chunks.buffer_is_empty() {
            let chunk = self.chunks.partial_copy_into(dest)?;

            if !chunk.is_empty() {
                let mut should_return = false;

                // If dest matches the chunk, return it
                should_return |= dest.remaining_capacity() == chunk.len();

                // if the `head` is empty then return early as well
                should_return |= self.head.buffer_is_empty();

                if should_return {
                    return Ok(chunk);
                }

                // Otherwise, write the trailing chunk to dest and continue to head
                dest.put_chunk(chunk);
            }
        }

        // Then drain head into dest
        self.head.partial_copy_into(dest)
    }

    fn copy_into<Dest>(&mut self, dest: &mut Dest) -> Result<(), Self::Error>
    where
        Dest: writer::Buffer + ?Sized,
    {
        self.chunks.copy_into(dest)?;
        self.head.copy_into(dest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_threshold_zero_references_everything() {
        // the default threshold of 0 holds every chunk by reference
        let mut b = ByteVec::builder(1024);
        assert_eq!(b.inline_threshold(), 0);
        b.put_bytes(Bytes::from_static(b"ab"));
        b.put_bytes(Bytes::from_static(b"cd"));
        let out = b.finish();
        assert_eq!(out, b"abcd");
        assert_eq!(out.chunks().len(), 2);
    }

    #[test]
    fn inline_threshold_compacts_small_chunks() {
        // small chunks (<= threshold) are copied into the contiguous head buffer
        let mut b = ByteVec::builder(1024).with_inline_threshold(16);
        b.put_bytes(Bytes::from_static(b"ab"));
        b.put_bytes(Bytes::from_static(b"cd"));
        let out = b.finish();
        assert_eq!(out, b"abcd");
        assert_eq!(out.chunks().len(), 1);
    }

    #[test]
    fn inline_threshold_still_references_large_chunks() {
        let big = Bytes::from(vec![7u8; 64]);
        let mut b = ByteVec::builder(1024).with_inline_threshold(16);
        b.put_bytes(Bytes::from_static(b"ab")); // <= 16 -> copied into head
        b.put_bytes(big); // > 16 -> flushes head, held by reference
        let out = b.finish();
        assert_eq!(out.len(), 2 + 64);
        assert_eq!(out.chunks().len(), 2);
    }

    #[test]
    fn write_with_len_prefix_frames_the_payload() {
        let mut b = ByteVec::builder(1024);
        b.put_slice(b"before");
        b.write_with_len_prefix(|w| {
            w.put_slice(b"payload");
        });
        b.put_slice(b"after");

        let out = b.finish();
        // "before" ++ (u64 be = 7) ++ "payload" ++ "after"
        let mut expected = Vec::new();
        expected.extend_from_slice(b"before");
        expected.extend_from_slice(&7u64.to_be_bytes());
        expected.extend_from_slice(b"payload");
        expected.extend_from_slice(b"after");
        assert_eq!(&out.copy_to_bytes()[..], &expected[..]);
    }

    #[test]
    fn split_to_across_head_and_chunks() {
        let mut b = ByteVec::builder(1024);
        b.put_bytes(Bytes::from_static(b"abcd")); // held as a chunk (threshold 0)
        b.put_slice(b"efgh"); // buffered in head
        assert_eq!(b.len(), 8);

        let front = b.split_to(6).expect("within bounds");
        assert_eq!(front, b"abcdef");
        assert_eq!(b.finish(), b"gh");
    }

    #[test]
    fn from_rope_roundtrips_through_builder() {
        let rope = ByteVec::from(b"seed");
        let mut b = Builder::from(rope);
        b.put_slice(b"-tail");
        let out: ByteVec = b.into();
        assert_eq!(out, b"seed-tail");
    }

    /// RED reproducer (breaker-bytevec): `for_socket_read` is a SAFE fn that trusts the SAFE
    /// callback's returned length and feeds it to `unsafe BytesMut::advance_mut`. A callback that
    /// returns `len > preferred_read_size` (but within the head's spare capacity) commits
    /// UNINITIALIZED heap memory as rope content — safe code exposing uninit bytes (observed: a
    /// 3-byte write claiming 100 yields a 100-byte rope whose tail is stale allocator garbage).
    /// A sound implementation must either clamp the commit to the provided slice's length or
    /// panic on the contract violation — either passes this test; committing past the slice fails.
    #[test]
    fn for_socket_read_never_commits_more_than_the_provided_slice() {
        let result = std::panic::catch_unwind(|| {
            let mut b = ByteVec::builder(1024);
            b.for_socket_read(8, |slice| {
                slice[0..3].copy_from_slice(b"abc");
                100 // a buggy callback claims more than the 8-byte slice it was given
            });
            b
        });
        // A panicking defense is acceptable (Err); a clamping defense must not commit past the
        // 8-byte slice the callback was actually handed.
        if let Ok(b) = result {
            assert!(
                b.len() <= 8,
                "committed {} bytes for an 8-byte read slice (uninitialized memory exposed)",
                b.len()
            );
        }
    }

    #[test]
    fn out_of_bounds_split_to_errors() {
        let mut b = ByteVec::builder(1024);
        b.put_slice(b"abc");
        assert_eq!(b.split_to(4), Err(ByteVecError::OutOfBounds(4)));
    }
}
