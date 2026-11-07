// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Struct codegen for the Wire derive macro.
//!
//! Handles both simple (non-versioned) and versioned struct codegen.
//! Simple structs produce plain Serialize/Deserialize/Wire impls.
//! Versioned structs (with `#[wire(tag = V.S)]`) produce per-version
//! serializers, deserializers, TYPE trees, and a VersionedWire impl.
//!
//! # Design motivation
//!
//! An alternative to cross-product dispatch would negotiate each struct's
//! version independently — the server and client agree on a version for
//! `ReadRequest`, a separate version for `ConsistencyConfig`, etc. This
//! avoids the cross-product but requires per-type version negotiation
//! state and loses a property we want to preserve: each RPC method is
//! identified by a single `global_id` hash derived from the complete Type
//! tree of its input. The `global_id` encodes the exact wire layout —
//! which fields are present, in what order, at which inner versions —
//! into one 64-bit value. Clients and servers negotiate by exchanging
//! these hashes, and a hash match guarantees both sides agree on the
//! exact byte layout without any additional version metadata on the wire.
//! The cross-product enumeration preserves this property: each combination
//! gets its own `global_id`, and the service layer maps
//! `global_id → (method, input_version_idx)` for dispatch.
//!
//! # Cross-product dispatch
//!
//! When a versioned struct contains versioned inner types, each combination
//! of outer version × inner field versions forms a distinct wire layout.
//! The derive macro assigns each combination a flat integer index and
//! generates dispatch code to decompose it.
//!
//! Consider a struct with 2 outer versions:
//!
//! ```text
//! struct Req {
//!     #[wire(tag = 1.1)] key: u64,                  // non-versioned
//!     #[wire(tag = 2.1)] config: Option<Config>,     // Config has 4 xp entries
//! }
//! ```
//!
//! The cross-product layout has two segments:
//! - Outer v1 (only `key` present): 1 entry (idx 0)
//! - Outer v2 (`key` + `config`): 1 × 4 = 4 entries (idx 1..4)
//! - Total: 5 entries
//!
//! For outer v2, the dispatch decomposes the local offset within the
//! segment into per-field indices using mixed-radix div/mod arithmetic.
//! Each field's divisor is the product of subsequent fields' cross-product
//! counts. For example, with fields A (4 versions), B (2 versions),
//! C (3 versions), flat index 17 decomposes as:
//!
//! ```text
//! A_idx = 17 / (2 × 3) = 2,  remainder = 5
//! B_idx = 5  / 3       = 1,  remainder = 2
//! C_idx = 2
//! ```
//!
//! Then calls `A.serialize_at(out, 2)`, `B.serialize_at(out, 1)`,
//! `C.serialize_at(out, 2)`, which recurse into each inner type's own
//! dispatch. This composes recursively through any nesting depth.
//!
//! # Inlining strategy
//!
//! All segment boundaries, strides, and offsets in the dispatch are
//! wrapped in `const { }` blocks, forcing the Rust compiler to evaluate
//! them as literal integers before LLVM sees the IR. The inherent method
//! `__wire_serialize_at` uses `#[inline(always)]` so it is always inlined
//! into the Serialize trait's `serialize_at`. The trait method uses
//! `#[inline]` (hint, not force), letting LLVM decide whether to inline
//! it into each call site. When the service codegen calls `serialize_at`
//! with a constant index, LLVM inlines both levels, evaluates all
//! `if LITERAL < LITERAL` comparisons at compile time, eliminates dead
//! branches, and folds `LITERAL / LITERAL` to a single value at each
//! level. The result is a monomorphic serializer with zero dispatch
//! overhead — assembly analysis confirms zero `callq` instructions to any
//! `serialize_at` or `__wire_serialize_at` function in the hot path.
//!
//! # Backward compatibility
//!
//! Flat indices change when a new version is added to any type in the
//! hierarchy. For example, adding Mode v3 changes NestedCL's
//! `VERSION_XPRODUCT_COUNT` from 4 to 6 and shifts all index-to-layout
//! mappings. This does not disrupt communication between nodes running
//! different software because nodes never exchange flat indices on the
//! wire.
//!
//! Each node independently builds a `global_id → (method,
//! input_version_idx)` mapping at startup from its own compiled
//! cross-product space. During negotiation, nodes exchange `global_id`
//! hashes and intersect their supported sets. The hash for each
//! pre-existing version combination is identical on both old and new
//! nodes (same Type tree produces the same hash), so existing
//! combinations appear in the intersection. New combinations (using
//! added versions) are excluded when the peer does not support them.
//! After negotiation, each side resolves the agreed hash to its own
//! local index. A hash match guarantees identical byte layout without
//! additional version metadata on the wire.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::Path;

use super::{
    common::{
        ItemKind, ItemWireAttrs, build_ser_ref_where_clause, build_where_clauses,
        emit_serialize_ref_impl, emit_wire_impls, emit_wire_impls_with_at, item_type_token,
        ser_fn_shape,
    },
    versioning::{self, VersionedLayout, is_option_type},
};

/// Entry point for struct codegen. Parses field attrs, then dispatches to
/// simple or versioned path based on whether any field has a tag.
pub(super) fn derive(
    wire_path: &Path,
    name: &syn::Ident,
    generics: &syn::Generics,
    data: &syn::DataStruct,
    min_version: Option<u32>,
    schema_override: Option<&syn::Type>,
    borrowed: bool,
) -> syn::parse::Result<TokenStream> {
    // Parse all struct field attrs in a single pass
    let mut parsed_fields = Vec::new();
    for field in &data.fields {
        let attrs = ItemWireAttrs::parse(&field.attrs)?;
        parsed_fields.push((field, attrs));
    }

    let kind = ItemKind::from(&data.fields);
    let has_tags = parsed_fields.iter().any(|(_, a)| a.tag.is_some());

    if has_tags {
        emit_versioned(
            wire_path,
            name,
            generics,
            parsed_fields,
            min_version,
            borrowed,
        )
    } else {
        let mut tokens = emit_simple(
            wire_path,
            name,
            generics,
            kind,
            &parsed_fields,
            schema_override,
        );
        if borrowed {
            tokens.extend(emit_simple_ser_ref(
                wire_path,
                name,
                generics,
                &parsed_fields,
            ));
        }
        Ok(tokens)
    }
}

/// Generate the `SerializeRef` impl for a non-versioned `#[wire(borrowed)]`
/// struct. Mirrors `emit_simple`'s serializer, borrowing each field.
fn emit_simple_ser_ref(
    wire_path: &Path,
    name: &syn::Ident,
    generics: &syn::Generics,
    parsed_fields: &[(&syn::Field, ItemWireAttrs)],
) -> TokenStream {
    let field_access = |i: usize, f: &syn::Field| match &f.ident {
        Some(name) => quote! { &self.#name },
        None => {
            let idx = syn::Index::from(i);
            quote! { &self.#idx }
        }
    };

    let fields: Vec<_> = parsed_fields
        .iter()
        .enumerate()
        .filter(|(_, (_, attrs))| !attrs.skip)
        .map(|(i, (f, _))| field_access(i, f))
        .collect();

    let body = quote! { #(<_ as #wire_path::Serialize>::serialize(#fields, out);)* };
    let at_body =
        quote! { #(<_ as #wire_path::Serialize>::serialize_at(#fields, out, version_idx);)* };

    emit_serialize_ref_impl(wire_path, name, generics, body, Some(at_body))
}

/// Generate codegen for non-versioned structs.
fn emit_simple(
    wire_path: &Path,
    name: &syn::Ident,
    generics: &syn::Generics,
    kind: ItemKind,
    parsed_fields: &[(&syn::Field, ItemWireAttrs)],
    schema_override: Option<&syn::Type>,
) -> TokenStream {
    if matches!(kind, ItemKind::Unit) {
        let id = quote!(#wire_path::Type::STRUCT);
        return emit_wire_impls(
            wire_path,
            name,
            generics,
            &id,
            quote! {},
            quote! { Ok(Self) },
            quote! {},
            schema_override,
        );
    }

    let is_named = matches!(kind, ItemKind::Named);

    let ser_fields: Vec<_> = parsed_fields
        .iter()
        .enumerate()
        .filter(|(_, (_, attrs))| !attrs.skip)
        .map(|(i, (f, _))| {
            if let Some(name) = &f.ident {
                quote! { <_ as #wire_path::Serialize>::serialize(self.#name, out); }
            } else {
                let idx = syn::Index::from(i);
                quote! { <_ as #wire_path::Serialize>::serialize(self.#idx, out); }
            }
        })
        .collect();

    // Generate serialize_at fields that propagate version_idx to inner types.
    // This is needed so that non-versioned wrapper structs (e.g., the generated
    // request::Read { pub request: ReadRequest }) forward the version index to
    // their versioned inner fields.
    let ser_at_fields: Vec<_> = parsed_fields
        .iter()
        .enumerate()
        .filter(|(_, (_, attrs))| !attrs.skip)
        .map(|(i, (f, _))| {
            if let Some(name) = &f.ident {
                quote! { <_ as #wire_path::Serialize>::serialize_at(self.#name, out, version_idx); }
            } else {
                let idx = syn::Index::from(i);
                quote! { <_ as #wire_path::Serialize>::serialize_at(self.#idx, out, version_idx); }
            }
        })
        .collect();

    let field_types: Vec<_> = parsed_fields
        .iter()
        .filter_map(|(f, attrs)| item_type_token(wire_path, &f.ty, attrs))
        .collect();

    let id = quote!(#wire_path::Type::STRUCT);

    if is_named {
        let de_fields: Vec<_> = parsed_fields
            .iter()
            .map(|(f, attrs)| {
                let name = f.ident.as_ref().unwrap();
                let ty = &f.ty;
                if attrs.skip {
                    quote! { #name: Default::default() }
                } else {
                    quote! { #name: <#ty as #wire_path::Deserialize>::deserialize(bytes)? }
                }
            })
            .collect();

        let de_at_fields: Vec<_> = parsed_fields
            .iter()
            .map(|(f, attrs)| {
                let name = f.ident.as_ref().unwrap();
                let ty = &f.ty;
                if attrs.skip {
                    quote! { #name: Default::default() }
                } else {
                    quote! { #name: <#ty as #wire_path::Deserialize>::deserialize_at(bytes, version_idx)? }
                }
            })
            .collect();

        emit_wire_impls_with_at(
            wire_path,
            name,
            generics,
            &id,
            quote! { #(#ser_fields)* },
            quote! { Ok(Self { #(#de_fields,)* }) },
            Some(quote! { #(#ser_at_fields)* }),
            Some(quote! { Ok(Self { #(#de_at_fields,)* }) }),
            quote! { #(#field_types),* },
            schema_override,
        )
    } else {
        let de_fields: Vec<_> = parsed_fields
            .iter()
            .map(|(f, _)| {
                let ty = &f.ty;
                quote! { <#ty as #wire_path::Deserialize>::deserialize(bytes)? }
            })
            .collect();

        let de_at_fields: Vec<_> = parsed_fields
            .iter()
            .map(|(f, _)| {
                let ty = &f.ty;
                quote! { <#ty as #wire_path::Deserialize>::deserialize_at(bytes, version_idx)? }
            })
            .collect();

        emit_wire_impls_with_at(
            wire_path,
            name,
            generics,
            &id,
            quote! { #(#ser_fields)* },
            quote! { Ok(Self(#(#de_fields,)*)) },
            Some(quote! { #(#ser_at_fields)* }),
            Some(quote! { Ok(Self(#(#de_at_fields,)*)) }),
            quote! { #(#field_types),* },
            schema_override,
        )
    }
}

// ── Cross-product helpers ─────────────────────────────────────────

/// Extract the inner type T from Option<T>, returning None if not Option.
fn extract_option_inner(ty: &syn::Type) -> Option<&syn::Type> {
    if let syn::Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            if segment.ident == "Option" {
                if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                    if let Some(syn::GenericArgument::Type(inner)) = args.args.first() {
                        return Some(inner);
                    }
                }
            }
        }
    }
    None
}

/// Get the inner type to use for VERSION_TYPES indexing. For Option<T>, returns T
/// (since Option's own VERSION_TYPES is always [1]; the interesting versioning is in T).
/// For other types, returns as-is.
fn version_types_inner_ty(ty: &syn::Type) -> &syn::Type {
    extract_option_inner(ty).unwrap_or(ty)
}

/// Generate the cross-product version count expression for a field type.
///
/// Returns `VERSION_XPRODUCT_COUNT` — the total number of cross-product
/// version combinations for the field's type. This is what the _at dispatch
/// strides must be based on, because `field.serialize_at(out, idx)` indexes
/// into the field type's full cross-product space.
///
/// For Option<T>, uses T's count. For other types, uses the type directly.
fn field_version_count_expr(wire_path: &Path, ty: &syn::Type) -> TokenStream {
    let inner = version_types_inner_ty(ty);
    quote! { <#inner as #wire_path::Wire>::VERSION_XPRODUCT_COUNT }
}

/// Build a product expression from a list of factors: `(a * b * c)`.
/// Returns `1usize` for an empty list.
fn build_product(factors: &[TokenStream]) -> TokenStream {
    match factors.len() {
        0 => quote! { 1usize },
        1 => factors[0].clone(),
        _ => {
            let mut acc = factors[0].clone();
            for f in &factors[1..] {
                acc = quote! { (#acc * #f) };
            }
            acc
        }
    }
}

/// Build a sum expression from a list of terms: `(a + b + c)`.
/// Returns `0usize` for an empty list.
fn build_sum(terms: &[TokenStream]) -> TokenStream {
    match terms.len() {
        0 => quote! { 0usize },
        1 => terms[0].clone(),
        _ => {
            let mut acc = terms[0].clone();
            for t in &terms[1..] {
                acc = quote! { (#acc + #t) };
            }
            acc
        }
    }
}

/// Generate the `VERSION_XPRODUCT_COUNT` const expression.
///
/// This is a compile-time integer — the total number of cross-product version
/// combinations. No arrays are materialized. The count is:
/// sum over outer versions of (product of fields' VERSION_XPRODUCT_COUNT values).
fn generate_xproduct_count(wire_path: &Path, layout: &VersionedLayout<&syn::Field>) -> TokenStream {
    let mut segment_sizes = Vec::new();

    for ver in layout.min_version..=layout.max_version {
        let factors: Vec<TokenStream> = layout
            .sorted_items
            .iter()
            .filter(|(_, attrs)| {
                !attrs.skip && !attrs.recurse && attrs.tag.as_ref().unwrap().version <= ver
            })
            .map(|(f, _)| field_version_count_expr(wire_path, &f.ty))
            .collect();
        if factors.is_empty() {
            segment_sizes.push(quote! { 1usize });
        } else {
            segment_sizes.push(build_product(&factors));
        }
    }

    build_sum(&segment_sizes)
}

/// Generate the cross-product `__wire_serialize_at` dispatch function.
///
/// The flat `version_idx` is decomposed into per-field inner version indices
/// using segmented mixed-radix arithmetic:
///
/// 1. Determine which outer version segment the index falls in
/// 2. Compute a local offset within that segment
/// 3. Decompose the local offset: field_i_idx = local / stride_i, then local %= stride_i
/// 4. Call `field.serialize_at(out, field_idx as u32)` for each present field
///
/// ## Concrete example: NestedCL with flat index 2
///
/// NestedCL xproduct layout: [v1×Mode@0, v1×Mode@1, v2×Mode@0×timeout, v2×Mode@1×timeout]
/// Segment boundaries: v1 ends at 2, v2 ends at 4.
///
/// ```text
/// flat = 2
/// → 2 >= 2 (past v1 segment) → outer version 2
/// → local offset: rem = 2 - 2 = 0
/// → field 0 (mode, stride=1): idx_0 = 0/1 = 0, rem = 0%1 = 0
/// → field 1 (timeout):        idx_1 = 0
/// → serialize_at(mode, 0) + serialize_at(timeout, 0) = Mode@v1 + timeout@latest
/// ```
fn generate_xproduct_ser_at(
    wire_path: &Path,
    layout: &VersionedLayout<&syn::Field>,
    by_ref: bool,
) -> TokenStream {
    let (fn_name, receiver, borrow) = ser_fn_shape(by_ref, "__wire_serialize_at");
    let mut cumulative_offset = quote! { 0usize };
    let mut branches = Vec::new();

    for ver in layout.min_version..=layout.max_version {
        // Wire fields at this outer version (skip/recurse excluded)
        let wire_fields: Vec<_> = layout
            .sorted_items
            .iter()
            .filter(|(_, attrs)| {
                !attrs.skip && !attrs.recurse && attrs.tag.as_ref().unwrap().version <= ver
            })
            .collect();

        let factors: Vec<TokenStream> = wire_fields
            .iter()
            .map(|(f, _)| field_version_count_expr(wire_path, &f.ty))
            .collect();
        let segment_size = build_product(&factors);
        let seg_end = quote! { (#cumulative_offset + #segment_size) };

        // Build the decomposition + serialize stmts with const {} blocks
        let mut stmts = Vec::new();
        let cum_offset = cumulative_offset.clone();
        stmts.push(quote! { let mut __rem = __flat - const { #cum_offset }; });

        for (i, (field, _)) in wire_fields.iter().enumerate() {
            let field_name = &field.ident;
            let idx_var = format_ident!("__idx_{}", i);

            if i < wire_fields.len() - 1 {
                let stride_factors: Vec<TokenStream> = wire_fields[i + 1..]
                    .iter()
                    .map(|(f, _)| field_version_count_expr(wire_path, &f.ty))
                    .collect();
                let stride = build_product(&stride_factors);

                stmts.push(quote! {
                    let #idx_var = __rem / const { #stride };
                    __rem = __rem % const { #stride };
                });
            } else {
                stmts.push(quote! { let #idx_var = __rem; });
            }

            stmts.push(quote! {
                <_ as #wire_path::Serialize>::serialize_at(#borrow self.#field_name, out, #idx_var as u32);
            });
        }

        let seg_end_const = seg_end.clone();
        branches.push((seg_end_const, stmts));
        cumulative_offset = seg_end;
    }

    // Build if-else chain with const {} on segment boundaries
    let mut body = quote! { unreachable!("invalid version index"); };
    for (seg_end, stmts) in branches.iter().rev() {
        body = quote! {
            if __flat < const { #seg_end } {
                #(#stmts)*
            } else {
                #body
            }
        };
    }

    quote! {
        /// Dispatch to the version-specific serializer by cross-product index.
        ///
        /// All segment boundaries, strides, and offsets are wrapped in `const { }`
        /// blocks, forcing the Rust compiler to evaluate them as literal integers
        /// before LLVM sees the IR. When the caller passes a compile-time constant
        /// index, LLVM receives `if LITERAL < LITERAL` comparisons and
        /// `LITERAL / LITERAL` arithmetic, which fold trivially to produce a
        /// zero-dispatch monomorphic serializer for any version combination.
        #[doc(hidden)]
        #[inline(always)]
        #[allow(dead_code, unused_assignments, clippy::modulo_one)]
        fn #fn_name<__WireBehavior: #wire_path::bytevec::Behavior>(#receiver, out: &mut #wire_path::bytevec::Builder<__WireBehavior>, version_idx: u32) {
            let __flat = version_idx as usize;
            #body
        }
    }
}

/// Generate the cross-product `__wire_deserialize_at` dispatch function.
///
/// Same decomposition as serialize, but constructs the struct using
/// `deserialize_at` for wire fields and defaults for absent fields.
fn generate_xproduct_de_at(
    wire_path: &Path,
    layout: &VersionedLayout<&syn::Field>,
    _latest_de_name: &syn::Ident,
) -> TokenStream {
    let mut cumulative_offset = quote! { 0usize };
    let mut branches = Vec::new();

    for ver in layout.min_version..=layout.max_version {
        // Wire fields at this outer version
        let wire_fields: Vec<_> = layout
            .sorted_items
            .iter()
            .filter(|(_, attrs)| {
                !attrs.skip && !attrs.recurse && attrs.tag.as_ref().unwrap().version <= ver
            })
            .collect();

        let factors: Vec<TokenStream> = wire_fields
            .iter()
            .map(|(f, _)| field_version_count_expr(wire_path, &f.ty))
            .collect();
        let segment_size = build_product(&factors);
        let seg_end = quote! { (#cumulative_offset + #segment_size) };

        // Build decomposition stmts with const {} blocks
        let mut decompose_stmts = Vec::new();
        let cum_offset = cumulative_offset.clone();
        decompose_stmts.push(quote! { let mut __rem = __flat - const { #cum_offset }; });

        let mut idx_vars = Vec::new();
        for (i, (field, _)) in wire_fields.iter().enumerate() {
            let idx_var = format_ident!("__idx_{}", i);

            if i < wire_fields.len() - 1 {
                let stride_factors: Vec<TokenStream> = wire_fields[i + 1..]
                    .iter()
                    .map(|(f, _)| field_version_count_expr(wire_path, &f.ty))
                    .collect();
                let stride = build_product(&stride_factors);
                decompose_stmts.push(quote! {
                    let #idx_var = __rem / const { #stride };
                    __rem = __rem % const { #stride };
                });
            } else {
                decompose_stmts.push(quote! { let #idx_var = __rem; });
            }
            idx_vars.push((field.ident.clone(), idx_var));
        }

        // Build field initializers for the struct constructor
        // Wire fields present at this version: deserialize_at with computed index
        // Wire fields absent at this version: use default
        // Skip fields: always Default::default()
        let mut field_inits = Vec::new();
        let mut wire_idx = 0usize;
        for (field, attrs) in &layout.sorted_items {
            let field_name = &field.ident;
            let ty = &field.ty;
            let tag = attrs.tag.as_ref().unwrap();

            if tag.version <= ver {
                // Present — deserialize_at with computed index
                let (_, ref idx_var) = idx_vars[wire_idx];
                field_inits.push(quote! {
                    #field_name: <#ty as #wire_path::Deserialize>::deserialize_at(bytes, #idx_var as u32)?
                });
                wire_idx += 1;
            } else {
                // Absent — fill default
                if let Some(ref default_expr) = attrs.default_expr {
                    field_inits.push(quote! { #field_name: #default_expr });
                } else if is_option_type(ty) {
                    field_inits.push(quote! { #field_name: None });
                } else {
                    field_inits.push(quote! { #field_name: Default::default() });
                }
            }
        }
        // Skip fields — always default
        for (field, _) in &layout.skip_items {
            let field_name = &field.ident;
            field_inits.push(quote! { #field_name: Default::default() });
        }

        branches.push((seg_end.clone(), decompose_stmts, field_inits));
        cumulative_offset = seg_end;
    }

    // Build if-else chain with const {} on segment boundaries
    let mut body = quote! { unreachable!("invalid version index"); };
    for (seg_end, decompose_stmts, field_inits) in branches.iter().rev() {
        body = quote! {
            if __flat < const { #seg_end } {
                #(#decompose_stmts)*
                Ok(Self {
                    #(#field_inits,)*
                })
            } else {
                #body
            }
        };
    }

    quote! {
        /// Dispatch to the version-specific deserializer by cross-product index.
        ///
        /// All segment boundaries, strides, and offsets are wrapped in `const { }`
        /// blocks, forcing the Rust compiler to evaluate them as literal integers
        /// before LLVM sees the IR. When the caller passes a compile-time constant
        /// index, LLVM receives literal comparisons and arithmetic that fold
        /// trivially to produce a zero-dispatch monomorphic deserializer.
        #[doc(hidden)]
        #[inline(always)]
        #[allow(dead_code, unused_assignments, clippy::modulo_one)]
        fn __wire_deserialize_at(
            bytes: &mut #wire_path::bytevec::ByteVec,
            version_idx: u32,
        ) -> Result<Self, #wire_path::de::Error> {
            let __flat = version_idx as usize;
            #body
        }
    }
}

/// Generate code to copy hash bytes for a single field from its inner type's
/// `__XPRODUCT_HASH_ENTRIES`. For `Option<T>`, wraps T's hash in the Option
/// TYPE tree structure. For other types, copies directly.
fn field_hash_copy_code(wire_path: &Path, ty: &syn::Type) -> TokenStream {
    let inner = version_types_inner_ty(ty);
    if extract_option_inner(ty).is_some() {
        // Option<T>: write Option enum header, None variant, Some variant wrapping T's hash
        quote! {
            {
                let (ref __src, __src_len) = <#inner as #wire_path::Wire>::__XPRODUCT_HASH_ENTRIES[__field_idx];
                // Option enum header
                __buf[__cur] = #wire_path::Type::ENUM; __cur += 1;
                __buf[__cur] = 1; __cur += 1;
                // None variant
                __buf[__cur] = 0; __cur += 1; // id
                __buf[__cur] = 1; __cur += 1; // open
                __buf[__cur] = 0; __cur += 1; // close
                // Some variant
                __buf[__cur] = 1; __cur += 1; // id
                __buf[__cur] = 1; __cur += 1; // open
                let mut __k = 0usize;
                while __k < __src_len as usize { __buf[__cur] = __src[__k]; __cur += 1; __k += 1; }
                __buf[__cur] = 0; __cur += 1; // close Some
                __buf[__cur] = 0; __cur += 1; // close Option
            }
        }
    } else {
        quote! {
            {
                let (ref __src, __src_len) = <#inner as #wire_path::Wire>::__XPRODUCT_HASH_ENTRIES[__field_idx];
                let mut __k = 0usize;
                while __k < __src_len as usize { __buf[__cur] = __src[__k]; __cur += 1; __k += 1; }
            }
        }
    }
}

/// Generate the `__XPRODUCT_HASH_ENTRIES` const array for a versioned struct.
///
/// Uses segmented mixed-radix decomposition (same layout as serialize_at) to
/// iterate all cross-product combinations. For each combination, writes the
/// struct TYPE tree hash bytes by copying from inner types' hash entries.
fn generate_xproduct_hash_entries(
    wire_path: &Path,
    layout: &VersionedLayout<&syn::Field>,
) -> TokenStream {
    let xp_count_expr = generate_xproduct_count(wire_path, layout);

    let mut segment_builders = Vec::new();

    for ver in layout.min_version..=layout.max_version {
        let wire_fields: Vec<_> = layout
            .sorted_items
            .iter()
            .filter(|(_, attrs)| {
                !attrs.skip && !attrs.recurse && attrs.tag.as_ref().unwrap().version <= ver
            })
            .collect();

        let factors: Vec<TokenStream> = wire_fields
            .iter()
            .map(|(f, _)| field_version_count_expr(wire_path, &f.ty))
            .collect();
        let seg_product = build_product(&factors);

        // For each field: decompose remainder, copy hash bytes
        let mut field_code_parts = Vec::new();
        for (f, _) in &wire_fields {
            let count_expr = field_version_count_expr(wire_path, &f.ty);
            let copy_code = field_hash_copy_code(wire_path, &f.ty);
            field_code_parts.push(quote! {
                let __field_idx = __remainder % #count_expr;
                __remainder /= #count_expr;
                #copy_code
            });
        }

        segment_builders.push(quote! {
            {
                let __seg_size: usize = #seg_product;
                let mut __local = 0usize;
                while __local < __seg_size {
                    let mut __buf = [0u8; #wire_path::XPRODUCT_HASH_BUF_SIZE];
                    let mut __cur = 0usize;

                    // Struct header
                    __buf[__cur] = #wire_path::Type::STRUCT; __cur += 1;
                    __buf[__cur] = 1; __cur += 1;

                    // Mixed-radix decomposition of __local into per-field indices
                    let mut __remainder = __local;
                    #(#field_code_parts)*

                    // Struct terminator
                    __buf[__cur] = 0; __cur += 1;

                    __entries[__global] = (__buf, __cur as u16);
                    __global += 1;
                    __local += 1;
                }
            }
        });
    }

    let total_expr = quote! { { #xp_count_expr } };

    quote! {
        #[doc(hidden)]
        #[allow(non_upper_case_globals, clippy::modulo_one)]
        const __WIRE_XPRODUCT_HASH_ENTRIES: [([u8; #wire_path::XPRODUCT_HASH_BUF_SIZE], u16); #total_expr] = {
            let mut __entries = [([0u8; #wire_path::XPRODUCT_HASH_BUF_SIZE], 0u16); #total_expr];
            let mut __global = 0usize;
            #(#segment_builders)*
            __entries
        };
    }
}

// ── Main versioned struct codegen ─────────────────────────────────

/// Generate versioned Wire impls for a struct with `#[wire(tag = V.S)]` annotations.
///
/// This is the main entry point for versioned struct codegen. It generates:
/// 1. Per-outer-version TYPE consts (for `Wire::TYPE` = latest)
/// 2. Per-outer-version serialize/deserialize fns (for `Serialize`/`Deserialize` trait defaults)
/// 3. Cross-product `VERSION_XPRODUCT_COUNT` + `xproduct_hash_bytes` for lazy hash computation
/// 4. Cross-product `__wire_serialize_at` / `__wire_deserialize_at` with segmented
///    mixed-radix decomposition
/// 5. All trait impls: Wire, VersionedWire, Serialize, Deserialize
fn emit_versioned(
    wire_path: &Path,
    name: &syn::Ident,
    generics: &syn::Generics,
    parsed_fields: Vec<(&syn::Field, ItemWireAttrs)>,
    min_version_attr: Option<u32>,
    borrowed: bool,
) -> syn::parse::Result<TokenStream> {
    let layout = versioning::validate_and_sort(parsed_fields, min_version_attr)?;

    // Struct-specific validation: fields at version > 1 must have a default or be Option<T>
    for (field, attrs) in &layout.sorted_items {
        let tag = attrs.tag.as_ref().unwrap();
        if tag.version > 1 && attrs.default_expr.is_none() && !is_option_type(&field.ty) {
            return Err(syn::Error::new_spanned(
                field,
                format!(
                    "field with tag {}.{} (version > 1) must have a default expression \
                     (e.g., #[wire(tag = {}.{}, default = expr)])",
                    tag.version, tag.sequence, tag.version, tag.sequence
                ),
            ));
        }
    }

    let (impl_generics, ty_generics, _) = generics.split_for_impl();
    let (wire_wc, ser_wc, de_wc) = build_where_clauses(wire_path, generics);
    let type_id = quote!(#wire_path::Type::STRUCT);

    // ── 1. Per-outer-version TYPE consts (used by Wire::TYPE, unchanged) ──
    let mut per_ver_type_consts = Vec::new();
    for ver in layout.min_version..=layout.max_version {
        let type_const_name = format_ident!("__WIRE_TYPE_V{}", ver);
        let type_children: Vec<TokenStream> = layout
            .sorted_items
            .iter()
            .filter(|(_, attrs)| attrs.tag.as_ref().unwrap().version <= ver)
            .filter_map(|(f, attrs)| item_type_token(wire_path, &f.ty, attrs))
            .collect();
        per_ver_type_consts.push(quote! {
            #[doc(hidden)]
            #[allow(non_upper_case_globals)]
            const #type_const_name: #wire_path::Type = #wire_path::Type {
                name: stringify!(#name),
                id: #type_id,
                children: &[#(#type_children),*],
            };
        });
    }
    let latest_type = format_ident!("__WIRE_TYPE_V{}", layout.max_version);

    // ── 2. Per-outer-version serialize/deserialize fns (for trait defaults) ──
    let (ser_fns, de_fns) = struct_versioned_serde(wire_path, &layout);

    // `#[wire(borrowed)]`: by-reference twins of the serializer fns, plus the
    // `SerializeRef` impl that dispatches to them.
    let ser_ref_impls = borrowed.then(|| {
        let ser_ref_wc = build_ser_ref_where_clause(wire_path, generics);
        let ser_ref_fns = struct_versioned_ser_fns(wire_path, &layout, true);
        let ser_ref_at_fn = generate_xproduct_ser_at(wire_path, &layout, true);
        let latest_ser_ref = format_ident!("__wire_serialize_ref_v{}", layout.max_version);
        let trait_impl = emit_serialize_ref_impl(
            wire_path,
            name,
            generics,
            quote! { self.#latest_ser_ref(out) },
            Some(quote! { self.__wire_serialize_ref_at(out, version_idx) }),
        );
        quote! {
            impl #impl_generics #name #ty_generics #ser_ref_wc {
                #(#ser_ref_fns)*
                #ser_ref_at_fn
            }

            #trait_impl
        }
    });
    let latest_ser = format_ident!("__wire_serialize_v{}", layout.max_version);
    let latest_de = format_ident!("__wire_deserialize_v{}", layout.max_version);

    // Collect the per-outer-version const names for VERSION_TYPES
    let per_ver_type_names: Vec<_> = (layout.min_version..=layout.max_version)
        .map(|ver| format_ident!("__WIRE_TYPE_V{}", ver))
        .collect();

    // ── 3. Cross-product count ──
    let xp_count_expr = generate_xproduct_count(wire_path, &layout);

    // ── 4. Cross-product _at dispatch ──
    let ser_at_fn = generate_xproduct_ser_at(wire_path, &layout, false);
    let de_at_fn = generate_xproduct_de_at(wire_path, &layout, &latest_de);

    // ── 5. Cross-product hash entries ──
    let xproduct_hash_entries = generate_xproduct_hash_entries(wire_path, &layout);

    let min_ver_lit = layout.min_version;
    let max_ver_lit = layout.max_version;

    // ── 6. Assemble everything ──
    Ok(quote! {
        // Inherent consts (per-outer-version TYPE trees + xproduct hash entries)
        impl #impl_generics #name #ty_generics #wire_wc {
            #(#per_ver_type_consts)*
            #xproduct_hash_entries
        }

        // Serialize inherent fns
        impl #impl_generics #name #ty_generics #ser_wc {
            #(#ser_fns)*
            #ser_at_fn
        }

        // Deserialize inherent fns
        impl #impl_generics #name #ty_generics #de_wc {
            #(#de_fns)*
            #de_at_fn
        }

        // Serialize trait impl
        impl #impl_generics #wire_path::Serialize for #name #ty_generics #ser_wc {
            fn serialize<__WireBehavior: #wire_path::bytevec::Behavior>(self, out: &mut #wire_path::bytevec::Builder<__WireBehavior>) {
                self.#latest_ser(out)
            }

            // Use `#[inline]` (hint) instead of `#[inline(always)]` (force) on the
            // trait method. The inherent `__wire_serialize_at` keeps `#[inline(always)]`
            // so it gets inlined into this method, but we let LLVM decide whether to
            // inline this trait method into each call site. Forcing inlining here would
            // copy the entire dispatch body (all version branches) into every caller,
            // bloating the caller's function body and potentially degrading instruction
            // cache performance even after dead-code elimination.
            #[inline]
            fn serialize_at<__WireBehavior: #wire_path::bytevec::Behavior>(self, out: &mut #wire_path::bytevec::Builder<__WireBehavior>, version_idx: u32) {
                self.__wire_serialize_at(out, version_idx)
            }
        }

        // Deserialize trait impl
        impl #impl_generics #wire_path::Deserialize for #name #ty_generics #de_wc {
            fn deserialize(bytes: &mut #wire_path::bytevec::ByteVec)
                -> Result<Self, #wire_path::de::Error>
            {
                Self::#latest_de(bytes)
            }

            // Same rationale as serialize_at: `#[inline]` lets LLVM decide, while
            // the inherent `__wire_deserialize_at` uses `#[inline(always)]` internally.
            #[inline]
            fn deserialize_at(bytes: &mut #wire_path::bytevec::ByteVec, version_idx: u32)
                -> Result<Self, #wire_path::de::Error>
            {
                Self::__wire_deserialize_at(bytes, version_idx)
            }
        }

        // Wire trait impl
        impl #impl_generics #wire_path::Wire for #name #ty_generics #wire_wc {
            const TYPE: #wire_path::Type = Self::#latest_type;
            const VERSION_TYPES: &'static [#wire_path::Type] = &[
                #(Self::#per_ver_type_names),*
            ];
            const VERSION_XPRODUCT_COUNT: usize = #xp_count_expr;
            const __XPRODUCT_HASH_ENTRIES: &'static [([u8; #wire_path::XPRODUCT_HASH_BUF_SIZE], u16)] = &Self::__WIRE_XPRODUCT_HASH_ENTRIES;
        }

        // VersionedWire trait impl
        impl #impl_generics #wire_path::VersionedWire for #name #ty_generics #wire_wc {
            const MIN_VERSION: u32 = #min_ver_lit;
            const MAX_VERSION: u32 = #max_ver_lit;
        }

        #ser_ref_impls
    })
}

/// Generate struct-specific per-version serialize and deserialize functions.
///
/// These are the existing per-outer-version functions used by the
/// Serialize/Deserialize trait impls (latest-version path). They always use
/// the latest inner type versions (plain `serialize`/`deserialize`, not `_at`).
fn struct_versioned_serde(
    wire_path: &Path,
    layout: &VersionedLayout<&syn::Field>,
) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let mut de_fns = Vec::new();

    let ser_fns = struct_versioned_ser_fns(wire_path, layout, false);

    for ver in layout.min_version..=layout.max_version {
        let de_fn_name = format_ident!("__wire_deserialize_v{}", ver);

        // Deserializer: read present fields, fill defaults for absent ones
        let de_wire_inits = layout.sorted_items.iter().map(|(f, attrs)| {
            let field_name = &f.ident;
            let ty = &f.ty;
            let tag = attrs.tag.as_ref().unwrap();
            if tag.version <= ver {
                quote! { #field_name: <#ty as #wire_path::Deserialize>::deserialize(bytes)? }
            } else if let Some(ref default_expr) = attrs.default_expr {
                quote! { #field_name: #default_expr }
            } else if is_option_type(ty) {
                quote! { #field_name: None }
            } else {
                quote! { #field_name: Default::default() }
            }
        });

        // Skip fields are not on the wire — always Default::default()
        let de_skip_inits = layout.skip_items.iter().map(|(f, _)| {
            let field_name = &f.ident;
            quote! { #field_name: Default::default() }
        });

        let de_field_inits: Vec<TokenStream> = de_wire_inits.chain(de_skip_inits).collect();

        de_fns.push(quote! {
            #[doc(hidden)]
            #[allow(dead_code)]
            fn #de_fn_name(bytes: &mut #wire_path::bytevec::ByteVec)
                -> Result<Self, #wire_path::de::Error>
            {
                Ok(Self {
                    #(#de_field_inits,)*
                })
            }
        });
    }

    (ser_fns, de_fns)
}

/// Generate the per-outer-version serializer fns (`__wire_serialize_vN`), or
/// with `by_ref` their by-reference twins (`__wire_serialize_ref_vN`, taking
/// `&self` and borrowing each field). Both write the same fields in the same
/// order, so their output is identical.
fn struct_versioned_ser_fns(
    wire_path: &Path,
    layout: &VersionedLayout<&syn::Field>,
    by_ref: bool,
) -> Vec<TokenStream> {
    (layout.min_version..=layout.max_version)
        .map(|ver| {
            let (ser_fn_name, receiver, borrow) =
                ser_fn_shape(by_ref, &format!("__wire_serialize_v{ver}"));

            // Serializer: write only fields present at this version
            let ser_stmts: Vec<TokenStream> = layout
                .sorted_items
                .iter()
                .filter(|(_, attrs)| attrs.tag.as_ref().unwrap().version <= ver)
                .map(|(f, _)| {
                    let field_name = &f.ident;
                    quote! {
                        <_ as #wire_path::Serialize>::serialize(#borrow self.#field_name, out);
                    }
                })
                .collect();

            quote! {
                #[doc(hidden)]
                #[allow(dead_code)]
                fn #ser_fn_name<__WireBehavior: #wire_path::bytevec::Behavior>(#receiver, out: &mut #wire_path::bytevec::Builder<__WireBehavior>) {
                    #(#ser_stmts)*
                }
            }
        })
        .collect()
}
