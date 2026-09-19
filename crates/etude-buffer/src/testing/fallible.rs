// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! A test adapter that injects read errors into an infallible reader.

use crate::reader::{Buffer, Infallible as _};
use core::convert::Infallible;

/// Wraps an infallible reader so its reads can be made to fail on demand, for exercising a
/// caller's error paths.
///
/// While an error is armed, every read returns it (by clone); with no error armed, reads pass
/// through to the wrapped reader.
pub struct Fallible<'a, R, E>
where
    R: ?Sized + Buffer<Error = Infallible>,
    E: 'static + Clone,
{
    inner: &'a mut R,
    error: Option<E>,
}

impl<'a, R, E> Fallible<'a, R, E>
where
    R: ?Sized + Buffer<Error = Infallible>,
    E: 'static + Clone,
{
    /// Wraps `inner` with no error armed; reads pass through until an error is set.
    #[inline]
    pub fn new(inner: &'a mut R) -> Self {
        Self { inner, error: None }
    }

    /// Arms `error`, consuming and returning `self` for chaining at construction.
    #[inline]
    pub fn with_error(mut self, error: E) -> Self {
        self.error = Some(error);
        self
    }

    /// Arms or clears the error returned by subsequent reads: `Some` to fail, `None` to pass
    /// through.
    #[inline]
    pub fn set_error(&mut self, error: Option<E>) {
        self.error = error;
    }

    #[inline]
    fn check_error(&self) -> Result<(), E> {
        if let Some(error) = self.error.as_ref() {
            Err(error.clone())
        } else {
            Ok(())
        }
    }
}

impl<R, E> Buffer for Fallible<'_, R, E>
where
    R: ?Sized + Buffer<Error = Infallible>,
    E: 'static + Clone,
{
    type Error = E;

    #[inline]
    fn buffered_len(&self) -> usize {
        self.inner.buffered_len()
    }

    #[inline]
    fn buffer_is_empty(&self) -> bool {
        self.inner.buffer_is_empty()
    }

    #[inline]
    fn read_chunk(&mut self, watermark: usize) -> Result<crate::reader::Chunk<'_>, Self::Error> {
        self.check_error()?;
        let chunk = self.inner.infallible_read_chunk(watermark);
        Ok(chunk)
    }

    #[inline]
    fn partial_copy_into<Dest>(
        &mut self,
        dest: &mut Dest,
    ) -> Result<crate::reader::Chunk<'_>, Self::Error>
    where
        Dest: crate::writer::Buffer + ?Sized,
    {
        self.check_error()?;
        let chunk = self.inner.infallible_partial_copy_into(dest);
        Ok(chunk)
    }

    #[inline]
    fn copy_into<Dest>(&mut self, dest: &mut Dest) -> Result<(), Self::Error>
    where
        Dest: crate::writer::Buffer + ?Sized,
    {
        self.check_error()?;
        self.inner.infallible_copy_into(dest);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::Chunk;

    #[test]
    fn fallible_test() {
        let mut reader = Chunk::Slice(b"hello");
        let mut reader = Fallible::new(&mut reader).with_error("unexpected eof");

        assert!(reader.read_chunk(1).is_err());
    }
}
