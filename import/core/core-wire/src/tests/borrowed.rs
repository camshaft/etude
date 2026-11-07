// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Serialization by reference (`SerializeRef`, `serialize_borrowed`) must be
//! byte-identical to by-value serialization, at every version index.

use crate::{Serialize, SerializeRef, TaggedWire, Wire, serialize, serialize_borrowed};
use bolero::check;
use bytes::Bytes;
use upstream_bytevec::{Builder, ByteVec};
use std::collections::{BTreeMap, VecDeque};

fn flat(bytes: &ByteVec) -> Vec<u8> {
    bytes
        .chunks()
        .flat_map(|chunk| chunk.iter().copied())
        .collect()
}

/// Asserts every by-reference path matches its by-value counterpart: the
/// tagged top-level encoding, the plain encoding (through `SerializeRef` and
/// through `&T: Serialize`), and `serialize_at` at every cross-product index.
fn assert_same<T: Serialize + SerializeRef + Wire + Clone>(value: &T) {
    assert_eq!(
        flat(&serialize(value.clone())),
        flat(&serialize_borrowed(value)),
        "serialize_borrowed differs from serialize"
    );

    let mut by_value = Builder::default();
    value.clone().serialize(&mut by_value);
    let by_value = flat(&by_value.finish());

    let mut by_ref = Builder::default();
    value.serialize_ref(&mut by_ref);
    assert_eq!(by_value, flat(&by_ref.finish()), "serialize_ref differs");

    let mut via_ref_impl = Builder::default();
    Serialize::serialize(value, &mut via_ref_impl);
    assert_eq!(
        by_value,
        flat(&via_ref_impl.finish()),
        "&T: Serialize differs"
    );

    for idx in 0..T::VERSION_XPRODUCT_COUNT as u32 {
        let mut by_value = Builder::default();
        value.clone().serialize_at(&mut by_value, idx);
        let mut by_ref = Builder::default();
        value.serialize_ref_at(&mut by_ref, idx);
        assert_eq!(
            flat(&by_value.finish()),
            flat(&by_ref.finish()),
            "serialize_ref_at differs at index {idx}"
        );
    }
}

macro_rules! generate_borrowed_test {
    ($test_name:ident, $type:ty) => {
        #[test]
        fn $test_name() {
            check!()
                .with_type::<$type>()
                .for_each(|value| assert_same(value));
        }
    };
}

generate_borrowed_test!(borrowed_bool, bool);
generate_borrowed_test!(borrowed_u8, u8);
generate_borrowed_test!(borrowed_u64, u64);
generate_borrowed_test!(borrowed_i128, i128);
generate_borrowed_test!(borrowed_usize, usize);
generate_borrowed_test!(borrowed_f64, f64);
generate_borrowed_test!(borrowed_array, [u8; 16]);
generate_borrowed_test!(borrowed_string, String);
generate_borrowed_test!(borrowed_vec_u8, Vec<u8>);
generate_borrowed_test!(borrowed_vec_u32, Vec<u32>);
generate_borrowed_test!(borrowed_vec_string, Vec<String>);
generate_borrowed_test!(borrowed_vecdeque, VecDeque<u16>);
generate_borrowed_test!(borrowed_option, Option<Vec<u8>>);
generate_borrowed_test!(borrowed_box, Box<String>);
generate_borrowed_test!(borrowed_result, Result<u32, String>);
generate_borrowed_test!(borrowed_btreemap, BTreeMap<u32, String>);
generate_borrowed_test!(borrowed_tuple, (u8, String, Vec<u16>, bool));
generate_borrowed_test!(borrowed_duration, core::time::Duration);
generate_borrowed_test!(borrowed_socket_addr, std::net::SocketAddr);
generate_borrowed_test!(borrowed_range, core::ops::Range<u32>);

#[test]
fn borrowed_bound() {
    use core::ops::Bound;
    check!().with_type::<u64>().for_each(|value| {
        assert_same(&Bound::Included(*value));
        assert_same(&Bound::Excluded(*value));
        assert_same(&Bound::<u64>::Unbounded);
    });
}

#[test]
fn borrowed_bytes() {
    check!()
        .with_type::<Vec<u8>>()
        .for_each(|value| assert_same(&Bytes::from(value.clone())));
}

#[test]
fn borrowed_bytevec() {
    check!()
        .with_type::<Vec<u8>>()
        .for_each(|value| assert_same(&ByteVec::from(value.clone())));
}

#[test]
fn borrowed_str_matches_string() {
    let owned = String::from("segment");
    assert_eq!(
        flat(&serialize(owned.clone())),
        flat(&serialize_borrowed(&owned)),
    );
    let mut from_str = Builder::default();
    "segment".serialize_ref(&mut from_str);
    let mut from_string = Builder::default();
    owned.serialize(&mut from_string);
    assert_eq!(flat(&from_str.finish()), flat(&from_string.finish()));
}

// ── Derived types ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, bolero::TypeGenerator, upstream_macros::Wire)]
#[wire(crate = crate, borrowed)]
struct Leaf {
    id: u64,
    name: String,
    data: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, bolero::TypeGenerator, upstream_macros::Wire)]
#[wire(crate = crate, borrowed)]
struct Pair(u32, #[wire(skip)] bool, String);

#[derive(Clone, Debug, PartialEq, bolero::TypeGenerator, upstream_macros::Wire)]
#[wire(crate = crate, borrowed)]
struct Unit;

#[derive(Clone, Debug, PartialEq, bolero::TypeGenerator, upstream_macros::Wire)]
#[wire(crate = crate, borrowed)]
struct Generic<T> {
    value: T,
    next: Option<(T, T)>,
}

#[derive(Clone, Debug, PartialEq, bolero::TypeGenerator, upstream_macros::Wire)]
#[wire(crate = crate, borrowed)]
enum Block {
    Text(String),
    Leaf {
        leaf: Leaf,
        extra: Option<Box<Leaf>>,
    },
    Pair(Pair, Unit),
    Empty,
}

#[derive(Clone, Debug, PartialEq, bolero::TypeGenerator, upstream_macros::Wire)]
#[wire(crate = crate, borrowed)]
struct Tree {
    blocks: Vec<Block>,
    index: BTreeMap<u32, Generic<u16>>,
    tail: Result<Leaf, String>,
}

/// A versioned enum: v2 adds a variant that falls back to `Eventual` at v1.
#[derive(Clone, Debug, PartialEq, bolero::TypeGenerator, upstream_macros::Wire)]
#[wire(crate = crate, borrowed)]
enum Mode {
    #[wire(tag = 1.0)]
    Linearizable,
    #[wire(tag = 1.1)]
    Eventual,
    #[wire(tag = 2.0, fallback = Mode::Eventual)]
    Bounded(u64),
}

/// A versioned struct with a versioned inner field, so `serialize_at` covers
/// the full cross product of outer and inner versions.
#[derive(Clone, Debug, PartialEq, bolero::TypeGenerator, upstream_macros::Wire)]
#[wire(crate = crate, borrowed)]
struct Level {
    #[wire(tag = 1.1)]
    mode: Mode,
    #[wire(tag = 1.2)]
    name: String,
    #[wire(tag = 2.1, default = None)]
    timeout: Option<u64>,
    #[wire(tag = 3.1, default = Vec::new())]
    modes: Vec<Mode>,
}

generate_borrowed_test!(borrowed_derived_struct, Leaf);
generate_borrowed_test!(borrowed_derived_tuple_struct_with_skip, Pair);
generate_borrowed_test!(borrowed_derived_unit_struct, Unit);
generate_borrowed_test!(borrowed_derived_generic, Generic<String>);
generate_borrowed_test!(borrowed_derived_enum, Block);
generate_borrowed_test!(borrowed_derived_nested, Tree);
generate_borrowed_test!(borrowed_versioned_enum, Mode);
generate_borrowed_test!(borrowed_versioned_struct, Level);
generate_borrowed_test!(borrowed_versioned_in_container, Option<Vec<Level>>);

#[test]
fn borrowed_versioned_cross_product_is_covered() {
    // 3 outer versions of `Level`, each multiplied by `Mode`'s 2 versions for
    // the `mode` field (and again for `modes` at v3): the loop in
    // `assert_same` must walk more than the outer version count.
    assert!(Level::VERSION_XPRODUCT_COUNT > 3);
}

#[derive(TaggedWire, Clone, Debug, PartialEq, Default)]
#[wire(crate = crate)]
struct Tagged {
    #[wire(tag = 1.1)]
    name: String,
    #[wire(tag = 1.2)]
    value: u64,
}

#[test]
fn borrowed_tagged_wire() {
    check!()
        .with_type::<(String, u64)>()
        .for_each(|(name, value)| {
            assert_same(&Tagged {
                name: name.clone(),
                value: *value,
            })
        });
}

#[derive(Clone, upstream_macros::Wire)]
#[wire(crate = crate, borrowed)]
struct Segment {
    role: String,
    body: ByteVec,
    attachment: Bytes,
}

/// A borrowed `ByteVec`/`Bytes` field is emitted by sharing its chunks: the
/// output references the same memory, and the value is left untouched.
#[test]
fn borrowed_chunks_are_shared_not_copied() {
    let payload = vec![7u8; 4096];
    let segment = Segment {
        role: "user".into(),
        body: ByteVec::from(payload.clone()),
        attachment: Bytes::from(payload.clone()),
    };

    let out = serialize_borrowed(&segment);
    let shares = |ptr: *const u8| out.chunks().any(|chunk| chunk.as_ptr() == ptr);

    let body_ptr = segment.body.chunks().next().unwrap().as_ptr();
    assert!(shares(body_ptr), "ByteVec chunk was copied");
    assert!(shares(segment.attachment.as_ptr()), "Bytes was copied");

    assert_eq!(flat(&segment.body), payload);
    assert_eq!(flat(&out), flat(&serialize(segment.clone())));
}
