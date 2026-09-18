// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

/// A sink capable of being written into by a [`reader::Stream`](crate::reader::Stream).
pub trait Stream {
    fn read_from<R>(&mut self, reader: &mut R) -> Result<(), etude_buffer::Error<R::Error>>
    where
        R: crate::reader::Stream + ?Sized;
}
