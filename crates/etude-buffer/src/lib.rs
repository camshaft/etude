// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Copy-avoiding byte reader/writer buffer traits.
//!
//! This crate defines the chunk-oriented [`reader::Buffer`] and [`writer::Buffer`] traits
//! used to move bytes between buffers while avoiding copies wherever possible, along with a
//! family of adapters (`chain`, `tracked`, `io_slice`, …) and, under the `testing` feature,
//! a stream-data model and generators for property/fuzz tests.
//!
//! The traits are cursor-free: anything that tracks offsets (stream position, final offset)
//! layers that on top downstream.

#![cfg_attr(not(any(test, feature = "std")), no_std)]

extern crate alloc;

#[cfg(any(test, feature = "std"))]
extern crate std;

#[macro_use]
extern crate etude_ensure;

mod error;
pub mod reader;
pub mod writer;

#[cfg(test)]
mod slice;

pub use error::Error;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
