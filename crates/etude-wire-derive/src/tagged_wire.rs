// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! `#[derive(TaggedWire)]` — generates `TaggedSerialize` and `TaggedDeserialize`
//! implementations using TLV encoding, and `TaggedFields`, the tag and name of
//! each field or variant, in declaration order.
//!
//! Module structure (dependency graph is acyclic):
//!
//! ```text
//! tagged_wire.rs  → common, structs, enums    (entry point — thin dispatcher)
//! structs.rs      → common                    (struct codegen)
//! enums.rs        → common                    (enum codegen)
//! common.rs       → (nothing)                 (shared types and attribute parsing)
//! ```
//!
//! # Structs
//!
//! Each field must have `#[wire(tag = V.S)]`. The generated code emits/reads
//! `[tag_v: u8][tag_s: u8][length: u64][value bytes]` per field.
//!
//! Fields may optionally have `#[wire(tag = V.S, default = expr)]` to specify a
//! custom default value when the field is absent during deserialization. Without
//! `default`, the field type's `Default` implementation is used.
//!
//! # Enums
//!
//! Each variant must have `#[wire(tag = V.S)]`. The active variant is encoded as:
//! `[tag_v: u8][tag_s: u8][length: u64][variant payload bytes]`
//!
//! - Unit variants have an empty payload (length = 0).
//! - Tuple variants serialize fields positionally.
//! - Struct variants serialize named fields positionally (by declaration order).
//!
//! Unknown tags during deserialization are handled by a `#[wire(fallback)]` variant
//! if one exists. If no fallback is declared, an error is returned for unknown tags.
//!
//! A struct field's value, and a variant's payload, must consume the field's bytes whole: bytes left unread
//! fail the decode with `InvalidLength` naming the field's type (a payload's last field's), so a stored field
//! whose type changed is an error rather than a default with the stored value lost.

mod common;
mod enums;
mod structs;

use common::find_wire_crate;
use proc_macro2::TokenStream;
use syn::{Data, DeriveInput};

/// Entry point for the `#[derive(TaggedWire)]` macro.
pub fn derive(input: DeriveInput) -> TokenStream {
    let wire_path = find_wire_crate(&input.attrs);

    match &input.data {
        Data::Struct(data) => structs::derive_struct(&input, &wire_path, data),
        Data::Enum(data) => enums::derive_enum(&input, &wire_path, data),
        Data::Union(_) => {
            syn::Error::new_spanned(&input, "TaggedWire does not support unions").to_compile_error()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    #[test]
    fn struct_default_crate_path() {
        let input = parse_quote! { struct Test { #[wire(tag = 1.1)] value: u32 } };
        let s = derive(input).to_string();
        assert!(
            s.contains("etude_wire ::"),
            "should use the default crate path etude_wire"
        );
        assert!(!s.contains("my_wire"), "should not use a custom path");
    }
}
