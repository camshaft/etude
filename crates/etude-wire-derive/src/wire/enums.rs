// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Enum codegen for the Wire derive macro.
//!
//! Handles both simple (non-versioned) and versioned enum codegen.
//! Simple enums produce plain Serialize/Deserialize/Wire impls.
//! Versioned enums (with `#[wire(tag = V.S)]` on variants) produce per-version
//! serializers (with fallback conversion), deserializers, TYPE trees, and a
//! VersionedWire impl.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::Path;

use super::{
    common::{
        ItemKind, ItemWireAttrs, build_ser_ref_where_clause, build_where_clauses,
        emit_serialize_ref_impl, emit_wire_impls, item_type_token, ser_fn_shape,
    },
    versioning::{self, VersionTypeConst},
};

/// A pre-parsed enum variant with its attrs, field kind, and pre-parsed fields.
struct ParsedVariant<'a> {
    variant: &'a syn::Variant,
    attrs: ItemWireAttrs,
    kind: ItemKind,
    fields: Vec<(&'a syn::Field, ItemWireAttrs)>,
}

/// A sorted variant ready for versioned codegen. Combines layout ordering
/// (from validate_and_sort) with field-level info (from initial parsing).
struct SortedVariant<'a> {
    variant: &'a syn::Variant,
    attrs: ItemWireAttrs,
    disc: u8,
    kind: ItemKind,
    fields: Vec<(&'a syn::Field, ItemWireAttrs)>,
}

/// Entry point for enum codegen. Parses variant/field attrs, then dispatches
/// to simple or versioned path based on whether any variant has a tag.
pub(super) fn derive(
    wire_path: &Path,
    name: &syn::Ident,
    generics: &syn::Generics,
    data: &syn::DataEnum,
    schema_override: Option<&syn::Type>,
    min_version: Option<u32>,
    borrowed: bool,
) -> syn::parse::Result<TokenStream> {
    assert!((1..=u8::MAX as usize).contains(&data.variants.len()));

    // Pre-parse all variant attrs and fields in a single pass
    let mut parsed = Vec::new();
    for v in &data.variants {
        let attrs = ItemWireAttrs::parse(&v.attrs)?;
        let kind = ItemKind::from(&v.fields);
        let mut fields = Vec::new();
        for f in &v.fields {
            let field_attrs = ItemWireAttrs::parse(&f.attrs)?;
            fields.push((f, field_attrs));
        }
        parsed.push(ParsedVariant {
            variant: v,
            attrs,
            kind,
            fields,
        });
    }

    let has_tags = parsed.iter().any(|pv| pv.attrs.tag.is_some());

    if has_tags {
        emit_versioned(wire_path, name, generics, parsed, min_version, borrowed)
    } else {
        Ok(emit_simple(
            wire_path,
            name,
            generics,
            &parsed,
            schema_override,
            borrowed,
        ))
    }
}

/// Generate codegen for non-versioned enums (existing behavior, unchanged).
fn emit_simple(
    wire_path: &Path,
    name: &syn::Ident,
    generics: &syn::Generics,
    parsed: &[ParsedVariant<'_>],
    schema_override: Option<&syn::Type>,
    borrowed: bool,
) -> TokenStream {
    let ser_variants = parsed.iter().enumerate().map(|(idx, pv)| {
        let idx = idx as u8;
        let variant = &pv.variant.ident;

        if pv.attrs.skip {
            return match pv.kind {
                ItemKind::Named => quote! { Self::#variant { .. } => { unreachable!("skip"); } },
                ItemKind::Unnamed => quote! { Self::#variant(..) => { unreachable!("skip"); } },
                ItemKind::Unit => quote! { Self::#variant => { unreachable!("skip"); } },
            };
        }

        match pv.kind {
            ItemKind::Named => {
                let names: Vec<_> = pv.fields.iter().map(|(f, _)| &f.ident).collect();
                quote! {
                    Self::#variant { #(#names),* } => {
                        <_ as #wire_path::Serialize>::serialize(#idx, out);
                        #(<_ as #wire_path::Serialize>::serialize(#names, out);)*
                    }
                }
            }
            ItemKind::Unnamed => {
                let names: Vec<_> = (0..pv.fields.len())
                    .map(|i| syn::Ident::new(&format!("_{i}"), Span::call_site()))
                    .collect();
                quote! {
                    Self::#variant(#(#names),*) => {
                        <_ as #wire_path::Serialize>::serialize(#idx, out);
                        #(<_ as #wire_path::Serialize>::serialize(#names, out);)*
                    }
                }
            }
            ItemKind::Unit => quote! {
                Self::#variant => { <_ as #wire_path::Serialize>::serialize(#idx, out); }
            },
        }
    });

    let de_variants =
        parsed
            .iter()
            .filter(|pv| !pv.attrs.skip)
            .enumerate()
            .map(|(idx, pv)| {
                let idx = idx as u8;
                let variant = &pv.variant.ident;

                match pv.kind {
                    ItemKind::Named => {
                        let des: Vec<_> = pv.fields.iter().map(|(f, _)| {
                    let name = &f.ident;
                    let ty = &f.ty;
                    quote! { #name: <#ty as #wire_path::Deserialize>::deserialize(bytes)? }
                }).collect();
                        quote! { #idx => Ok(Self::#variant { #(#des,)* }) }
                    }
                    ItemKind::Unnamed => {
                        let des: Vec<_> = pv
                            .fields
                            .iter()
                            .map(|(f, _)| {
                                let ty = &f.ty;
                                quote! { <#ty as #wire_path::Deserialize>::deserialize(bytes)? }
                            })
                            .collect();
                        quote! { #idx => Ok(Self::#variant(#(#des,)*)) }
                    }
                    ItemKind::Unit => quote! { #idx => Ok(Self::#variant) },
                }
            });

    let variant_types = parsed
        .iter()
        .filter(|pv| !pv.attrs.skip)
        .enumerate()
        .map(|(id, pv)| {
            let children: Vec<_> = pv
                .fields
                .iter()
                .filter_map(|(f, attrs)| item_type_token(wire_path, &f.ty, attrs))
                .collect();

            let name = pv.variant.ident.to_string();
            let id: u8 = id
                .try_into()
                .expect("cannot have more than 255 enum variants");
            quote! { #wire_path::Type { name: #name, id: #id, children: &[#(#children),*] } }
        });

    let id = quote!(#wire_path::Type::ENUM);

    let ser_impl = quote! { match self { #(#ser_variants,)* } };
    let de_impl = quote! {
        match <u8 as #wire_path::Deserialize>::deserialize(bytes)? {
            #(#de_variants,)*
            v => { Err(#wire_path::de::Error::new(#wire_path::de::ErrorKind::InvalidVariant, #wire_path::type_of::<Self>())) }
        }
    };
    let wire_impl = quote! { #(#variant_types),* };

    // `#[wire(borrowed)]`: the same match over `&self` binds each field by
    // reference, and `&Field: Serialize` forwards to `Field: SerializeRef`.
    let ser_ref_impl = borrowed
        .then(|| emit_serialize_ref_impl(wire_path, name, generics, ser_impl.clone(), None));

    let mut tokens = emit_wire_impls(
        wire_path,
        name,
        generics,
        &id,
        ser_impl,
        de_impl,
        wire_impl,
        schema_override,
    );
    tokens.extend(ser_ref_impl);
    tokens
}

// ── Versioned enum codegen ────────────────────────────────────────

/// Generate versioned Wire impls for an enum with `#[wire(tag = V.S)]` on variants.
fn emit_versioned(
    wire_path: &Path,
    name: &syn::Ident,
    generics: &syn::Generics,
    parsed: Vec<ParsedVariant<'_>>,
    min_version_attr: Option<u32>,
    borrowed: bool,
) -> syn::parse::Result<TokenStream> {
    // Reject tags on variant fields — variant field versioning is not supported.
    for pv in &parsed {
        if pv.attrs.tag.is_some() {
            for (field, field_attrs) in &pv.fields {
                if field_attrs.tag.is_some() {
                    return Err(syn::Error::new_spanned(
                        field,
                        "variant field versioning is not yet supported; \
                         only variant-level #[wire(tag = V.S)] is allowed in versioned enums",
                    ));
                }
            }
        }
    }

    // Collect variant-level attrs for validate_and_sort (takes ownership)
    let variant_items: Vec<(&syn::Variant, ItemWireAttrs)> = parsed
        .iter()
        .map(|pv| {
            let attrs = ItemWireAttrs {
                skip: pv.attrs.skip,
                recurse: pv.attrs.recurse,
                tag: pv.attrs.tag.clone(),
                default_expr: pv.attrs.default_expr.clone(),
            };
            (pv.variant, attrs)
        })
        .collect();

    let layout = versioning::validate_and_sort(variant_items, min_version_attr)?;

    // Enum-specific validation: variants at version > 1 must have fallback (default_expr)
    for (variant, attrs) in &layout.sorted_items {
        let tag = attrs.tag.as_ref().unwrap();
        if tag.version > 1 && attrs.default_expr.is_none() {
            return Err(syn::Error::new_spanned(
                variant,
                format!(
                    "variant with tag {}.{} (version > 1) must have a fallback expression \
                     (e.g., #[wire(tag = {}.{}, fallback = EnumName::OlderVariant)])",
                    tag.version, tag.sequence, tag.version, tag.sequence
                ),
            ));
        }
    }

    // Build SortedVariant list — combines sorted order with field-level info.
    // Match by variant ident to pull field info from parsed data.
    let parsed_map: std::collections::HashMap<_, _> = parsed
        .iter()
        .map(|pv| (pv.variant.ident.clone(), pv))
        .collect();

    let sorted: Vec<SortedVariant<'_>> = layout
        .sorted_items
        .iter()
        .enumerate()
        .map(|(disc, (variant, attrs))| {
            let pv = parsed_map.get(&variant.ident).unwrap();
            SortedVariant {
                variant: pv.variant,
                attrs: ItemWireAttrs {
                    skip: attrs.skip,
                    recurse: attrs.recurse,
                    tag: attrs.tag.clone(),
                    default_expr: attrs.default_expr.clone(),
                },
                disc: disc as u8,
                kind: pv.kind,
                fields: pv
                    .fields
                    .iter()
                    .map(|(f, a)| {
                        (
                            *f,
                            ItemWireAttrs {
                                skip: a.skip,
                                recurse: a.recurse,
                                tag: a.tag.clone(),
                                default_expr: a.default_expr.clone(),
                            },
                        )
                    })
                    .collect(),
            }
        })
        .collect();

    let type_consts = version_type_consts_enum(
        wire_path,
        name,
        &sorted,
        layout.min_version,
        layout.max_version,
    );
    let (ser_fns, de_fns) =
        enum_versioned_serde(wire_path, &sorted, layout.min_version, layout.max_version);

    let (impl_generics, ty_generics, _) = generics.split_for_impl();
    let (_, ser_wc, de_wc) = build_where_clauses(wire_path, generics);

    let latest_ser = format_ident!("__wire_serialize_v{}", layout.max_version);
    let latest_de = format_ident!("__wire_deserialize_v{}", layout.max_version);

    let ser_trait_impl = quote! {
        impl #impl_generics #wire_path::Serialize for #name #ty_generics #ser_wc {
            fn serialize<__WireBehavior: #wire_path::bytevec::Behavior>(self, out: &mut #wire_path::bytevec::Builder<__WireBehavior>) {
                self.#latest_ser(out)
            }

            #[inline(always)]
            fn serialize_at<__WireBehavior: #wire_path::bytevec::Behavior>(self, out: &mut #wire_path::bytevec::Builder<__WireBehavior>, version_idx: u32) {
                self.__wire_serialize_at(out, version_idx)
            }
        }
    };

    let de_trait_impl = quote! {
        impl #impl_generics #wire_path::Deserialize for #name #ty_generics #de_wc {
            fn deserialize(bytes: &mut #wire_path::bytevec::ByteVec)
                -> Result<Self, #wire_path::de::Error>
            {
                Self::#latest_de(bytes)
            }

            #[inline(always)]
            fn deserialize_at(bytes: &mut #wire_path::bytevec::ByteVec, version_idx: u32)
                -> Result<Self, #wire_path::de::Error>
            {
                Self::__wire_deserialize_at(bytes, version_idx)
            }
        }
    };

    // `#[wire(borrowed)]`: by-reference twins of the serializer fns, the
    // version dispatch over them, and the `SerializeRef` impl.
    let ser_ref_impls = borrowed.then(|| {
        let ser_ref_wc = build_ser_ref_where_clause(wire_path, generics);
        let ser_ref_fns = enum_versioned_ser_fns(
            wire_path,
            &sorted,
            layout.min_version,
            layout.max_version,
            true,
        );
        let ser_ref_at_fn = versioning::generate_ser_ref_dispatch(
            wire_path,
            layout.min_version,
            layout.max_version,
        );
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

    let mut tokens = versioning::emit_versioned_wire_impls(
        wire_path,
        name,
        generics,
        layout.min_version,
        layout.max_version,
        &type_consts,
        ser_fns,
        de_fns,
        ser_trait_impl,
        de_trait_impl,
    );
    tokens.extend(ser_ref_impls);
    Ok(tokens)
}

/// Generate per-version TYPE tree constants for a versioned enum.
fn version_type_consts_enum(
    wire_path: &Path,
    name: &syn::Ident,
    sorted: &[SortedVariant<'_>],
    min_version: u32,
    max_version: u32,
) -> Vec<VersionTypeConst> {
    let mut consts = Vec::new();

    for ver in min_version..=max_version {
        let type_const_name = format_ident!("__WIRE_TYPE_V{}", ver);

        let type_children: Vec<TokenStream> = sorted
            .iter()
            .filter(|sv| sv.attrs.tag.as_ref().unwrap().version <= ver)
            .map(|sv| {
                let variant_name = sv.variant.ident.to_string();
                let disc = sv.disc;
                let field_children: Vec<TokenStream> = sv
                    .fields
                    .iter()
                    .filter_map(|(f, attrs)| item_type_token(wire_path, &f.ty, attrs))
                    .collect();
                quote! {
                    #wire_path::Type {
                        name: #variant_name,
                        id: #disc,
                        children: &[#(#field_children),*],
                    }
                }
            })
            .collect();

        consts.push(VersionTypeConst {
            const_def: quote! {
                #[doc(hidden)]
                #[allow(non_upper_case_globals)]
                const #type_const_name: #wire_path::Type = #wire_path::Type {
                    name: stringify!(#name),
                    id: #wire_path::Type::ENUM,
                    children: &[#(#type_children),*],
                };
            },
            array_entry: quote! { Self::#type_const_name },
        });
    }

    consts
}

/// Generate per-version serialize and deserialize functions for a versioned enum.
fn enum_versioned_serde(
    wire_path: &Path,
    sorted: &[SortedVariant<'_>],
    min_version: u32,
    max_version: u32,
) -> (Vec<TokenStream>, Vec<TokenStream>) {
    let ser_fns = enum_versioned_ser_fns(wire_path, sorted, min_version, max_version, false);
    let mut de_fns = Vec::new();

    for ver in min_version..=max_version {
        let de_fn_name = format_ident!("__wire_deserialize_v{}", ver);

        // Deserializer: only variants present at this version
        let de_arms: Vec<TokenStream> = sorted.iter()
            .filter(|sv| sv.attrs.tag.as_ref().unwrap().version <= ver)
            .map(|sv| {
                let variant_ident = &sv.variant.ident;
                let disc = sv.disc;

                match sv.kind {
                    ItemKind::Unit => quote! { #disc => Ok(Self::#variant_ident) },
                    ItemKind::Unnamed => {
                        let des: Vec<_> = sv.fields.iter().map(|(f, _)| {
                            let ty = &f.ty;
                            quote! { <#ty as #wire_path::Deserialize>::deserialize(bytes)? }
                        }).collect();
                        quote! { #disc => Ok(Self::#variant_ident(#(#des,)*)) }
                    }
                    ItemKind::Named => {
                        let des: Vec<_> = sv.fields.iter().map(|(f, _)| {
                            let field_name = &f.ident;
                            let ty = &f.ty;
                            quote! { #field_name: <#ty as #wire_path::Deserialize>::deserialize(bytes)? }
                        }).collect();
                        quote! { #disc => Ok(Self::#variant_ident { #(#des,)* }) }
                    }
                }
            })
            .collect();

        de_fns.push(quote! {
            #[doc(hidden)]
            #[allow(dead_code)]
            fn #de_fn_name(bytes: &mut #wire_path::bytevec::ByteVec)
                -> Result<Self, #wire_path::de::Error>
            {
                match <u8 as #wire_path::Deserialize>::deserialize(bytes)? {
                    #(#de_arms,)*
                    _ => Err(#wire_path::de::Error::new(
                        #wire_path::de::ErrorKind::InvalidVariant,
                        #wire_path::type_of::<Self>(),
                    )),
                }
            }
        });
    }

    (ser_fns, de_fns)
}

/// Generate the per-version serializer fns for a versioned enum
/// (`__wire_serialize_vN`), or with `by_ref` their by-reference twins
/// (`__wire_serialize_ref_vN`, taking `&self`). The match arms are the same
/// tokens either way: over `&self` each field binds by reference and
/// `&Field: Serialize` forwards to `Field: SerializeRef`. A fallback is a
/// fresh value, so it serializes through the same-shape fn.
fn enum_versioned_ser_fns(
    wire_path: &Path,
    sorted: &[SortedVariant<'_>],
    min_version: u32,
    max_version: u32,
    by_ref: bool,
) -> Vec<TokenStream> {
    let mut ser_fns = Vec::new();

    for ver in min_version..=max_version {
        let (ser_fn_name, receiver, _) = ser_fn_shape(by_ref, &format!("__wire_serialize_v{ver}"));

        // Serializer: for each variant, either write normally or use fallback
        let ser_arms: Vec<TokenStream> = sorted.iter()
            .map(|sv| {
                let variant_ident = &sv.variant.ident;
                let tag = sv.attrs.tag.as_ref().unwrap();
                let disc = sv.disc;

                if tag.version <= ver {
                    // Variant present at this version — serialize normally
                    match sv.kind {
                        ItemKind::Unit => quote! {
                            Self::#variant_ident => {
                                <_ as #wire_path::Serialize>::serialize(#disc, out);
                            }
                        },
                        ItemKind::Unnamed => {
                            let names: Vec<_> = (0..sv.fields.len())
                                .map(|i| syn::Ident::new(&format!("_{i}"), Span::call_site()))
                                .collect();
                            quote! {
                                Self::#variant_ident(#(#names),*) => {
                                    <_ as #wire_path::Serialize>::serialize(#disc, out);
                                    #(<_ as #wire_path::Serialize>::serialize(#names, out);)*
                                }
                            }
                        }
                        ItemKind::Named => {
                            let names: Vec<_> = sv.fields.iter().map(|(f, _)| &f.ident).collect();
                            quote! {
                                Self::#variant_ident { #(#names),* } => {
                                    <_ as #wire_path::Serialize>::serialize(#disc, out);
                                    #(<_ as #wire_path::Serialize>::serialize(#names, out);)*
                                }
                            }
                        }
                    }
                } else {
                    // Variant not present — use fallback
                    let fallback = sv.attrs.default_expr.as_ref().unwrap();
                    match sv.kind {
                        ItemKind::Unit => quote! {
                            Self::#variant_ident => { #fallback.#ser_fn_name(out); }
                        },
                        ItemKind::Unnamed => {
                            let names: Vec<_> = (0..sv.fields.len())
                                .map(|i| syn::Ident::new(&format!("_{i}"), Span::call_site()))
                                .collect();
                            quote! {
                                Self::#variant_ident(#(#names),*) => { #fallback.#ser_fn_name(out); }
                            }
                        }
                        ItemKind::Named => quote! {
                            Self::#variant_ident { .. } => { #fallback.#ser_fn_name(out); }
                        },
                    }
                }
            })
            .collect();

        ser_fns.push(quote! {
            #[doc(hidden)]
            #[allow(dead_code)]
            fn #ser_fn_name<__WireBehavior: #wire_path::bytevec::Behavior>(#receiver, out: &mut #wire_path::bytevec::Builder<__WireBehavior>) {
                match self { #(#ser_arms)* }
            }
        });
    }

    ser_fns
}
