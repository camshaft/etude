// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! SHA-256 as a `const fn`.
//!
//! [`Sha256`]'s `new`, `update` and `finalize` are `const fn`, so the compiler
//! computes the digest of static data, such as a stable id of a type or a
//! method, at no runtime cost. The crate is `no_std`, uses only `core` and has
//! no dependencies.

#![no_std]
#![deny(missing_docs)]
#![allow(clippy::new_without_default)]

mod constants;
mod sha;
mod util;

use constants::H256;
use util::memcpy;

macro_rules! sha {
    (
        $(#[$doc:meta])* $name:ident,
        $size:literal,
        $inner:ty,
        $iv:ident
    ) => {
        $(#[$doc])*
        #[derive(Clone)]
        pub struct $name {
            inner: $inner,
        }

        impl $name {
            /// The internal block size of the hash function.
            pub const BLOCK_SIZE: usize = <$inner>::BLOCK_SIZE;
            /// The digest size of the hash function.
            pub const DIGEST_SIZE: usize = $size;

            /// Construct a new instance.
            pub const fn new() -> Self {
                Self {
                    inner: <$inner>::new($iv),
                }
            }

            /// Add input data to the hash context.
            #[must_use]
            pub const fn update(mut self, input: &[u8]) -> Self {
                self.inner.update(input);
                self
            }

            /// Finalize the context and compute the digest.
            #[must_use]
            pub const fn finalize(self) -> [u8; Self::DIGEST_SIZE] {
                let digest = self.inner.finalize();
                let mut truncated = [0; Self::DIGEST_SIZE];
                memcpy(&mut truncated, 0, &digest, 0, Self::DIGEST_SIZE);
                truncated
            }

            /// Finalize the context and return the digest's first 8 bytes as a
            /// big-endian `u64`.
            #[must_use]
            pub const fn finalize_u64(self) -> u64 {
                let [a, b, c, d, e, f, g, h, ..] = self.finalize();
                u64::from_be_bytes([a, b, c, d, e, f, g, h])
            }
        }
    };
}

sha!(
    /// The SHA-256 hash function.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use etude_sha2_const::Sha256;
    /// const DIGEST: [u8; 32] = Sha256::new()
    ///     .update(b"The quick brown fox ")
    ///     .update(b"jumps over the lazy dog")
    ///     .finalize();
    /// ```
    Sha256,
    32,
    sha::Sha256,
    H256
);
