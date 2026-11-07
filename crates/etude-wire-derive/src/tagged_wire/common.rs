// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Shared types and attribute parsing for `#[derive(TaggedWire)]`.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Expr, Fields, Ident, Type};

/// Parsed `#[wire(tag = V.S, default = expr)]` from a struct field.
pub(super) struct ParsedField {
    pub ident: Ident,
    pub ty: Type,
    pub tag_v: u8,
    pub tag_s: u8,
    pub default: Option<Expr>,
}

/// Parsed `#[wire(tag = V.S, fallback)]` from an enum variant.
pub(super) struct ParsedVariant {
    pub ident: Ident,
    pub fields: Fields,
    pub tag_v: u8,
    pub tag_s: u8,
    pub is_fallback: bool,
}

/// What a struct field's `#[wire(...)]` attributes resolve to.
pub(super) enum FieldKind {
    /// A tagged field: `#[wire(tag = V.S)]`, optionally `#[wire(default = expr)]`.
    Tagged {
        tag_v: u8,
        tag_s: u8,
        default: Option<Expr>,
    },
    /// The unknown-catch field: `#[wire(unknown)]`. Collects fields whose tag the
    /// current build does not recognize and re-emits them on serialize, so an
    /// older build does not drop tags written by a newer one. Carries no `tag`.
    Unknown,
}

/// Parse a struct field's `#[wire(...)]` attributes into a [`FieldKind`].
///
/// Returns `Ok(Some(FieldKind::Tagged { .. }))` for a `#[wire(tag = V.S)]` field,
/// `Ok(Some(FieldKind::Unknown))` for the `#[wire(unknown)]` catch field, or
/// `Ok(None)` when the field is neither (the caller turns that into an error).
/// Giving one field both `tag` and `unknown` is rejected here.
pub(super) fn parse_field_attrs(attrs: &[Attribute]) -> syn::parse::Result<Option<FieldKind>> {
    let mut tag_result: Option<(u8, u8)> = None;
    let mut default_result: Option<Expr> = None;
    let mut is_unknown = false;

    for attr in attrs {
        if !attr.path().is_ident("wire") {
            continue;
        }

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("tag") {
                if is_unknown {
                    return Err(meta.error("a field cannot have both `tag` and `unknown`"));
                }
                let value: syn::LitFloat = meta
                    .value()?
                    .parse()
                    .map_err(|_| meta.error("tag must be in V.S format (e.g., tag = 1.1)"))?;
                let s = value.to_string();
                let parts: Vec<&str> = s.split('.').collect();
                if parts.len() != 2 {
                    return Err(meta.error("tag must be in V.S format"));
                }
                let v: u8 = parts[0]
                    .parse()
                    .map_err(|_| meta.error("tag V must be a u8"))?;
                let seq: u8 = parts[1]
                    .parse()
                    .map_err(|_| meta.error("tag S must be a u8"))?;
                tag_result = Some((v, seq));
            } else if meta.path.is_ident("unknown") {
                if tag_result.is_some() {
                    return Err(meta.error("a field cannot have both `tag` and `unknown`"));
                }
                is_unknown = true;
            } else if meta.path.is_ident("default") {
                let expr: Expr = meta.value()?.parse()?;
                default_result = Some(expr);
            } else if meta.path.is_ident("crate") {
                // Skip — handled at the item level
                let _: syn::Path = meta.value()?.parse()?;
            }
            Ok(())
        })?;
    }

    if is_unknown {
        // The catch field carries no tag (a `tag` alongside `unknown` was already
        // rejected above). A `default` on it is meaningless and ignored.
        return Ok(Some(FieldKind::Unknown));
    }

    match tag_result {
        Some((tag_v, tag_s)) => Ok(Some(FieldKind::Tagged {
            tag_v,
            tag_s,
            default: default_result,
        })),
        None => Ok(None),
    }
}

/// Parse `#[wire(tag = V.S, fallback)]` from an enum variant's attributes.
/// Returns `Some(((v, s), is_fallback))` if a `wire` attribute with `tag` is found.
pub(super) fn parse_variant_attrs(
    attrs: &[Attribute],
) -> syn::parse::Result<Option<((u8, u8), bool)>> {
    let mut tag_result = None;
    let mut is_fallback = false;

    for attr in attrs {
        if !attr.path().is_ident("wire") {
            continue;
        }

        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("tag") {
                let value: syn::LitFloat = meta
                    .value()?
                    .parse()
                    .map_err(|_| meta.error("tag must be in V.S format (e.g., tag = 1.1)"))?;
                let s = value.to_string();
                let parts: Vec<&str> = s.split('.').collect();
                if parts.len() != 2 {
                    return Err(meta.error("tag must be in V.S format"));
                }
                let v: u8 = parts[0]
                    .parse()
                    .map_err(|_| meta.error("tag V must be a u8"))?;
                let seq: u8 = parts[1]
                    .parse()
                    .map_err(|_| meta.error("tag S must be a u8"))?;
                tag_result = Some((v, seq));
            } else if meta.path.is_ident("fallback") {
                is_fallback = true;
            }
            Ok(())
        })?;
    }

    match tag_result {
        Some(tag) => Ok(Some((tag, is_fallback))),
        None => Ok(None),
    }
}

/// Look for `#[wire(crate = path)]` on the item to determine the wire crate path.
pub(super) fn find_wire_crate(attrs: &[Attribute]) -> TokenStream {
    for attr in attrs {
        if !attr.path().is_ident("wire") {
            continue;
        }
        let mut crate_path: Option<syn::Path> = None;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("crate") {
                let value = meta.value()?;
                let path: syn::Path = value.parse()?;
                crate_path = Some(path);
            }
            Ok(())
        });
        if let Some(path) = crate_path {
            return quote! { #path };
        }
    }
    quote! { etude_wire }
}
