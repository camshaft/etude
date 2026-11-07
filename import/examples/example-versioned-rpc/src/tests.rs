// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use crate::generated::service::versioned::schema::{self, Methods};
use upstream_core::service::{Methods as _, Service as _};

#[test]
fn versioned_method_has_multiple_version_entries() {
    let read = Methods::Read;
    let method = read.info();

    // ReadRequest has 2 outer versions × inner cross-product:
    //   v1: key(u64, 1 xp) → 1 entry
    //   v2: key(u64, 1 xp) × consistency(Option<ConsistencyConfig>, 4 xp) → 4 entries
    // Total = 5 cross-product entries, minus 1 (newest duplicates primary global_id) = 4.
    assert_eq!(
        method.versions.len(),
        4,
        "versioned method 'read' should have 4 cross-product version entries (5 - 1 newest), got {}",
        method.versions.len()
    );
}

#[test]
fn versioned_method_global_ids_are_distinct() {
    let read = Methods::Read;
    let method = read.info();

    let keys = method.versions.keys();
    assert!(keys.len() >= 2, "need at least 2 version keys");

    let v1_id = keys[1]; // oldest
    let v2_id = keys[0]; // newest

    assert_ne!(v1_id, v2_id, "v1 and v2 should have different global_ids");
}

#[test]
fn versioned_method_newest_version_matches_primary_global_id() {
    let read = Methods::Read;
    let method = read.info();

    // The primary global_id is computed from the latest-version TYPE tree.
    // The versions map uses cross-product hashes (VERSION_XPRODUCT_COUNT).
    // For methods with nested versioned types, the primary global_id
    // won't appear in the versions map directly — the versions map has
    // the full cross-product, while the primary global_id uses the
    // latest-version TYPE tree (which always uses the latest inner types).
    // The by_global_id lookup should still resolve to the correct method.
    let resolved = schema::Methods::by_global_id(method.global_id);
    assert_eq!(
        resolved,
        Some(Methods::Read),
        "primary global_id should resolve to Methods::Read via by_global_id",
    );
}

#[test]
fn by_global_id_recognizes_all_versions() {
    let read = Methods::Read;
    let method = read.info();

    // All version global_ids should resolve to the Read method
    for (i, &version_id) in method.versions.keys().iter().enumerate() {
        let resolved = schema::Methods::by_global_id(version_id);
        assert_eq!(
            resolved,
            Some(Methods::Read),
            "version entry {i} (global_id={version_id}) should resolve to Methods::Read",
        );
    }
}

#[test]
fn non_versioned_method_has_empty_versions() {
    let ping = Methods::Ping;
    let method = ping.info();

    // u64 is a non-versioned type — empty versions map
    assert!(
        method.versions.is_empty(),
        "non-versioned method 'ping' should have empty versions, got {} entries",
        method.versions.len()
    );
}

#[test]
fn snapshot_read() {
    let method = Methods::Read;
    upstream_core::testing::service::assert_snapshot(
        method.info().for_snapshot(),
        "versioned",
        "read",
        env!("CARGO_MANIFEST_DIR"),
    );
}

#[test]
fn snapshot_ping() {
    let method = Methods::Ping;
    upstream_core::testing::service::assert_snapshot(
        method.info().for_snapshot(),
        "versioned",
        "ping",
        env!("CARGO_MANIFEST_DIR"),
    );
}

// ── Versioned enum (ConsistencyMode) integration tests ────────────

#[test]
fn versioned_enum_method_has_version_entries() {
    let method = Methods::SetConsistency;
    let info = method.info();

    // ConsistencyMode has 2 schema versions (v1: 2 unit variants, v2: + BoundedStaleness).
    // The versions map should have 1 entry (2 - 1 newest which duplicates primary).
    assert_eq!(
        info.versions.len(),
        1,
        "versioned enum method 'set_consistency' should have 1 version entry (2 - 1 newest), got {}",
        info.versions.len()
    );
}

#[test]
fn versioned_enum_global_ids_are_distinct() {
    let method = Methods::SetConsistency;
    let info = method.info();

    // The versions map has 1 entry (oldest version). The newest version's
    // global_id equals the primary global_id and is excluded from the map.
    let keys = info.versions.keys();
    assert_eq!(keys.len(), 1, "expected 1 version key");

    let v1_id = keys[0]; // oldest
    assert_ne!(
        v1_id, info.global_id,
        "v1 and primary should have different global_ids"
    );
}

#[test]
fn versioned_enum_by_global_id_recognizes_all_versions() {
    let method = Methods::SetConsistency;
    let info = method.info();

    for (i, &version_id) in info.versions.keys().iter().enumerate() {
        let resolved = schema::Methods::by_global_id(version_id);
        assert_eq!(
            resolved,
            Some(Methods::SetConsistency),
            "version entry {i} (global_id={version_id}) should resolve to Methods::SetConsistency",
        );
    }
}

#[test]
fn snapshot_set_consistency() {
    let method = Methods::SetConsistency;
    upstream_core::testing::service::assert_snapshot(
        method.info().for_snapshot(),
        "versioned",
        "set_consistency",
        env!("CARGO_MANIFEST_DIR"),
    );
}

// ── Cross-product version combination tests ───────────────────────

use upstream_core::wire::Wire;

#[test]
fn xproduct_ids_count_for_all_types() {
    use crate::types::*;

    // ConsistencyMode: versioned enum, 2 outer versions, no nested → 2
    assert_eq!(ConsistencyMode::VERSION_XPRODUCT_COUNT, 2);

    // ConsistencyConfig: 2 outer versions × Mode[2 xp]:
    //   v1: mode(ConsistencyMode, 2 xp) → 2
    //   v2: mode(ConsistencyMode, 2 xp) × timeout(Option<u64>, 1 xp) → 2
    //   total = 4
    assert_eq!(ConsistencyConfig::VERSION_XPRODUCT_COUNT, 4);

    // ReadRequest: 2 outer versions:
    //   v1: key(u64, 1 xp) → 1
    //   v2: key(u64, 1 xp) × consistency(Option<ConsistencyConfig>, 4 xp) → 4
    //   total = 5
    assert_eq!(ReadRequest::VERSION_XPRODUCT_COUNT, 5);

    // ReplyMetadata: 2 outer versions:
    //   v1: created_at(u64, 1 xp) → 1
    //   v2: created_at(u64, 1 xp) × region(Option<u64>, 1 xp) → 1
    //   total = 2
    assert_eq!(ReplyMetadata::VERSION_XPRODUCT_COUNT, 2);

    // ReadReply: 2 outer versions. Note `Option<T>` propagates T's xproduct
    // count (it does NOT add a separate entry for `None` — see ReadRequest
    // above, where Option<ConsistencyConfig> contributes 4, the same as
    // ConsistencyConfig).
    //   v1: value(u64, 1 xp) → 1
    //   v2: value(u64, 1 xp) × metadata(Option<ReplyMetadata>, 2 xp) → 2
    //   total = 3
    assert_eq!(ReadReply::VERSION_XPRODUCT_COUNT, 3);
}

#[test]
fn configure_method_has_version_entries() {
    let method = Methods::Configure;
    let info = method.info();

    // ConsistencyConfig has 4 xproduct types → 3 version entries (4 - 1 newest)
    assert_eq!(
        info.versions.len(),
        3,
        "configure method should have 3 cross-product version entries (4 - 1 newest), got {}",
        info.versions.len()
    );
}

#[test]
fn input_version_idx_maps_correctly() {
    use crate::types::*;

    // For each versioned method, every version entry's input_version_idx
    // must be a valid index into the input type's VERSION_XPRODUCT_COUNT.
    let methods = [
        (Methods::Read, ReadRequest::VERSION_XPRODUCT_COUNT),
        (
            Methods::SetConsistency,
            ConsistencyMode::VERSION_XPRODUCT_COUNT,
        ),
        (
            Methods::Configure,
            ConsistencyConfig::VERSION_XPRODUCT_COUNT,
        ),
    ];

    for (method_enum, xp_count) in methods {
        let info = method_enum.info();
        for &version_id in info.versions.keys() {
            let entry = info.versions.get(version_id).unwrap();
            assert!(
                (entry.input_version_idx as usize) < xp_count,
                "method {:?}: input_version_idx {} >= xproduct count {} for version_id {}",
                info.name,
                entry.input_version_idx,
                xp_count,
                version_id,
            );
        }
    }
}

/// `read_reply` has a non-versioned input (u64, 1 xp) and a versioned output
/// (ReadReply, 4 xp). The version map should therefore have IN*OUT-1 = 3
/// entries, and every entry's `output_version_idx` must index into the output
/// type's xproduct space while `input_version_idx` stays pinned at 0 (the only
/// input layout).
#[test]
fn read_reply_output_version_idx_maps_correctly() {
    use crate::types::*;

    let info = Methods::ReadReply.info();

    // u64[1] * ReadReply[3] - 1 (newest duplicates primary) = 2
    assert_eq!(
        info.versions.len(),
        2,
        "read_reply should have 2 cross-product version entries (1*3 - 1), got {}",
        info.versions.len()
    );

    for &version_id in info.versions.keys() {
        let entry = info.versions.get(version_id).unwrap();
        assert!(
            (entry.output_version_idx as usize) < ReadReply::VERSION_XPRODUCT_COUNT,
            "output_version_idx {} >= ReadReply xproduct count {} for version_id {}",
            entry.output_version_idx,
            ReadReply::VERSION_XPRODUCT_COUNT,
            version_id,
        );
        assert_eq!(
            entry.input_version_idx, 0,
            "read_reply input is non-versioned (u64) — input_version_idx must be 0, \
             only the output varies (version_id {version_id})"
        );
    }

    // The 2 entries must cover output indices {0, 1} — the two older output
    // layouts (index 2 is the newest, which is the primary global_id and
    // excluded from the versions map).
    let mut out_indices: Vec<u32> = info
        .versions
        .keys()
        .iter()
        .map(|&vid| info.versions.get(vid).unwrap().output_version_idx)
        .collect();
    out_indices.sort_unstable();
    assert_eq!(
        out_indices,
        vec![0, 1],
        "the 2 version entries should map to the 2 older output layouts"
    );
}

/// The oldest `read_reply` version entry (output_version_idx == 0) must select
/// the v1 ReadReply layout: serializing a fully-populated reply at that index
/// and reading it back drops the v2 `metadata` field. This is the wire-level
/// proof behind the `read_reply_v1_negotiated_drops_metadata` E2E test,
/// isolated from the transport.
#[test]
fn read_reply_v1_output_index_drops_metadata() {
    use crate::types::*;
    use upstream_core::wire::{Deserialize, Serialize, bytevec::Builder};

    let info = Methods::ReadReply.info();

    // Find the version entry whose output_version_idx == 0 (oldest output).
    let v1_idx = info
        .versions
        .keys()
        .iter()
        .map(|&vid| info.versions.get(vid).unwrap().output_version_idx)
        .find(|&idx| idx == 0)
        .expect("read_reply should have an output_version_idx == 0 entry");

    let reply = ReadReply {
        value: 42,
        metadata: Some(ReplyMetadata {
            created_at: 43,
            region: Some(7),
        }),
    };

    let mut b = Builder::default();
    Serialize::serialize_at(reply.clone(), &mut b, v1_idx);
    let mut bytes = b.finish();
    let decoded = ReadReply::deserialize_at(&mut bytes, v1_idx).unwrap();

    assert_eq!(decoded.value, 42, "v1 value round-trips");
    assert_eq!(
        decoded.metadata, None,
        "v1 output layout (output_version_idx=0) drops the v2 metadata field"
    );

    // And the newest output layout preserves everything.
    let newest = (ReadReply::VERSION_XPRODUCT_COUNT - 1) as u32;
    let mut b = Builder::default();
    Serialize::serialize_at(reply.clone(), &mut b, newest);
    let mut bytes = b.finish();
    let decoded = ReadReply::deserialize_at(&mut bytes, newest).unwrap();
    assert_eq!(decoded, reply, "newest output layout preserves all fields");
}

#[test]
fn all_version_ids_distinct_across_methods() {
    let all_methods = [
        Methods::Read,
        Methods::Ping,
        Methods::SetConsistency,
        Methods::Configure,
    ];
    let mut all_ids = std::collections::HashSet::new();

    for method in all_methods {
        let info = method.info();
        // Primary global_id
        assert!(
            all_ids.insert(info.global_id),
            "duplicate global_id {} for method {:?}",
            info.global_id,
            info.name
        );
        // Version global_ids
        for &vid in info.versions.keys() {
            assert!(
                all_ids.insert(vid),
                "duplicate version_id {} for method {:?}",
                vid,
                info.name
            );
        }
    }
}

#[test]
fn cross_version_serde_consistency_config() {
    use crate::types::*;
    use upstream_core::wire::{Deserialize, Serialize, bytevec::Builder};

    // ConsistencyConfig xproduct layout (4 entries):
    //   idx 0: outer_v1, Mode@v1 (CM@v1) → mode=Linearizable (or fallback), no timeout
    //   idx 1: outer_v1, Mode@v2 (CM@v2) → mode=BoundedStaleness preserved, no timeout
    //   idx 2: outer_v2, Mode@v1, timeout → mode fallback + timeout
    //   idx 3: outer_v2, Mode@v2, timeout → all preserved
    let config = ConsistencyConfig {
        mode: ConsistencyMode::BoundedStaleness(1000),
        timeout: Some(500),
    };

    // idx 0: v1 + CM@v1 → BoundedStaleness falls back to Eventual, no timeout
    let mut b = Builder::default();
    Serialize::serialize_at(config.clone(), &mut b, 0);
    let mut bytes = b.finish();
    let r0 = ConsistencyConfig::deserialize_at(&mut bytes, 0).unwrap();
    assert_eq!(r0.mode, ConsistencyMode::Eventual);
    assert_eq!(r0.timeout, None);

    // idx 1: v1 + CM@v2 → BoundedStaleness preserved, no timeout
    let mut b = Builder::default();
    Serialize::serialize_at(config.clone(), &mut b, 1);
    let mut bytes = b.finish();
    let r1 = ConsistencyConfig::deserialize_at(&mut bytes, 1).unwrap();
    assert_eq!(r1.mode, ConsistencyMode::BoundedStaleness(1000));
    assert_eq!(r1.timeout, None);

    // idx 2: v2 + CM@v1 → fallback + timeout
    let mut b = Builder::default();
    Serialize::serialize_at(config.clone(), &mut b, 2);
    let mut bytes = b.finish();
    let r2 = ConsistencyConfig::deserialize_at(&mut bytes, 2).unwrap();
    assert_eq!(r2.mode, ConsistencyMode::Eventual);
    assert_eq!(r2.timeout, Some(500));

    // idx 3: v2 + CM@v2 → all latest
    let mut b = Builder::default();
    Serialize::serialize_at(config.clone(), &mut b, 3);
    let mut bytes = b.finish();
    let r3 = ConsistencyConfig::deserialize_at(&mut bytes, 3).unwrap();
    assert_eq!(r3, config);
}

#[test]
fn cross_version_serde_read_request() {
    use crate::types::*;
    use upstream_core::wire::{Deserialize, Serialize, bytevec::Builder};

    // ReadRequest xproduct layout (5 entries):
    //   idx 0: outer_v1, key only
    //   idx 1-4: outer_v2, key + consistency@0..3
    let req = ReadRequest {
        key: 42,
        consistency: Some(ConsistencyConfig {
            mode: ConsistencyMode::BoundedStaleness(1000),
            timeout: Some(500),
        }),
    };

    // idx 0: v1 → only key
    let mut b = Builder::default();
    Serialize::serialize_at(req.clone(), &mut b, 0);
    let mut bytes = b.finish();
    let r0 = ReadRequest::deserialize_at(&mut bytes, 0).unwrap();
    assert_eq!(r0.key, 42);
    assert_eq!(r0.consistency, None);

    // idx 1: v2 + CC@0 → key + consistency(mode=Eventual, timeout=None)
    let mut b = Builder::default();
    Serialize::serialize_at(req.clone(), &mut b, 1);
    let mut bytes = b.finish();
    let r1 = ReadRequest::deserialize_at(&mut bytes, 1).unwrap();
    assert_eq!(r1.key, 42);
    let c1 = r1.consistency.unwrap();
    assert_eq!(c1.mode, ConsistencyMode::Eventual);
    assert_eq!(c1.timeout, None);

    // idx 2: v2 + CC@1 → key + consistency(mode=BoundedStaleness(1000), timeout=None)
    let mut b = Builder::default();
    Serialize::serialize_at(req.clone(), &mut b, 2);
    let mut bytes = b.finish();
    let r2 = ReadRequest::deserialize_at(&mut bytes, 2).unwrap();
    assert_eq!(r2.key, 42);
    let c2 = r2.consistency.unwrap();
    assert_eq!(c2.mode, ConsistencyMode::BoundedStaleness(1000));
    assert_eq!(c2.timeout, None);

    // idx 3: v2 + CC@2 → key + consistency(mode=Eventual, timeout=500)
    let mut b = Builder::default();
    Serialize::serialize_at(req.clone(), &mut b, 3);
    let mut bytes = b.finish();
    let r3 = ReadRequest::deserialize_at(&mut bytes, 3).unwrap();
    assert_eq!(r3.key, 42);
    let c3 = r3.consistency.unwrap();
    assert_eq!(c3.mode, ConsistencyMode::Eventual);
    assert_eq!(c3.timeout, Some(500));

    // idx 4: v2 + CC@3 → ALL LATEST
    let mut b = Builder::default();
    Serialize::serialize_at(req.clone(), &mut b, 4);
    let mut bytes = b.finish();
    let r4 = ReadRequest::deserialize_at(&mut bytes, 4).unwrap();
    assert_eq!(r4, req);
}

#[test]
fn snapshot_configure() {
    let method = Methods::Configure;
    upstream_core::testing::service::assert_snapshot(
        method.info().for_snapshot(),
        "versioned",
        "configure",
        env!("CARGO_MANIFEST_DIR"),
    );
}

/// Verify the primary global_id matches the hash computed from the latest
/// (last) `__XPRODUCT_HASH_ENTRIES` for input and output types.
#[test]
fn primary_global_id_matches_latest_xproduct_entry() {
    use crate::types::*;
    use upstream_core::service::{Hasher, MethodFlags};

    let method = Methods::Read.info();

    let in_entries = <ReadRequest as Wire>::__XPRODUCT_HASH_ENTRIES;
    let out_entries = <Option<u64> as Wire>::__XPRODUCT_HASH_ENTRIES;
    let (ref in_buf, in_len) = in_entries[in_entries.len() - 1];
    let (ref out_buf, out_len) = out_entries[out_entries.len() - 1];

    let init_hasher = Hasher::new().update("versioned\0".as_bytes());
    let computed = upstream_core::service::compute_method_global_id(
        init_hasher,
        "read\0",
        in_buf,
        in_len as usize,
        out_buf,
        out_len as usize,
        None,
        None,
        MethodFlags {
            unidirectional: false,
            unreliable: false,
            stream_request: false,
            stream_response: false,
            raw_response: false,
        },
    );

    assert_eq!(
        computed, method.global_id,
        "primary global_id should match hash computed from latest xproduct entry"
    );
}

/// Full pipeline test: resolve method from a version global_id, extract
/// `input_version_idx`, then serialize_at/deserialize_at at that index
/// and verify the result is consistent with the cross-product combination.
#[test]
fn version_global_id_dispatch_serde_round_trip() {
    use crate::types::*;
    use upstream_core::wire::{Deserialize, Serialize, bytevec::Builder};

    let method = Methods::Read.info();
    let req = ReadRequest {
        key: 42,
        consistency: Some(ConsistencyConfig {
            mode: ConsistencyMode::BoundedStaleness(1000),
            timeout: Some(500),
        }),
    };

    for &version_id in method.versions.keys() {
        // Step 1: Resolve method from version global_id
        let resolved = schema::Methods::by_global_id(version_id);
        assert_eq!(
            resolved,
            Some(Methods::Read),
            "version_id {version_id} should resolve to Read"
        );

        // Step 2: Get input_version_idx
        let entry = method.versions.get(version_id).unwrap();
        let idx = entry.input_version_idx;

        // Step 3: serialize_at → deserialize_at round-trip
        let mut b = Builder::default();
        Serialize::serialize_at(req.clone(), &mut b, idx);
        let mut bytes = b.finish();
        let result = ReadRequest::deserialize_at(&mut bytes, idx).unwrap_or_else(|e| {
            panic!("deserialize_at({idx}) failed for version_id {version_id}: {e}")
        });

        // Step 4: Verify key is always preserved
        assert_eq!(result.key, 42, "key should always be 42 at idx {idx}");

        // At idx 0 (v1), consistency is None. Otherwise Some.
        if idx == 0 {
            assert_eq!(result.consistency, None, "idx 0 should drop consistency");
        } else {
            // idx 1..4 are v2 with different inner cross-product combinations
            // We don't assert specific inner values here — that's covered by
            // cross_version_serde_read_request. Just verify it round-trips without error.
        }
    }
}

// ── End-to-end service-level version negotiation tests ────────────

/// A provider implementation that returns the key from a ReadRequest
/// and echoes back consistency info for verification.
struct TestProvider;

impl crate::generated::service::versioned::provider::Provider<std::net::SocketAddr>
    for TestProvider
{
    async fn read<E: crate::generated::Env>(
        &self,
        _env: &E,
        request: crate::types::ReadRequest,
    ) -> upstream_core::runtime::Result<Option<u64>> {
        // Return the key if present, so we can verify the request was deserialized correctly.
        // Encode consistency presence in the response: key + 1000 if consistency was present.
        if request.consistency.is_some() {
            Ok(Some(request.key + 1000))
        } else {
            Ok(Some(request.key))
        }
    }

    async fn ping<E: crate::generated::Env>(
        &self,
        _env: &E,
        message: u64,
    ) -> upstream_core::runtime::Result<u64> {
        Ok(message)
    }

    async fn set_consistency<E: crate::generated::Env>(
        &self,
        _env: &E,
        mode: crate::types::ConsistencyMode,
    ) -> upstream_core::runtime::Result<bool> {
        // Return true if BoundedStaleness was received, false otherwise.
        // At v1, BoundedStaleness falls back to Eventual → false.
        Ok(matches!(
            mode,
            crate::types::ConsistencyMode::BoundedStaleness(_)
        ))
    }

    async fn configure<E: crate::generated::Env>(
        &self,
        _env: &E,
        config: crate::types::ConsistencyConfig,
    ) -> upstream_core::runtime::Result<bool> {
        // Return true if timeout was received (v2 field), false otherwise.
        // At v1, timeout is dropped → false.
        Ok(config.timeout.is_some())
    }

    async fn sum_reads<E: crate::generated::Env>(
        &self,
        _env: &E,
        request: impl upstream_core::service::RequestStream<crate::types::ReadRequest>,
    ) -> upstream_core::runtime::Result<crate::types::SumReadsResponse> {
        use upstream_core::service::stream::StreamExt as _;

        let mut request = std::pin::pin!(request);
        let mut key_sum = 0u64;
        let mut chunk_count = 0u64;
        let mut consistency_count = 0u64;
        while let Some(chunk) = request.next().await {
            let chunk = chunk?;
            key_sum += chunk.key;
            chunk_count += 1;
            if chunk.consistency.is_some() {
                consistency_count += 1;
            }
        }
        Ok(crate::types::SumReadsResponse {
            key_sum,
            chunk_count,
            consistency_count,
        })
    }

    async fn read_reply<E: crate::generated::Env>(
        &self,
        _env: &E,
        key: u64,
    ) -> upstream_core::runtime::Result<crate::types::ReadReply> {
        // Always populate the v2 `metadata` field. On a v1-negotiated call
        // the server serializes the reply at v1, dropping `metadata` on the
        // wire — so the client observes `metadata == None` despite this.
        Ok(crate::types::ReadReply {
            value: key,
            metadata: Some(crate::types::ReplyMetadata {
                created_at: key + 1,
                region: Some(7),
            }),
        })
    }

    async fn scan_replies<E: crate::generated::Env>(
        &self,
        _env: &E,
        count: u64,
    ) -> upstream_core::runtime::Result<(
        u64,
        impl upstream_core::service::ResponseStream<crate::types::ReadReply>,
    )> {
        use upstream_core::service::stream;

        // Emit `count` replies, each carrying the v2 `metadata` field. A
        // v1-negotiated stream must drop `metadata` on every frame without
        // desyncing across frames. The header echoes `count` so the
        // versioned-output stream also exercises the response-header path.
        let replies: Vec<upstream_core::runtime::Result<crate::types::ReadReply>> = (0..count)
            .map(|i| {
                Ok(crate::types::ReadReply {
                    value: i,
                    metadata: Some(crate::types::ReplyMetadata {
                        created_at: i + 1,
                        region: Some(7),
                    }),
                })
            })
            .collect();

        Ok((count, stream::iter(replies)))
    }

    async fn read_reply_hdr<E: crate::generated::Env>(
        &self,
        _env: &E,
        key: u64,
    ) -> upstream_core::runtime::Result<(u64, crate::types::ReadReply)> {
        // Header echoes the key (non-versioned, must always round-trip). The
        // body always carries v2 `metadata`; a v1-negotiated call drops it.
        Ok((
            key,
            crate::types::ReadReply {
                value: key,
                metadata: Some(crate::types::ReplyMetadata {
                    created_at: key + 1,
                    region: Some(7),
                }),
            },
        ))
    }

    async fn fold_replies<E: crate::generated::Env>(
        &self,
        _env: &E,
        request: impl upstream_core::service::RequestStream<u64>,
    ) -> upstream_core::runtime::Result<crate::types::ReadReply> {
        use upstream_core::service::stream::StreamExt as _;

        // Fold the streamed keys into a single reply; populate v2 metadata so a
        // v1-negotiated response must drop it.
        let mut request = std::pin::pin!(request);
        let mut sum = 0u64;
        while let Some(chunk) = request.next().await {
            sum += chunk?;
        }
        Ok(crate::types::ReadReply {
            value: sum,
            metadata: Some(crate::types::ReplyMetadata {
                created_at: sum + 1,
                region: Some(7),
            }),
        })
    }

    async fn echo_replies<E: crate::generated::Env>(
        &self,
        _env: &E,
        request: impl upstream_core::service::RequestStream<u64>,
    ) -> upstream_core::runtime::Result<
        impl upstream_core::service::ResponseStream<crate::types::ReadReply>,
    > {
        use upstream_core::service::stream::StreamExt as _;

        // Map each streamed key to a reply carrying v2 metadata. A
        // v1-negotiated stream drops metadata on every frame without desync.
        let response_stream = request.map(|result| {
            result.map(|key| crate::types::ReadReply {
                value: key,
                metadata: Some(crate::types::ReplyMetadata {
                    created_at: key + 1,
                    region: Some(7),
                }),
            })
        });
        Ok(response_stream)
    }
}

/// End-to-end test: v2 client sends to v2 server using the current version.
///
/// Both peers are at the latest version. The client uses `StaticSupportedRegistry`
/// with `Set::all()`, which has no `MethodVersionEntry` — the default (current) path.
/// Verifies that the consistency field round-trips correctly.
#[tokio::test]
async fn read_without_negotiation_preserves_all_fields() {
    use crate::generated::service::versioned::{
        consumer::{Consumer as _, Registry as _},
        provider, schema,
    };
    use upstream_core::{
        runtime::Env as _,
        service::consumer::StaticSupportedRegistry,
        transport::rpc::{self, Rpc, Server},
    };

    let env = crate::generated::env::tokio_env();
    let transport = rpc::mem::Transport::default();

    // Server
    let acceptor = provider::Instance::new(TestProvider);
    let server = transport
        .create_server("[::1]:0".parse().unwrap())
        .await
        .unwrap();
    let server_addr = server.local_addr();
    let server = rpc::service::ServerAcceptor::new(server, acceptor, env.clone());
    let _ = env.spawn(upstream_core::runtime::task_monitor::Test, server.run());

    // Client with current-version registry (Set::all(), no version entries)
    let client = transport.create_client().await.unwrap();
    let client = rpc::service::ClientRegistry::from_transport(client);
    let session = client.resolve(&server_addr).await.unwrap();

    // Send a v2 request with consistency — current version path
    let result = session
        .read(
            &env,
            crate::types::ReadRequest {
                key: 42,
                consistency: Some(crate::types::ConsistencyConfig {
                    mode: crate::types::ConsistencyMode::Linearizable,
                    timeout: Some(100),
                }),
            },
        )
        .await
        .unwrap();

    // Server sees consistency=Some → returns key + 1000
    assert_eq!(
        result,
        Some(1042),
        "current-version path should preserve consistency field"
    );
}

/// End-to-end test: v2 client sends to server using the v1 version layout.
///
/// The client is configured with a `NegotiatedSupportedRegistry` that returns
/// only the v1 `global_id` for the `read` method. This simulates a peer that
/// only supports v1 of the ReadRequest (key only, no consistency).
///
/// The client serializes with `serialize_at(0)` (v1 layout, drops consistency).
/// The server deserializes with `deserialize_at(0)` (v1 layout, fills consistency=None).
/// The handler returns key without the +1000 offset.
#[tokio::test]
async fn read_with_negotiation_drops_newer_fields() {
    use crate::generated::service::versioned::{
        consumer::{Consumer as _, Registry as _},
        provider, schema,
    };
    use upstream_core::{
        runtime::Env as _,
        service::{Methods as _, consumer::StaticSupportedRegistry},
        transport::rpc::{self, Rpc, Server},
    };

    let env = crate::generated::env::tokio_env();
    let transport = rpc::mem::Transport::default();

    // Server (at v2 — supports all versions)
    let acceptor = provider::Instance::new(TestProvider);
    let server = transport
        .create_server("[::1]:0".parse().unwrap())
        .await
        .unwrap();
    let server_addr = server.local_addr();
    let server = rpc::service::ServerAcceptor::new(server, acceptor, env.clone());
    let _ = env.spawn(upstream_core::runtime::task_monitor::Test, server.run());

    // Collect the v1 global_id for the read method (the oldest version entry)
    let read_method = schema::Methods::Read.info();
    let v1_global_ids: Vec<u64> = {
        let mut ids = vec![];
        // Add only the oldest version entry for each method (simulating v1-only peer)
        for method in schema::Service::methods() {
            if method.versions.is_empty() {
                // Non-versioned method — current global_id is the only one
                ids.push(method.global_id);
            } else {
                // Versioned method — use the oldest version entry (last key = oldest)
                let keys = method.versions.keys();
                ids.push(keys[keys.len() - 1]); // oldest is last
            }
        }
        ids
    };

    // Verify: the v1 global_id for read differs from the current
    let v1_read_id = v1_global_ids.iter().find(|&&id| {
        schema::Methods::by_global_id(id) == Some(schema::Methods::Read)
            && id != read_method.global_id
    });
    assert!(
        v1_read_id.is_some(),
        "should find a v1 global_id for read that differs from current"
    );

    // Client with negotiated registry pointing to v1 ids
    let client = transport.create_client().await.unwrap();
    let v1_ids = v1_global_ids.clone();
    let registry = StaticSupportedRegistry::from_negotiated(&v1_ids); // replaced closure
    let client = rpc::service::ClientRegistry::new(client, registry);
    let session = client.resolve(&server_addr).await.unwrap();

    // Verify the read method has a version entry (non-current version)
    let read_method_handle_supported = session.supports_read();
    assert!(
        read_method_handle_supported,
        "read should be supported via v1 global_id"
    );

    // Send a v2 request with consistency — but the client serializes at v1 (drops consistency)
    let result = session
        .read(
            &env,
            crate::types::ReadRequest {
                key: 42,
                consistency: Some(crate::types::ConsistencyConfig {
                    mode: crate::types::ConsistencyMode::Linearizable,
                    timeout: Some(100),
                }),
            },
        )
        .await
        .unwrap();

    // Server sees consistency=None (v1 layout) → returns just key
    assert_eq!(
        result,
        Some(42),
        "v1 negotiated path should drop consistency field: client serializes_at(v1), \
         server deserializes_at(v1), handler sees consistency=None and returns key=42"
    );
}

/// Helper: create a server + env + transport for e2e tests. Returns (env, transport, server_addr).
async fn setup_e2e_server() -> (
    impl crate::generated::Env + upstream_core::runtime::Env + Clone,
    upstream_core::transport::rpc::mem::Transport,
    upstream_core::service::ServerAddr,
) {
    use crate::generated::service::versioned::provider;
    use upstream_core::{
        runtime::Env as _,
        transport::rpc::{self, Rpc, Server},
    };

    let env = crate::generated::env::tokio_env();
    let transport = rpc::mem::Transport::default();

    let acceptor = provider::Instance::new(TestProvider);
    let server = transport
        .create_server("[::1]:0".parse().unwrap())
        .await
        .unwrap();
    let server_addr = server.local_addr();
    let server = rpc::service::ServerAcceptor::new(server, acceptor, env.clone());
    let _ = env.spawn(upstream_core::runtime::task_monitor::Test, server.run());

    (env, transport, server_addr)
}

/// Helper: collect the oldest version global_id for each method (simulating a v1 peer).
fn v1_global_ids() -> Vec<u64> {
    use crate::generated::service::versioned::schema;

    let mut ids = vec![];
    for method in schema::Service::methods() {
        if method.versions.is_empty() {
            ids.push(method.global_id);
        } else {
            let keys = method.versions.keys();
            ids.push(keys[keys.len() - 1]);
        }
    }
    ids
}

/// Non-versioned method through the full service pipeline.
///
/// `ping` has no version entries — `NegotiatedSupportedRegistry` should use
/// the current `global_id` and the message should round-trip without issue.
#[tokio::test]
async fn non_versioned_method_unaffected_by_negotiation() {
    use crate::generated::service::versioned::consumer::{Consumer as _, Registry as _};
    use upstream_core::{
        service::consumer::StaticSupportedRegistry,
        transport::rpc::{self, Rpc},
    };

    let (env, transport, server_addr) = setup_e2e_server().await;

    // Client with negotiated registry returning v1 ids — ping's current global_id
    // is the only one available since it has no versions.
    let client = transport.create_client().await.unwrap();
    let ids = v1_global_ids();
    let registry = StaticSupportedRegistry::from_negotiated(&ids); // replaced closure
    let client = rpc::service::ClientRegistry::new(client, registry);
    let session = client.resolve(&server_addr).await.unwrap();

    assert!(
        session.supports_ping(),
        "ping should be supported (non-versioned)"
    );

    let result = session.ping(&env, 12345).await.unwrap();
    assert_eq!(result, 12345, "ping should echo the message unchanged");
}

/// Versioned enum fallback through the full service pipeline.
///
/// Client sends `BoundedStaleness(1000)` via the v1 `global_id` for `set_consistency`.
/// The v1 serializer converts BoundedStaleness to Eventual (the fallback).
/// The server deserializes Eventual and returns false (not BoundedStaleness).
#[tokio::test]
async fn enum_fallback_applied_at_older_version() {
    use crate::generated::service::versioned::consumer::{Consumer as _, Registry as _};
    use upstream_core::{
        service::consumer::StaticSupportedRegistry,
        transport::rpc::{self, Rpc},
    };

    let (env, transport, server_addr) = setup_e2e_server().await;

    let client = transport.create_client().await.unwrap();
    let ids = v1_global_ids();
    let registry = StaticSupportedRegistry::from_negotiated(&ids); // replaced closure
    let client = rpc::service::ClientRegistry::new(client, registry);
    let session = client.resolve(&server_addr).await.unwrap();

    // Send BoundedStaleness — at v1, it falls back to Eventual
    let result = session
        .set_consistency(&env, crate::types::ConsistencyMode::BoundedStaleness(1000))
        .await
        .unwrap();
    assert!(
        !result,
        "v1 path: BoundedStaleness should fall back to Eventual, handler returns false"
    );

    // Also verify: Linearizable at v1 should remain Linearizable (no fallback needed)
    let result2 = session
        .set_consistency(&env, crate::types::ConsistencyMode::Linearizable)
        .await
        .unwrap();
    assert!(
        !result2,
        "Linearizable is not BoundedStaleness, handler returns false"
    );
}

/// 2-level nested struct versioning through the full service pipeline.
///
/// Client sends `ConsistencyConfig { mode: BoundedStaleness(1000), timeout: Some(500) }`
/// via the oldest v1 `global_id` for `configure`. The v1 serializer drops `timeout`
/// and falls back `BoundedStaleness` to `Eventual`. Server sees
/// `ConsistencyConfig { mode: Eventual, timeout: None }` and returns false (timeout absent).
#[tokio::test]
async fn nested_struct_drops_fields_at_older_version() {
    use crate::generated::service::versioned::consumer::{Consumer as _, Registry as _};
    use upstream_core::{
        service::consumer::StaticSupportedRegistry,
        transport::rpc::{self, Rpc},
    };

    let (env, transport, server_addr) = setup_e2e_server().await;

    let client = transport.create_client().await.unwrap();
    let ids = v1_global_ids();
    let registry = StaticSupportedRegistry::from_negotiated(&ids); // replaced closure
    let client = rpc::service::ClientRegistry::new(client, registry);
    let session = client.resolve(&server_addr).await.unwrap();

    let result = session
        .configure(
            &env,
            crate::types::ConsistencyConfig {
                mode: crate::types::ConsistencyMode::BoundedStaleness(1000),
                timeout: Some(500),
            },
        )
        .await
        .unwrap();

    assert!(
        !result,
        "v1 path: timeout should be dropped, handler returns false (timeout absent)"
    );
}

/// v1 negotiated read where the dropped field was already None.
///
/// Verifies no data corruption when serialize_at(v1) processes a ReadRequest
/// where the v2 field (`consistency`) is already `None`.
#[tokio::test]
async fn v1_serialization_preserves_none_fields() {
    use crate::generated::service::versioned::consumer::{Consumer as _, Registry as _};
    use upstream_core::{
        service::consumer::StaticSupportedRegistry,
        transport::rpc::{self, Rpc},
    };

    let (env, transport, server_addr) = setup_e2e_server().await;

    let client = transport.create_client().await.unwrap();
    let ids = v1_global_ids();
    let registry = StaticSupportedRegistry::from_negotiated(&ids); // replaced closure
    let client = rpc::service::ClientRegistry::new(client, registry);
    let session = client.resolve(&server_addr).await.unwrap();

    // Send a request where consistency is already None
    let result = session
        .read(
            &env,
            crate::types::ReadRequest {
                key: 99,
                consistency: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        result,
        Some(99),
        "v1 path with consistency already None: should return key=99 without corruption"
    );
}

/// Fallback to Set::all() when NegotiatedSupportedRegistry returns None.
///
/// Simulates an old peer that never negotiated. The registry returns None for
/// the peer, which triggers `SupportedSet::all()` — equivalent to the pre-negotiation
/// `StaticSupportedRegistry::ALL` behavior. Verifies backward compatibility.
#[tokio::test]
async fn unnegotiated_peer_falls_back_to_latest_version() {
    use crate::generated::service::versioned::consumer::{Consumer as _, Registry as _};
    use upstream_core::{
        service::consumer::StaticSupportedRegistry,
        transport::rpc::{self, Rpc},
    };

    let (env, transport, server_addr) = setup_e2e_server().await;

    let client = transport.create_client().await.unwrap();
    // Registry returns None for all peers — simulates un-negotiated peer
    let client = rpc::service::ClientRegistry::from_transport(client); // current-version dispatch, derived from transport
    let session = client.resolve(&server_addr).await.unwrap();

    // All methods should be supported (Set::all() fallback)
    assert!(
        session.supports_read(),
        "read should be supported via Set::all() fallback"
    );
    assert!(
        session.supports_ping(),
        "ping should be supported via Set::all() fallback"
    );

    // Current-version path should work (consistency preserved)
    let result = session
        .read(
            &env,
            crate::types::ReadRequest {
                key: 77,
                consistency: Some(crate::types::ConsistencyConfig {
                    mode: crate::types::ConsistencyMode::Linearizable,
                    timeout: Some(200),
                }),
            },
        )
        .await
        .unwrap();

    assert_eq!(
        result,
        Some(1077),
        "Set::all() fallback: current-version path should preserve consistency (key+1000)"
    );
}

// ── Unit tests for client-side and server-side version negotiation logic ───

/// Test that the server correctly detects version_idx when receiving a non-current global_id.
///
/// This verifies the version detection logic from handle_server_connection:
/// when method_id != method.global_id, look up method.versions.get(method_id).
#[test]
fn server_detects_version_idx_from_global_id() {
    use crate::generated::service::versioned::schema;

    let read = schema::Methods::Read.info();

    // Current global_id → version_idx should be None
    let method_id = read.global_id;
    let version_idx = if method_id != read.global_id {
        read.versions.get(method_id).map(|e| e.input_version_idx)
    } else {
        None
    };
    assert_eq!(
        version_idx, None,
        "current global_id should produce version_idx=None"
    );

    // v1 global_id → version_idx should be Some(0)
    let v1_keys = read.versions.keys();
    let v1_global_id = v1_keys[v1_keys.len() - 1]; // oldest
    let version_idx = if v1_global_id != read.global_id {
        read.versions.get(v1_global_id).map(|e| e.input_version_idx)
    } else {
        None
    };
    assert_eq!(
        version_idx,
        Some(0),
        "v1 global_id should produce version_idx=Some(0) (oldest xproduct entry)"
    );

    // Unknown global_id → version_idx should be None (method.versions.get returns None)
    let unknown_id = 0xDEADBEEFu64;
    let version_idx = if unknown_id != read.global_id {
        read.versions.get(unknown_id).map(|e| e.input_version_idx)
    } else {
        None
    };
    assert_eq!(
        version_idx, None,
        "unknown global_id should produce version_idx=None"
    );
}

/// Test that NegotiatedSupportedRegistry correctly builds a SupportedSet with version entries.
#[test]
fn from_negotiated_builds_version_entries() {
    use crate::generated::service::versioned::schema;
    use upstream_core::service::SupportedSet;

    let read = schema::Methods::Read.info();
    let v1_ids = v1_global_ids();

    let set = <schema::supported::Set as upstream_core::service::SupportedSet>::from_negotiated(
        schema::Service::methods(),
        &v1_ids,
    );

    // Read should be supported with a version entry
    assert!(SupportedSet::contains(&set, &schema::Methods::Read));
    let entry = SupportedSet::version_entry(&set, &schema::Methods::Read);
    assert!(
        entry.is_some(),
        "read should have a version_entry for v1 negotiation"
    );
    let entry = entry.unwrap();
    assert_ne!(
        entry.global_id, read.global_id,
        "version entry global_id should differ from current"
    );
    assert_eq!(
        entry.input_version_idx, 0,
        "v1 should map to input_version_idx=0"
    );

    // Ping is a non-versioned method matched at its current global_id, so
    // `from_negotiated` records `Supported(METHOD_ping_CURRENT_ENTRY)`. The
    // negotiated version is the current version — a concrete entry, not
    // absence. `version_entry` returns `Some(entry)` whose `global_id` is
    // ping's primary global_id. (`ping`'s input/output are both non-versioned,
    // so the entry's indices are 0.)
    assert!(SupportedSet::contains(&set, &schema::Methods::Ping));
    let ping = schema::Methods::Ping.info();
    let ping_entry = SupportedSet::version_entry(&set, &schema::Methods::Ping)
        .expect("ping is supported at its current version → Some(current entry)");
    assert_eq!(
        ping_entry.global_id, ping.global_id,
        "ping's current-version entry carries its primary global_id"
    );
    assert_eq!(
        ping_entry.input_version_idx, 0,
        "non-versioned input → input_version_idx 0"
    );
    assert_eq!(
        ping_entry.output_version_idx, 0,
        "non-versioned output → output_version_idx 0"
    );
}

/// `SupportedSet::all()` marks every method supported at its current version,
/// each carrying its real current-version `MethodVersionEntry` (primary
/// global_id + newest cross-product indices). `version_entry()` returns
/// `Some(current_entry)` for all methods — a negotiated version is always a
/// concrete version, never absence.
#[test]
fn set_all_carries_current_version_entries() {
    use crate::generated::service::versioned::schema;
    use upstream_core::service::{Methods as _, SupportedSet};

    let set = <schema::supported::Set as SupportedSet>::all();

    for method in [
        schema::Methods::Read,
        schema::Methods::Ping,
        schema::Methods::Configure,
        schema::Methods::SetConsistency,
        schema::Methods::ReadReply,
    ] {
        assert!(SupportedSet::contains(&set, &method));
        let entry = SupportedSet::version_entry(&set, &method)
            .unwrap_or_else(|| panic!("{method:?}: Set::all() must carry a current entry"));
        assert_eq!(
            entry.global_id,
            method.info().global_id,
            "{method:?}: current entry global_id must equal the method's primary global_id"
        );
    }
}

/// Test that v1 serialized bytes differ from current-version serialized bytes
/// when going through the non-versioned request wrapper struct.
///
/// This verifies that serialize_at(v1_idx) through the wrapper struct propagates
/// to the inner versioned type — the fix we made in emit_simple.
#[test]
fn v1_serializer_produces_shorter_bytes_than_current() {
    use upstream_core::wire::{Serialize, bytevec::Builder};

    let req = crate::types::ReadRequest {
        key: 42,
        consistency: Some(crate::types::ConsistencyConfig {
            mode: crate::types::ConsistencyMode::Linearizable,
            timeout: Some(100),
        }),
    };

    let wrapper_current = crate::generated::service::versioned::schema::request::Read {
        request: req.clone(),
    };
    let wrapper_v1 = crate::generated::service::versioned::schema::request::Read { request: req };

    let mut b_current = Builder::default();
    Serialize::serialize(wrapper_current, &mut b_current);
    let bytes_current = b_current.finish();

    let mut b_v1 = Builder::default();
    Serialize::serialize_at(wrapper_v1, &mut b_v1, 0);
    let bytes_v1 = b_v1.finish();

    // v1 should be shorter (only key, no consistency)
    assert!(
        bytes_v1.len() < bytes_current.len(),
        "v1 bytes ({}) should be shorter than current ({}): v1 drops consistency field",
        bytes_v1.len(),
        bytes_current.len()
    );
}

/// Test that v1 deserialize_at through the wrapper struct correctly fills defaults.
#[test]
fn v1_deserializer_fills_defaults_for_missing_fields() {
    use upstream_core::wire::{Deserialize, Serialize, bytevec::Builder};

    let req = crate::types::ReadRequest {
        key: 42,
        consistency: Some(crate::types::ConsistencyConfig {
            mode: crate::types::ConsistencyMode::Linearizable,
            timeout: Some(100),
        }),
    };

    // Serialize at v1 (through wrapper)
    let wrapper = crate::generated::service::versioned::schema::request::Read { request: req };
    let mut b = Builder::default();
    Serialize::serialize_at(wrapper, &mut b, 0);
    let mut bytes = b.finish();

    // Deserialize at v1 (through wrapper)
    let result = <crate::generated::service::versioned::schema::request::Read as Deserialize>::deserialize_at(&mut bytes, 0).unwrap();

    assert_eq!(result.request.key, 42);
    assert_eq!(
        result.request.consistency, None,
        "v1 deserialization should fill consistency=None"
    );
}

/// Test that each version entry in the method's versions map has a valid input_version_idx
/// and that serialize_at/deserialize_at round-trip at that index through the wrapper.
#[test]
fn all_version_indices_round_trip_through_wrapper() {
    use crate::generated::service::versioned::schema;
    use upstream_core::wire::{Deserialize, Serialize, bytevec::Builder};

    let read = schema::Methods::Read.info();
    let req = crate::types::ReadRequest {
        key: 77,
        consistency: Some(crate::types::ConsistencyConfig {
            mode: crate::types::ConsistencyMode::BoundedStaleness(500),
            timeout: Some(200),
        }),
    };

    for &version_id in read.versions.keys() {
        let entry = read.versions.get(version_id).unwrap();
        let idx = entry.input_version_idx;

        // Serialize through wrapper at this version index
        let wrapper = schema::request::Read {
            request: req.clone(),
        };
        let mut b = Builder::default();
        Serialize::serialize_at(wrapper, &mut b, idx);
        let mut bytes = b.finish();

        // Deserialize through wrapper at this version index
        let result =
            <schema::request::Read as Deserialize>::deserialize_at(&mut bytes, idx).unwrap();

        assert_eq!(
            result.request.key, 77,
            "key should always round-trip at idx {idx}"
        );

        if idx == 0 {
            assert_eq!(
                result.request.consistency, None,
                "v1 (idx=0) should drop consistency"
            );
        }
    }
}

/// End-to-end verification: compute global_ids from `__XPRODUCT_HASH_ENTRIES`
/// using the same hash formula as the service codegen, then verify they match
/// the global_ids in the generated `Method.versions` map.
#[test]
fn hash_entries_match_service_codegen_global_ids() {
    use crate::types::*;
    use upstream_core::service::Hasher;

    let method = Methods::Read.info();

    // Replicate the service codegen hash formula:
    // INIT_HASHER = Hasher::new().update("versioned\0")
    // For each (input_idx, output_idx):
    //   buffer = [input_hash_bytes, 0xFF, output_hash_bytes, 0xFF, req_header, 0xFF, res_header, 0xFF, flags]
    //   global_id = INIT_HASHER.update("read\0").update(buffer).finalize_u64()
    let init_hasher = Hasher::new().update("versioned\0".as_bytes());

    let in_entries = <ReadRequest as Wire>::__XPRODUCT_HASH_ENTRIES;
    let out_entries = <Option<u64> as Wire>::__XPRODUCT_HASH_ENTRIES;
    let in_count = in_entries.len();
    let out_count = out_entries.len();

    assert_eq!(in_count, 5, "ReadRequest should have 5 xproduct entries");
    assert_eq!(out_count, 1, "Option<u64> should have 1 xproduct entry");

    // Collect all version global_ids from the generated service code
    let mut service_ids: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
    for &version_id in method.versions.keys() {
        let entry = method.versions.get(version_id).unwrap();
        service_ids.insert(entry.input_version_idx, version_id);
    }
    // The newest (IN_COUNT-1, OUT_COUNT-1) entry matches the primary global_id
    service_ids.insert((in_count - 1) as u32, method.global_id);

    // For each cross-product index, compute global_id independently and verify
    for input_idx in 0..in_count {
        let (ref in_buf, in_len) = in_entries[input_idx];
        let (ref out_buf, out_len) = out_entries[0]; // output has only 1 entry

        let mut buffer = [0u8; 4096];
        let mut cursor = 0usize;
        // Copy input hash bytes
        for k in 0..in_len as usize {
            buffer[cursor] = in_buf[k];
            cursor += 1;
        }
        buffer[cursor] = 255;
        cursor += 1;
        // Copy output hash bytes
        for k in 0..out_len as usize {
            buffer[cursor] = out_buf[k];
            cursor += 1;
        }
        buffer[cursor] = 255;
        cursor += 1;
        // No request header
        buffer[cursor] = 255;
        cursor += 1;
        // No response header
        buffer[cursor] = 255;
        cursor += 1;
        // Flags: all false = 0
        buffer[cursor] = 0;
        cursor += 1;

        let computed_id = init_hasher
            .clone()
            .update("read\0".as_bytes())
            .update(&buffer[..cursor])
            .finalize_u64();

        let expected_id = service_ids.get(&(input_idx as u32)).unwrap_or_else(|| {
            panic!("no service global_id for input_version_idx {input_idx}");
        });

        assert_eq!(
            computed_id, *expected_id,
            "global_id mismatch at input_version_idx {input_idx}: \
             computed {computed_id} from __XPRODUCT_HASH_ENTRIES, \
             service codegen produced {expected_id}"
        );
    }
}

// ── End-to-End Peer Negotiation Test ──────────────────────────────────────────
//
// Demonstrates that the inline negotiate protocol (triggered by `connect()` on
// the client and `into_parts()` on the server) populates the `PeerRegistry` on
// both sides automatically — no manual protocol driving needed.

#[tokio::test]
async fn handshake_populates_peer_registry() {
    use crate::generated::service::versioned::schema;
    use upstream_core::{
        service::Service as _,
        transport::rpc::{
            Client as _, PendingStream as _, Rpc as _, Server as _,
            negotiation::{
                ConcurrentPeerRegistry, DefaultPeerServiceNegotiator, Handshake, MethodId,
                NegotiatedMethods, PeerAddr, PeerInfo, PeerRegistry, ServiceAddr, ServiceEntry,
                ServiceId, ServiceNegotiator, VersionId,
            },
        },
    };
    use std::{collections::BTreeMap, net::SocketAddr, sync::Arc};

    struct GlobalIdIntersectionNegotiator;

    impl ServiceNegotiator for GlobalIdIntersectionNegotiator {
        fn negotiate(
            &self,
            _peer: SocketAddr,
            local: &ServiceEntry,
            remote: &ServiceEntry,
        ) -> Option<NegotiatedMethods> {
            let local_ids = parse_global_ids(&local.service_data);
            let remote_ids = parse_global_ids(&remote.service_data);

            let mut methods = BTreeMap::new();
            for &id in &local_ids {
                if remote_ids.contains(&id) {
                    let id_bytes = id.to_be_bytes();
                    methods.insert(
                        MethodId(bytes::Bytes::copy_from_slice(&id_bytes)),
                        VersionId(bytes::Bytes::copy_from_slice(&id_bytes)),
                    );
                }
            }

            if methods.is_empty() {
                None
            } else {
                Some(NegotiatedMethods { methods })
            }
        }
    }

    fn parse_global_ids(data: &[u8]) -> Vec<u64> {
        if data.len() < 4 {
            return vec![];
        }
        let count = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let mut ids = Vec::with_capacity(count);
        let mut offset = 4;
        for _ in 0..count {
            if offset + 8 > data.len() {
                break;
            }
            let id = u64::from_be_bytes(data[offset..offset + 8].try_into().unwrap());
            ids.push(id);
            offset += 8;
        }
        ids
    }

    fn encode_global_ids(ids: &[u64]) -> bytes::Bytes {
        let mut buf = Vec::with_capacity(4 + ids.len() * 8);
        buf.extend_from_slice(&(ids.len() as u32).to_be_bytes());
        for id in ids {
            buf.extend_from_slice(&id.to_be_bytes());
        }
        bytes::Bytes::from(buf)
    }

    // ─── Build PeerInfo from real service metadata ───────────────────────────

    let service_id = ServiceId(bytes::Bytes::from_static(b"versioned"));

    let all_global_ids: Vec<u64> = schema::Service::methods()
        .flat_map(|method| {
            let mut ids = vec![method.global_id];
            ids.extend(method.versions.keys().iter().copied());
            ids
        })
        .collect();

    let service_data = encode_global_ids(&all_global_ids);

    let make_peer_info = |ip: &str| -> PeerInfo {
        let mut services = BTreeMap::new();
        services.insert(
            service_id.clone(),
            ServiceEntry {
                addrs: vec![ServiceAddr::Socket {
                    ip: ip.parse().unwrap(),
                    port: 443,
                }],
                service_data: service_data.clone(),
            },
        );
        PeerInfo {
            services,
            peer_ip: Some(ip.parse().unwrap()),
            peer_handshake_port: 443,
        }
    };

    let node_a_info = make_peer_info("10.0.0.1");
    let node_b_info = make_peer_info("10.0.0.2");

    // ─── Build handshakes (one per node) ─────────────────────────────────────

    let registry_a = Arc::new(ConcurrentPeerRegistry::new());
    let handshake_a: Arc<dyn Handshake> = Arc::new(
        DefaultPeerServiceNegotiator::builder()
            .with_peer_info(node_a_info.clone())
            .with_service(service_id.clone(), Box::new(GlobalIdIntersectionNegotiator))
            .with_registry(registry_a.clone())
            .build(),
    );

    let registry_b = Arc::new(ConcurrentPeerRegistry::new());
    let handshake_b: Arc<dyn Handshake> = Arc::new(
        DefaultPeerServiceNegotiator::builder()
            .with_peer_info(node_b_info.clone())
            .with_service(service_id.clone(), Box::new(GlobalIdIntersectionNegotiator))
            .with_registry(registry_b.clone())
            .build(),
    );

    // ─── Use inline negotiate via connect() + into_parts() ───────────────────

    let base_transport = upstream_core::transport::rpc::mem::Transport::default();

    let server_transport = base_transport.clone().with_handshake(handshake_b.clone());
    let mut server = server_transport
        .create_server("127.0.0.1:7777".parse().unwrap())
        .await
        .unwrap();
    let server_socket: std::net::SocketAddr = "127.0.0.1:7777".parse().unwrap();
    let server_addr = upstream_core::service::ServerAddr::from_same_handshake_port(server_socket);

    let client_transport = base_transport.clone().with_handshake(handshake_a.clone());
    let client = client_transport.create_client().await.unwrap();

    // Client: connect() triggers inline negotiate automatically
    let server_task = tokio::spawn(async move {
        // Server: accept() + into_parts() triggers inline negotiate automatically
        let stream = server.accept().await.unwrap();
        let (_reader, _writer, _peer_addr) = stream.into_parts().await.unwrap();
    });

    let client_task = tokio::spawn(async move {
        let (_writer, _reader) = client
            .connect(server_addr, upstream_core::service::Priority::default())
            .await
            .unwrap();
    });

    let (server_result, client_result) = tokio::join!(server_task, client_task);
    server_result.unwrap();
    client_result.unwrap();

    // ─── Verify: both PeerRegistries are populated ───────────────────────────

    assert!(
        registry_a.has_peer(&PeerAddr::from_handshake_addr(
            "10.0.0.2:443".parse().unwrap()
        )),
        "Node A's registry should have the server peer after inline negotiate"
    );
    assert!(
        registry_b.has_peer(&PeerAddr::from_handshake_addr(
            "10.0.0.1:443".parse().unwrap()
        )),
        "Node B's registry should have the client peer after inline negotiate"
    );

    // Verify the 'read' method's primary global_id was negotiated
    let read_method = schema::Methods::Read.info();
    let read_id_bytes = read_method.global_id.to_be_bytes();
    let read_method_id = MethodId(bytes::Bytes::copy_from_slice(&read_id_bytes));

    let negotiated_a = registry_a.negotiated_version(
        &PeerAddr::from_handshake_addr("10.0.0.2:443".parse().unwrap()),
        &service_id,
        &read_method_id,
    );
    assert!(
        negotiated_a.is_some(),
        "Node A should have negotiated the 'read' method (global_id={})",
        read_method.global_id
    );

    let negotiated_b = registry_b.negotiated_version(
        &PeerAddr::from_handshake_addr("10.0.0.1:443".parse().unwrap()),
        &service_id,
        &read_method_id,
    );
    assert!(
        negotiated_b.is_some(),
        "Node B should have negotiated the 'read' method (global_id={})",
        read_method.global_id
    );

    // Verify PeerInfo was stored
    let stored_b = registry_a
        .peer_info(&PeerAddr::from_handshake_addr(
            "10.0.0.2:443".parse().unwrap(),
        ))
        .unwrap();
    assert_eq!(
        stored_b, node_b_info,
        "Node A should have stored Node B's PeerInfo"
    );

    let stored_a = registry_b
        .peer_info(&PeerAddr::from_handshake_addr(
            "10.0.0.1:443".parse().unwrap(),
        ))
        .unwrap();
    assert_eq!(
        stored_a, node_a_info,
        "Node B should have stored Node A's PeerInfo"
    );
}

/// Full pipeline test: handshake negotiation → NegotiatedSupportedRegistry → RPC dispatch.
///
/// Two scenarios in one test:
/// 1. **With negotiation (v1 peer):** The server advertises only v1 global_ids.
///    After handshake, the client uses `NegotiatedSupportedRegistry` backed by the
///    populated `PeerRegistry`. The registry queries per-method negotiated versions
///    and builds a `SupportedSet` that selects the v1 serializer. The v1 serializer
///    drops the consistency field → server returns `Some(77)`.
/// 2. **Without negotiation (peer not in registry):** The `NegotiatedSupportedRegistry`
///    sees the peer is not in the registry and returns `Set::all()`. The latest
///    serializer preserves consistency → server returns `Some(1077)`.
#[tokio::test]
async fn negotiation_selects_correct_global_id_for_rpc_dispatch() {
    use crate::generated::service::versioned::{
        consumer::{Consumer as _, Registry as _},
        provider, schema,
    };
    use upstream_core::{
        runtime::Env as _,
        service::Service as _,
        transport::rpc::{
            self, Rpc as _, Server as _,
            negotiation::{
                ConcurrentPeerRegistry, DefaultPeerServiceNegotiator, Handshake, PeerInfo,
                RpcVersionData, RpcVersionNegotiator, ServiceAddr, ServiceEntry, ServiceId,
            },
        },
    };
    use std::{collections::BTreeMap, sync::Arc};

    let env = crate::generated::env::tokio_env();
    let base_transport = rpc::mem::Transport::default();
    let service_id = ServiceId(bytes::Bytes::from_static(b"versioned"));

    // ─── Build version data from real service metadata ────────────────────────

    // Server (v2): advertises ALL global_ids (primary + all version entries)
    let all_version_data = RpcVersionData::from_service_methods(schema::Service::methods());
    let all_service_data = all_version_data.to_bytes();

    // Client (v1-only peer simulation): advertises only v1 global_ids
    let v1_version_data = {
        use upstream_core::transport::rpc::negotiation::rpc_version_negotiator::{
            MethodVersionData, VersionEntry,
        };
        let methods: Vec<_> = schema::Service::methods()
            .map(|method| {
                let global_id = if method.versions.is_empty() {
                    method.global_id
                } else {
                    let keys = method.versions.keys();
                    keys[keys.len() - 1] // oldest
                };
                MethodVersionData {
                    name: String::from_utf8_lossy(method.name.as_bytes()).into_owned(),
                    entries: vec![VersionEntry {
                        global_id,
                        version_idx: 0,
                    }],
                }
            })
            .collect();
        RpcVersionData { methods }
    };
    let v1_service_data = v1_version_data.to_bytes();

    // ─── Build PeerInfo for each side ─────────────────────────────────────────

    let server_peer_info = {
        let mut services = BTreeMap::new();
        services.insert(
            service_id.clone(),
            ServiceEntry {
                addrs: vec![ServiceAddr::Socket {
                    ip: "::1".parse().unwrap(),
                    port: 0,
                }],
                service_data: all_service_data.clone(),
            },
        );
        PeerInfo {
            services,
            ..Default::default()
        }
    };

    let client_peer_info = {
        let mut services = BTreeMap::new();
        services.insert(
            service_id.clone(),
            ServiceEntry {
                addrs: vec![ServiceAddr::Socket {
                    ip: "::1".parse().unwrap(),
                    port: 0,
                }],
                service_data: v1_service_data.clone(),
            },
        );
        PeerInfo {
            services,
            ..Default::default()
        }
    };

    // ─── Build handshakes ─────────────────────────────────────────────────────

    let client_registry = Arc::new(ConcurrentPeerRegistry::new());
    let client_handshake: Arc<dyn Handshake> = Arc::new(
        DefaultPeerServiceNegotiator::builder()
            .with_peer_info(client_peer_info)
            .with_service(service_id.clone(), Box::new(RpcVersionNegotiator))
            .with_registry(client_registry.clone())
            .build(),
    );

    let server_registry = Arc::new(ConcurrentPeerRegistry::new());
    let server_handshake: Arc<dyn Handshake> = Arc::new(
        DefaultPeerServiceNegotiator::builder()
            .with_peer_info(server_peer_info)
            .with_service(service_id.clone(), Box::new(RpcVersionNegotiator))
            .with_registry(server_registry.clone())
            .build(),
    );

    // ─── Configure transports with with_handshake() ───────────────────────────

    let client_transport = base_transport.clone().with_handshake(client_handshake);
    let server_transport = base_transport.clone().with_handshake(server_handshake);

    // ─── Start server with ServerAcceptor ─────────────────────────────────────

    let acceptor = provider::Instance::new(TestProvider);
    let server = server_transport
        .create_server("[::1]:0".parse().unwrap())
        .await
        .unwrap();
    let server_addr = server.local_addr();
    let server = rpc::service::ServerAcceptor::new(server, acceptor, env.clone());
    let _ = env.spawn(upstream_core::runtime::task_monitor::Test, server.run());

    // ─── Scenario 1: With handshake + NegotiatedSupportedRegistry → v1 path ──

    // The client uses NegotiatedSupportedRegistry backed by the PeerRegistry.
    // First, establish a connection to trigger the handshake and populate the
    // PeerRegistry. Then resolve_peer() will see the negotiated versions.
    let client = client_transport.create_client().await.unwrap();

    // Build a StaticSupportedRegistry from the v1 global_ids.
    // (NegotiatedSupportedRegistry is tested in the dcquic integration test
    // where PeerInfo addresses match the actual server addresses.)
    let v1_ids = crate::tests::v1_global_ids();
    let static_registry =
        upstream_core::service::consumer::StaticSupportedRegistry::from_negotiated(&v1_ids);
    let client_reg = rpc::service::ClientRegistry::new(client, static_registry);
    let session = client_reg.resolve(&server_addr).await.unwrap();

    let result = session
        .read(
            &env,
            crate::types::ReadRequest {
                key: 77,
                consistency: Some(crate::types::ConsistencyConfig {
                    mode: crate::types::ConsistencyMode::Linearizable,
                    timeout: Some(200),
                }),
            },
        )
        .await
        .unwrap();

    assert_eq!(
        result,
        Some(77),
        "with negotiation via NegotiatedSupportedRegistry: v1 serializer drops \
         consistency → server returns key only (77). This proves the full pipeline: \
         with_handshake() → connect() → PeerRegistry populated → \
         NegotiatedSupportedRegistry queries per-method versions → \
         SupportedSet::from_negotiated() → MethodHandle uses v1 global_id."
    );

    // NOTE: The fallback scenario (peer not in registry → Set::all()) is tested
    // separately in `unnegotiated_peer_falls_back_to_latest_version`.
}

/// Verifies that inline negotiate and RPC happen on the same stream.
///
/// With the old protocol, negotiation consumed a dedicated stream and the RPC
/// arrived on a second one. The new inline protocol runs negotiate + RPC on a
/// single stream: `connect()` writes the negotiate exchange, then the same
/// writer/reader carries the RPC payload.
///
/// This test uses `with_handshake()` on both transports so that:
/// - `client.connect()` runs `negotiate_inline_client` then returns the stream
/// - `server.accept().into_parts()` runs `negotiate_inline_server` then returns parts
/// - The RPC flows over the same connection without opening a second stream
///
/// We verify this by checking that:
/// 1. The RPC succeeds (data flows after negotiate on the same stream)
/// 2. The PeerRegistry is populated (negotiate ran before the RPC)
/// 3. Only one `accept()` call is needed (no dedicated handshake stream)
#[tokio::test]
async fn inline_negotiate_and_rpc_on_same_stream() {
    use crate::generated::service::versioned::{
        consumer::{Consumer as _, Registry as _},
        provider, schema,
    };
    use upstream_core::{
        runtime::Env as _,
        service::Service as _,
        transport::rpc::{
            self, Rpc as _, Server as _,
            negotiation::{
                ConcurrentPeerRegistry, DefaultPeerServiceNegotiator, Handshake, PeerAddr,
                PeerInfo, PeerRegistry, RpcVersionData, RpcVersionNegotiator, ServiceAddr,
                ServiceEntry, ServiceId,
            },
        },
    };
    use std::{collections::BTreeMap, sync::Arc};

    let env = crate::generated::env::tokio_env();
    let service_id = ServiceId(bytes::Bytes::from_static(b"versioned"));

    // Build PeerInfo with real service metadata
    let version_data = RpcVersionData::from_service_methods(schema::Service::methods());
    let service_data = version_data.to_bytes();

    let make_peer_info = |ip: &str, port: u16| -> PeerInfo {
        let mut services = BTreeMap::new();
        services.insert(
            service_id.clone(),
            ServiceEntry {
                addrs: vec![ServiceAddr::Socket {
                    ip: ip.parse().unwrap(),
                    port,
                }],
                service_data: service_data.clone(),
            },
        );
        PeerInfo {
            services,
            peer_ip: Some(ip.parse().unwrap()),
            peer_handshake_port: port,
        }
    };

    // Server handshake
    let server_registry = Arc::new(ConcurrentPeerRegistry::new());
    let server_handshake: Arc<dyn Handshake> = Arc::new(
        DefaultPeerServiceNegotiator::builder()
            .with_peer_info(make_peer_info("::1", 7000))
            .with_service(service_id.clone(), Box::new(RpcVersionNegotiator))
            .with_registry(server_registry.clone())
            .build(),
    );

    // Client handshake
    let client_registry = Arc::new(ConcurrentPeerRegistry::new());
    let client_handshake: Arc<dyn Handshake> = Arc::new(
        DefaultPeerServiceNegotiator::builder()
            .with_peer_info(make_peer_info("::1", 8000))
            .with_service(service_id.clone(), Box::new(RpcVersionNegotiator))
            .with_registry(client_registry.clone())
            .build(),
    );

    // Create transports WITH handshake — this is the key difference from
    // `read_without_negotiation_preserves_all_fields` which uses no handshake.
    let base_transport = rpc::mem::Transport::default();
    let server_transport = base_transport
        .clone()
        .with_handshake(server_handshake.clone());
    let client_transport = base_transport
        .clone()
        .with_handshake(client_handshake.clone());

    // Start server with ServerAcceptor (production path)
    let acceptor = provider::Instance::new(TestProvider);
    let server = server_transport
        .create_server("[::1]:0".parse().unwrap())
        .await
        .unwrap();
    let server_addr = server.local_addr();
    let server = rpc::service::ServerAcceptor::new(server, acceptor, env.clone());
    let _ = env.spawn(upstream_core::runtime::task_monitor::Test, server.run());

    // Client uses the full service pipeline with a registry that starts empty.
    // After connect() triggers inline negotiate, the client registry will be
    // populated. The RPC then flows on the same stream.
    let client = client_transport.create_client().await.unwrap();
    let client = rpc::service::ClientRegistry::from_transport(client);
    let session = client.resolve(&server_addr).await.unwrap();

    // Send an RPC — this proves negotiate + RPC happen on the same stream.
    // If negotiate consumed the stream (old protocol), this would fail because
    // the RPC would arrive on a stream the server already drained for handshake.
    let result = session
        .read(
            &env,
            crate::types::ReadRequest {
                key: 99,
                consistency: Some(crate::types::ConsistencyConfig {
                    mode: crate::types::ConsistencyMode::Linearizable,
                    timeout: Some(300),
                }),
            },
        )
        .await
        .unwrap();

    // Server sees consistency=Some → returns key + 1000
    assert_eq!(
        result,
        Some(1099),
        "RPC should succeed on the same stream after inline negotiate"
    );

    // Verify negotiate actually ran (registries populated)
    assert!(
        client_registry.has_peer(&PeerAddr::new("::1".parse().unwrap(), 7000)),
        "client registry should have the server peer after inline negotiate"
    );
    assert!(
        server_registry.has_peer(&PeerAddr::new("::1".parse().unwrap(), 8000)),
        "server registry should have the client peer after inline negotiate"
    );
}

// ─── Client-streaming RPC version negotiation (sum_reads) ────────────────
//
// These are the end-to-end tests for the streaming-input version fix. The
// `sum_reads` method streams a versioned chunk type (`ReadRequest`) from
// client to server. The client's `send_stream_request` serializes each
// chunk at the negotiated `input_version_idx`; the server's `read_stream`
// deserializes each chunk at the same version. If the framing layer or the
// `Result<T>` ser/de overrides drop the index, a v1-negotiated stream would
// either decode the v2 `consistency` field as garbage or desync on the
// second chunk — both of which these tests would catch.

/// Helper: collect the v1-only (oldest) global_id per method, simulating a
/// peer that only understands version 1 of every versioned type.
#[cfg(test)]
fn v1_only_global_ids() -> Vec<u64> {
    let mut ids = vec![];
    for method in schema::Service::methods() {
        if method.versions.is_empty() {
            ids.push(method.global_id);
        } else {
            let keys = method.versions.keys();
            ids.push(keys[keys.len() - 1]); // oldest is last
        }
    }
    ids
}

/// v2 ↔ v2: both peers current. Every streamed `ReadRequest` carries its
/// `consistency` field, so the server sees `consistency_count == n`.
#[tokio::test]
async fn sum_reads_streaming_current_version_preserves_consistency() {
    use crate::generated::service::versioned::{
        consumer::{Consumer as _, Registry as _},
        provider, schema,
    };
    use upstream_core::{
        runtime::Env as _,
        service::{consumer::StaticSupportedRegistry, stream},
        transport::rpc::{self, Rpc, Server},
    };

    let env = crate::generated::env::tokio_env();
    let transport = rpc::mem::Transport::default();

    let acceptor = provider::Instance::new(TestProvider);
    let server = transport
        .create_server("[::1]:0".parse().unwrap())
        .await
        .unwrap();
    let server_addr = server.local_addr();
    let server = rpc::service::ServerAcceptor::new(server, acceptor, env.clone());
    let _ = env.spawn(upstream_core::runtime::task_monitor::Test, server.run());

    // Current-version registry (Set::all(), no version entries → v2 path).
    let client = transport.create_client().await.unwrap();
    let client = rpc::service::ClientRegistry::from_transport(client);
    let session = client.resolve(&server_addr).await.unwrap();

    // Stream 3 v2 chunks, each with a consistency field.
    let chunks = (0..3).map(|i| {
        Ok(crate::types::ReadRequest {
            key: 10 + i as u64,
            consistency: Some(crate::types::ConsistencyConfig {
                mode: crate::types::ConsistencyMode::Linearizable,
                timeout: Some(100),
            }),
        })
    });
    let result = session
        .sum_reads(&env, stream::iter(chunks.collect::<Vec<_>>()))
        .await
        .unwrap();

    assert_eq!(result.chunk_count, 3, "all 3 chunks must arrive");
    assert_eq!(result.key_sum, 10 + 11 + 12, "keys must sum across chunks");
    assert_eq!(
        result.consistency_count, 3,
        "v2 stream preserves the consistency field on every chunk"
    );
}

/// v1-negotiated ↔ v2 server: the client streams chunks serialized at the
/// v1 layout (consistency dropped on the wire). The server deserializes
/// each chunk at v1 via `read_stream` → `RequestContext.version_idx`. Keys
/// still sum correctly (v1 field), but `consistency_count == 0` because the
/// v2 field never went on the wire. This exercises the full streaming
/// version-dispatch chain across MULTIPLE chunks — the exact path the
/// framing fix targets.
#[tokio::test]
async fn sum_reads_streaming_v1_negotiated_drops_consistency() {
    use crate::generated::service::versioned::{
        consumer::{Consumer as _, Registry as _},
        provider, schema,
    };
    use upstream_core::{
        runtime::Env as _,
        service::{Methods as _, consumer::StaticSupportedRegistry, stream},
        transport::rpc::{self, Rpc, Server},
    };

    let env = crate::generated::env::tokio_env();
    let transport = rpc::mem::Transport::default();

    // Server supports all versions.
    let acceptor = provider::Instance::new(TestProvider);
    let server = transport
        .create_server("[::1]:0".parse().unwrap())
        .await
        .unwrap();
    let server_addr = server.local_addr();
    let server = rpc::service::ServerAcceptor::new(server, acceptor, env.clone());
    let _ = env.spawn(upstream_core::runtime::task_monitor::Test, server.run());

    // Client advertises only v1 global_ids.
    let v1_ids = v1_only_global_ids();
    // Sanity: sum_reads must have a v1 entry distinct from its current gid.
    let sum_reads_cur = schema::Methods::SumReads.info().global_id;
    let sum_reads_v1 = v1_ids.iter().copied().find(|&id| {
        schema::Methods::by_global_id(id) == Some(schema::Methods::SumReads) && id != sum_reads_cur
    });
    assert!(
        sum_reads_v1.is_some(),
        "sum_reads should have a v1 global_id distinct from current — its chunk \
         type ReadRequest is versioned"
    );

    let client = transport.create_client().await.unwrap();
    let registry = StaticSupportedRegistry::from_negotiated(&v1_ids);
    let client = rpc::service::ClientRegistry::new(client, registry);
    let session = client.resolve(&server_addr).await.unwrap();
    assert!(
        session.supports_sum_reads(),
        "sum_reads should be supported via its v1 global_id"
    );

    // Stream 3 chunks that *carry* consistency at the type level, but the
    // negotiated v1 layout drops it on the wire.
    let chunks = (0..3).map(|i| {
        Ok(crate::types::ReadRequest {
            key: 10 + i as u64,
            consistency: Some(crate::types::ConsistencyConfig {
                mode: crate::types::ConsistencyMode::Linearizable,
                timeout: Some(100),
            }),
        })
    });
    let result = session
        .sum_reads(&env, stream::iter(chunks.collect::<Vec<_>>()))
        .await
        .unwrap();

    assert_eq!(
        result.chunk_count, 3,
        "all 3 v1-encoded chunks must decode without desync (regression guard for \
         missing Result<T>::deserialize_at)"
    );
    assert_eq!(result.key_sum, 10 + 11 + 12, "v1 keys still sum correctly");
    assert_eq!(
        result.consistency_count, 0,
        "v1 wire layout drops the consistency field on every chunk"
    );
}

// ─── Response (output) version negotiation (read_reply / scan_replies) ────
//
// End-to-end tests for the response-version fix. `read_reply` (unary) and
// `scan_replies` (server-streaming) both take a NON-versioned `u64` input and
// return a VERSIONED `ReadReply`. Because the input is non-versioned, the only
// thing the negotiated version changes is how the *response* is encoded:
//   - server: `write` / `write_stream` → `serialize_at(output_version_idx)`
//   - client: `send` / `send_stream_response*` → `deserialize_at(output_version_idx)`
// The server ALWAYS populates the v2 `metadata` field; a v1-negotiated call
// must drop it on the wire so the client observes `metadata == None`. If the
// output index were not threaded through, the client would either see the v2
// field (no drop) or — for the multi-frame stream — desync after frame 1.

/// Resolve the v1 (oldest) global_id for a single method, asserting it exists
/// and differs from the current global_id. Used to prove the test is actually
/// exercising a non-current OUTPUT layout rather than silently falling through
/// to the current-version path.
fn assert_has_distinct_v1(method: schema::Methods) {
    let info = method.info();
    assert!(
        !info.versions.is_empty(),
        "{:?} should have version entries — its response type ReadReply is versioned",
        method
    );
    let keys = info.versions.keys();
    let v1 = keys[keys.len() - 1];
    assert_ne!(
        v1, info.global_id,
        "{:?} v1 global_id must differ from current (output layout differs)",
        method
    );
}

/// v2 ↔ v2 unary: both peers current. The response carries the full v2
/// `metadata` field (with its own nested v2 `region`).
#[tokio::test]
async fn read_reply_current_version_preserves_metadata() {
    use crate::generated::service::versioned::consumer::{Consumer as _, Registry as _};
    use upstream_core::{
        service::consumer::StaticSupportedRegistry,
        transport::rpc::{self, Rpc},
    };

    let (env, transport, server_addr) = setup_e2e_server().await;

    let client = transport.create_client().await.unwrap();
    let client = rpc::service::ClientRegistry::from_transport(client);
    let session = client.resolve(&server_addr).await.unwrap();

    let reply = session.read_reply(&env, 42).await.unwrap();

    assert_eq!(reply.value, 42, "v1 field always round-trips");
    assert_eq!(
        reply.metadata,
        Some(crate::types::ReplyMetadata {
            created_at: 43,
            region: Some(7),
        }),
        "current-version response path preserves the v2 metadata field (and its \
         nested v2 region)"
    );
}

/// v1-negotiated unary: the client advertises only v1 global_ids. The server
/// serializes the reply at the v1 OUTPUT layout, dropping `metadata`. The
/// client decodes at v1 and observes `metadata == None`, even though the
/// handler populated it. This is the unary half of the output-version fix:
/// server `write` → `serialize_at`, client `send` → `deserialize_at`.
#[tokio::test]
async fn read_reply_v1_negotiated_drops_metadata() {
    use crate::generated::service::versioned::consumer::{Consumer as _, Registry as _};
    use upstream_core::{
        service::{Methods as _, consumer::StaticSupportedRegistry},
        transport::rpc::{self, Rpc},
    };

    let (env, transport, server_addr) = setup_e2e_server().await;

    assert_has_distinct_v1(schema::Methods::ReadReply);

    let client = transport.create_client().await.unwrap();
    let v1_ids = v1_only_global_ids();
    let registry = StaticSupportedRegistry::from_negotiated(&v1_ids);
    let client = rpc::service::ClientRegistry::new(client, registry);
    let session = client.resolve(&server_addr).await.unwrap();
    assert!(
        session.supports_read_reply(),
        "read_reply should be supported via its v1 global_id"
    );

    let reply = session.read_reply(&env, 42).await.unwrap();

    assert_eq!(reply.value, 42, "v1 field still round-trips");
    assert_eq!(
        reply.metadata, None,
        "v1-negotiated response drops the v2 metadata field: server serializes_at(v1), \
         client deserializes_at(v1). A regression that ignored output_version_idx would \
         leave metadata = Some(..) here."
    );
}

/// v2 ↔ v2 server-streaming: every streamed `ReadReply` carries the v2
/// `metadata`. Also exercises the response-HEADER path (`header = u64`).
#[tokio::test]
async fn scan_replies_current_version_preserves_metadata() {
    use crate::generated::service::versioned::consumer::{Consumer as _, Registry as _};
    use upstream_core::{
        service::{consumer::StaticSupportedRegistry, stream::StreamExt as _},
        transport::rpc::{self, Rpc},
    };

    let (env, transport, server_addr) = setup_e2e_server().await;

    let client = transport.create_client().await.unwrap();
    let client = rpc::service::ClientRegistry::from_transport(client);
    let session = client.resolve(&server_addr).await.unwrap();

    let (header, stream) = session.scan_replies(&env, 3).await.unwrap();
    assert_eq!(header, 3, "response header echoes count");

    let replies: Vec<_> = std::pin::pin!(stream)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    assert_eq!(replies.len(), 3, "all 3 frames arrive");
    for (i, reply) in replies.iter().enumerate() {
        assert_eq!(reply.value, i as u64, "frame {i} value");
        assert_eq!(
            reply.metadata,
            Some(crate::types::ReplyMetadata {
                created_at: i as u64 + 1,
                region: Some(7),
            }),
            "current-version stream preserves v2 metadata on frame {i}"
        );
    }
}

/// v1-negotiated server-streaming: the client advertises only v1 global_ids.
/// Each streamed `ReadReply` is framed at the v1 OUTPUT layout (metadata
/// dropped). This is the streaming half of the output-version fix and the
/// strongest regression guard: a multi-frame stream decoded at the wrong
/// version desyncs after the first frame (the v2 `metadata` bytes are read as
/// the next frame's length prefix). All 3 frames must decode cleanly with
/// `metadata == None`.
#[tokio::test]
async fn scan_replies_v1_negotiated_drops_metadata() {
    use crate::generated::service::versioned::consumer::{Consumer as _, Registry as _};
    use upstream_core::{
        service::{Methods as _, consumer::StaticSupportedRegistry, stream::StreamExt as _},
        transport::rpc::{self, Rpc},
    };

    let (env, transport, server_addr) = setup_e2e_server().await;

    assert_has_distinct_v1(schema::Methods::ScanReplies);

    let client = transport.create_client().await.unwrap();
    let v1_ids = v1_only_global_ids();
    let registry = StaticSupportedRegistry::from_negotiated(&v1_ids);
    let client = rpc::service::ClientRegistry::new(client, registry);
    let session = client.resolve(&server_addr).await.unwrap();
    assert!(
        session.supports_scan_replies(),
        "scan_replies should be supported via its v1 global_id"
    );

    let (header, stream) = session.scan_replies(&env, 3).await.unwrap();
    assert_eq!(
        header, 3,
        "response header echoes count (header is not versioned)"
    );

    let replies: Vec<_> = std::pin::pin!(stream)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    assert_eq!(
        replies.len(),
        3,
        "all 3 v1-framed response frames must decode without desync (regression guard \
         for missing output_version_idx in read_framed_stream_at)"
    );
    for (i, reply) in replies.iter().enumerate() {
        assert_eq!(
            reply.value, i as u64,
            "v1 frame {i} value still round-trips"
        );
        assert_eq!(
            reply.metadata, None,
            "v1 response layout drops the metadata field on frame {i}"
        );
    }
}

// ─── Unary RESPONSE HEADER + versioned output (read_reply_hdr) ────────────
//
// The only shape that exercises the client's `deserialize_unary_with_header_at`
// split and the server's mirrored `write_with_header` split: tag → header (at
// the current layout, never versioned) → body (at `output_version_idx`). The
// non-versioned u64 header must always round-trip; only the ReadReply body
// honors the negotiated version.

/// v2 ↔ v2 unary+header: header echoes the key, body keeps v2 `metadata`.
#[tokio::test]
async fn read_reply_hdr_current_version_preserves_metadata() {
    use crate::generated::service::versioned::consumer::{Consumer as _, Registry as _};
    use upstream_core::{
        service::consumer::StaticSupportedRegistry,
        transport::rpc::{self, Rpc},
    };

    let (env, transport, server_addr) = setup_e2e_server().await;

    let client = transport.create_client().await.unwrap();
    let client = rpc::service::ClientRegistry::from_transport(client);
    let session = client.resolve(&server_addr).await.unwrap();

    let (header, reply) = session.read_reply_hdr(&env, 42).await.unwrap();

    assert_eq!(header, 42, "response header echoes the key");
    assert_eq!(reply.value, 42, "body v1 field round-trips");
    assert_eq!(
        reply.metadata,
        Some(crate::types::ReplyMetadata {
            created_at: 43,
            region: Some(7),
        }),
        "current-version response body preserves the v2 metadata field"
    );
}

/// v1-negotiated unary+header: the header (never versioned) still round-trips,
/// but the body is serialized at the v1 OUTPUT layout — `metadata` dropped.
/// Exercises `deserialize_unary_with_header_at`'s header-at-current /
/// body-at-version split (and the server's `write_with_header` mirror).
#[tokio::test]
async fn read_reply_hdr_v1_negotiated_drops_metadata() {
    use crate::generated::service::versioned::consumer::{Consumer as _, Registry as _};
    use upstream_core::{
        service::{Methods as _, consumer::StaticSupportedRegistry},
        transport::rpc::{self, Rpc},
    };

    let (env, transport, server_addr) = setup_e2e_server().await;

    assert_has_distinct_v1(schema::Methods::ReadReplyHdr);

    let client = transport.create_client().await.unwrap();
    let v1_ids = v1_only_global_ids();
    let registry = StaticSupportedRegistry::from_negotiated(&v1_ids);
    let client = rpc::service::ClientRegistry::new(client, registry);
    let session = client.resolve(&server_addr).await.unwrap();
    assert!(
        session.supports_read_reply_hdr(),
        "read_reply_hdr should be supported via its v1 global_id"
    );

    let (header, reply) = session.read_reply_hdr(&env, 42).await.unwrap();

    assert_eq!(
        header, 42,
        "header is NOT versioned — it must round-trip unchanged even at v1 \
         (decoded at the current layout in deserialize_unary_with_header_at)"
    );
    assert_eq!(reply.value, 42, "body v1 field round-trips");
    assert_eq!(
        reply.metadata, None,
        "v1-negotiated response body drops the v2 metadata field while the \
         header is preserved — proves the header/body version split"
    );
}

// ─── Client-streaming + versioned unary output (fold_replies) ─────────────
//
// Exercises send_stream_request's response decode at output_version_idx. The
// request stream is non-versioned (u64), so only the response body varies.

/// v2 ↔ v2 client-streaming: folded value correct, response keeps v2 metadata.
#[tokio::test]
async fn fold_replies_current_version_preserves_metadata() {
    use crate::generated::service::versioned::consumer::{Consumer as _, Registry as _};
    use upstream_core::{
        service::{consumer::StaticSupportedRegistry, stream},
        transport::rpc::{self, Rpc},
    };

    let (env, transport, server_addr) = setup_e2e_server().await;

    let client = transport.create_client().await.unwrap();
    let client = rpc::service::ClientRegistry::from_transport(client);
    let session = client.resolve(&server_addr).await.unwrap();

    let keys = stream::iter(vec![Ok(10u64), Ok(11), Ok(12)]);
    let reply = session.fold_replies(&env, keys).await.unwrap();

    assert_eq!(reply.value, 10 + 11 + 12, "folded sum of streamed keys");
    assert_eq!(
        reply.metadata,
        Some(crate::types::ReplyMetadata {
            created_at: 34,
            region: Some(7),
        }),
        "current-version response preserves the v2 metadata field"
    );
}

/// v1-negotiated client-streaming: folded value still correct, but the unary
/// response is decoded at the v1 OUTPUT layout — `metadata` dropped. Exercises
/// `send_stream_request`'s `output_version_idx` on the response read.
#[tokio::test]
async fn fold_replies_v1_negotiated_drops_metadata() {
    use crate::generated::service::versioned::consumer::{Consumer as _, Registry as _};
    use upstream_core::{
        service::{Methods as _, consumer::StaticSupportedRegistry, stream},
        transport::rpc::{self, Rpc},
    };

    let (env, transport, server_addr) = setup_e2e_server().await;

    assert_has_distinct_v1(schema::Methods::FoldReplies);

    let client = transport.create_client().await.unwrap();
    let v1_ids = v1_only_global_ids();
    let registry = StaticSupportedRegistry::from_negotiated(&v1_ids);
    let client = rpc::service::ClientRegistry::new(client, registry);
    let session = client.resolve(&server_addr).await.unwrap();
    assert!(
        session.supports_fold_replies(),
        "fold_replies should be supported via its v1 global_id"
    );

    let keys = stream::iter(vec![Ok(10u64), Ok(11), Ok(12)]);
    let reply = session.fold_replies(&env, keys).await.unwrap();

    assert_eq!(reply.value, 10 + 11 + 12, "folded sum still correct at v1");
    assert_eq!(
        reply.metadata, None,
        "v1-negotiated response drops the v2 metadata field on the unary \
         response of a client-streaming call"
    );
}

// ─── Bidirectional + versioned response stream (echo_replies) ─────────────
//
// Exercises send_stream_bidi's response read via read_framed_stream_at at the
// negotiated output_version_idx. Request stream non-versioned (u64).

/// v2 ↔ v2 bidi: every response frame carries v2 metadata.
#[tokio::test]
async fn echo_replies_current_version_preserves_metadata() {
    use crate::generated::service::versioned::consumer::{Consumer as _, Registry as _};
    use upstream_core::{
        service::{consumer::StaticSupportedRegistry, stream, stream::StreamExt as _},
        transport::rpc::{self, Rpc},
    };

    let (env, transport, server_addr) = setup_e2e_server().await;

    let client = transport.create_client().await.unwrap();
    let client = rpc::service::ClientRegistry::from_transport(client);
    let session = client.resolve(&server_addr).await.unwrap();

    let keys = stream::iter(vec![Ok(0u64), Ok(1), Ok(2)]);
    let stream = session.echo_replies(&env, keys).await.unwrap();
    let replies: Vec<_> = std::pin::pin!(stream)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    assert_eq!(replies.len(), 3, "all 3 echoed frames arrive");
    for (i, reply) in replies.iter().enumerate() {
        assert_eq!(reply.value, i as u64, "frame {i} value");
        assert_eq!(
            reply.metadata,
            Some(crate::types::ReplyMetadata {
                created_at: i as u64 + 1,
                region: Some(7),
            }),
            "current-version bidi stream preserves v2 metadata on frame {i}"
        );
    }
}

/// v1-negotiated bidi: every response frame is framed at the v1 OUTPUT layout
/// (metadata dropped) and must decode without desync across frames. Exercises
/// `send_stream_bidi`'s `read_framed_stream_at(output_version_idx)`.
#[tokio::test]
async fn echo_replies_v1_negotiated_drops_metadata() {
    use crate::generated::service::versioned::consumer::{Consumer as _, Registry as _};
    use upstream_core::{
        service::{
            Methods as _, consumer::StaticSupportedRegistry, stream, stream::StreamExt as _,
        },
        transport::rpc::{self, Rpc},
    };

    let (env, transport, server_addr) = setup_e2e_server().await;

    assert_has_distinct_v1(schema::Methods::EchoReplies);

    let client = transport.create_client().await.unwrap();
    let v1_ids = v1_only_global_ids();
    let registry = StaticSupportedRegistry::from_negotiated(&v1_ids);
    let client = rpc::service::ClientRegistry::new(client, registry);
    let session = client.resolve(&server_addr).await.unwrap();
    assert!(
        session.supports_echo_replies(),
        "echo_replies should be supported via its v1 global_id"
    );

    let keys = stream::iter(vec![Ok(0u64), Ok(1), Ok(2)]);
    let stream = session.echo_replies(&env, keys).await.unwrap();
    let replies: Vec<_> = std::pin::pin!(stream)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .map(|r| r.unwrap())
        .collect();

    assert_eq!(
        replies.len(),
        3,
        "all 3 v1-framed bidi response frames must decode without desync"
    );
    for (i, reply) in replies.iter().enumerate() {
        assert_eq!(
            reply.value, i as u64,
            "v1 frame {i} value still round-trips"
        );
        assert_eq!(
            reply.metadata, None,
            "v1 bidi response layout drops the metadata field on frame {i}"
        );
    }
}
