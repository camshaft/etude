// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Shared types and utilities for the Wire derive macro.
//!
//! This module has no dependencies on other wire submodules, breaking
//! what would otherwise be a cyclic dependency between wire.rs and its
//! submodules (structs, enums, versioning).

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{Attribute, Fields, Path};

/// All top-level `#[wire(...)]` attributes collected in a single pass.
pub(super) struct WireAttrs {
    pub wire_path: Path,
    pub schema: Option<syn::Type>,
    /// Minimum retained version (e.g., `#[wire(min_version = 2)]`)
    pub min_version: Option<u32>,
    /// Also derive `SerializeRef` (`#[wire(borrowed)]`), so the value can be
    /// serialized by reference. Opt-in, since it requires every field type to
    /// implement `SerializeRef`.
    pub borrowed: bool,
}

impl WireAttrs {
    pub fn parse(attrs: &mut Vec<Attribute>) -> syn::parse::Result<Self> {
        let mut crate_path: Option<Path> = None;
        let mut schema: Option<syn::Type> = None;
        let mut min_version: Option<u32> = None;
        let mut borrowed = false;
        let mut errors: Option<syn::parse::Error> = None;

        attrs.retain(|attr| {
            if !attr.path().is_ident("wire") {
                return true;
            }

            if let Err(err) = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("crate") {
                    if crate_path.is_some() {
                        return Err(meta.error("duplicate wire crate attribute"));
                    }
                    crate_path = Some(meta.value()?.call(Path::parse_mod_style)?);
                    Ok(())
                } else if meta.path.is_ident("schema") {
                    if schema.is_some() {
                        return Err(meta.error("duplicate wire schema attribute"));
                    }
                    schema = Some(meta.value()?.parse()?);
                    Ok(())
                } else if meta.path.is_ident("min_version") {
                    if min_version.is_some() {
                        return Err(meta.error("duplicate wire min_version attribute"));
                    }
                    let value: syn::LitInt = meta.value()?.parse()?;
                    min_version = Some(value.base10_parse()?);
                    Ok(())
                } else if meta.path.is_ident("borrowed") {
                    if borrowed {
                        return Err(meta.error("duplicate wire borrowed attribute"));
                    }
                    borrowed = true;
                    Ok(())
                } else {
                    Err(meta.error("unknown wire attribute"))
                }
            }) {
                match &mut errors {
                    None => errors = Some(err),
                    Some(errors) => errors.combine(err),
                }
            }

            false
        });

        if let Some(errors) = errors {
            return Err(errors);
        }

        Ok(Self {
            wire_path: crate_path.unwrap_or_else(|| syn::parse_quote!(::upstream_core::wire)),
            schema,
            min_version,
            borrowed,
        })
    }
}

/// A parsed tag in `VERSION.SEQUENCE` format, used on both struct fields and enum variants.
#[derive(Clone)]
pub(super) struct ItemTag {
    pub version: u32,
    pub sequence: u32,
}

/// Unified wire attributes for struct fields and enum variants, parsed in a single pass.
///
/// Both `default` and `fallback` annotations parse into `default_expr` — they serve the
/// same purpose (what to use when the item isn't available at a given version) in different
/// contexts: `default` for struct fields (deserializer fills a value), `fallback` for enum
/// variants (serializer converts to an older variant).
pub(super) struct ItemWireAttrs {
    pub skip: bool,
    pub recurse: bool,
    pub tag: Option<ItemTag>,
    pub default_expr: Option<syn::Expr>,
}

impl ItemWireAttrs {
    pub fn parse(attrs: &[Attribute]) -> syn::parse::Result<Self> {
        let mut skip = false;
        let mut recurse = false;
        let mut tag: Option<ItemTag> = None;
        let mut default_expr: Option<syn::Expr> = None;

        for attr in attrs {
            if !attr.path().is_ident("wire") {
                continue;
            }

            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("skip") {
                    skip = true;
                    Ok(())
                } else if meta.path.is_ident("recurse") {
                    recurse = true;
                    Ok(())
                } else if meta.path.is_ident("tag") {
                    let value: syn::LitFloat = meta
                        .value()?
                        .parse()
                        .map_err(|_| meta.error("tag must be in V.S format (e.g., tag = 1.1)"))?;
                    let s = value.to_string();
                    let parts: Vec<&str> = s.split('.').collect();
                    if parts.len() != 2 {
                        return Err(meta.error("tag must be in V.S format (e.g., tag = 1.1)"));
                    }
                    let version: u32 = parts[0]
                        .parse()
                        .map_err(|_| meta.error("tag version part must be a positive integer"))?;
                    let sequence: u32 = parts[1]
                        .parse()
                        .map_err(|_| meta.error("tag sequence part must be a positive integer"))?;
                    if version == 0 {
                        return Err(meta.error("tag version must be >= 1"));
                    }
                    tag = Some(ItemTag { version, sequence });
                    Ok(())
                } else if meta.path.is_ident("default") || meta.path.is_ident("fallback") {
                    if meta.input.peek(syn::Token![=]) {
                        let expr: syn::Expr = meta.value()?.parse()?;
                        default_expr = Some(expr);
                    } else {
                        default_expr = Some(syn::parse_quote!(Default::default()));
                    }
                    Ok(())
                } else {
                    Err(meta.error("unknown wire attribute"))
                }
            })?;
        }

        if skip && tag.is_some() {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "an item cannot have both #[wire(skip)] and #[wire(tag = ...)]; \
                 skip means the item is not on the wire, tag means it is",
            ));
        }

        if recurse && tag.is_some() {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "an item cannot have both #[wire(recurse)] and #[wire(tag = ...)]; \
                 recursive versioned types would cause infinite cross-product expansion",
            ));
        }

        Ok(Self {
            skip,
            recurse,
            tag,
            default_expr,
        })
    }
}

/// Discriminant for struct/variant field shape.
#[derive(Clone, Copy)]
pub(super) enum ItemKind {
    Named,
    Unnamed,
    Unit,
}

impl From<&Fields> for ItemKind {
    fn from(fields: &Fields) -> Self {
        match fields {
            Fields::Named(_) => Self::Named,
            Fields::Unnamed(_) => Self::Unnamed,
            Fields::Unit => Self::Unit,
        }
    }
}

/// Generate a TYPE tree child token for a field/variant, or `None` if skipped.
pub(super) fn item_type_token(
    wire_path: &Path,
    ty: &syn::Type,
    attrs: &ItemWireAttrs,
) -> Option<TokenStream> {
    if attrs.skip {
        return None;
    }
    Some(if attrs.recurse {
        quote! { #wire_path::Type::RECURSIVE }
    } else {
        quote! { <#ty as #wire_path::Wire>::TYPE }
    })
}

/// Build the three where clauses (Wire, Serialize, Deserialize) from generics.
pub(super) fn build_where_clauses(
    wire_path: &Path,
    generics: &syn::Generics,
) -> (syn::WhereClause, syn::WhereClause, syn::WhereClause) {
    let base = generics
        .split_for_impl()
        .2
        .cloned()
        .unwrap_or_else(|| syn::WhereClause {
            where_token: syn::token::Where {
                span: Span::call_site(),
            },
            predicates: syn::punctuated::Punctuated::new(),
        });
    let mut wire_wc = base.clone();
    let mut ser_wc = base.clone();
    let mut de_wc = base;

    for param in generics.type_params() {
        let p = &param.ident;
        wire_wc
            .predicates
            .push(syn::parse_quote!(#p: #wire_path::Wire));
        ser_wc
            .predicates
            .push(syn::parse_quote!(#p: #wire_path::Serialize));
        de_wc
            .predicates
            .push(syn::parse_quote!(#p: #wire_path::Deserialize));
    }

    (wire_wc, ser_wc, de_wc)
}

/// Where clause for the `SerializeRef` impl: the type's own where clause plus
/// `T: SerializeRef` for each type parameter.
pub(super) fn build_ser_ref_where_clause(
    wire_path: &Path,
    generics: &syn::Generics,
) -> syn::WhereClause {
    let mut wc = generics
        .split_for_impl()
        .2
        .cloned()
        .unwrap_or_else(|| syn::WhereClause {
            where_token: syn::token::Where {
                span: Span::call_site(),
            },
            predicates: syn::punctuated::Punctuated::new(),
        });
    for param in generics.type_params() {
        let p = &param.ident;
        wc.predicates
            .push(syn::parse_quote!(#p: #wire_path::SerializeRef));
    }
    wc
}

/// Emit the `SerializeRef` impl for `#[wire(borrowed)]` types.
///
/// `body` and `at_body` are the by-reference serializer bodies (with `self:
/// &Self`); `at_body`, if provided, overrides `serialize_ref_at`. Generated
/// bodies mirror the by-value ones, so the output is byte-identical.
pub(super) fn emit_serialize_ref_impl(
    wire_path: &Path,
    name: &syn::Ident,
    generics: &syn::Generics,
    body: TokenStream,
    at_body: Option<TokenStream>,
) -> TokenStream {
    let (impl_generics, ty_generics, _) = generics.split_for_impl();
    let wc = build_ser_ref_where_clause(wire_path, generics);

    let at_override = at_body.map(|body| {
        quote! {
            #[inline]
            fn serialize_ref_at<__WireBehavior: #wire_path::bytevec::Behavior>(&self, out: &mut #wire_path::bytevec::Builder<__WireBehavior>, version_idx: u32) {
                #body
            }
        }
    });

    quote! {
        impl #impl_generics #wire_path::SerializeRef for #name #ty_generics #wc {
            fn serialize_ref<__WireBehavior: #wire_path::bytevec::Behavior>(&self, out: &mut #wire_path::bytevec::Builder<__WireBehavior>) { #body }
            #at_override
        }
    }
}

/// Emit Wire/Serialize/Deserialize trait impls.
///
/// `ser_at_impl` and `de_at_impl`, if provided, generate `serialize_at`/`deserialize_at`
/// overrides that propagate the version index to inner fields. Non-versioned structs
/// need this to forward version indices through wrapper types to inner versioned fields.
pub(super) fn emit_wire_impls(
    wire_path: &Path,
    name: &syn::Ident,
    generics: &syn::Generics,
    id: &TokenStream,
    ser_impl: TokenStream,
    de_impl: TokenStream,
    wire_impl: TokenStream,
    schema_override: Option<&syn::Type>,
) -> TokenStream {
    emit_wire_impls_with_at(
        wire_path,
        name,
        generics,
        id,
        ser_impl,
        de_impl,
        None,
        None,
        wire_impl,
        schema_override,
    )
}

/// Emit Wire/Serialize/Deserialize trait impls with optional serialize_at/deserialize_at.
pub(super) fn emit_wire_impls_with_at(
    wire_path: &Path,
    name: &syn::Ident,
    generics: &syn::Generics,
    id: &TokenStream,
    ser_impl: TokenStream,
    de_impl: TokenStream,
    ser_at_impl: Option<TokenStream>,
    de_at_impl: Option<TokenStream>,
    wire_impl: TokenStream,
    schema_override: Option<&syn::Type>,
) -> TokenStream {
    let (impl_generics, ty_generics, _) = generics.split_for_impl();
    let (wire_wc, ser_wc, de_wc) = build_where_clauses(wire_path, generics);

    let wire_type_impl = if let Some(schema_ty) = schema_override {
        quote! { const TYPE: #wire_path::Type = #wire_path::Type { name: stringify!(#name), id: #id, children: &[<#schema_ty as #wire_path::Wire>::TYPE] }; }
    } else {
        quote! { const TYPE: #wire_path::Type = #wire_path::Type { name: stringify!(#name), id: #id, children: &[#wire_impl] }; }
    };

    let ser_at_override = ser_at_impl.map(|body| {
        quote! {
            #[inline]
            fn serialize_at<__WireBehavior: #wire_path::bytevec::Behavior>(self, out: &mut #wire_path::bytevec::Builder<__WireBehavior>, version_idx: u32) {
                #body
            }
        }
    });

    let de_at_override = de_at_impl.map(|body| {
        quote! {
            #[inline]
            fn deserialize_at(bytes: &mut #wire_path::bytevec::ByteVec, version_idx: u32)
                -> Result<Self, #wire_path::de::Error>
            {
                #body
            }
        }
    });

    quote! {
        impl #impl_generics #wire_path::Serialize for #name #ty_generics #ser_wc {
            fn serialize<__WireBehavior: #wire_path::bytevec::Behavior>(self, out: &mut #wire_path::bytevec::Builder<__WireBehavior>) { #ser_impl }
            #ser_at_override
        }
        impl #impl_generics #wire_path::Deserialize for #name #ty_generics #de_wc {
            fn deserialize(bytes: &mut #wire_path::bytevec::ByteVec) -> Result<Self, #wire_path::de::Error> { #de_impl }
            #de_at_override
        }
        impl #impl_generics #wire_path::Wire for #name #ty_generics #wire_wc { #wire_type_impl }
    }
}

/// Name, receiver, and field-borrow tokens for a generated serializer fn:
/// `(name, self, )` by value, or `(name with _ref, &self, &)` by reference.
pub(super) fn ser_fn_shape(by_ref: bool, base: &str) -> (syn::Ident, TokenStream, TokenStream) {
    if by_ref {
        let name = base.replacen("__wire_serialize", "__wire_serialize_ref", 1);
        (format_ident!("{}", name), quote!(&self), quote!(&))
    } else {
        (format_ident!("{}", base), quote!(self), quote!())
    }
}
