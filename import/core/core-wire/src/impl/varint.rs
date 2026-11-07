// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Variable-length integer encoding for the wire format.
//!
//! [`VarU64`] is a `u64` newtype that serializes as unsigned LEB128 (7 data
//! bits per byte, high bit = continuation) instead of fixed-width big-endian.
//! Small values cost fewer bytes: `0..=127` is 1 byte, `..=16383` is 2, and so
//! on, up to 10 bytes for the largest `u64`. Use it for fields that are small
//! and slowly growing (counters, versions, lengths) where the fixed 8 bytes are
//! mostly zero on the wire.
//!
//! Unlike a QUIC `VarInt` (capped at 2^62-1), `VarU64` covers the full `u64`
//! range, so it is a drop-in for any `u64` field. It implements [`Deref`]/
//! [`DerefMut`] to `u64` and `From` both ways, so call sites read and construct
//! it transparently.

use crate::{ByteVec, Deserialize, Serialize, Type, Wire, bytevec, de, type_of};
use core::ops::{Deref, DerefMut};
use upstream_buffer::writer::Storage as _;

/// A `u64` that encodes as unsigned LEB128 on the wire. See module docs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VarU64(pub u64);

impl VarU64 {
    /// Maximum number of bytes a `u64` LEB128 encoding can occupy
    /// (`ceil(64 / 7) == 10`).
    const MAX_ENCODED_LEN: u32 = 10;
}

impl Wire for VarU64 {
    // A DISTINCT scalar id from `u64` (`'4'`): the on-wire encoding is LEB128,
    // not fixed-width big-endian, so a `VarU64` field is NOT wire-compatible with
    // a `u64` field. Giving it its own id (`'v'`) means the schema/type hash and
    // the wire-compat snapshot tests correctly treat swapping `u64` <-> `VarU64`
    // as a breaking change, rather than silently accepting it.
    const TYPE: Type = Type {
        name: "VarU64",
        id: b'v',
        children: &[],
    };
}

crate::ser::impl_serialize_ref_copy!(VarU64);

impl Serialize for VarU64 {
    fn serialize<B: bytevec::Behavior>(self, out: &mut bytevec::Builder<B>) {
        // Encode into a stack buffer and emit it with a single `put_slice`,
        // rather than one call per byte. A `u64` needs at most
        // `MAX_ENCODED_LEN` bytes.
        let mut buf = [0u8; Self::MAX_ENCODED_LEN as usize];
        let mut v = self.0;
        let mut len = 0;
        loop {
            let mut byte = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                byte |= 0x80;
            }
            buf[len] = byte;
            len += 1;
            if v == 0 {
                break;
            }
        }
        out.put_slice(&buf[..len]);
    }
}

impl Deserialize for VarU64 {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, de::Error> {
        let mut result: u64 = 0;
        let mut shift: u32 = 0;
        loop {
            // Reads one byte; errors (InvalidLength) if the input is truncated
            // mid-value, matching how sibling impls consume fixed-width scalars.
            let byte = u8::deserialize(bytes)?;

            // Reject overlong encodings: more than 10 bytes cannot fit in a u64,
            // and the 10th byte may only carry the single remaining high bit.
            let group = shift / 7;
            if group >= Self::MAX_ENCODED_LEN || (group == Self::MAX_ENCODED_LEN - 1 && byte > 0x01)
            {
                return Err(de::Error::new(
                    de::ErrorKind::InvalidLength,
                    type_of::<Self>(),
                ));
            }

            result |= ((byte & 0x7f) as u64) << shift;
            if byte & 0x80 == 0 {
                return Ok(VarU64(result));
            }
            shift += 7;
        }
    }
}

impl From<u64> for VarU64 {
    fn from(v: u64) -> Self {
        VarU64(v)
    }
}

impl From<VarU64> for u64 {
    fn from(v: VarU64) -> Self {
        v.0
    }
}

impl Deref for VarU64 {
    type Target = u64;

    fn deref(&self) -> &u64 {
        &self.0
    }
}

impl DerefMut for VarU64 {
    fn deref_mut(&mut self) -> &mut u64 {
        &mut self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(v: u64) -> (VarU64, usize) {
        let mut buf = bytevec::Builder::default();
        VarU64(v).serialize(&mut buf);
        let bytes = buf.finish();
        let encoded_len = bytes.len();
        let mut reader: ByteVec = bytes;
        let decoded = VarU64::deserialize(&mut reader).expect("decode");
        assert!(reader.is_empty(), "all bytes consumed for {v}");
        (decoded, encoded_len)
    }

    #[test]
    fn round_trips_across_magnitudes() {
        for v in [
            0u64,
            1,
            127,
            128,
            16_383,
            16_384,
            u32::MAX as u64,
            u64::MAX / 2,
            u64::MAX - 1,
            u64::MAX,
        ] {
            let (decoded, _len) = round_trip(v);
            assert_eq!(decoded.0, v, "round trip for {v}");
        }
    }

    #[test]
    fn small_values_are_compact() {
        assert_eq!(round_trip(0).1, 1);
        assert_eq!(round_trip(127).1, 1);
        assert_eq!(round_trip(128).1, 2);
        assert_eq!(round_trip(16_383).1, 2);
        // The full-width u64 takes 10 bytes; far better only matters for small.
        assert_eq!(round_trip(u64::MAX).1, 10);
    }

    #[test]
    fn rejects_truncated_input() {
        // A lone continuation byte (high bit set) with nothing following.
        let mut buf = bytevec::Builder::default();
        buf.put_slice(&[0x80]);
        let mut reader = buf.finish();
        assert!(VarU64::deserialize(&mut reader).is_err());
    }

    #[test]
    fn rejects_overlong_encoding() {
        // 11 continuation bytes — more than a u64 can hold.
        let mut buf = bytevec::Builder::default();
        buf.put_slice(&[0x80; 11]);
        let mut reader = buf.finish();
        assert!(VarU64::deserialize(&mut reader).is_err());
    }
}
