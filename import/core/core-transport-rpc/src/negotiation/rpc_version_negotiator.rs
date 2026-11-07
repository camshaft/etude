// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! RPC version negotiator — the production `ServiceNegotiator` implementation.
//!
//! For each method, sorts entries by descending `version_idx` (newer first) and
//! selects the first `global_id` present in the remote's advertised set. This
//! ensures peers always negotiate the newest compatible wire layout regardless
//! of serialization order.

use super::{
    traits::ServiceNegotiator,
    types::{MethodId, NegotiatedMethods, ServiceEntry, VersionId},
};
use bytes::Bytes;
use upstream_bytevec::ByteVec;
use upstream_wire::{Deserialize, Serialize, TaggedDeserialize, TaggedSerialize, TaggedWire};
use std::{
    collections::{BTreeMap, HashSet},
    net::SocketAddr,
};

/// A single version entry: a `global_id` paired with its version index.
/// Higher `version_idx` means a newer version.
#[derive(TaggedWire, Clone, Debug, PartialEq, Eq, Default)]
#[wire(crate = upstream_wire)]
pub struct VersionEntry {
    #[wire(tag = 1.1)]
    pub global_id: u64,
    #[wire(tag = 1.2)]
    pub version_idx: u32,
}

/// Per-method version data for RPC negotiation.
#[derive(TaggedWire, Clone, Debug, PartialEq, Eq, Default)]
#[wire(crate = upstream_wire)]
pub struct MethodVersionData {
    #[wire(tag = 1.1)]
    pub name: String,
    #[wire(tag = 1.2)]
    pub entries: Vec<VersionEntry>,
}

/// The data encoded into `ServiceEntry.service_data` for RPC version negotiation.
#[derive(TaggedWire, Clone, Debug, Default, PartialEq, Eq)]
#[wire(crate = upstream_wire)]
pub struct RpcVersionData {
    #[wire(tag = 1.1)]
    pub methods: Vec<MethodVersionData>,
}

impl RpcVersionData {
    /// Serialize to bytes for embedding in `ServiceEntry.service_data`.
    pub fn to_bytes(&self) -> Bytes {
        let mut out = upstream_bytevec::Builder::default();
        self.tagged_serialize(&mut out);
        out.finish().copy_to_bytes()
    }

    /// Deserialize from bytes.
    pub fn from_bytes(data: ByteVec) -> Result<Self, upstream_wire::de::Error> {
        let mut data = data;
        Self::tagged_deserialize(&mut data)
    }

    /// Build `RpcVersionData` from a service's method metadata.
    pub fn from_service_methods<'a>(
        methods: impl Iterator<Item = &'a upstream_service::Method>,
    ) -> Self {
        let mut result = Vec::new();
        for method in methods {
            let mut entries = Vec::new();
            let newest_idx = method.versions.len() as u32;
            entries.push(VersionEntry {
                global_id: method.global_id,
                version_idx: newest_idx,
            });
            for &gid in method.versions.keys() {
                if let Some(entry) = method.versions.get(gid) {
                    entries.push(VersionEntry {
                        global_id: gid,
                        version_idx: entry.input_version_idx,
                    });
                }
            }
            result.push(MethodVersionData {
                name: String::from_utf8_lossy(method.name.as_bytes()).into_owned(),
                entries,
            });
        }
        RpcVersionData { methods: result }
    }
}

// ─── RpcVersionNegotiator ────────────────────────────────────────────────────

/// The production `ServiceNegotiator` for RPC schema versioning.
pub struct RpcVersionNegotiator;

impl ServiceNegotiator for RpcVersionNegotiator {
    fn negotiate(
        &self,
        _peer: SocketAddr,
        local: &ServiceEntry,
        remote: &ServiceEntry,
    ) -> Option<NegotiatedMethods> {
        let local_data =
            RpcVersionData::from_bytes(ByteVec::from(local.service_data.clone())).ok()?;
        let remote_data =
            RpcVersionData::from_bytes(ByteVec::from(remote.service_data.clone())).ok()?;

        let remote_ids: HashSet<u64> = remote_data
            .methods
            .iter()
            .flat_map(|m| m.entries.iter().map(|e| e.global_id))
            .collect();

        let mut negotiated = BTreeMap::new();

        for method in &local_data.methods {
            let mut sorted = method.entries.clone();
            sorted.sort_by(|a, b| b.version_idx.cmp(&a.version_idx));

            for entry in &sorted {
                if remote_ids.contains(&entry.global_id) {
                    negotiated.insert(
                        MethodId(Bytes::copy_from_slice(method.name.as_bytes())),
                        VersionId(Bytes::copy_from_slice(&entry.global_id.to_be_bytes())),
                    );
                    break;
                }
            }
        }

        if negotiated.is_empty() {
            None
        } else {
            Some(NegotiatedMethods {
                methods: negotiated,
            })
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{super::types::ServiceAddr, *};

    #[test]
    fn rpc_version_data_round_trip() {
        let data = RpcVersionData {
            methods: vec![
                MethodVersionData {
                    name: "read".to_string(),
                    entries: vec![
                        VersionEntry {
                            global_id: 0xCCCC,
                            version_idx: 2,
                        },
                        VersionEntry {
                            global_id: 0xBBBB,
                            version_idx: 1,
                        },
                        VersionEntry {
                            global_id: 0xAAAA,
                            version_idx: 0,
                        },
                    ],
                },
                MethodVersionData {
                    name: "write".to_string(),
                    entries: vec![
                        VersionEntry {
                            global_id: 0xDDDD,
                            version_idx: 1,
                        },
                        VersionEntry {
                            global_id: 0xEEEE,
                            version_idx: 0,
                        },
                    ],
                },
            ],
        };

        let bytes = data.to_bytes();
        let decoded = RpcVersionData::from_bytes(ByteVec::from(bytes)).unwrap();
        assert_eq!(data, decoded);
    }

    #[test]
    fn negotiates_newest_common_version() {
        let local_data = RpcVersionData {
            methods: vec![MethodVersionData {
                name: "read".to_string(),
                entries: vec![
                    VersionEntry {
                        global_id: 300,
                        version_idx: 2,
                    },
                    VersionEntry {
                        global_id: 200,
                        version_idx: 1,
                    },
                    VersionEntry {
                        global_id: 100,
                        version_idx: 0,
                    },
                ],
            }],
        };

        let remote_data = RpcVersionData {
            methods: vec![MethodVersionData {
                name: "read".to_string(),
                entries: vec![
                    VersionEntry {
                        global_id: 200,
                        version_idx: 1,
                    },
                    VersionEntry {
                        global_id: 100,
                        version_idx: 0,
                    },
                ],
            }],
        };

        let local_entry = ServiceEntry {
            addrs: vec![ServiceAddr::Socket {
                ip: "10.0.0.1".parse().unwrap(),
                port: 443,
            }],
            service_data: local_data.to_bytes(),
        };
        let remote_entry = ServiceEntry {
            addrs: vec![ServiceAddr::Socket {
                ip: "10.0.0.2".parse().unwrap(),
                port: 443,
            }],
            service_data: remote_data.to_bytes(),
        };

        let negotiator = RpcVersionNegotiator;
        let result = negotiator
            .negotiate("10.0.0.2:443".parse().unwrap(), &local_entry, &remote_entry)
            .unwrap();

        let version = result
            .methods
            .get(&MethodId(Bytes::from_static(b"read")))
            .unwrap();
        assert_eq!(
            version,
            &VersionId(Bytes::copy_from_slice(&200u64.to_be_bytes()))
        );
    }

    #[test]
    fn negotiates_correctly_regardless_of_wire_order() {
        let local_data = RpcVersionData {
            methods: vec![MethodVersionData {
                name: "read".to_string(),
                entries: vec![
                    VersionEntry {
                        global_id: 100,
                        version_idx: 0,
                    },
                    VersionEntry {
                        global_id: 300,
                        version_idx: 2,
                    },
                    VersionEntry {
                        global_id: 200,
                        version_idx: 1,
                    },
                ],
            }],
        };

        let remote_data = RpcVersionData {
            methods: vec![MethodVersionData {
                name: "read".to_string(),
                entries: vec![
                    VersionEntry {
                        global_id: 200,
                        version_idx: 1,
                    },
                    VersionEntry {
                        global_id: 100,
                        version_idx: 0,
                    },
                ],
            }],
        };

        let local_entry = ServiceEntry {
            addrs: vec![],
            service_data: local_data.to_bytes(),
        };
        let remote_entry = ServiceEntry {
            addrs: vec![],
            service_data: remote_data.to_bytes(),
        };

        let negotiator = RpcVersionNegotiator;
        let result = negotiator
            .negotiate("10.0.0.2:443".parse().unwrap(), &local_entry, &remote_entry)
            .unwrap();

        let version = result
            .methods
            .get(&MethodId(Bytes::from_static(b"read")))
            .unwrap();
        assert_eq!(
            version,
            &VersionId(Bytes::copy_from_slice(&200u64.to_be_bytes()))
        );
    }

    #[test]
    fn no_common_version_returns_none() {
        let local_data = RpcVersionData {
            methods: vec![MethodVersionData {
                name: "read".to_string(),
                entries: vec![
                    VersionEntry {
                        global_id: 300,
                        version_idx: 1,
                    },
                    VersionEntry {
                        global_id: 200,
                        version_idx: 0,
                    },
                ],
            }],
        };

        let remote_data = RpcVersionData {
            methods: vec![MethodVersionData {
                name: "read".to_string(),
                entries: vec![VersionEntry {
                    global_id: 100,
                    version_idx: 0,
                }],
            }],
        };

        let local_entry = ServiceEntry {
            addrs: vec![],
            service_data: local_data.to_bytes(),
        };
        let remote_entry = ServiceEntry {
            addrs: vec![],
            service_data: remote_data.to_bytes(),
        };

        let negotiator = RpcVersionNegotiator;
        let result =
            negotiator.negotiate("10.0.0.2:443".parse().unwrap(), &local_entry, &remote_entry);
        assert!(result.is_none());
    }

    #[test]
    fn same_version_negotiates_to_current() {
        let data = RpcVersionData {
            methods: vec![
                MethodVersionData {
                    name: "read".to_string(),
                    entries: vec![VersionEntry {
                        global_id: 0xAAAA,
                        version_idx: 0,
                    }],
                },
                MethodVersionData {
                    name: "write".to_string(),
                    entries: vec![VersionEntry {
                        global_id: 0xBBBB,
                        version_idx: 0,
                    }],
                },
            ],
        };

        let entry = ServiceEntry {
            addrs: vec![],
            service_data: data.to_bytes(),
        };

        let negotiator = RpcVersionNegotiator;
        let result = negotiator
            .negotiate("10.0.0.2:443".parse().unwrap(), &entry, &entry)
            .unwrap();

        assert_eq!(result.methods.len(), 2);
        assert_eq!(
            result
                .methods
                .get(&MethodId(Bytes::from_static(b"read")))
                .unwrap(),
            &VersionId(Bytes::copy_from_slice(&0xAAAAu64.to_be_bytes()))
        );
    }
}
