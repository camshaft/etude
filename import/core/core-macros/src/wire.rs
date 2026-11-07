// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Wire derive macro implementation.
//!
//! Module structure (dependency graph is acyclic):
//!
//! ```text
//! wire.rs         → common, structs, enums    (entry point — thin dispatcher)
//! structs.rs      → common, versioning        (struct codegen)
//! enums.rs        → common, versioning        (enum codegen)
//! versioning.rs   → common                    (shared versioning logic)
//! common.rs       → (nothing)                 (shared types and utilities)
//! ```

mod common;
mod enums;
mod structs;
mod versioning;

use common::WireAttrs;
use proc_macro2::TokenStream;
use syn::{Data, DeriveInput};

pub(crate) fn derive_wire(mut input: DeriveInput) -> TokenStream {
    let WireAttrs {
        wire_path,
        schema: schema_override,
        min_version,
        borrowed,
    } = match WireAttrs::parse(&mut input.attrs) {
        Ok(v) => v,
        Err(err) => return err.into_compile_error(),
    };

    let name = &input.ident;
    let generics = &input.generics;

    let result = match &input.data {
        Data::Struct(data) => structs::derive(
            &wire_path,
            name,
            generics,
            data,
            min_version,
            schema_override.as_ref(),
            borrowed,
        ),
        Data::Enum(data) => enums::derive(
            &wire_path,
            name,
            generics,
            data,
            schema_override.as_ref(),
            min_version,
            borrowed,
        ),
        Data::Union(_) => panic!("Unions cannot derive Wire"),
    };

    match result {
        Ok(tokens) => tokens,
        Err(err) => err.into_compile_error(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    /// Helper to assert the generated output contains all three trait impls.
    fn assert_has_wire_traits(s: &str) {
        assert!(s.contains("Serialize"), "missing Serialize impl");
        assert!(s.contains("Deserialize"), "missing Deserialize impl");
        assert!(s.contains("Wire"), "missing Wire impl");
    }

    // ── Struct tests ──────────────────────────────────────────────

    #[test]
    fn struct_named_fields() {
        let input = parse_quote! { struct Test { a: u64, b: String } };
        let s = derive_wire(input).to_string();
        assert_has_wire_traits(&s);
        assert!(s.contains("u64"), "missing u64 in TYPE tree");
        assert!(s.contains("String"), "missing String in TYPE tree");
    }

    #[test]
    fn struct_unnamed_fields() {
        let input = parse_quote! { struct Pair(u32, u64); };
        let s = derive_wire(input).to_string();
        assert_has_wire_traits(&s);
    }

    #[test]
    fn struct_unit() {
        let input = parse_quote! { struct Unit; };
        let s = derive_wire(input).to_string();
        assert_has_wire_traits(&s);
        assert!(
            s.contains("children : & []"),
            "unit struct should have empty children"
        );
    }

    #[test]
    fn struct_skip_field() {
        let input = parse_quote! {
            struct WithSkip { a: u64, #[wire(skip)] _phantom: std::marker::PhantomData<()> }
        };
        let s = derive_wire(input).to_string();
        assert_has_wire_traits(&s);
        assert!(
            s.contains("Default :: default ()"),
            "skipped field should use Default"
        );
    }

    #[test]
    fn struct_generic() {
        let input =
            parse_quote! { struct Generic<T> where T: Clone + Debug + PartialEq { value: T } };
        let s = derive_wire(input).to_string();
        assert_has_wire_traits(&s);
        assert!(s.contains("Wire"), "missing Wire bound on generic");
    }

    #[test]
    fn borrowed_is_opt_in() {
        let plain = derive_wire(parse_quote! { struct Test { a: u64 } }).to_string();
        assert!(
            !plain.contains("SerializeRef"),
            "SerializeRef without opt-in"
        );

        let input = parse_quote! { #[wire(borrowed)] struct Test { a: u64 } };
        let s = derive_wire(input).to_string();
        assert!(s.contains("SerializeRef"), "missing SerializeRef impl");
        assert!(s.contains("& self . a"), "field not borrowed");
    }

    #[test]
    fn borrowed_versioned_emits_ref_dispatch() {
        let input = parse_quote! {
            #[wire(borrowed)]
            enum Mode {
                #[wire(tag = 1.0)] A,
                #[wire(tag = 2.0, fallback = Mode::A)] B(u64),
            }
        };
        let s = derive_wire(input).to_string();
        assert!(s.contains("__wire_serialize_ref_v2"), "missing ref fn");
        assert!(
            s.contains("__wire_serialize_ref_at"),
            "missing ref dispatch"
        );
    }

    #[test]
    fn borrowed_duplicate_is_rejected() {
        let input = parse_quote! { #[wire(borrowed, borrowed)] struct Test { a: u64 } };
        let s = derive_wire(input).to_string();
        assert!(s.contains("duplicate wire borrowed attribute"));
    }

    #[test]
    fn struct_custom_crate_path() {
        let input = parse_quote! { #[wire(crate = upstream_wire)] struct Test { value: u32 } };
        let s = derive_wire(input).to_string();
        assert!(s.contains("upstream_wire"), "should use custom crate path");
        assert!(
            !s.contains("upstream_core"),
            "should not use default crate path"
        );
    }

    #[test]
    fn struct_custom_crate_nested_path() {
        let input =
            parse_quote! { #[wire(crate = crate::custom::wire)] struct Test { value: u32 } };
        let s = derive_wire(input).to_string();
        assert!(s.contains("crate :: custom :: wire"));
    }

    #[test]
    fn struct_schema_override() {
        let input = parse_quote! {
            #[wire(schema = Vec<T>)]
            struct Wrapper<T> { data: Vec<u8>, #[wire(skip)] _marker: std::marker::PhantomData<T> }
        };
        let s = derive_wire(input).to_string();
        assert_has_wire_traits(&s);
        assert!(s.contains("Vec < T >"), "schema override should use Vec<T>");
    }

    #[test]
    fn struct_schema_with_crate() {
        let input = parse_quote! { #[wire(crate = my_wire, schema = Vec<T>)] struct Wrapper<T> { data: Vec<u8> } };
        let s = derive_wire(input).to_string();
        assert!(s.contains("my_wire"));
        assert!(s.contains("Vec < T >"));
    }

    // ── Struct versioning tests ───────────────────────────────────

    #[test]
    fn struct_versioned_basic() {
        let input = parse_quote! {
            struct ConsistencyLevel {
                #[wire(tag = 1.1)] mode: u32,
                #[wire(tag = 2.1, default = None)] timeout: Option<u64>,
            }
        };
        let s = derive_wire(input).to_string();
        assert!(s.contains("__WIRE_TYPE_V1"), "missing v1 TYPE tree");
        assert!(s.contains("__WIRE_TYPE_V2"), "missing v2 TYPE tree");
        assert!(s.contains("VersionedWire"), "missing VersionedWire impl");
        assert!(s.contains("__wire_serialize_v1"), "missing v1 serializer");
        assert!(s.contains("__wire_serialize_v2"), "missing v2 serializer");
        assert!(
            s.contains("__wire_deserialize_v1"),
            "missing v1 deserializer"
        );
        assert!(
            s.contains("__wire_deserialize_v2"),
            "missing v2 deserializer"
        );
        assert!(s.contains("VERSION_TYPES"), "missing VERSION_TYPES array");
    }

    #[test]
    fn struct_versioned_min_version_prunes() {
        let input = parse_quote! {
            #[wire(min_version = 2)]
            struct Foo { #[wire(tag = 1.1)] a: u64, #[wire(tag = 2.1, default = None)] b: Option<String> }
        };
        let s = derive_wire(input).to_string();
        assert!(
            !s.contains("__WIRE_TYPE_V1"),
            "v1 should be pruned by min_version"
        );
        assert!(s.contains("__WIRE_TYPE_V2"), "v2 should remain");
    }

    #[test]
    fn struct_versioned_field_ordering_by_tag() {
        let input = parse_quote! {
            struct Foo {
                #[wire(tag = 2.1, default = false)] c: bool,
                #[wire(tag = 1.1)] a: u64,
                #[wire(tag = 1.2)] b: String,
            }
        };
        let s = derive_wire(input).to_string();
        assert!(s.contains("__wire_serialize_v1"), "missing v1 serializer");
        assert!(s.contains("__wire_serialize_v2"), "missing v2 serializer");
    }

    #[test]
    fn struct_versioned_non_versioned_unchanged() {
        let input = parse_quote! { struct Simple { a: u64, b: String } };
        let s = derive_wire(input).to_string();
        assert!(
            !s.contains("VersionedWire"),
            "non-versioned should not have VersionedWire"
        );
        assert!(
            !s.contains("__WIRE_TYPE_V"),
            "non-versioned should not have version TYPE trees"
        );
        assert_has_wire_traits(&s);
    }

    #[test]
    fn struct_versioned_rejects_mixed_tagged_untagged() {
        let input = parse_quote! { struct Bad { #[wire(tag = 1.1)] a: u64, b: String } };
        assert!(derive_wire(input).to_string().contains("compile_error"));
    }

    #[test]
    fn struct_versioned_rejects_duplicate_tag() {
        let input =
            parse_quote! { struct Bad { #[wire(tag = 1.1)] a: u64, #[wire(tag = 1.1)] b: String } };
        assert!(derive_wire(input).to_string().contains("compile_error"));
    }

    #[test]
    fn struct_versioned_rejects_missing_default_for_v2_field() {
        let input =
            parse_quote! { struct Bad { #[wire(tag = 1.1)] a: u64, #[wire(tag = 2.1)] b: String } };
        assert!(derive_wire(input).to_string().contains("compile_error"));
    }

    #[test]
    fn struct_versioned_rejects_tag_with_skip() {
        let input = parse_quote! {
            struct Bad {
                #[wire(tag = 1.1)] a: u64,
                #[wire(tag = 1.2, skip)] b: u32,
            }
        };
        assert!(
            derive_wire(input).to_string().contains("compile_error"),
            "tag + skip on the same field should produce a compile error"
        );
    }

    #[test]
    fn struct_versioned_skip_without_tag_allowed() {
        let input = parse_quote! {
            struct WithSkip {
                #[wire(tag = 1.1)] a: u64,
                #[wire(tag = 2.1, default = None)] b: Option<String>,
                #[wire(skip)] _phantom: std::marker::PhantomData<()>,
            }
        };
        let s = derive_wire(input).to_string();
        assert!(
            !s.contains("compile_error"),
            "skip-only fields should be allowed in versioned structs"
        );
        assert!(
            s.contains("Default :: default ()"),
            "skip field should use Default::default() in deserializer"
        );
    }

    // ── Enum tests ────────────────────────────────────────────────

    #[test]
    fn enum_unit_variants() {
        let input = parse_quote! { enum Color { Red, Green, Blue } };
        let s = derive_wire(input).to_string();
        assert_has_wire_traits(&s);
        assert!(s.contains("0u8"), "missing discriminant for first variant");
    }

    #[test]
    fn enum_unnamed_fields() {
        let input = parse_quote! { enum Value { Int(u64), Float(f64) } };
        let s = derive_wire(input).to_string();
        assert_has_wire_traits(&s);
        assert!(s.contains("u64"), "missing u64 in TYPE tree");
        assert!(s.contains("f64"), "missing f64 in TYPE tree");
    }

    #[test]
    fn enum_named_fields() {
        let input = parse_quote! { enum Event { Click { x: u32, y: u32 }, Scroll { delta: i32 } } };
        let s = derive_wire(input).to_string();
        assert_has_wire_traits(&s);
        assert!(
            s.contains("Click"),
            "missing Click variant name in TYPE tree"
        );
        assert!(
            s.contains("Scroll"),
            "missing Scroll variant name in TYPE tree"
        );
    }

    #[test]
    fn enum_mixed_variant_kinds() {
        let input = parse_quote! {
            enum Message { Ping, Data(Vec<u8>), Error { code: u32, msg: String } }
        };
        let s = derive_wire(input).to_string();
        assert_has_wire_traits(&s);
        assert!(s.contains("Ping"));
        assert!(s.contains("Data"));
        assert!(s.contains("Error"));
    }

    #[test]
    fn enum_skip_variant() {
        let input = parse_quote! {
            enum WithSkip { Active(u32), #[wire(skip)] Deprecated(u32) }
        };
        let s = derive_wire(input).to_string();
        assert_has_wire_traits(&s);
        assert!(
            s.contains("unreachable"),
            "skipped variant should be unreachable"
        );
    }

    #[test]
    fn enum_recursive_variant() {
        let input = parse_quote! {
            enum Tree { Leaf(u32), Branch(#[wire(recurse)] Box<Tree>, #[wire(recurse)] Box<Tree>) }
        };
        let s = derive_wire(input).to_string();
        assert_has_wire_traits(&s);
        assert!(
            s.contains("RECURSIVE"),
            "recursive fields should use Type::RECURSIVE"
        );
    }

    #[test]
    fn enum_single_variant() {
        let input = parse_quote! { enum Single { Only(u64) } };
        let s = derive_wire(input).to_string();
        assert_has_wire_traits(&s);
    }

    #[test]
    fn enum_with_nested_struct_field() {
        let input = parse_quote! { enum Wrapper { Empty, WithData(Inner) } };
        let s = derive_wire(input).to_string();
        assert_has_wire_traits(&s);
        assert!(s.contains("Inner"), "TYPE tree should reference Inner type");
    }

    // ── Enum versioning tests ─────────────────────────────────────

    #[test]
    fn enum_versioned_basic() {
        let input = parse_quote! {
            enum ConsistencyMode {
                #[wire(tag = 1.0)] Linearizable,
                #[wire(tag = 1.1)] Eventual,
                #[wire(tag = 2.0, fallback = ConsistencyMode::Eventual)] BoundedStaleness(u64),
            }
        };
        let s = derive_wire(input).to_string();
        assert!(s.contains("__WIRE_TYPE_V1"), "missing v1 TYPE tree");
        assert!(s.contains("__WIRE_TYPE_V2"), "missing v2 TYPE tree");
        assert!(s.contains("VersionedWire"), "missing VersionedWire impl");
        assert!(s.contains("__wire_serialize_v1"), "missing v1 serializer");
        assert!(s.contains("__wire_serialize_v2"), "missing v2 serializer");
        assert!(
            s.contains("__wire_deserialize_v1"),
            "missing v1 deserializer"
        );
        assert!(
            s.contains("__wire_deserialize_v2"),
            "missing v2 deserializer"
        );
        assert!(s.contains("VERSION_TYPES"), "missing VERSION_TYPES array");
    }

    #[test]
    fn enum_versioned_rejects_missing_fallback() {
        let input = parse_quote! { enum Bad { #[wire(tag = 1.0)] A, #[wire(tag = 2.0)] B } };
        assert!(
            derive_wire(input).to_string().contains("compile_error"),
            "variant at version > 1 without fallback should produce a compile error"
        );
    }

    #[test]
    fn enum_versioned_rejects_mixed_tagged_untagged() {
        let input = parse_quote! { enum Bad { #[wire(tag = 1.0)] A, B } };
        assert!(
            derive_wire(input).to_string().contains("compile_error"),
            "mixed tagged/untagged variants should produce a compile error"
        );
    }

    #[test]
    fn enum_versioned_rejects_duplicate_tag() {
        let input = parse_quote! { enum Bad { #[wire(tag = 1.0)] A, #[wire(tag = 1.0)] B } };
        assert!(
            derive_wire(input).to_string().contains("compile_error"),
            "duplicate tags should produce a compile error"
        );
    }

    #[test]
    fn enum_versioned_non_versioned_unchanged() {
        let input = parse_quote! { enum Simple { A, B, C } };
        let s = derive_wire(input).to_string();
        assert!(
            !s.contains("VersionedWire"),
            "non-versioned enum should not have VersionedWire"
        );
        assert!(
            !s.contains("__WIRE_TYPE_V"),
            "non-versioned enum should not have version TYPE trees"
        );
        assert_has_wire_traits(&s);
    }

    #[test]
    fn enum_versioned_min_version_prunes() {
        let input = parse_quote! {
            #[wire(min_version = 2)]
            enum Pruned {
                #[wire(tag = 1.0)] A,
                #[wire(tag = 1.1)] B,
                #[wire(tag = 2.0, fallback = Pruned::B)] C(u32),
            }
        };
        let s = derive_wire(input).to_string();
        assert!(
            !s.contains("__WIRE_TYPE_V1"),
            "v1 should be pruned by min_version"
        );
        assert!(s.contains("__WIRE_TYPE_V2"), "v2 should remain");
    }

    #[test]
    fn enum_versioned_rejects_variant_field_tags() {
        let input = parse_quote! {
            enum Bad {
                #[wire(tag = 1.0)] A,
                #[wire(tag = 2.0, fallback = Bad::A)] B(#[wire(tag = 1.1)] u32),
            }
        };
        assert!(
            derive_wire(input).to_string().contains("compile_error"),
            "variant field versioning is not supported — tags on variant fields should be rejected"
        );
    }

    // ── serialize_at / deserialize_at generation tests ────────────

    #[test]
    fn struct_versioned_generates_at_dispatch() {
        let input = parse_quote! {
            struct CL {
                #[wire(tag = 1.1)] mode: u32,
                #[wire(tag = 2.1, default = None)] timeout: Option<u64>,
            }
        };
        let s = derive_wire(input).to_string();
        assert!(
            s.contains("__wire_serialize_at"),
            "missing __wire_serialize_at inherent fn"
        );
        assert!(
            s.contains("__wire_deserialize_at"),
            "missing __wire_deserialize_at inherent fn"
        );
        assert!(
            s.contains("serialize_at"),
            "missing serialize_at trait override"
        );
        assert!(
            s.contains("deserialize_at"),
            "missing deserialize_at trait override"
        );
    }

    #[test]
    fn enum_versioned_generates_at_dispatch() {
        let input = parse_quote! {
            enum Mode {
                #[wire(tag = 1.0)] A,
                #[wire(tag = 1.1)] B,
                #[wire(tag = 2.0, fallback = Mode::B)] C(u32),
            }
        };
        let s = derive_wire(input).to_string();
        assert!(
            s.contains("serialize_at"),
            "missing serialize_at trait override for versioned enum"
        );
        assert!(
            s.contains("deserialize_at"),
            "missing deserialize_at trait override for versioned enum"
        );
    }

    #[test]
    fn struct_non_versioned_no_at_dispatch() {
        let input = parse_quote! { struct Simple { a: u64, b: String } };
        let s = derive_wire(input).to_string();
        assert!(
            !s.contains("__wire_serialize_at"),
            "non-versioned should not have __wire_serialize_at"
        );
        assert!(
            !s.contains("__wire_deserialize_at"),
            "non-versioned should not have __wire_deserialize_at"
        );
    }

    #[test]
    fn struct_versioned_at_dispatch_arm_count() {
        let input = parse_quote! {
            struct Foo {
                #[wire(tag = 1.1)] a: u64,
                #[wire(tag = 2.1, default = None)] b: Option<u32>,
                #[wire(tag = 3.1, default = false)] c: bool,
            }
        };
        let s = derive_wire(input).to_string();
        // 3 outer versions → one serializer and deserializer per version, each
        // reachable from the cross-product `_at` dispatch (a const if-chain over
        // per-version segments, so there are no literal match arms to count).
        assert!(
            s.contains("fn __wire_serialize_at"),
            "missing serialize _at dispatch"
        );
        assert!(
            s.contains("fn __wire_deserialize_at"),
            "missing deserialize _at dispatch"
        );
        for ver in 1..=3 {
            assert!(
                s.contains(&format!("fn __wire_serialize_v{ver}")),
                "missing serializer for v{ver}"
            );
            assert!(
                s.contains(&format!("fn __wire_deserialize_v{ver}")),
                "missing deserializer for v{ver}"
            );
        }
        assert!(
            !s.contains("fn __wire_serialize_v4"),
            "unexpected fourth version"
        );
    }

    #[test]
    fn struct_versioned_nested_generates_at() {
        let input = parse_quote! {
            struct ReadRequest {
                #[wire(tag = 1.1)] key: u64,
                #[wire(tag = 2.1, default = None)] consistency: Option<Inner>,
            }
        };
        let s = derive_wire(input).to_string();
        assert!(
            s.contains("__wire_serialize_at"),
            "nested versioned struct should have __wire_serialize_at"
        );
        assert!(
            s.contains("__wire_deserialize_at"),
            "nested versioned struct should have __wire_deserialize_at"
        );
    }

    // ── Cross-type nesting tests ──────────────────────────────────

    #[test]
    fn struct_with_enum_field() {
        let input = parse_quote! { struct Request { id: u64, status: Status } };
        let s = derive_wire(input).to_string();
        assert_has_wire_traits(&s);
        assert!(
            s.contains("Status"),
            "TYPE tree should reference Status enum"
        );
    }

    #[test]
    fn struct_with_nested_struct_field() {
        let input = parse_quote! { struct Outer { header: Header, payload: Vec<u8> } };
        let s = derive_wire(input).to_string();
        assert_has_wire_traits(&s);
        assert!(
            s.contains("Header"),
            "TYPE tree should reference Header struct"
        );
    }

    #[test]
    fn struct_with_option_field() {
        let input = parse_quote! { struct MaybeValue { value: Option<u64> } };
        let s = derive_wire(input).to_string();
        assert_has_wire_traits(&s);
        assert!(
            s.contains("Option"),
            "TYPE tree should reference Option type"
        );
    }
}
