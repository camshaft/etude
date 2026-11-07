// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Wire protocol serialization for upstream
//!
//! This crate provides a type-safe serialization framework for upstream's
//! wire protocol. It handles versioning and efficient binary encoding of
//! data structures used in the RPC system.

#[cfg(test)]
#[macro_use]
mod tests;

pub mod de;
pub mod ser;
pub mod tagged;

mod r#impl;
pub use r#impl::varint::VarU64;

use core::fmt;
pub use de::{Deserialize, Error as DeserializeError};
use upstream_bytevec::ByteVec;
pub use upstream_macros::{TaggedWire, Wire};
pub use ser::{Serialize, SerializeRef};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
pub use tagged::{
    TaggedDeserialize, TaggedField, TaggedFields, TaggedSerialize, Unknown, UnknownField,
};

const DEFAULT_BUFFER_CAPACITY: usize = 1 << 8;

pub fn serialize<T: Serialize + Wire>(value: T) -> ByteVec {
    let mut out = bytevec::Builder::new(DEFAULT_BUFFER_CAPACITY);
    value.serialize_tagged(&mut out);
    out.finish()
}

/// Serializes a borrowed value, leaving it with the caller.
///
/// The output is byte-identical to [`serialize`] of the same value: the
/// type's global id, then its encoding. Use it to encode or hash a value that
/// must be kept, without a clone first.
pub fn serialize_borrowed<T: SerializeRef + Wire + ?Sized>(value: &T) -> ByteVec {
    let mut out = bytevec::Builder::new(DEFAULT_BUFFER_CAPACITY);
    value.serialize_ref_tagged(&mut out);
    out.finish()
}

pub fn deserialize<T: Deserialize>(buffer: &mut ByteVec) -> Result<T, de::Error> {
    T::deserialize_tagged(buffer)
}

/// exported so derived types can refer to just a single crate path
#[doc(hidden)]
pub use upstream_bytevec as bytevec;

#[derive(Clone, Copy)]
pub struct Type {
    pub name: &'static str,
    pub id: u8,
    pub children: &'static [Type],
}

impl fmt::Debug for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // if f.alternate() {
        //     return f
        //         .debug_struct("Type")
        //         .field("name", &self.name)
        //         .field("id", &self.id)
        //         .field("children", &self.children)
        //         .finish();
        // }

        match self.id {
            Self::STRUCT => {
                f.write_str("struct ")?;
                let mut s = f.debug_tuple(self.name);
                for child in self.children {
                    s.field(&child);
                }
                s.finish()
            }
            Self::ENUM => {
                f.write_str("enum ")?;
                let mut s = f.debug_tuple(self.name);
                for child in self.children {
                    struct Variant(&'static Type);

                    impl fmt::Debug for Variant {
                        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                            let mut s = f.debug_tuple(self.0.name);
                            for child in self.0.children {
                                s.field(&child);
                            }
                            s.finish()
                        }
                    }

                    s.field(&Variant(child));
                }
                s.finish()
            }
            b'[' => {
                debug_assert!(self.children.len() == 1);
                let child = &self.children[0];
                if child.id == b'1' {
                    return f.write_str("Bytes");
                }

                let mut s = f.debug_list();
                s.entry(&self.children[0]);
                s.finish()
            }
            b'm' => {
                let mut s = f.debug_map();
                debug_assert!(self.children.len() == 2);
                s.entry(&self.children[0], &self.children[1]);
                s.finish()
            }
            b'@' => f.write_str("<recurse>"),
            _ if self.children.is_empty() => f.write_str(self.name),
            _ => f
                .debug_struct("Type")
                .field("name", &self.name)
                .field("id", &self.id)
                .field("children", &self.children)
                .finish(),
        }
    }
}

impl Type {
    pub const STRUCT: u8 = b's';
    pub const ENUM: u8 = b'E';

    pub const RECURSIVE: Self = Self {
        name: "Self",
        id: b'@',
        children: &[],
    };

    pub const fn global_id(&self) -> u64 {
        let mut buffer = [0u8; 4096];
        // start at one so we always have a `0` between the name and the sub-types
        let mut cursor = 1;
        self.serialize_for_hash(&mut buffer, &mut cursor);
        Self::global_id_from_hash_buffer(self.name, &buffer, cursor)
    }

    /// Compute a global_id from pre-filled hash buffer bytes.
    ///
    /// The buffer layout is: `buffer[0] = 0` (sentinel), `buffer[1..]` = hash bytes
    /// from `serialize_for_hash`. `cursor` is the total length including the sentinel.
    /// This is used by the xproduct code to compute global_ids from materialized
    /// hash entries without reconstructing a `Type` tree.
    #[doc(hidden)]
    pub const fn global_id_from_hash_buffer(name: &str, buffer: &[u8], cursor: usize) -> u64 {
        let (buffer, _) = buffer.split_at(cursor);
        let h = upstream_sha2_const::Sha256::new()
            .update(name.as_bytes())
            .update(buffer)
            .finalize();
        u64::from_be_bytes([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]])
    }

    #[doc(hidden)]
    pub const fn serialize_for_hash(&self, buffer: &mut [u8], cursor: &mut usize) {
        buffer[*cursor] = self.id;
        *cursor += 1;
        buffer[*cursor] = 1;
        *cursor += 1;

        let mut idx = 0;

        while idx < self.children.len() {
            self.children[idx].serialize_for_hash(buffer, cursor);
            idx += 1;
        }

        buffer[*cursor] = 0;
        *cursor += 1;
    }
}

pub const fn type_of<W: Wire>() -> &'static Type {
    &W::TYPE
}

/// The per-outer-version TYPE trees for `W`, oldest-first. A non-versioned type
/// returns a single entry equal to `TYPE`; a versioned type returns one entry
/// per retained outer version. References the type's existing `VERSION_TYPES`
/// const — it does not materialize new data.
pub const fn version_types_of<W: Wire>() -> &'static [Type] {
    W::VERSION_TYPES
}

/// Buffer size for cross-product hash entries. Each entry stores the
/// `serialize_for_hash` bytes for one cross-product combination.
pub const XPRODUCT_HASH_BUF_SIZE: usize = 4096;

pub trait Wire {
    const TYPE: Type;

    /// Per-outer-version TYPE trees for this type.
    ///
    /// Non-versioned types have a single entry equal to `TYPE`.
    /// Versioned types (with `#[wire(tag = V.S)]` annotations) have one entry per
    /// retained outer version, ordered from `min_version` to `max_version`.
    const VERSION_TYPES: &'static [Type] = &[Self::TYPE];

    /// Total number of cross-product version combinations.
    ///
    /// For types with versioned inner fields, this is the full cross-product count:
    /// sum over outer versions of (product of inner VERSION_XPRODUCT_COUNT values).
    ///
    /// Non-versioned types have a count of 1.
    /// Simple versioned types (no versioned inner fields) have one entry per outer version.
    ///
    /// The service codegen uses this count to iterate and compute per-combination
    /// `global_id` hashes on-the-fly via `xproduct_hash_bytes`. The
    /// `serialize_at`/`deserialize_at` dispatch indexes into this space.
    const VERSION_XPRODUCT_COUNT: usize = 1;

    /// Pre-computed hash bytes for each cross-product combination.
    ///
    /// Each entry is `(buffer, length)` where `buffer` contains the
    /// `serialize_for_hash` output for the fully-nested TYPE tree at that
    /// cross-product index. The service codegen combines these bytes
    /// with `Type::global_id_from_hash_buffer()` for the final SHA-256.
    /// This guarantees each cross-product combination gets a distinct
    /// `global_id` reflecting its exact wire layout.
    ///
    /// Non-versioned types have a single entry from `TYPE.serialize_for_hash`.
    /// Versioned types with nested versioned fields generate an override
    /// with one entry per cross-product combination, using mixed-radix
    /// decomposition to compose inner types' hash entries.
    #[doc(hidden)]
    const __XPRODUCT_HASH_ENTRIES: &'static [([u8; XPRODUCT_HASH_BUF_SIZE], u16)] = &[{
        let mut buf = [0u8; XPRODUCT_HASH_BUF_SIZE];
        let mut cursor = 0usize;
        Self::TYPE.serialize_for_hash(&mut buf, &mut cursor);
        (buf, cursor as u16)
    }];
}

/// Trait for Wire types that support multiple schema versions.
///
/// Each version produces a distinct `Type` tree and therefore a distinct `global_id`.
/// The Wire derive macro implements this when fields carry `#[wire(tag = V.S)]` annotations.
/// Non-versioned types do not implement this trait.
pub trait VersionedWire: Wire {
    /// The lowest retained version (advanced by `#[wire(min_version = N)]`).
    const MIN_VERSION: u32;

    /// The highest version (inferred from `max(tag version parts)`).
    const MAX_VERSION: u32;

    /// Number of retained versions: `MAX_VERSION - MIN_VERSION + 1`.
    const VERSION_COUNT: u32 = Self::MAX_VERSION - Self::MIN_VERSION + 1;
}

impl Wire for core::convert::Infallible {
    const TYPE: Type = Type {
        name: "never",
        id: b'!',
        children: &[],
    };
}

impl<T> Wire for Option<T>
where
    T: Wire,
{
    const TYPE: Type = Type {
        name: "Option",
        id: Type::ENUM,
        children: &[
            Type {
                name: "None",
                id: 0,
                children: &[],
            },
            Type {
                name: "Some",
                id: 1,
                children: &[T::TYPE],
            },
        ],
    };
}

impl<T> Wire for Box<T>
where
    T: Wire,
{
    const TYPE: Type = T::TYPE;
}

impl<T> Wire for Vec<T>
where
    T: Wire,
{
    const TYPE: Type = Type {
        name: "list",
        id: b'[',
        children: &[T::TYPE],
    };
}

impl<T> Wire for VecDeque<T>
where
    T: Wire,
{
    const TYPE: Type = Type {
        name: "list",
        id: b'[',
        children: &[T::TYPE],
    };
}

impl<K, V> Wire for BTreeMap<K, V>
where
    K: Wire,
    V: Wire,
{
    const TYPE: Type = Type {
        name: "map",
        id: b'm',
        children: &[K::TYPE, V::TYPE],
    };
}

impl<T> Wire for BTreeSet<T>
where
    T: Wire,
{
    const TYPE: Type = Type {
        name: "list",
        id: b'[',
        children: &[T::TYPE],
    };
}

impl Wire for bytes::Bytes {
    const TYPE: Type = <Vec<u8>>::TYPE;
}

impl Wire for ByteVec {
    const TYPE: Type = <Vec<u8>>::TYPE;
}

impl<O: bytevec::tagged::Owner> Wire for bytevec::Tagged<O> {
    const TYPE: Type = <Vec<u8>>::TYPE;
}

impl Wire for String {
    const TYPE: Type = Type {
        name: "String",
        id: b'u',
        children: &[],
    };
}

impl Wire for std::sync::Arc<str> {
    const TYPE: Type = Type {
        name: "String",
        id: b'u',
        children: &[],
    };
}

impl Wire for bool {
    const TYPE: Type = Type {
        name: "bool",
        id: b'B',
        children: &[],
    };
}

// Unsigned integer types
impl Wire for u8 {
    const TYPE: Type = Type {
        name: "u8",
        id: b'1',
        children: &[],
    };
}

impl Wire for u16 {
    const TYPE: Type = Type {
        name: "u16",
        id: b'2',
        children: &[],
    };
}

impl Wire for u32 {
    const TYPE: Type = Type {
        name: "u32",
        id: b'3',
        children: &[],
    };
}

impl Wire for u64 {
    const TYPE: Type = Type {
        name: "u64",
        id: b'4',
        children: &[],
    };
}

impl Wire for u128 {
    const TYPE: Type = Type {
        name: "u128",
        id: b'5',
        children: &[],
    };
}

impl Wire for usize {
    const TYPE: Type = Type {
        name: "usize",
        id: b'6',
        children: &[],
    };
}

// Signed integer types
impl Wire for i8 {
    const TYPE: Type = Type {
        name: "i8",
        id: b'a',
        children: &[],
    };
}

impl Wire for i16 {
    const TYPE: Type = Type {
        name: "i16",
        id: b'b',
        children: &[],
    };
}

impl Wire for i32 {
    const TYPE: Type = Type {
        name: "i32",
        id: b'c',
        children: &[],
    };
}

impl Wire for i64 {
    const TYPE: Type = Type {
        name: "i64",
        id: b'd',
        children: &[],
    };
}

impl Wire for i128 {
    const TYPE: Type = Type {
        name: "i128",
        id: b'e',
        children: &[],
    };
}

impl Wire for isize {
    const TYPE: Type = Type {
        name: "isize",
        id: b'f',
        children: &[],
    };
}

// Floating-point types
impl Wire for f32 {
    const TYPE: Type = Type {
        name: "f32",
        id: b'g',
        children: &[],
    };
}

impl Wire for f64 {
    const TYPE: Type = Type {
        name: "f64",
        id: b'h',
        children: &[],
    };
}

impl<T: Wire, E: Wire> Wire for Result<T, E> {
    const TYPE: Type = Type {
        name: "Result",
        id: Type::ENUM,
        children: &[
            Type {
                name: "Ok",
                id: 0,
                children: &[T::TYPE],
            },
            Type {
                name: "Err",
                id: 1,
                children: &[E::TYPE],
            },
        ],
    };
}

impl<T: Wire, const N: usize> Wire for [T; N] {
    const TYPE: Type = Type {
        name: "array",
        id: Type::STRUCT,
        children: &[T::TYPE; N],
    };
}

macro_rules! impl_tuple_wire {
    // Base case - empty tuple
    () => {
        impl Wire for () {
            const TYPE: Type = Type {
                name: "nil",
                id: b'0',
                children: &[],
            };
        }
    };
    // Recursive case - tuple with at least one element
    ($t0:ident $(,$rest:ident)*) => {
        impl_tuple_wire!($($rest),*);

        impl<$t0: Wire, $($rest: Wire),*> Wire for ($t0, $($rest),*) {
            const TYPE: Type = Type {
                name: "tuple",
                id: Type::STRUCT, // same as a `struct`
                children: &[$t0::TYPE, $($rest::TYPE),*],
            };
        }
    };
}

// Implement for tuples up to 16 elements
impl_tuple_wire!(
    T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15
);
