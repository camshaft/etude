// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use crate::{reader::Buffer, writer};
use bytes::{Bytes, BytesMut};

/// A contiguous run of bytes handed out by a reader.
///
/// A chunk is either borrowed from the source or an owned reference-counted buffer, so a caller
/// can forward it to a writer — deferring or avoiding a copy — rather than copying it eagerly.
/// It is `#[must_use]` because dropping a chunk without placing it discards its bytes.
#[derive(Clone, Debug)]
#[must_use = "Chunk should not be discarded"]
pub enum Chunk<'a> {
    /// Bytes borrowed directly from the source.
    Slice(&'a [u8]),
    /// An owned, reference-counted [`Bytes`] buffer, adoptable without copying.
    Bytes(Bytes),
    /// An owned, mutable [`BytesMut`] buffer, adoptable without copying.
    BytesMut(BytesMut),
}

impl Default for Chunk<'_> {
    #[inline]
    fn default() -> Self {
        Self::empty()
    }
}

impl Chunk<'_> {
    /// Returns an empty chunk (a borrowed empty slice).
    #[inline]
    pub fn empty() -> Self {
        Self::Slice(&[])
    }
}

impl<'a> From<&'a [u8]> for Chunk<'a> {
    #[inline]
    fn from(chunk: &'a [u8]) -> Self {
        Self::Slice(chunk)
    }
}

impl From<Bytes> for Chunk<'_> {
    #[inline]
    fn from(chunk: Bytes) -> Self {
        Self::Bytes(chunk)
    }
}

impl From<BytesMut> for Chunk<'_> {
    #[inline]
    fn from(chunk: BytesMut) -> Self {
        Self::BytesMut(chunk)
    }
}

impl core::ops::Deref for Chunk<'_> {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Slice(t) => t,
            Self::Bytes(t) => t,
            Self::BytesMut(t) => t,
        }
    }
}

impl AsRef<[u8]> for Chunk<'_> {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl Buffer for Chunk<'_> {
    type Error = core::convert::Infallible;

    #[inline]
    fn buffered_len(&self) -> usize {
        self.len()
    }

    #[inline]
    fn read_chunk(&mut self, watermark: usize) -> Result<Chunk<'_>, Self::Error> {
        match self {
            Self::Slice(v) => v.read_chunk(watermark),
            Self::Bytes(v) => v.read_chunk(watermark),
            Self::BytesMut(v) => v.read_chunk(watermark),
        }
    }

    #[inline]
    fn partial_copy_into<Dest>(&mut self, dest: &mut Dest) -> Result<Chunk<'_>, Self::Error>
    where
        Dest: writer::Buffer + ?Sized,
    {
        match self {
            Self::Slice(v) => v.partial_copy_into(dest),
            Self::Bytes(v) => v.partial_copy_into(dest),
            Self::BytesMut(v) => v.partial_copy_into(dest),
        }
    }

    #[inline]
    fn copy_into<Dest>(&mut self, dest: &mut Dest) -> Result<(), Self::Error>
    where
        Dest: writer::Buffer + ?Sized,
    {
        match self {
            Self::Slice(v) => v.copy_into(dest),
            Self::Bytes(v) => v.copy_into(dest),
            Self::BytesMut(v) => v.copy_into(dest),
        }
    }
}
