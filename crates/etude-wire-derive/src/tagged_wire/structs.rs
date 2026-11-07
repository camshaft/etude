// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Struct codegen for `#[derive(TaggedWire)]`.

use super::common::{FieldKind, ParsedField, parse_field_attrs};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, Fields, ext::IdentExt};

pub(super) fn derive_struct(
    input: &DeriveInput,
    wire_path: &TokenStream,
    data: &syn::DataStruct,
) -> TokenStream {
    let name = &input.ident;

    let fields = match &data.fields {
        Fields::Named(fields) => &fields.named,
        _ => {
            return syn::Error::new_spanned(input, "TaggedWire structs must have named fields")
                .to_compile_error();
        }
    };

    // Partition the fields into the known (tagged) fields and the optional
    // `#[wire(unknown)]` catch field. At most one catch field is allowed; a field
    // that is neither tagged nor the catch is an error.
    let mut parsed_fields = Vec::new();
    let mut unknown_field: Option<syn::Ident> = None;
    for field in fields {
        let ident = field.ident.clone().unwrap();
        let ty = field.ty.clone();

        match parse_field_attrs(&field.attrs) {
            Ok(Some(FieldKind::Tagged {
                tag_v,
                tag_s,
                default,
            })) => {
                parsed_fields.push(ParsedField {
                    ident,
                    ty,
                    tag_v,
                    tag_s,
                    default,
                });
            }
            Ok(Some(FieldKind::Unknown)) => {
                if unknown_field.is_some() {
                    return syn::Error::new_spanned(
                        field,
                        "TaggedWire: a struct may have at most one #[wire(unknown)] field",
                    )
                    .to_compile_error();
                }
                unknown_field = Some(ident);
            }
            Ok(None) => {
                return syn::Error::new_spanned(
                    field,
                    "TaggedWire: every field must have #[wire(tag = V.S)] or #[wire(unknown)]",
                )
                .to_compile_error();
            }
            Err(e) => return e.to_compile_error(),
        }
    }

    // ── TaggedSerialize body ──
    //
    // Without a catch field the body is unchanged: each field is written in
    // declaration order as `[tag_v][tag_s][len][value]`.
    //
    // With a catch field, each known field value and each preserved unknown entry
    // is collected keyed by `(tag_v, tag_s)`, the merged set is sorted ascending,
    // and then written. This keeps the bytes deterministic regardless of which
    // build wrote the record (the sealed-record digest depends on byte stability)
    // and re-emits preserved tags interleaved with the known ones.
    let serialize_body = if let Some(unknown_ident) = &unknown_field {
        let known_idents: Vec<&syn::Ident> = parsed_fields.iter().map(|f| &f.ident).collect();
        let known_v: Vec<u8> = parsed_fields.iter().map(|f| f.tag_v).collect();
        let known_s: Vec<u8> = parsed_fields.iter().map(|f| f.tag_s).collect();
        quote! {
            let mut __entries: ::std::vec::Vec<(u8, u8, #wire_path::bytevec::ByteVec)> =
                ::std::vec::Vec::new();
            #(
                {
                    let mut __scratch = #wire_path::bytevec::Builder::new(64usize);
                    #wire_path::Serialize::serialize(self.#known_idents.clone(), &mut __scratch);
                    __entries.push((#known_v as u8, #known_s as u8, __scratch.finish()));
                }
            )*
            for __uf in self.#unknown_ident.iter() {
                __entries.push((__uf.tag_v, __uf.tag_s, __uf.bytes.clone()));
            }
            __entries.sort_by_key(|__e| (__e.0, __e.1));
            for (__tag_v, __tag_s, mut __value) in __entries {
                #wire_path::Serialize::serialize(__tag_v, __out);
                #wire_path::Serialize::serialize(__tag_s, __out);
                __out.write_with_len_prefix(|__out| {
                    __out.append(&mut __value);
                });
            }
        }
    } else {
        let serialize_fields: Vec<TokenStream> = parsed_fields
            .iter()
            .map(|f| {
                let ident = &f.ident;
                let v = f.tag_v;
                let s = f.tag_s;
                quote! {
                    #wire_path::Serialize::serialize(#v as u8, __out);
                    #wire_path::Serialize::serialize(#s as u8, __out);
                    __out.write_with_len_prefix(|__out| {
                        #wire_path::Serialize::serialize(self.#ident.clone(), __out);
                    });
                }
            })
            .collect();
        quote! { #(#serialize_fields)* }
    };

    // ── TaggedDeserialize ──
    let field_decls: Vec<TokenStream> = parsed_fields
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let ty = &f.ty;
            quote! { let mut #ident: Option<#ty> = None; }
        })
        .collect();

    // Collector for the catch field, declared only when one is present.
    let unknown_decl = if let Some(unknown_ident) = &unknown_field {
        quote! { let mut #unknown_ident = #wire_path::Unknown::new(); }
    } else {
        quote! {}
    };

    let match_arms: Vec<TokenStream> = parsed_fields
        .iter()
        .map(|f| {
            let ident = &f.ident;
            let ty = &f.ty;
            let v = f.tag_v;
            let s = f.tag_s;
            // The field's value must consume the field's bytes whole: bytes it leaves unread are a value of another
            // shape than the field's type (a stored `u64` read as an `Option<u64>` takes one byte as the tag and would
            // leave seven), so the decode fails naming the field's type, as a length-prefixed value's does
            // (`Deserialize::deserialize_with_len_prefix`), rather than answer a default and lose what was stored.
            quote! {
                (#v, #s) => {
                    let mut __fbytes = __bytes.split_to(__field_len as usize)
                        .map_err(|_| #wire_path::de::Error::new(
                            #wire_path::de::ErrorKind::InvalidLength,
                            &<u8 as #wire_path::Wire>::TYPE,
                        ))?;
                    let __value = <#ty as #wire_path::Deserialize>::deserialize(&mut __fbytes)?;
                    if !__fbytes.is_empty() {
                        return Err(#wire_path::de::Error::new(
                            #wire_path::de::ErrorKind::InvalidLength,
                            &<#ty as #wire_path::Wire>::TYPE,
                        ));
                    }
                    #ident = Some(__value);
                }
            }
        })
        .collect();

    // The unknown-tag arm. With a catch field the field is preserved (its tag and
    // raw value bytes); without one it keeps the forward-compatible skip-and-drop.
    let unknown_arm = if let Some(unknown_ident) = &unknown_field {
        quote! {
            _ => {
                let __ubytes = __bytes.split_to(__field_len as usize)
                    .map_err(|_| #wire_path::de::Error::new(
                        #wire_path::de::ErrorKind::InvalidLength,
                        &<u8 as #wire_path::Wire>::TYPE,
                    ))?;
                #unknown_ident.push(#wire_path::UnknownField {
                    tag_v: __tag_v,
                    tag_s: __tag_s,
                    bytes: __ubytes,
                });
            }
        }
    } else {
        quote! {
            _ => {
                // Skip unknown field (forward compatibility)
                let _ = __bytes.split_to_copy(__field_len as usize)
                    .map_err(|_| #wire_path::de::Error::new(
                        #wire_path::de::ErrorKind::InvalidLength,
                        &<u8 as #wire_path::Wire>::TYPE,
                    ))?;
            }
        }
    };

    let construct_fields: Vec<TokenStream> = parsed_fields
        .iter()
        .map(|f| {
            let ident = &f.ident;
            if let Some(default_expr) = &f.default {
                quote! { #ident: #ident.unwrap_or_else(|| #default_expr), }
            } else {
                quote! { #ident: #ident.unwrap_or_default(), }
            }
        })
        .collect();

    // Set the catch field from the collector at construct time.
    let construct_unknown = if let Some(unknown_ident) = &unknown_field {
        quote! { #unknown_ident: #unknown_ident, }
    } else {
        quote! {}
    };

    // -- TaggedFields --
    //
    // Each tagged field's tag and name, in declaration order, from the same parsed attributes the encoding reads; the
    // catch field has no tag and isn't listed.
    let tagged_fields: Vec<TokenStream> = parsed_fields
        .iter()
        .map(|f| {
            let v = f.tag_v;
            let s = f.tag_s;
            let field_name = f.ident.unraw().to_string();
            quote! {
                #wire_path::TaggedField { tag_v: #v, tag_s: #s, name: #field_name },
            }
        })
        .collect();

    quote! {
        impl #wire_path::TaggedFields for #name {
            const TAGGED_FIELDS: &'static [#wire_path::TaggedField] = &[
                #(#tagged_fields)*
            ];
        }

        impl #wire_path::TaggedSerialize for #name {
            fn tagged_serialize<__WireBehavior: #wire_path::bytevec::Behavior>(&self, __out: &mut #wire_path::bytevec::Builder<__WireBehavior>) {
                #serialize_body
            }
        }

        impl #wire_path::TaggedDeserialize for #name {
            fn tagged_deserialize(__bytes: &mut #wire_path::bytevec::ByteVec) -> Result<Self, #wire_path::de::Error> {
                #(#field_decls)*
                #unknown_decl

                while !__bytes.is_empty() {
                    let __tag_v = <u8 as #wire_path::Deserialize>::deserialize(__bytes)?;
                    let __tag_s = <u8 as #wire_path::Deserialize>::deserialize(__bytes)?;
                    let __field_len = <u64 as #wire_path::Deserialize>::deserialize(__bytes)? as usize;

                    match (__tag_v, __tag_s) {
                        #(#match_arms)*
                        #unknown_arm
                    }
                }

                Ok(#name {
                    #(#construct_fields)*
                    #construct_unknown
                })
            }
        }

        impl #wire_path::Wire for #name {
            const TYPE: #wire_path::Type = #wire_path::Type {
                name: stringify!(#name),
                id: #wire_path::Type::STRUCT,
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
