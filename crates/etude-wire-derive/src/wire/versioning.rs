// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Shared versioning logic for the Wire derive macro.
//!
//! Provides tag validation, version range computation, and trait emission
//! that is reused by both struct and enum versioned codegen.
//!
//! Type-specific logic (struct field TYPE trees, enum variant TYPE trees,
//! per-version serialize/deserialize bodies) lives in structs.rs and enums.rs.

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, format_ident, quote};
use syn::Path;

use super::common::{ItemWireAttrs, build_where_clauses};

/// Check if a type is `Option<T>` by looking at the path segments.
pub(super) fn is_option_type(ty: &syn::Type) -> bool {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident == "Option";
        }
    }
    false
}

/// Computed version metadata after validation and sorting.
///
/// Generic over the item type: `&syn::Field` for structs, `&syn::Variant` for enums.
pub(super) struct VersionedLayout<T> {
    /// Wire items sorted by (version, sequence). Does not include skip items.
    pub sorted_items: Vec<(T, ItemWireAttrs)>,
    /// Items with `#[wire(skip)]` — not on the wire, always `Default::default()`.
    pub skip_items: Vec<(T, ItemWireAttrs)>,
    pub min_version: u32,
    pub max_version: u32,
}

/// Validate tags, sort items by tag, and compute version range.
///
/// Shared validation for both struct fields and enum variants:
/// - Separates skip items (no tag required)
/// - All wire items must have tags (no mixed tagged/untagged)
/// - Tags must be unique
/// - Sort by (version, sequence) to determine wire order / discriminant order
///
/// Type-specific validation (struct: default required for v>1,
/// enum: fallback required for v>1) is done by the caller after this returns.
pub(super) fn validate_and_sort<T: ToTokens>(
    parsed_items: Vec<(T, ItemWireAttrs)>,
    min_version_attr: Option<u32>,
) -> syn::parse::Result<VersionedLayout<T>> {
    // Separate skip items from wire items.
    let mut wire_items = Vec::new();
    let mut skip_items = Vec::new();
    for (item, attrs) in parsed_items {
        if attrs.skip {
            skip_items.push((item, attrs));
        } else {
            wire_items.push((item, attrs));
        }
    }

    // Validate: all wire items must have tags (no mixed tagged/untagged)
    for (item, attrs) in &wire_items {
        if attrs.tag.is_none() {
            return Err(syn::Error::new_spanned(
                item,
                "all items in a versioned type must have a #[wire(tag = V.S)] annotation",
            ));
        }
    }

    // Validate tag uniqueness
    let mut seen_tags = std::collections::HashSet::new();
    for (item, attrs) in &wire_items {
        let tag = attrs.tag.as_ref().unwrap();
        let key = (tag.version, tag.sequence);
        if !seen_tags.insert(key) {
            return Err(syn::Error::new_spanned(
                item,
                format!("duplicate wire tag {}.{}", tag.version, tag.sequence),
            ));
        }
    }

    // Sort by (version, sequence)
    wire_items.sort_by_key(|(_, attrs)| {
        let tag = attrs.tag.as_ref().unwrap();
        (tag.version, tag.sequence)
    });

    let max_version = wire_items
        .iter()
        .map(|(_, attrs)| attrs.tag.as_ref().unwrap().version)
        .max()
        .unwrap_or(1);
    let min_version = min_version_attr.unwrap_or(1);

    if min_version > max_version {
        return Err(syn::Error::new(
            Span::call_site(),
            format!(
                "min_version ({min_version}) exceeds max version ({max_version}) derived from tags"
            ),
        ));
    }

    Ok(VersionedLayout {
        sorted_items: wire_items,
        skip_items,
        min_version,
        max_version,
    })
}

/// Per-version TYPE tree constant and its entry for the VERSION_TYPES array.
pub(super) struct VersionTypeConst {
    pub const_def: TokenStream,
    pub array_entry: TokenStream,
}

/// Generate `__wire_serialize_at` and `__wire_deserialize_at` dispatch functions.
///
/// These allow an outer versioned type to call into an inner versioned type's
/// serializer/deserializer by version index (0-based into VERSION_TYPES).
/// The `#[inline(always)]` guarantees zero dispatch overhead when the caller
/// passes a constant literal index — the compiler resolves the match at compile time.
fn generate_version_dispatch(
    wire_path: &Path,
    min_version: u32,
    max_version: u32,
) -> (TokenStream, TokenStream) {
    let arms_ser: Vec<_> = (min_version..=max_version)
        .enumerate()
        .map(|(idx, ver)| {
            let idx = idx as u32;
            let fn_name = format_ident!("__wire_serialize_v{}", ver);
            quote! { #idx => self.#fn_name(out), }
        })
        .collect();

    let arms_de: Vec<_> = (min_version..=max_version)
        .enumerate()
        .map(|(idx, ver)| {
            let idx = idx as u32;
            let fn_name = format_ident!("__wire_deserialize_v{}", ver);
            quote! { #idx => Self::#fn_name(bytes), }
        })
        .collect();

    let ser_at = quote! {
        /// Dispatch to the version-specific serializer by VERSION_TYPES index.
        ///
        /// When called with a constant literal index, the compiler inlines the
        /// target function and eliminates the match entirely.
        #[doc(hidden)]
        #[inline(always)]
        #[allow(dead_code)]
        fn __wire_serialize_at<__WireBehavior: #wire_path::bytevec::Behavior>(self, out: &mut #wire_path::bytevec::Builder<__WireBehavior>, version_idx: u32) {
            match version_idx {
                #(#arms_ser)*
                _ => unreachable!("invalid version index"),
            }
        }
    };

    let de_at = quote! {
        /// Dispatch to the version-specific deserializer by VERSION_TYPES index.
        ///
        /// When called with a constant literal index, the compiler inlines the
        /// target function and eliminates the match entirely.
        #[doc(hidden)]
        #[inline(always)]
        #[allow(dead_code)]
        fn __wire_deserialize_at(bytes: &mut #wire_path::bytevec::ByteVec, version_idx: u32)
            -> Result<Self, #wire_path::de::Error>
        {
            match version_idx {
                #(#arms_de)*
                _ => unreachable!("invalid version index"),
            }
        }
    };

    (ser_at, de_at)
}

/// Generate `__wire_serialize_ref_at`, the by-reference twin of
/// `__wire_serialize_at`, dispatching to `__wire_serialize_ref_vN`.
pub(super) fn generate_ser_ref_dispatch(
    wire_path: &Path,
    min_version: u32,
    max_version: u32,
) -> TokenStream {
    let arms: Vec<_> = (min_version..=max_version)
        .enumerate()
        .map(|(idx, ver)| {
            let idx = idx as u32;
            let fn_name = format_ident!("__wire_serialize_ref_v{}", ver);
            quote! { #idx => self.#fn_name(out), }
        })
        .collect();

    quote! {
        #[doc(hidden)]
        #[inline(always)]
        #[allow(dead_code)]
        fn __wire_serialize_ref_at<__WireBehavior: #wire_path::bytevec::Behavior>(&self, out: &mut #wire_path::bytevec::Builder<__WireBehavior>, version_idx: u32) {
            match version_idx {
                #(#arms)*
                _ => unreachable!("invalid version index"),
            }
        }
    }
}

/// Emit the Wire and VersionedWire trait impls for a versioned type.
///
/// The caller provides the Serialize and Deserialize impls separately since
/// those are type-specific (struct vs. enum have different constructor syntax).
pub(super) fn emit_versioned_wire_impls(
    wire_path: &Path,
    name: &syn::Ident,
    generics: &syn::Generics,
    min_version: u32,
    max_version: u32,
    type_consts: &[VersionTypeConst],
    ser_fns: Vec<TokenStream>,
    de_fns: Vec<TokenStream>,
    ser_trait_impl: TokenStream,
    de_trait_impl: TokenStream,
) -> TokenStream {
    let (impl_generics, ty_generics, _) = generics.split_for_impl();
    let (wire_wc, ser_wc, de_wc) = build_where_clauses(wire_path, generics);

    let const_defs: Vec<_> = type_consts.iter().map(|c| &c.const_def).collect();
    let array_entries: Vec<_> = type_consts.iter().map(|c| &c.array_entry).collect();

    let latest_type = format_ident!("__WIRE_TYPE_V{}", max_version);
    let min_ver_lit = min_version;
    let max_ver_lit = max_version;

    let (ser_at_fn, de_at_fn) = generate_version_dispatch(wire_path, min_version, max_version);

    quote! {
        impl #impl_generics #name #ty_generics #wire_wc {
            #(#const_defs)*
        }

        impl #impl_generics #name #ty_generics #ser_wc {
            #(#ser_fns)*
            #ser_at_fn
        }

        impl #impl_generics #name #ty_generics #de_wc {
            #(#de_fns)*
            #de_at_fn
        }

        #ser_trait_impl
        #de_trait_impl

        impl #impl_generics #wire_path::Wire for #name #ty_generics #wire_wc {
            const TYPE: #wire_path::Type = Self::#latest_type;
            const VERSION_TYPES: &'static [#wire_path::Type] = &[#(#array_entries),*];
            // Enums don't have versioned inner fields, so xproduct count = version count
            const VERSION_XPRODUCT_COUNT: usize = { #max_ver_lit - #min_ver_lit + 1 } as usize;
            // Enums: one hash entry per outer version, from VERSION_TYPES
            const __XPRODUCT_HASH_ENTRIES: &'static [([u8; #wire_path::XPRODUCT_HASH_BUF_SIZE], u16)] = {
                let types: &[#wire_path::Type] = Self::VERSION_TYPES;
                let n = types.len();
                let mut entries = [([0u8; #wire_path::XPRODUCT_HASH_BUF_SIZE], 0u16); { #max_ver_lit - #min_ver_lit + 1 } as usize];
                let mut i = 0;
                while i < n {
                    let mut buf = [0u8; #wire_path::XPRODUCT_HASH_BUF_SIZE];
                    let mut cur = 0usize;
                    types[i].serialize_for_hash(&mut buf, &mut cur);
                    entries[i] = (buf, cur as u16);
                    i += 1;
                }
                &{ entries }
            };
        }

        impl #impl_generics #wire_path::VersionedWire for #name #ty_generics #wire_wc {
            const MIN_VERSION: u32 = #min_ver_lit;
            const MAX_VERSION: u32 = #max_ver_lit;
        }
    }
}
