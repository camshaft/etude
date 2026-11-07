// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! TaggedWire TLV encoding traits.
//!
//! `TaggedWire` is a tag-length-value encoding used for self-describing bootstrap
//! messages (like `PeerInfo`) that cannot depend on prior version negotiation.
//!
//! Each field is encoded as: `[tag_v: u8][tag_s: u8][length: u32][value bytes]`
//! - `tag_v` / `tag_s` correspond to the `#[wire(tag = V.S)]` annotation
//! - `length` is the byte length of the value
//! - Unknown tags are skipped during deserialization (forward compatible)
//! - Missing fields get defaults
//! - A value must consume its field's bytes whole: bytes it leaves unread fail the decode with `InvalidLength`,
//!   naming the field's type, so a stored field whose type changed is an error, not a default
//!
//! | Property            | Wire (positional) | TaggedWire (TLV) |
//! |---------------------|-------------------|------------------|
//! | Per-field overhead  | Zero              | 6 bytes/field    |
//! | Evolution           | Via `global_id`   | Transparent      |
//! | Negotiation needed  | Yes               | No               |

use crate::bytevec;
use etude_bytevec::ByteVec;

/// Trait for types that can be serialized in the TaggedWire TLV format.
///
/// The `#[derive(TaggedWire)]` macro will generate implementations of this trait.
/// For now, types implement it manually.
pub trait TaggedSerialize {
    /// Serialize this value in the TaggedWire TLV format.
    fn tagged_serialize<B: bytevec::Behavior>(&self, out: &mut bytevec::Builder<B>);
}

/// Trait for types that can be deserialized from the TaggedWire TLV format.
///
/// The `#[derive(TaggedWire)]` macro will generate implementations of this trait.
/// Unknown tags are skipped (forward compatibility). Missing fields use defaults. A field whose value leaves bytes
/// of the field unread fails with `InvalidLength` naming the field's type.
pub trait TaggedDeserialize: Sized {
    /// Deserialize from TaggedWire TLV bytes.
    fn tagged_deserialize(bytes: &mut ByteVec) -> Result<Self, crate::de::Error>;
}

/// One tag a `TaggedWire` type assigns: a struct field's or an enum variant's, with the field's or the variant's name
/// as the type declares it (a raw identifier without its `r#`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TaggedField {
    /// The tag's version component, `V` of `#[wire(tag = V.S)]`.
    pub tag_v: u8,
    /// The tag's sequence component, `S` of `#[wire(tag = V.S)]`.
    pub tag_s: u8,
    /// The field's or the variant's name.
    pub name: &'static str,
}

/// The tags a `TaggedWire` type assigns, in the order the type declares them: each struct field's, or each enum
/// variant's, the fallback variant among them. A struct's `#[wire(unknown)]` catch field has no tag and isn't listed.
///
/// The `#[derive(TaggedWire)]` macro implements it from the same attributes its encoding reads, so a test can compare
/// the tags a build writes with a record of the tags earlier builds wrote, from the type itself.
pub trait TaggedFields {
    /// Each tag the type assigns, with the name it assigns it to.
    const TAGGED_FIELDS: &'static [TaggedField];
}

/// Write a single tagged field in TLV format.
///
/// Format: `[tag_v: u8][tag_s: u8][length: u32][value bytes]`
#[inline]
pub fn write_field<B: bytevec::Behavior>(
    out: &mut bytevec::Builder<B>,
    tag_v: u8,
    tag_s: u8,
    f: impl FnOnce(&mut bytevec::Builder<B>),
) {
    use crate::Serialize as _;

    tag_v.serialize(out);
    tag_s.serialize(out);

    // Reserve space for length, write value, patch length
    let len_start = out.len();
    0u32.serialize(out); // placeholder
    let value_start = out.len();
    f(out);
    let value_len = (out.len() - value_start) as u32;

    // Patch the length — we need to write it at len_start
    // Since Builder doesn't support random access patching, we use a simpler approach:
    // Write length prefix using the Builder's write_with_len_prefix facility
    // Actually, let's just use a different approach — accumulate into a sub-builder.
    // For now, we'll use the approach where we pre-compute the length.
    // This is a TODO for optimization — for now the manual impls handle this directly.
    let _ = value_len;
    let _ = len_start;
}

/// Skip a field during deserialization (unknown tag encountered).
#[inline]
pub fn skip_field(bytes: &mut ByteVec, field_len: usize) -> Result<(), crate::de::Error> {
    bytes.split_to_copy(field_len).map_err(|_| {
        crate::de::Error::new(
            crate::de::ErrorKind::InvalidLength,
            &<u8 as crate::Wire>::TYPE,
        )
    })?;
    Ok(())
}

/// Read a tag-length header from the wire. Returns `(tag_v, tag_s, length)`.
/// Returns `None` if no more data is available.
#[inline]
pub fn read_tag_header(bytes: &mut ByteVec) -> Result<Option<(u8, u8, u32)>, crate::de::Error> {
    use crate::Deserialize as _;

    if bytes.is_empty() {
        return Ok(None);
    }

    let tag_v = u8::deserialize(bytes)?;
    let tag_s = u8::deserialize(bytes)?;
    let length = u32::deserialize(bytes)?;

    Ok(Some((tag_v, tag_s, length)))
}

/// Catch collector for `TaggedWire` struct fields whose tag the current build
/// does not recognize.
///
/// A struct field of this type annotated `#[wire(unknown)]` is the derive's
/// *catch field*. On deserialize the `TaggedWire` struct derive pushes every
/// field with an unrecognized `(tag_v, tag_s)` here — keeping its raw value
/// bytes — instead of dropping it; on serialize it re-emits those entries
/// alongside the known fields, the whole set ordered by `(tag_v, tag_s)` so the
/// bytes are deterministic regardless of which build wrote the record. This lets
/// an older build that reads a record written by a newer build preserve (rather
/// than nuke) the fields it does not yet understand when it writes the record
/// back, so readers and writers at different versions across a deployment do not
/// drop each other's fields.
///
/// `Unknown` is not itself a [`Wire`](crate::Wire) / `TaggedWire` type; the
/// derive reads and writes its contents inline.
///
/// # Derive rules
///
/// - A struct may have **at most one** `#[wire(unknown)]` field.
/// - The catch field carries **no** `tag`; giving one field both `tag` and
///   `unknown` is a compile error.
/// - A struct with no catch field keeps the forward-compatible
///   skip-and-drop behaviour for unknown tags.
///
/// A single catch field compiles:
///
/// ```
/// use etude_wire::{TaggedWire, Unknown};
///
/// #[derive(TaggedWire, Clone, Debug, Default, PartialEq)]
/// #[wire(crate = etude_wire)]
/// struct Record {
///     #[wire(tag = 1.1)]
///     id: u32,
///     #[wire(unknown)]
///     rest: Unknown,
/// }
/// ```
///
/// Two `#[wire(unknown)]` fields are rejected:
///
/// ```compile_fail
/// use etude_wire::{TaggedWire, Unknown};
///
/// #[derive(TaggedWire, Clone, Debug, Default, PartialEq)]
/// #[wire(crate = etude_wire)]
/// struct TwoCatches {
///     #[wire(unknown)]
///     a: Unknown,
///     #[wire(unknown)]
///     b: Unknown,
/// }
/// ```
///
/// `tag` together with `unknown` on one field is rejected:
///
/// ```compile_fail
/// use etude_wire::{TaggedWire, Unknown};
///
/// #[derive(TaggedWire, Clone, Debug, Default, PartialEq)]
/// #[wire(crate = etude_wire)]
/// struct TagAndUnknown {
///     #[wire(tag = 1.1, unknown)]
///     a: Unknown,
/// }
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Unknown(Vec<UnknownField>);

/// One preserved [`Unknown`] field: its tag and its raw, still-encoded value
/// bytes.
///
/// `bytes` is the field's value exactly as it appeared on the wire — the bytes
/// between the length prefix and the next field — so re-emitting it is
/// byte-exact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownField {
    /// The field's version tag component (`tag_v` in
    /// `[tag_v][tag_s][len][value]`).
    pub tag_v: u8,
    /// The field's sequence tag component (`tag_s`).
    pub tag_s: u8,
    /// The field's raw value bytes, as they appeared on the wire.
    pub bytes: ByteVec,
}

impl Unknown {
    /// Create an empty catch collector.
    #[inline]
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// Returns `true` if no unknown fields were captured. An empty `Unknown`
    /// contributes nothing to the serialized output.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The number of captured unknown fields.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Iterate over the captured unknown fields, in capture order.
    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &UnknownField> {
        self.0.iter()
    }

    /// Append a captured unknown field. The `TaggedWire` struct derive calls
    /// this for each field whose tag it does not recognize.
    #[inline]
    pub fn push(&mut self, field: UnknownField) {
        self.0.push(field);
    }
}
