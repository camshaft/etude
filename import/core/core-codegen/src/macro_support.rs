// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Support functions for macros.
//!
//! This module contains functions that are used by the macros in this crate.
//! They are not intended to be used directly by users of this crate.

// Re-export dependencies for proc macros
pub use linkme;
pub use proc_macro2::TokenStream;
pub use quote::quote;

pub trait UnidirectionalOutput {}

impl UnidirectionalOutput for () {}

/// Compile-time assertion that a unidirectional method returns `()`.
///
/// This function is used by the service macro to validate that methods marked with
/// `#[unidirectional]` return `()`.
pub fn assert_unidirectional_output<T: UnidirectionalOutput>() {}
