// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use crate::{
    reader::{Buffer, Chunk},
    writer,
};

// unwrapping an infallible error doesn't panic
// https://godbolt.org/z/7v5MWdvGa

/// [`Buffer`] implementation that cannot fail
pub trait Infallible: Buffer<Error = core::convert::Infallible> {
    #[inline(always)]
    fn infallible_read_chunk(&mut self, watermark: usize) -> Chunk<'_> {
        self.read_chunk(watermark).unwrap()
    }

    #[inline(always)]
    fn infallible_partial_copy_into<Dest>(&mut self, dest: &mut Dest) -> Chunk<'_>
    where
        Dest: writer::Buffer + ?Sized,
    {
        self.partial_copy_into(dest).unwrap()
    }

    #[inline]
    fn infallible_copy_into<Dest>(&mut self, dest: &mut Dest)
    where
        Dest: writer::Buffer + ?Sized,
    {
        self.copy_into(dest).unwrap()
    }
}

impl<T> Infallible for T where T: Buffer<Error = core::convert::Infallible> + ?Sized {}
