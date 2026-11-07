// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Enum codegen for `#[derive(TaggedWire)]`.

use super::common::{ParsedVariant, parse_variant_attrs};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, Fields, Ident, Type, ext::IdentExt};

/// The check that a variant's payload, decoded into its fields from `__fbytes`, consumed the bytes whole: bytes left
/// unread fail the decode with `InvalidLength`, named for `last`, the payload's last field type, the one that stopped
/// short of the end; a payload with no fields has nothing to check.
fn whole_payload(wire_path: &TokenStream, last: Option<&Type>) -> TokenStream {
    match last {
        Some(ty) => quote! {
            if !__fbytes.is_empty() {
                return Err(#wire_path::de::Error::new(
                    #wire_path::de::ErrorKind::InvalidLength,
                    &<#ty as #wire_path::Wire>::TYPE,
                ));
            }
        },
        None => quote! {},
    }
}

pub(super) fn derive_enum(
    input: &DeriveInput,
    wire_path: &TokenStream,
    data: &syn::DataEnum,
) -> TokenStream {
    let name = &input.ident;

    let mut parsed_variants = Vec::new();
    for variant in &data.variants {
        let (tag, is_fallback) = match parse_variant_attrs(&variant.attrs) {
            Ok(Some(v)) => v,
            Ok(None) => {
                return syn::Error::new_spanned(
                    variant,
                    "TaggedWire: every variant must have #[wire(tag = V.S)]",
                )
                .to_compile_error();
            }
            Err(e) => return e.to_compile_error(),
        };

        parsed_variants.push(ParsedVariant {
            ident: variant.ident.clone(),
            fields: variant.fields.clone(),
            tag_v: tag.0,
            tag_s: tag.1,
            is_fallback,
        });
    }

    // ── TaggedSerialize ──
    let serialize_arms: Vec<TokenStream> = parsed_variants
        .iter()
        .map(|v| {
            let vident = &v.ident;
            let tag_v = v.tag_v;
            let tag_s = v.tag_s;

            match &v.fields {
                Fields::Unit => {
                    quote! {
                        #name::#vident => {
                            #wire_path::Serialize::serialize(#tag_v as u8, __out);
                            #wire_path::Serialize::serialize(#tag_s as u8, __out);
                            __out.write_with_len_prefix(|_| {});
                        }
                    }
                }
                Fields::Unnamed(fields) => {
                    let bindings: Vec<Ident> = (0..fields.unnamed.len())
                        .map(|i| Ident::new(&format!("__f{i}"), proc_macro2::Span::call_site()))
                        .collect();
                    let serialize_fields: Vec<TokenStream> = bindings
                        .iter()
                        .map(|b| quote! { #wire_path::Serialize::serialize(#b.clone(), __out); })
                        .collect();
                    quote! {
                        #name::#vident(#(#bindings),*) => {
                            #wire_path::Serialize::serialize(#tag_v as u8, __out);
                            #wire_path::Serialize::serialize(#tag_s as u8, __out);
                            __out.write_with_len_prefix(|__out| {
                                #(#serialize_fields)*
                            });
                        }
                    }
                }
                Fields::Named(fields) => {
                    let field_idents: Vec<&Ident> = fields
                        .named
                        .iter()
                        .map(|f| f.ident.as_ref().unwrap())
                        .collect();
                    let serialize_fields: Vec<TokenStream> = field_idents
                        .iter()
                        .map(|f| quote! { #wire_path::Serialize::serialize(#f.clone(), __out); })
                        .collect();
                    quote! {
                        #name::#vident { #(#field_idents),* } => {
                            #wire_path::Serialize::serialize(#tag_v as u8, __out);
                            #wire_path::Serialize::serialize(#tag_s as u8, __out);
                            __out.write_with_len_prefix(|__out| {
                                #(#serialize_fields)*
                            });
                        }
                    }
                }
            }
        })
        .collect();

    // ── TaggedDeserialize ──
    let deser_match_arms: Vec<TokenStream> = parsed_variants
        .iter()
        .map(|v| {
            let vident = &v.ident;
            let tag_v = v.tag_v;
            let tag_s = v.tag_s;

            match &v.fields {
                Fields::Unit => {
                    quote! {
                        (#tag_v, #tag_s) => {
                            let _ = __bytes.split_to_copy(__field_len as usize)
                                .map_err(|_| #wire_path::de::Error::new(
                                    #wire_path::de::ErrorKind::InvalidLength,
                                    &<u8 as #wire_path::Wire>::TYPE,
                                ))?;
                            Ok(#name::#vident)
                        }
                    }
                }
                // The payload's fields must consume the variant's bytes whole, as a struct's field must (see
                // `structs.rs`): bytes left unread fail the decode, named for the payload's last field, the one that
                // stopped short.
                Fields::Unnamed(fields) => {
                    let field_types: Vec<&Type> = fields.unnamed.iter().map(|f| &f.ty).collect();
                    let last_type = field_types.last().copied();
                    let deser_fields: Vec<TokenStream> = field_types
                        .iter()
                        .map(|ty| {
                            quote! { <#ty as #wire_path::Deserialize>::deserialize(&mut __fbytes)? }
                        })
                        .collect();
                    let consumed = whole_payload(wire_path, last_type);
                    quote! {
                        (#tag_v, #tag_s) => {
                            let mut __fbytes = __bytes.split_to(__field_len as usize)
                                .map_err(|_| #wire_path::de::Error::new(
                                    #wire_path::de::ErrorKind::InvalidLength,
                                    &<u8 as #wire_path::Wire>::TYPE,
                                ))?;
                            let __value = #name::#vident(#(#deser_fields),*);
                            #consumed
                            Ok(__value)
                        }
                    }
                }
                Fields::Named(fields) => {
                    let field_idents: Vec<&Ident> = fields
                        .named
                        .iter()
                        .map(|f| f.ident.as_ref().unwrap())
                        .collect();
                    let field_types: Vec<&Type> = fields.named.iter().map(|f| &f.ty).collect();
                    let deser_bindings: Vec<TokenStream> = field_idents
                        .iter()
                        .zip(field_types.iter())
                        .map(|(id, ty)| {
                            quote! { let #id = <#ty as #wire_path::Deserialize>::deserialize(&mut __fbytes)?; }
                        })
                        .collect();
                    let consumed = whole_payload(wire_path, field_types.last().copied());
                    quote! {
                        (#tag_v, #tag_s) => {
                            let mut __fbytes = __bytes.split_to(__field_len as usize)
                                .map_err(|_| #wire_path::de::Error::new(
                                    #wire_path::de::ErrorKind::InvalidLength,
                                    &<u8 as #wire_path::Wire>::TYPE,
                                ))?;
                            #(#deser_bindings)*
                            #consumed
                            Ok(#name::#vident { #(#field_idents),* })
                        }
                    }
                }
            }
        })
        .collect();

    // Fallback arm for unknown tags
    let fallback_variant = parsed_variants.iter().find(|v| v.is_fallback);
    let unknown_arm = if let Some(fb) = fallback_variant {
        let fb_ident = &fb.ident;
        match &fb.fields {
            Fields::Unit => {
                quote! {
                    _ => {
                        let _ = __bytes.split_to_copy(__field_len as usize)
                            .map_err(|_| #wire_path::de::Error::new(
                                #wire_path::de::ErrorKind::InvalidLength,
                                &<u8 as #wire_path::Wire>::TYPE,
                            ))?;
                        Ok(#name::#fb_ident)
                    }
                }
            }
            Fields::Unnamed(fields) => {
                let defaults: Vec<TokenStream> = fields
                    .unnamed
                    .iter()
                    .map(|_| quote! { Default::default() })
                    .collect();
                quote! {
                    _ => {
                        let _ = __bytes.split_to_copy(__field_len as usize)
                            .map_err(|_| #wire_path::de::Error::new(
                                #wire_path::de::ErrorKind::InvalidLength,
                                &<u8 as #wire_path::Wire>::TYPE,
                            ))?;
                        Ok(#name::#fb_ident(#(#defaults),*))
                    }
                }
            }
            Fields::Named(fields) => {
                let field_idents: Vec<&Ident> = fields
                    .named
                    .iter()
                    .map(|f| f.ident.as_ref().unwrap())
                    .collect();
                let defaults: Vec<TokenStream> = field_idents
                    .iter()
                    .map(|id| quote! { #id: Default::default() })
                    .collect();
                quote! {
                    _ => {
                        let _ = __bytes.split_to_copy(__field_len as usize)
                            .map_err(|_| #wire_path::de::Error::new(
                                #wire_path::de::ErrorKind::InvalidLength,
                                &<u8 as #wire_path::Wire>::TYPE,
                            ))?;
                        Ok(#name::#fb_ident { #(#defaults),* })
                    }
                }
            }
        }
    } else {
        quote! {
            _ => {
                Err(#wire_path::de::Error::new(
                    #wire_path::de::ErrorKind::InvalidVariant,
                    &<#name as #wire_path::Wire>::TYPE,
                ))
            }
        }
    };

    // -- TaggedFields --
    //
    // Each variant's tag and name, in declaration order, the fallback variant among them.
    let tagged_variants: Vec<TokenStream> = parsed_variants
        .iter()
        .map(|v| {
            let tag_v = v.tag_v;
            let tag_s = v.tag_s;
            let variant_name = v.ident.unraw().to_string();
            quote! {
                #wire_path::TaggedField { tag_v: #tag_v, tag_s: #tag_s, name: #variant_name },
            }
        })
        .collect();

    quote! {
        impl #wire_path::TaggedFields for #name {
            const TAGGED_FIELDS: &'static [#wire_path::TaggedField] = &[
                #(#tagged_variants)*
            ];
        }

        impl #wire_path::TaggedSerialize for #name {
            fn tagged_serialize<__WireBehavior: #wire_path::bytevec::Behavior>(&self, __out: &mut #wire_path::bytevec::Builder<__WireBehavior>) {
                match self {
                    #(#serialize_arms)*
                }
            }
        }

        impl #wire_path::TaggedDeserialize for #name {
            fn tagged_deserialize(__bytes: &mut #wire_path::bytevec::ByteVec) -> Result<Self, #wire_path::de::Error> {
                if __bytes.is_empty() {
                    return Err(#wire_path::de::Error::new(
                        #wire_path::de::ErrorKind::InvalidLength,
                        &<#name as #wire_path::Wire>::TYPE,
                    ));
                }

                let __tag_v = <u8 as #wire_path::Deserialize>::deserialize(__bytes)?;
                let __tag_s = <u8 as #wire_path::Deserialize>::deserialize(__bytes)?;
                let __field_len = <u64 as #wire_path::Deserialize>::deserialize(__bytes)? as usize;

                match (__tag_v, __tag_s) {
                    #(#deser_match_arms)*
                    #unknown_arm
                }
            }
        }

        impl #wire_path::Wire for #name {
            const TYPE: #wire_path::Type = #wire_path::Type {
                name: stringify!(#name),
                id: #wire_path::Type::ENUM,
                children: &[],
            };
        }

        impl #wire_path::Serialize for #name {
            fn serialize<__WireBehavior: #wire_path::bytevec::Behavior>(self, out: &mut #wire_path::bytevec::Builder<__WireBehavior>) {
                out.write_with_len_prefix(|out| {
                    #wire_path::TaggedSerialize::tagged_serialize(&self, out);
                });
            }
        }

        impl #wire_path::SerializeRef for #name {
            fn serialize_ref<__WireBehavior: #wire_path::bytevec::Behavior>(&self, out: &mut #wire_path::bytevec::Builder<__WireBehavior>) {
                out.write_with_len_prefix(|out| {
                    #wire_path::TaggedSerialize::tagged_serialize(self, out);
                });
            }
        }

        impl #wire_path::Deserialize for #name {
            fn deserialize(bytes: &mut #wire_path::bytevec::ByteVec) -> Result<Self, #wire_path::de::Error> {
                let len = <u64 as #wire_path::Deserialize>::deserialize(bytes)? as usize;
                let mut bounded = bytes.split_to(len)
                    .map_err(|_| #wire_path::de::Error::new(
                        #wire_path::de::ErrorKind::InvalidLength,
                        &<u8 as #wire_path::Wire>::TYPE,
                    ))?;
                #wire_path::TaggedDeserialize::tagged_deserialize(&mut bounded)
            }
        }
    }
}
