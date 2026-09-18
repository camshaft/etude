// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Test helpers.

pub mod data;
pub use data::Data;

pub mod fallible;
pub use fallible::Fallible;

#[cfg(any(test, feature = "generator"))]
use bolero_generator::prelude::*;

/// A fixed-capacity, stack-allocated vector of up to `LEN` elements.
///
/// Handy for property/model-checking harnesses (bolero / Kani) that need a bounded
/// collection with a `TypeGenerator`.
#[cfg(any(test, feature = "generator"))]
#[derive(Clone, Copy, Debug, TypeGenerator)]
pub struct InlineVec<T, const LEN: usize> {
    values: [T; LEN],

    #[generator(_code = "0..LEN")]
    len: usize,
}

#[cfg(any(test, feature = "generator"))]
impl<T, const LEN: usize> core::ops::Deref for InlineVec<T, LEN> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.values[..self.len]
    }
}

#[cfg(any(test, feature = "generator"))]
impl<T, const LEN: usize> core::ops::DerefMut for InlineVec<T, LEN> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.values[..self.len]
    }
}
