// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! PeerInfo and ServiceEntry TaggedWire serialization helpers.
//!
//! `PeerInfo`, `ServiceEntry`, and `ServiceAddr` use `#[derive(TaggedWire)]`
//! for TLV encoding.

use super::types::{PeerInfo, ServiceAddr, ServiceEntry, ServiceId};
use upstream_bytevec::ByteVec;
use upstream_wire::{TaggedDeserialize, TaggedSerialize};

use bytes::Bytes;

impl PeerInfo {
    /// Serialize PeerInfo into bytes using TaggedWire TLV encoding.
    pub fn to_bytes(&self) -> Bytes {
        let mut out = upstream_bytevec::Builder::default();
        self.tagged_serialize(&mut out);
        out.finish().copy_to_bytes()
    }

    /// Deserialize PeerInfo from bytes.
    pub fn from_bytes(mut data: ByteVec) -> std::io::Result<Self> {
        Self::tagged_deserialize(&mut data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
    }
}

impl ServiceEntry {
    /// Deserialize a ServiceEntry directly from raw TaggedWire TLV bytes.
    ///
    /// Unlike the `Deserialize` impl (which expects a length prefix from being
    /// nested inside another TaggedWire struct), this reads the raw TLV fields
    /// directly. Used in tests to verify forward-compatibility behavior.
    pub fn from_bytes_raw(mut data: ByteVec) -> std::io::Result<Self> {
        Self::tagged_deserialize(&mut data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
    }
}
