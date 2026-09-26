// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! A [`Builder`] for efficiently constructing a [`ByteVec`] by buffering writes into a head buffer
//! and folding completed chunks into the rope.

use super::{ByteVec, ByteVecError};
use bytes::{Bytes, BytesMut};
use etude_buffer::writer::Buffer as _;
use etude_buffer::{reader, writer};

const DEFAULT_CAPACITY: usize = 1 << 17;

/// The policy a [`Builder`] runs on: how it sizes its head buffer, when it inlines small chunks, and
/// how a completed chunk buffer becomes immutable [`Bytes`].
///
/// A [`Builder`] is *parameterized* over this behavior trait, so a caller can control every builder
/// knob by supplying one `Behavior` impl — without reimplementing the builder. Every method has a
/// default matching the plain builder, so an impl overrides only the knobs it cares about, and the
/// zero-sized [`DefaultBehavior`] (all defaults) leaves existing callers unchanged.
///
/// The motivating override is [`Behavior::freeze`] for secret hygiene: wrap the completed buffer in
/// a zeroize-on-drop owner and produce the chunk via [`Bytes::from_owner`] instead of
/// [`BytesMut::freeze`], so the backing allocation is wiped when the last chunk drops.
pub trait Behavior {
    /// The head-buffer capacity a builder uses when one is not given explicitly (e.g. via
    /// [`Builder::default`] or [`Builder::from_behavior`]). Defaults to 128 KiB.
    fn default_capacity(&self) -> usize {
        DEFAULT_CAPACITY
    }

    /// Chunks with `len <= inline_threshold()` handed to `put_bytes`/`put_bytes_mut` are copied into
    /// the contiguous head buffer; larger chunks are held by reference (zero-copy). Defaults to `0`
    /// (reference everything). A builder created from this behavior starts at this value;
    /// [`Builder::with_inline_threshold`] can still override it per instance.
    fn inline_threshold(&self) -> usize {
        0
    }

    /// Converts a completed chunk buffer into the immutable [`Bytes`] stored in the rope. Defaults
    /// to [`BytesMut::freeze`] — reuse the buffer in place, no copy.
    fn freeze(&self, buf: BytesMut) -> Bytes {
        buf.freeze()
    }
}

/// The default [`Behavior`]: 128 KiB capacity, no inline threshold, and [`BytesMut::freeze`] — a
/// zero-sized type, so `Builder<DefaultBehavior>` costs and behaves exactly as the plain builder.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultBehavior;

impl Behavior for DefaultBehavior {}

/// A builder for efficiently constructing a [`ByteVec`] by buffering writes.
///
/// The builder maintains a head buffer for direct writes and a rope of completed chunks. This allows
/// for efficient buffering of writes while preserving the chunked, structurally-shared nature of
/// [`ByteVec`].
///
/// The type parameter `B` is the [`Behavior`] — the builder's capacity/threshold/freeze policy. It
/// defaults to [`DefaultBehavior`], so `Builder` (unparameterized) behaves exactly as before; use
/// [`Builder::with_behavior`] or [`Builder::from_behavior`] to select another.
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
pub struct Builder<B = DefaultBehavior> {
    chunks: ByteVec,
    head: BytesMut,
    capacity: usize,
    /// Chunks with `len <= inline_threshold` are copied into the contiguous head buffer;
    /// larger chunks are held by reference (zero-copy). `0` means never copy — reference
    /// everything. Initialized from [`Behavior::inline_threshold`]; overridable per instance via
    /// [`Builder::with_inline_threshold`].
    inline_threshold: usize,
    /// The [`Behavior`] policy — capacity/threshold defaults and how a completed [`BytesMut`] chunk
    /// becomes immutable [`Bytes`]. Defaults to [`DefaultBehavior`]; selected via
    /// [`Builder::with_behavior`]/[`Builder::from_behavior`].
    behavior: B,
}

impl Default for Builder<DefaultBehavior> {
    fn default() -> Self {
        Self::from_behavior(DefaultBehavior)
    }
}

impl Builder<DefaultBehavior> {
    /// The head-buffer capacity used by [`Builder::default`] (128 KiB).
    pub const DEFAULT_CAPACITY: usize = DEFAULT_CAPACITY;

    /// Creates a new [`Builder`] with the specified capacity for the head buffer.
    ///
    /// The capacity determines the size of the internal buffer used for direct writes.
    /// When this buffer is full, it will be flushed to the rope of chunks.
    ///
    /// The builder uses the [`DefaultBehavior`]; call [`Builder::with_behavior`] to select another.
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
            inline_threshold: DefaultBehavior.inline_threshold(),
            behavior: DefaultBehavior,
        }
    }
}

impl<B: Behavior> Builder<B> {
    /// Creates a [`Builder`] driven by the given [`Behavior`], taking its head-buffer capacity and
    /// inline threshold from the behavior's [`default_capacity`](Behavior::default_capacity) and
    /// [`inline_threshold`](Behavior::inline_threshold).
    ///
    /// This is the "control the builder with one impl" entry point — the behavior supplies every
    /// knob. Use [`ByteVec::builder`] instead to pin an explicit capacity with the default behavior.
    pub fn from_behavior(behavior: B) -> Self {
        Builder {
            chunks: ByteVec::new(),
            head: BytesMut::new(),
            capacity: behavior.default_capacity(),
            inline_threshold: behavior.inline_threshold(),
            behavior,
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

    /// Selects the [`Behavior`]: the builder's capacity/threshold/freeze policy, returning a builder
    /// parameterized over the new behavior (the buffered contents and current capacity/threshold are
    /// carried over — only the freeze behavior and future policy queries change).
    ///
    /// The behavior's [`freeze`](Behavior::freeze) is applied at every point the builder turns a
    /// [`BytesMut`] into a chunk: flushing the head buffer (on capacity overflow, [`Builder::split`],
    /// [`Builder::split_to`], [`Builder::append`]/[`Builder::extend`], and [`Builder::finish`]) and
    /// freezing a [`BytesMut`] handed to [`put_bytes_mut`](writer::Buffer::put_bytes_mut). It is
    /// *not* applied to chunks supplied already-frozen as [`Bytes`]. To also adopt the behavior's
    /// default capacity and threshold, construct with [`Builder::from_behavior`] instead.
    ///
    /// The default is [`DefaultBehavior`] ([`BytesMut::freeze`], in place, no copy). Parameterizing
    /// over a trait keeps the builder generic while letting the caller add new behaviors.
    ///
    /// # Examples
    ///
    /// Wipe secret-carrying chunks when they drop by providing a [`Behavior`] impl that wraps the
    /// buffer in a zeroize-on-drop owner and produces the chunk via [`Bytes::from_owner`] instead
    /// of [`BytesMut::freeze`]. The builder never sees the secret policy — it just calls the trait:
    ///
    /// ```
    /// use etude_bytevec::{ByteVec, Behavior};
    /// use etude_buffer::writer::Buffer;
    /// use bytes::{Bytes, BytesMut};
    ///
    /// // An owner that zeroes its backing buffer when the last chunk referencing it drops.
    /// struct ZeroizeOnDrop(BytesMut);
    /// impl AsRef<[u8]> for ZeroizeOnDrop {
    ///     fn as_ref(&self) -> &[u8] {
    ///         &self.0
    ///     }
    /// }
    /// impl Drop for ZeroizeOnDrop {
    ///     fn drop(&mut self) {
    ///         self.0.fill(0);
    ///     }
    /// }
    ///
    /// // A behavior that only overrides `freeze` (capacity/threshold keep their defaults).
    /// struct Zeroizing;
    /// impl Behavior for Zeroizing {
    ///     fn freeze(&self, buf: BytesMut) -> Bytes {
    ///         Bytes::from_owner(ZeroizeOnDrop(buf))
    ///     }
    /// }
    ///
    /// let mut builder = ByteVec::builder(1024).with_behavior(Zeroizing);
    /// builder.put_slice(b"secret");
    /// let secret = builder.finish();
    /// assert_eq!(secret, b"secret");
    /// ```
    pub fn with_behavior<B2: Behavior>(self, behavior: B2) -> Builder<B2> {
        Builder {
            chunks: self.chunks,
            head: self.head,
            capacity: self.capacity,
            inline_threshold: self.inline_threshold,
            behavior,
        }
    }

    /// Converts a completed [`BytesMut`] chunk into immutable [`Bytes`] via the configured
    /// [`Behavior`] (see [`Builder::with_behavior`]).
    #[inline]
    fn freeze_chunk(&self, buf: BytesMut) -> Bytes {
        self.behavior.freeze(buf)
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
            let buf = self.head.split_to(remaining);
            let chunk = self.freeze_chunk(buf);
            self.chunks.push_back(chunk);
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
        let Builder {
            mut chunks,
            head,
            behavior,
            ..
        } = self;
        if !head.is_empty() {
            chunks.push_back(behavior.freeze(head));
        }
        chunks
    }

    /// Calls the provided function and prefixes the written data with a `u64` big-endian length.
    pub fn write_with_len_prefix<Body: FnOnce(&mut Self)>(&mut self, f: Body) {
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
    pub fn for_socket_read<Cb: FnOnce(&mut bytes::buf::UninitSlice) -> usize>(
        &mut self,
        preferred_read_size: usize,
        f: Cb,
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
        // can only have initialized bytes within that slice. A reported length beyond it (a buggy or
        // hostile socket read claiming more than the buffer it was given) must not reach the unsafe
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
            let chunk = self.freeze_chunk(head);
            self.chunks.push_back(chunk);
        }
    }

    // flushes the current buffer, if non-empty
    fn flush(&mut self) {
        if !self.head.is_empty() {
            let buf = self.head.split();
            let chunk = self.freeze_chunk(buf);
            self.chunks.push_back(chunk);
        }
    }
}

impl From<ByteVec> for Builder<DefaultBehavior> {
    fn from(chunks: ByteVec) -> Self {
        Builder {
            chunks,
            head: BytesMut::new(),
            capacity: DEFAULT_CAPACITY,
            inline_threshold: DefaultBehavior.inline_threshold(),
            behavior: DefaultBehavior,
        }
    }
}

impl<B: Behavior> From<Builder<B>> for ByteVec {
    fn from(writer: Builder<B>) -> Self {
        writer.finish()
    }
}

impl<B: Behavior> writer::Buffer for Builder<B> {
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

    fn put_uninit_slice<Fill, Error>(&mut self, payload_len: usize, f: Fill) -> Result<bool, Error>
    where
        Fill: FnOnce(&mut bytes::buf::UninitSlice) -> Result<(), Error>,
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
        let chunk = self.freeze_chunk(bytes);
        self.chunks.push_back(chunk);
    }
}

impl<B: Behavior> reader::Buffer for Builder<B> {
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

    /// red reproducer (breaker-bytevec): `for_socket_read` is a safe fn that trusts the safe
    /// callback's returned length and feeds it to `unsafe BytesMut::advance_mut`. A callback that
    /// returns `len > preferred_read_size` (but within the head's spare capacity) commits
    /// uninitialized heap memory as rope content — safe code exposing uninit bytes (observed: a
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

    // The `Behavior` freeze is exercised through a module-global counter. Only this test's
    // `CountingBehavior` references `FREEZE_CALLS`, so there is no cross-test race under parallel runs.
    static FREEZE_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    #[derive(Debug)]
    struct CountingBehavior;
    impl Behavior for CountingBehavior {
        fn freeze(&self, buf: BytesMut) -> Bytes {
            FREEZE_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            buf.freeze()
        }
    }

    #[test]
    fn default_behavior_freeze_is_bytes_mut_freeze() {
        // A builder with the default behavior still produces correct content.
        let mut b = ByteVec::builder(1024);
        b.put_slice(b"plain");
        assert_eq!(b.finish(), b"plain");
    }

    #[test]
    fn custom_behavior_freeze_runs_at_every_chunk_boundary() {
        use std::sync::atomic::Ordering::SeqCst;
        FREEZE_CALLS.store(0, SeqCst);

        // Small capacity so a second head write overflows and flushes the first.
        let mut b = ByteVec::builder(4).with_behavior(CountingBehavior);

        b.put_slice(b"abcd"); // fills the 4-byte head
        b.put_slice(b"ef"); // overflow -> flush_and_reserve freezes "abcd"           (call 1)
        let front = b.split_to(5).unwrap(); // freezes head prefix "e" to reach index 5 (call 2)
        assert_eq!(front, b"abcde");
        b.put_bytes_mut(BytesMut::from(&b"XY"[..])); // flush freezes head "f" (call 3), then
        //                                              freezes the "XY" BytesMut chunk        (call 4)
        b.put_slice(b"gh"); // buffered in the (now empty) head
        let out = b.finish(); // finish freezes the trailing head "gh"                (call 5)
        assert_eq!(out, b"fXYgh");

        assert_eq!(
            FREEZE_CALLS.load(SeqCst),
            5,
            "the freeze behavior must run for every BytesMut the builder turns into a chunk"
        );
    }

    // Owner-drop tracking for the `Bytes::from_owner` zeroize-style behavior. Only this test and its
    // owner reference `OWNERS_DROPPED`.
    static OWNERS_DROPPED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    struct TrackedZeroize(BytesMut);
    impl AsRef<[u8]> for TrackedZeroize {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }
    impl Drop for TrackedZeroize {
        fn drop(&mut self) {
            self.0.fill(0);
            OWNERS_DROPPED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[derive(Debug)]
    struct Zeroizing;
    impl Behavior for Zeroizing {
        fn freeze(&self, buf: BytesMut) -> Bytes {
            Bytes::from_owner(TrackedZeroize(buf))
        }
    }

    #[test]
    fn from_owner_behavior_preserves_content_and_drops_owner() {
        use std::sync::atomic::Ordering::SeqCst;
        OWNERS_DROPPED.store(0, SeqCst);

        let secret = {
            let mut b = ByteVec::builder(1024).with_behavior(Zeroizing);
            b.put_slice(b"secret-key");
            let out = b.finish();
            // Content is identical to a plain freeze — the owner is transparent to readers.
            assert_eq!(out, b"secret-key");
            assert_eq!(
                OWNERS_DROPPED.load(SeqCst),
                0,
                "owner still alive while held"
            );
            out
        };
        // The chunk still holds the owner alive here.
        assert_eq!(secret, b"secret-key");
        assert_eq!(OWNERS_DROPPED.load(SeqCst), 0);

        drop(secret);
        // Dropping the last reference runs the zeroize-on-drop owner exactly once.
        assert_eq!(
            OWNERS_DROPPED.load(SeqCst),
            1,
            "the zeroize-on-drop owner must run when the last chunk drops"
        );
    }

    #[test]
    fn from_behavior_takes_capacity_and_threshold_from_the_behavior() {
        // A behavior can define every builder knob; `from_behavior` adopts capacity + threshold.
        #[derive(Debug)]
        struct BigInline;
        impl Behavior for BigInline {
            fn default_capacity(&self) -> usize {
                4096
            }
            fn inline_threshold(&self) -> usize {
                8
            }
        }

        let mut b = Builder::from_behavior(BigInline);
        // The inline threshold came from the behavior.
        assert_eq!(b.inline_threshold(), 8);
        // Chunks <= 8 bytes are copied into the single head buffer rather than referenced.
        b.put_bytes(Bytes::from_static(b"ab"));
        b.put_bytes(Bytes::from_static(b"cd"));
        let out = b.finish();
        assert_eq!(out, b"abcd");
        assert_eq!(out.chunks().len(), 1);
    }
}
