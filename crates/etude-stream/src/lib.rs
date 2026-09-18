// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Offset-aware stream reader/writer traits.
//!
//! Where [`etude_buffer`] defines the cursor-free [`Buffer`](etude_buffer::reader::Buffer)
//! traits for pushing bytes around, this crate layers a tracked *offset* on top:
//!
//! - [`reader::Stream`] — a [`Buffer`](etude_buffer::reader::Buffer) that also reports its
//!   current and final offsets, with adapters (`checked`, `complete`, `incremental`,
//!   `limit`).
//! - [`writer::Stream`] — a sink that can pull from a [`reader::Stream`].
//!
//! Offsets are plain `u64`.

#![cfg_attr(not(any(test, feature = "std")), no_std)]

extern crate alloc;

#[cfg(any(test, feature = "std"))]
extern crate std;

#[macro_use]
extern crate etude_ensure;

pub mod reader;
pub mod writer;

#[cfg(any(test, feature = "testing"))]
pub mod testing;
