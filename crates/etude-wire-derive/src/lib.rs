// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Derive macros for `etude-wire`.
//!
//! - `#[derive(Wire)]` implements `etude_wire`'s positional `Serialize`, `Deserialize` and
//!   `Wire` for a struct or an enum, with the versions its `#[wire(...)]` attributes declare.
//! - `#[derive(TaggedWire)]` implements `etude_wire`'s tag-length-value encoding, with a
//!   `#[wire(tag = V.S)]` tag on each field of a struct or each variant of an enum.
//!
//! The generated code refers to the `etude_wire` crate by that name. Where the crate is renamed
//! or re-exported, give its path with `#[wire(crate = path)]` on the item.

mod tagged_wire;
mod wire;

use proc_macro::TokenStream;
use syn::parse_macro_input;

/// Derives Serialize and Deserialize traits for a struct or an enum.
///
/// # Example
/// ```ignore
/// use etude_wire::Wire;
///
/// #[derive(Wire)]
/// struct Message {
///     id: u64,
///     payload: Vec<u8>,
/// }
/// ```
#[proc_macro_derive(Wire, attributes(wire))]
pub fn derive_wire(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input);
    TokenStream::from(wire::derive_wire(input))
}

/// Derives the TaggedSerialize and TaggedDeserialize traits, the tag-length-value encoding,
/// for a struct or an enum.
///
/// Each field of a struct, and each variant of an enum, gives its tag as `#[wire(tag = V.S)]`.
#[proc_macro_derive(TaggedWire, attributes(wire))]
pub fn derive_tagged_wire(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input);
    TokenStream::from(tagged_wire::derive(input))
}
