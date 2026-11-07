// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use heck::ToSnakeCase;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Data, DeriveInput, Error, Lit};

pub(crate) fn derive_as_variant(mut input: DeriveInput) -> TokenStream {
    let name = &input.ident;
    let data = match &input.data {
        Data::Enum(data) => data,
        _ => {
            return Error::new_spanned(name, "AsVariant can only be derived for enums")
                .to_compile_error();
        }
    };

    // Get the crate path (default to ::upstream_core::metrics)
    let crate_path = match crate::util::crate_path(
        &mut input.attrs,
        "variant",
        syn::parse_quote!(::upstream_core::metrics),
    ) {
        Ok(path) => path,
        Err(err) => return err.to_compile_error(),
    };

    // Generate variant definitions
    let variant_defs = data
        .variants
        .iter()
        .enumerate()
        .map(|(id, variant)| {
            // Check for name override on the variant
            let mut variant_name = match get_variant_name(&variant.attrs) {
                Ok(Some(name)) => name,
                Ok(None) => variant.ident.to_string().to_snake_case(),
                Err(err) => return err.to_compile_error(),
            };

            if !variant_name.ends_with('\0') {
                variant_name.push('\0');
            }

            quote! {
                #crate_path::info::variant::Builder {
                    id: #id,
                    name: #crate_path::info::Str::new(#variant_name),
                }
                .build()
            }
        })
        .collect::<Vec<_>>();

    // Generate variant_idx match arms
    let variant_idx_arms = data
        .variants
        .iter()
        .enumerate()
        .map(|(id, variant)| {
            let variant_ident = &variant.ident;

            // Handle different variant types
            match &variant.fields {
                syn::Fields::Named(_) => {
                    quote! {
                        #name::#variant_ident { .. } => #id
                    }
                }
                syn::Fields::Unnamed(_) => {
                    quote! {
                        #name::#variant_ident(..) => #id
                    }
                }
                syn::Fields::Unit => {
                    quote! {
                        #name::#variant_ident => #id
                    }
                }
            }
        })
        .collect::<Vec<_>>();

    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    quote! {
        impl #impl_generics #crate_path::AsVariant for #name #ty_generics #where_clause {
            const VARIANTS: &'static [#crate_path::info::Variant] = &[
                #(#variant_defs),*
            ];

            fn variant_idx(&self) -> usize {
                match self {
                    #(#variant_idx_arms),*
                }
            }
        }

        impl #impl_generics #crate_path::AsVariant for &#name #ty_generics #where_clause {
            const VARIANTS: &'static [#crate_path::info::Variant] = &[
                #(#variant_defs),*
            ];

            fn variant_idx(&self) -> usize {
                match self {
                    #(#variant_idx_arms),*
                }
            }
        }
    }
}

// Get the name override from #[variant(name = "override")]
fn get_variant_name(attrs: &[Attribute]) -> Result<Option<String>, Error> {
    let mut variant_name = None;

    for attr in attrs {
        if !attr.path().is_ident("variant") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                if variant_name.is_some() {
                    return Err(meta.error("duplicate variant name attribute"));
                }
                let value = meta.value()?;
                let lit: Lit = value.parse()?;
                if let Lit::Str(s) = lit {
                    variant_name = Some(s.value());
                    Ok(())
                } else {
                    Err(meta.error("variant name must be a string literal"))
                }
            } else if meta.path.is_ident("crate") {
                // Skip crate attributes - they're handled at the enum level
                Ok(())
            } else {
                Err(meta.error("unsupported variant attribute"))
            }
        })?;
    }

    Ok(variant_name)
}
