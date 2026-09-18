// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Copy-avoiding byte reader/writer buffer traits.
//!
//! This crate defines the `reader` and `writer` abstractions used to move bytes between
//! buffers while avoiding copies wherever possible:
//!
//! - [`reader::Storage`] / [`writer::Storage`] — the low-level chunk-oriented traits,
//!   plus a family of adapters (`limit`, `chain`, `tracked`, …).
//! - [`Reader`] / [`Writer`] — offset-aware streaming views layered on top of `Storage`.
//!
//! Offsets are plain `u64`; callers that use a narrower on-the-wire encoding convert at
//! their own boundary.

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
pub use reader::Reader;
pub use writer::Writer;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
