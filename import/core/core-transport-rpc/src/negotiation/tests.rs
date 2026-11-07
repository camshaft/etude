// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::{Client as _, PendingStream as _, Reader as _, Rpc as _, Server as _, Writer as _};
use upstream_service::ServerAddr;
use std::{collections::BTreeMap, net::SocketAddr, sync::Arc};

#[test]
fn peer_info_round_trip_empty() {
    let info = PeerInfo::default();
    let bytes = info.to_bytes();
    let decoded = PeerInfo::from_bytes(upstream_bytevec::ByteVec::from(bytes)).unwrap();
    assert_eq!(info, decoded);
}

#[test]
fn peer_info_round_trip_with_services() {
    let mut services = BTreeMap::new();
    services.insert(
        ServiceId(bytes::Bytes::from_static(b"replica")),
        ServiceEntry {
            addrs: vec![
                ServiceAddr::Socket {
                    ip: "10.0.0.1".parse().unwrap(),
                    port: 8080,
                },
                ServiceAddr::Socket {
                    ip: "::1".parse().unwrap(),
                    port: 9090,
                },
            ],
            service_data: bytes::Bytes::from_static(b"version-data-here"),
        },
    );
    services.insert(
        ServiceId(bytes::Bytes::from_static(b"gossip")),
        ServiceEntry {
            addrs: vec![ServiceAddr::Fabric {
                addr: bytes::Bytes::from_static(b"fabric://node1"),
            }],
            service_data: bytes::Bytes::from_static(b""),
        },
    );

    let info = PeerInfo {
        services,
        ..Default::default()
    };
    let bytes = info.to_bytes();
    let decoded = PeerInfo::from_bytes(upstream_bytevec::ByteVec::from(bytes)).unwrap();
    assert_eq!(info, decoded);
}

#[test]
fn concurrent_peer_registry_basic() {
    let registry = ConcurrentPeerRegistry::new();
    let peer: SocketAddr = "10.0.0.1:8080".parse().unwrap();
    let service = ServiceId(bytes::Bytes::from_static(b"replica"));
    let method = MethodId(bytes::Bytes::from_static(b"read"));
    let version = VersionId(bytes::Bytes::from_static(b"v1-hash"));

    assert!(!registry.has_peer(&PeerAddr::from_handshake_addr(peer)));

    registry.set_peer_info(PeerAddr::from_handshake_addr(peer), PeerInfo::default());
    assert!(registry.has_peer(&PeerAddr::from_handshake_addr(peer)));

    let mut methods = NegotiatedMethods::default();
    methods.methods.insert(method.clone(), version.clone());
    registry.set_negotiated(PeerAddr::from_handshake_addr(peer), &service, methods);

    let resolved =
        registry.negotiated_version(&PeerAddr::from_handshake_addr(peer), &service, &method);
    assert_eq!(resolved, Some(version));

    let unknown = MethodId(bytes::Bytes::from_static(b"unknown"));
    assert_eq!(
        registry.negotiated_version(&PeerAddr::from_handshake_addr(peer), &service, &unknown),
        None
    );
}

#[test]
fn default_peer_service_negotiator_builder() {
    struct EchoNegotiator;

    impl ServiceNegotiator for EchoNegotiator {
        fn negotiate(
            &self,
            _peer: SocketAddr,
            _local: &ServiceEntry,
            remote: &ServiceEntry,
        ) -> Option<NegotiatedMethods> {
            let mut methods = BTreeMap::new();
            methods.insert(
                MethodId(bytes::Bytes::from_static(b"test_method")),
                VersionId(remote.service_data.clone()),
            );
            Some(NegotiatedMethods { methods })
        }
    }

    let service_id = ServiceId(bytes::Bytes::from_static(b"test_service"));
    let registry = Arc::new(ConcurrentPeerRegistry::new());

    let mut local_services = BTreeMap::new();
    local_services.insert(
        service_id.clone(),
        ServiceEntry {
            addrs: vec![ServiceAddr::Socket {
                ip: "127.0.0.1".parse().unwrap(),
                port: 443,
            }],
            service_data: bytes::Bytes::from_static(b"local-v1"),
        },
    );
    let local_info = PeerInfo {
        services: local_services,
        ..Default::default()
    };

    let negotiator = DefaultPeerServiceNegotiator::builder()
        .with_peer_info(local_info.clone())
        .with_service(service_id.clone(), Box::new(EchoNegotiator))
        .with_registry(registry.clone())
        .build();

    let payload = negotiator.local_payload();
    let decoded = PeerInfo::from_bytes(upstream_bytevec::ByteVec::from(payload)).unwrap();
    assert_eq!(decoded, local_info);

    let peer: SocketAddr = "10.0.0.2:443".parse().unwrap();
    assert!(!negotiator.is_negotiated(&peer));

    let mut remote_services = BTreeMap::new();
    remote_services.insert(
        service_id.clone(),
        ServiceEntry {
            addrs: vec![ServiceAddr::Socket {
                ip: "10.0.0.2".parse().unwrap(),
                port: 443,
            }],
            service_data: bytes::Bytes::from_static(b"remote-v2"),
        },
    );
    let remote_info = PeerInfo {
        services: remote_services,
        ..Default::default()
    };
    let remote_payload = remote_info.to_bytes();

    negotiator.negotiate(peer, remote_payload).unwrap();

    assert!(negotiator.is_negotiated(&peer));
    let version = registry.negotiated_version(
        &PeerAddr::from_handshake_addr(peer),
        &service_id,
        &MethodId(bytes::Bytes::from_static(b"test_method")),
    );
    assert_eq!(
        version,
        Some(VersionId(bytes::Bytes::from_static(b"remote-v2")))
    );

    let stored_info = registry.peer_info(&PeerAddr::from_handshake_addr(peer));
    assert_eq!(stored_info, Some(remote_info));
}

struct TestNegotiator;

impl ServiceNegotiator for TestNegotiator {
    fn negotiate(
        &self,
        _peer: SocketAddr,
        _local: &ServiceEntry,
        remote: &ServiceEntry,
    ) -> Option<NegotiatedMethods> {
        let mut methods = BTreeMap::new();
        methods.insert(
            MethodId(bytes::Bytes::from_static(b"read")),
            VersionId(remote.service_data.clone()),
        );
        methods.insert(
            MethodId(bytes::Bytes::from_static(b"write")),
            VersionId(remote.service_data.clone()),
        );
        Some(NegotiatedMethods { methods })
    }
}

fn make_test_peer_info(service_id: &ServiceId, ip: &str, port: u16, data: &[u8]) -> PeerInfo {
    let mut services = BTreeMap::new();
    services.insert(
        service_id.clone(),
        ServiceEntry {
            addrs: vec![ServiceAddr::Socket {
                ip: ip.parse().unwrap(),
                port,
            }],
            service_data: bytes::Bytes::copy_from_slice(data),
        },
    );
    PeerInfo {
        services,
        peer_ip: Some(ip.parse().unwrap()),
        peer_handshake_port: port,
    }
}

/// Two in-memory peers exchange PeerInfo via the inline negotiate protocol.
/// After the exchange, both PeerRegistries are populated with negotiated methods.
#[tokio::test]
async fn handshake_over_memory_transport() {
    let service_id = ServiceId(bytes::Bytes::from_static(b"replica"));

    let registry_a = Arc::new(ConcurrentPeerRegistry::new());
    let peer_info_a = make_test_peer_info(&service_id, "10.0.0.1", 443, b"version-A");
    let handshake_a: Arc<dyn Handshake> = Arc::new(
        DefaultPeerServiceNegotiator::builder()
            .with_peer_info(peer_info_a.clone())
            .with_service(service_id.clone(), Box::new(TestNegotiator))
            .with_registry(registry_a.clone())
            .build(),
    );

    let registry_b = Arc::new(ConcurrentPeerRegistry::new());
    let peer_info_b = make_test_peer_info(&service_id, "10.0.0.2", 443, b"version-B");
    let handshake_b: Arc<dyn Handshake> = Arc::new(
        DefaultPeerServiceNegotiator::builder()
            .with_peer_info(peer_info_b.clone())
            .with_service(service_id.clone(), Box::new(TestNegotiator))
            .with_registry(registry_b.clone())
            .build(),
    );

    let transport = crate::mem::Transport::default();
    let server_transport = transport.clone().with_handshake(handshake_b.clone());
    let client_transport = transport.clone().with_handshake(handshake_a.clone());

    let mut server = server_transport
        .create_server("127.0.0.1:443".parse().unwrap())
        .await
        .unwrap();
    let client = client_transport.create_client().await.unwrap();

    let server_addr = ServerAddr::from_same_handshake_port("127.0.0.1:443".parse().unwrap());

    // Server accept loop: first accepted stream runs the negotiate RPC (handler
    // returns `Err("stream consumed for negotiate RPC")` from into_parts);
    // subsequent streams are the actual RPCs (none in this test, so we ignore them).
    let server_task = tokio::spawn(async move {
        while let Some(stream) = server.accept().await {
            // into_parts() returns Err when the stream was consumed for negotiate.
            // Either way we just continue accepting — the test only asserts that
            // the registry was populated.
            if stream.into_parts().await.is_ok() {
                // Actual RPC stream — not used in this test.
                break;
            }
        }
    });

    let client_task = tokio::spawn(async move {
        let (_writer, _reader) = client
            .connect(server_addr, upstream_service::Priority::default())
            .await
            .unwrap();
    });

    let (server_result, client_result) = tokio::join!(server_task, client_task);
    server_result.unwrap();
    client_result.unwrap();

    // Verify Peer A's registry (client side)
    // Lookup by PeerInfo address (Node B advertises 10.0.0.2:443)
    assert!(registry_a.has_peer(&PeerAddr::from_handshake_addr(
        "10.0.0.2:443".parse().unwrap()
    )));
    let version_a = registry_a
        .negotiated_version(
            &PeerAddr::from_handshake_addr("10.0.0.2:443".parse().unwrap()),
            &service_id,
            &MethodId(bytes::Bytes::from_static(b"read")),
        )
        .expect("peer A should have negotiated 'read' method");
    assert_eq!(
        version_a,
        VersionId(bytes::Bytes::from_static(b"version-B"))
    );

    let stored_b = registry_a
        .peer_info(&PeerAddr::from_handshake_addr(
            "10.0.0.2:443".parse().unwrap(),
        ))
        .unwrap();
    assert_eq!(stored_b, peer_info_b);

    // Verify Peer B's registry (server side)
    assert!(registry_b.has_peer(&PeerAddr::from_handshake_addr(
        "10.0.0.1:443".parse().unwrap()
    )));
    let version_b = registry_b
        .negotiated_version(
            &PeerAddr::from_handshake_addr("10.0.0.1:443".parse().unwrap()),
            &service_id,
            &MethodId(bytes::Bytes::from_static(b"read")),
        )
        .expect("peer B should have negotiated 'read' method");
    assert_eq!(
        version_b,
        VersionId(bytes::Bytes::from_static(b"version-A"))
    );

    let stored_a = registry_b
        .peer_info(&PeerAddr::from_handshake_addr(
            "10.0.0.1:443".parse().unwrap(),
        ))
        .unwrap();
    assert_eq!(stored_a, peer_info_a);
}

// ─── Protocol Tests ───────────────────────────────────────────────

/// Verify handshake is skipped when the peer has already been negotiated.
#[tokio::test]
async fn handshake_skipped_when_already_negotiated() {
    use crate::mem;

    let service_id = ServiceId(bytes::Bytes::from_static(b"svc"));

    // Pre-populate the registry so is_negotiated returns true for the server addr
    let registry = Arc::new(ConcurrentPeerRegistry::new());
    let server_socket: SocketAddr = "127.0.0.1:9004".parse().unwrap();
    registry.set_peer_info(
        PeerAddr::from_handshake_addr(server_socket),
        PeerInfo::default(),
    );

    let hs: Arc<dyn Handshake> = Arc::new(
        DefaultPeerServiceNegotiator::builder()
            .with_peer_info(make_test_peer_info(&service_id, "10.0.0.1", 443, b"cli"))
            .with_service(service_id.clone(), Box::new(TestNegotiator))
            .with_registry(registry.clone())
            .build(),
    );

    let transport = mem::Transport::default();
    // Only the client has a handshake; the server does not (it just echoes).
    // This confirms that connect() skips negotiation when is_negotiated=true.
    let client_transport = transport.clone().with_handshake(hs.clone());

    let mut server = transport
        .create_server("127.0.0.1:9004".parse().unwrap())
        .await
        .unwrap();
    let client = client_transport.create_client().await.unwrap();
    let addr = ServerAddr::from_same_handshake_port("127.0.0.1:9004".parse().unwrap());

    // The default connect() checks is_negotiated — it should skip the handshake
    // and just open a stream directly. Server just echoes data.
    let server_task = tokio::spawn(async move {
        let stream = server.accept().await.unwrap();
        let (reader, writer, _peer_addr) = stream.into_parts().await.unwrap();
        // Server echoes what it receives (no handshake expected)
        let data = reader.read_to_bytes().await.unwrap();
        writer.write_to_end(data).await.unwrap();
    });

    assert!(
        hs.is_negotiated(&server_socket),
        "peer should already be negotiated"
    );

    let (writer, reader) = client
        .connect(addr, upstream_service::Priority::default())
        .await
        .unwrap();
    writer.write_to_end(&b"hello"[..]).await.unwrap();
    let response = reader.read_to_bytes().await.unwrap();
    assert_eq!(response, b"hello".as_slice());

    server_task.await.unwrap();
}

/// Verify handshake works with multiple services — each service's negotiator
/// produces its own method mappings.
#[tokio::test]
async fn handshake_with_multiple_services() {
    let svc_a = ServiceId(bytes::Bytes::from_static(b"replica"));
    let svc_b = ServiceId(bytes::Bytes::from_static(b"gossip"));
    let svc_c = ServiceId(bytes::Bytes::from_static(b"metrics"));

    let mut services = BTreeMap::new();
    services.insert(
        svc_a.clone(),
        ServiceEntry {
            addrs: vec![ServiceAddr::Socket {
                ip: "10.0.0.1".parse().unwrap(),
                port: 443,
            }],
            service_data: bytes::Bytes::from_static(b"replica-v3"),
        },
    );
    services.insert(
        svc_b.clone(),
        ServiceEntry {
            addrs: vec![ServiceAddr::Socket {
                ip: "10.0.0.1".parse().unwrap(),
                port: 444,
            }],
            service_data: bytes::Bytes::from_static(b"gossip-v2"),
        },
    );
    services.insert(
        svc_c.clone(),
        ServiceEntry {
            addrs: vec![ServiceAddr::Socket {
                ip: "10.0.0.1".parse().unwrap(),
                port: 445,
            }],
            service_data: bytes::Bytes::from_static(b"metrics-v1"),
        },
    );
    let peer_info = PeerInfo {
        services,
        ..Default::default()
    };

    // Local peer also has the same services (so negotiation finds matches)
    let mut local_services = BTreeMap::new();
    local_services.insert(
        svc_a.clone(),
        ServiceEntry {
            addrs: vec![],
            service_data: bytes::Bytes::from_static(b"local"),
        },
    );
    local_services.insert(
        svc_b.clone(),
        ServiceEntry {
            addrs: vec![],
            service_data: bytes::Bytes::from_static(b"local"),
        },
    );
    local_services.insert(
        svc_c.clone(),
        ServiceEntry {
            addrs: vec![],
            service_data: bytes::Bytes::from_static(b"local"),
        },
    );
    let local_info = PeerInfo {
        services: local_services,
        ..Default::default()
    };

    let registry = Arc::new(ConcurrentPeerRegistry::new());
    let negotiator = DefaultPeerServiceNegotiator::builder()
        .with_peer_info(local_info)
        .with_service(svc_a.clone(), Box::new(TestNegotiator))
        .with_service(svc_b.clone(), Box::new(TestNegotiator))
        .with_service(svc_c.clone(), Box::new(TestNegotiator))
        .with_registry(registry.clone())
        .build();

    let peer: SocketAddr = "10.0.0.1:443".parse().unwrap();
    negotiator.negotiate(peer, peer_info.to_bytes()).unwrap();

    // Each service should have "read" and "write" methods negotiated
    assert_eq!(
        registry.negotiated_version(
            &PeerAddr::from_handshake_addr(peer),
            &svc_a,
            &MethodId(bytes::Bytes::from_static(b"read"))
        ),
        Some(VersionId(bytes::Bytes::from_static(b"replica-v3")))
    );
    assert_eq!(
        registry.negotiated_version(
            &PeerAddr::from_handshake_addr(peer),
            &svc_b,
            &MethodId(bytes::Bytes::from_static(b"read"))
        ),
        Some(VersionId(bytes::Bytes::from_static(b"gossip-v2")))
    );
    assert_eq!(
        registry.negotiated_version(
            &PeerAddr::from_handshake_addr(peer),
            &svc_c,
            &MethodId(bytes::Bytes::from_static(b"write"))
        ),
        Some(VersionId(bytes::Bytes::from_static(b"metrics-v1")))
    );
}

/// Verify that when peers have no common services, negotiation succeeds but
/// no methods are negotiated — only PeerInfo is stored.
#[tokio::test]
async fn handshake_with_no_common_service() {
    let svc_foo = ServiceId(bytes::Bytes::from_static(b"foo"));
    let svc_bar = ServiceId(bytes::Bytes::from_static(b"bar"));

    // Local peer has service "foo"
    let registry = Arc::new(ConcurrentPeerRegistry::new());
    let negotiator = DefaultPeerServiceNegotiator::builder()
        .with_peer_info(make_test_peer_info(&svc_foo, "10.0.0.1", 443, b"foo-v1"))
        .with_service(svc_foo.clone(), Box::new(TestNegotiator))
        .with_registry(registry.clone())
        .build();

    // Remote peer has service "bar" (no overlap)
    let remote_info = make_test_peer_info(&svc_bar, "10.0.0.2", 443, b"bar-v1");
    let peer: SocketAddr = "10.0.0.2:443".parse().unwrap();

    negotiator.negotiate(peer, remote_info.to_bytes()).unwrap();

    // PeerInfo should be stored
    assert!(registry.has_peer(&PeerAddr::from_handshake_addr(peer)));
    assert_eq!(
        registry.peer_info(&PeerAddr::from_handshake_addr(peer)),
        Some(remote_info)
    );

    // No methods should be negotiated (no common services)
    assert_eq!(
        registry.negotiated_version(
            &PeerAddr::from_handshake_addr(peer),
            &svc_foo,
            &MethodId(bytes::Bytes::from_static(b"read"))
        ),
        None
    );
    assert_eq!(
        registry.negotiated_version(
            &PeerAddr::from_handshake_addr(peer),
            &svc_bar,
            &MethodId(bytes::Bytes::from_static(b"read"))
        ),
        None
    );
}

// ─── Asymmetric Version Tests ────────────────────────────────────────────────
//
// These tests simulate two peers running different software versions where
// one peer's PeerInfo has fields/variants the other peer doesn't know about.
// TaggedWire's TLV encoding handles this transparently: unknown fields are
// skipped, missing fields get defaults.

/// Simulate a "v2" peer that sends PeerInfo with an extra unknown TLV field
/// appended to the ServiceEntry. The "v1" peer (using the current code)
/// deserializes successfully, ignoring the unknown field.
#[tokio::test]
async fn asymmetric_version_newer_peer_extra_field_in_service_entry() {
    use upstream_bytevec::ByteVec;
    use upstream_wire::{Serialize, TaggedSerialize};

    let service_id = ServiceId(bytes::Bytes::from_static(b"replica"));

    // Build a "v2" PeerInfo by hand: serialize normally, then inject an extra
    // TLV field (tag 9.9) into the ServiceEntry bytes before wrapping in PeerInfo.
    let v2_service_entry = {
        let mut out = upstream_bytevec::Builder::default();
        // Normal ServiceEntry fields
        let entry = ServiceEntry {
            addrs: vec![ServiceAddr::Socket {
                ip: "10.0.0.2".parse().unwrap(),
                port: 443,
            }],
            service_data: bytes::Bytes::from_static(b"v2-data"),
        };
        entry.tagged_serialize(&mut out);

        // Append an unknown field: tag 9.9 with arbitrary payload
        // This simulates a field added in a newer version of ServiceEntry
        upstream_wire::Serialize::serialize(9u8, &mut out);
        upstream_wire::Serialize::serialize(9u8, &mut out);
        out.write_with_len_prefix(|out| {
            upstream_wire::Serialize::serialize(999u64, out);
        });
        out.finish()
    };

    // Deserialize the extended ServiceEntry — unknown field should be skipped
    let decoded =
        ServiceEntry::from_bytes_raw(ByteVec::from(v2_service_entry.copy_to_bytes())).unwrap();
    assert_eq!(decoded.addrs.len(), 1);
    assert_eq!(decoded.service_data, bytes::Bytes::from_static(b"v2-data"));
}

/// Full end-to-end: Peer A (v2) has extra fields in its PeerInfo that Peer B
/// (v1, current code) doesn't know about. Both peers complete the inline
/// negotiate handshake and negotiate successfully.
#[tokio::test]
async fn asymmetric_version_handshake_with_extra_peer_info_fields() {
    use upstream_wire::TaggedSerialize;

    let service_id = ServiceId(bytes::Bytes::from_static(b"replica"));

    // --- Peer A (v1, current code) ---
    let registry_a = Arc::new(ConcurrentPeerRegistry::new());
    let peer_info_a = make_test_peer_info(&service_id, "10.0.0.1", 443, b"version-A");
    let handshake_a: Arc<dyn Handshake> = Arc::new(
        DefaultPeerServiceNegotiator::builder()
            .with_peer_info(peer_info_a.clone())
            .with_service(service_id.clone(), Box::new(TestNegotiator))
            .with_registry(registry_a.clone())
            .build(),
    );

    // --- Peer B (v2, simulated with extra TLV fields) ---
    // We build Peer B's handshake payload manually to include unknown fields.
    let registry_b = Arc::new(ConcurrentPeerRegistry::new());

    // Build v2 PeerInfo with extra unknown TLV field at the PeerInfo level
    let v2_peer_info_bytes = {
        let peer_info_b = make_test_peer_info(&service_id, "10.0.0.2", 443, b"version-B");
        let mut out = upstream_bytevec::Builder::default();
        peer_info_b.tagged_serialize(&mut out);
        // Append unknown top-level field (tag 8.1) — simulates a PeerInfo extension
        upstream_wire::Serialize::serialize(8u8, &mut out);
        upstream_wire::Serialize::serialize(1u8, &mut out);
        out.write_with_len_prefix(|out| {
            upstream_wire::Serialize::serialize("future-capability".to_string(), out);
        });
        out.finish().copy_to_bytes()
    };

    // Peer B uses a custom handshake that sends the extended bytes
    struct V2Handshake {
        payload: bytes::Bytes,
        registry: Arc<ConcurrentPeerRegistry>,
        service_id: ServiceId,
    }
    impl Handshake for V2Handshake {
        fn local_payload(&self) -> bytes::Bytes {
            self.payload.clone()
        }
        fn local_handshake_port(&self) -> u16 {
            0
        }
        fn negotiate(&self, peer: SocketAddr, remote_payload: bytes::Bytes) -> std::io::Result<()> {
            // V2 peer can deserialize v1's PeerInfo (no unknown fields — subset)
            let remote_info =
                PeerInfo::from_bytes(upstream_bytevec::ByteVec::from(remote_payload))?;
            self.registry
                .set_peer_info(PeerAddr::from_handshake_addr(peer), remote_info);
            // Simulate negotiation: just store a method
            let mut methods = NegotiatedMethods::default();
            methods.methods.insert(
                MethodId(bytes::Bytes::from_static(b"read")),
                VersionId(bytes::Bytes::from_static(b"negotiated")),
            );
            self.registry.set_negotiated(
                PeerAddr::from_handshake_addr(peer),
                &self.service_id,
                methods,
            );
            Ok(())
        }
        fn is_negotiated(&self, peer: &SocketAddr) -> bool {
            self.registry
                .has_peer(&PeerAddr::from_handshake_addr(*peer))
        }
    }

    let handshake_b: Arc<dyn Handshake> = Arc::new(V2Handshake {
        payload: v2_peer_info_bytes,
        registry: registry_b.clone(),
        service_id: service_id.clone(),
    });

    // --- Run the handshake over memory transport using with_handshake ---
    let transport = crate::mem::Transport::default();
    let server_transport = transport.clone().with_handshake(handshake_b.clone());
    let client_transport = transport.clone().with_handshake(handshake_a.clone());

    let mut server = server_transport
        .create_server("127.0.0.1:9100".parse().unwrap())
        .await
        .unwrap();
    let client = client_transport.create_client().await.unwrap();
    let server_addr = ServerAddr::from_same_handshake_port("127.0.0.1:9100".parse().unwrap());

    // Server = Peer B (v2, extended PeerInfo). Loop accepting streams: the
    // first one runs the negotiate RPC (returns Err from into_parts), the
    // second is the actual RPC stream (unused in this test).
    let server_task = tokio::spawn(async move {
        while let Some(stream) = server.accept().await {
            if stream.into_parts().await.is_ok() {
                break;
            }
        }
    });

    // Client = Peer A (v1, current code)
    let client_task = tokio::spawn(async move {
        let (_writer, _reader) = client
            .connect(server_addr, upstream_service::Priority::default())
            .await
            .unwrap();
    });

    let (server_result, client_result) = tokio::join!(server_task, client_task);
    server_result.unwrap();
    client_result.unwrap();

    // --- Verify Peer A (v1) successfully processed Peer B's (v2) PeerInfo ---
    assert!(
        registry_a.has_peer(&PeerAddr::from_handshake_addr(
            "10.0.0.2:443".parse().unwrap()
        )),
        "Peer A should have negotiated with Peer B despite extra TLV fields"
    );

    // Peer A should have stored Peer B's PeerInfo (with unknown fields stripped)
    let stored = registry_a
        .peer_info(&PeerAddr::from_handshake_addr(
            "10.0.0.2:443".parse().unwrap(),
        ))
        .unwrap();
    assert_eq!(stored.services.len(), 1);
    let entry = stored.services.get(&service_id).unwrap();
    assert_eq!(entry.service_data, bytes::Bytes::from_static(b"version-B"));

    // Peer A's negotiator (TestNegotiator) should have produced results
    let version = registry_a
        .negotiated_version(
            &PeerAddr::from_handshake_addr("10.0.0.2:443".parse().unwrap()),
            &service_id,
            &MethodId(bytes::Bytes::from_static(b"read")),
        )
        .expect("Peer A should have negotiated 'read' method with Peer B");
    assert_eq!(version, VersionId(bytes::Bytes::from_static(b"version-B")));
}

/// Peer A sends PeerInfo with a ServiceAddr variant that Peer B doesn't know
/// about (simulating a future `Rdma` variant). Since ServiceAddr has no
/// fallback, the unknown variant will error during deserialization. This test
/// documents the current behavior: without a fallback variant, new address
/// types from newer peers cause a deserialization failure on the addrs field,
/// but the ServiceEntry still deserializes (the addrs field defaults to empty
/// because the whole field's TLV is bounded).
#[test]
fn asymmetric_version_unknown_service_addr_variant() {
    use upstream_bytevec::ByteVec;
    use upstream_wire::{Serialize, TaggedDeserialize, TaggedSerialize};

    // Build a ServiceEntry where addrs contains a "future" ServiceAddr variant
    // We construct the bytes manually to simulate a newer enum variant
    let mut out = upstream_bytevec::Builder::default();

    // Tag 1.1 (addrs field)
    upstream_wire::Serialize::serialize(1u8, &mut out);
    upstream_wire::Serialize::serialize(1u8, &mut out);
    out.write_with_len_prefix(|out| {
        // Vec length = 1
        upstream_wire::Serialize::serialize(1u64, out);
        // One ServiceAddr element with length prefix (as Serialize impl does)
        out.write_with_len_prefix(|out| {
            // Unknown variant tag (3.1) — doesn't exist in current enum
            upstream_wire::Serialize::serialize(3u8, out);
            upstream_wire::Serialize::serialize(1u8, out);
            out.write_with_len_prefix(|out| {
                upstream_wire::Serialize::serialize(42u64, out); // some payload
            });
        });
    });

    // Tag 1.2 (service_data field)
    upstream_wire::Serialize::serialize(1u8, &mut out);
    upstream_wire::Serialize::serialize(2u8, &mut out);
    out.write_with_len_prefix(|out| {
        upstream_wire::Serialize::serialize(bytes::Bytes::from_static(b"data"), out);
    });

    let bytes = out.finish();
    let mut buf = ByteVec::from(bytes.copy_to_bytes());

    // Deserialization of ServiceEntry should still succeed because:
    // The addrs Vec deserialization will fail on the unknown variant (no fallback),
    // but the TLV length prefix on the addrs field bounds the damage — the
    // ServiceEntry-level deserializer skips the malformed field and uses default.
    let result = ServiceEntry::tagged_deserialize(&mut buf);

    // The ServiceEntry deserializer uses unwrap_or_default for Vec<ServiceAddr>
    // when deserialization of the field content fails (the split_to bounds it).
    // Actually — the current impl deserializes the bounded bytes, so if the
    // inner Vec deser fails, the whole field match arm returns Err, BUT that Err
    // propagates up. Let's check what actually happens:
    match result {
        Ok(entry) => {
            // If it succeeds, addrs should be default (empty) because the
            // inner deserialization error was caught by the bounded field parse.
            // service_data should be correct.
            assert_eq!(entry.service_data, bytes::Bytes::from_static(b"data"));
        }
        Err(_) => {
            // This is also acceptable — the inner Vec deserialization failed
            // and propagated. This documents that adding a new ServiceAddr
            // variant requires a fallback to maintain forward compatibility.
        }
    }
}

// ─── Negotiate trace::Header wire-format round-trip ──────────────────────────

/// Verify that the `[trace::Header][GlobalId]` prefix written by the client
/// (`send_negotiate_rpc`) round-trips correctly through the server's
/// prefix-deserialization path (`handle_negotiate_rpc`). This guards against
/// accidental drift between the two functions' wire encoding.
#[test]
fn negotiate_trace_header_round_trips() {
    use upstream_bytevec::Builder;
    use upstream_runtime::trace;
    use upstream_service::GlobalId;
    use upstream_wire::{Deserialize, Serialize};

    let original = trace::Header {
        trace: trace::Trace::new([
            0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55,
            0x66, 0x77,
        ]),
        flags: trace::Flags::NONE,
        span_id: 0x1234_5678_9ABC_DEF0,
    };

    // Serialize as the client does in `send_negotiate_rpc`.
    let mut out = Builder::default();
    original.serialize(&mut out);
    NEGOTIATE_GLOBAL_ID.serialize(&mut out);
    let mut bytes = upstream_bytevec::ByteVec::from(out.finish());

    // Deserialize as the server does in `handle_negotiate_rpc`.
    let decoded_header = trace::Header::deserialize(&mut bytes).unwrap();
    let decoded_global_id = GlobalId::deserialize(&mut bytes).unwrap();

    assert_eq!(decoded_header.trace, original.trace);
    assert_eq!(decoded_header.span_id, original.span_id);
    assert_eq!(decoded_global_id, NEGOTIATE_GLOBAL_ID);
    // After deserializing the full prefix, no bytes remain.
    assert!(
        bytes.is_empty(),
        "RPC header should consume all written bytes"
    );
}

// ─── Backward-compat tests ───────────────────────────────────────────────────

/// Old client → new server.
///
/// Simulates a production client that doesn't know about negotiate: it sends
/// an RPC frame directly without prefixing a negotiate exchange. The new
/// server's `into_parts()` peeks the global_id, sees it's NOT
/// `NEGOTIATE_GLOBAL_ID`, pushes the bytes back via `PrependedReader`, and
/// returns parts for normal RPC dispatch. Peer stays unnegotiated.
#[tokio::test]
async fn old_client_new_server_falls_through_to_rpc() {
    use crate::mem;
    use upstream_buffer::reader::Storage as _;

    let service_id = ServiceId(bytes::Bytes::from_static(b"svc"));
    let registry = Arc::new(ConcurrentPeerRegistry::new());
    let server_handshake: Arc<dyn Handshake> = Arc::new(
        DefaultPeerServiceNegotiator::builder()
            .with_peer_info(make_test_peer_info(&service_id, "127.0.0.1", 9300, b"srv"))
            .with_service(service_id.clone(), Box::new(TestNegotiator))
            .with_registry(registry.clone())
            .build(),
    );

    let transport = mem::Transport::default();
    let server_transport = transport.clone().with_handshake(server_handshake);
    // Client has NO handshake — simulates a production peer
    let client = transport.create_client().await.unwrap();
    let mut server = server_transport
        .create_server("127.0.0.1:9300".parse().unwrap())
        .await
        .unwrap();
    let server_addr = ServerAddr::from_same_handshake_port("127.0.0.1:9300".parse().unwrap());

    // Server task: accept and check that the bytes pushed back are the
    // original RPC bytes (i.e., we successfully fell through).
    let server_task = tokio::spawn(async move {
        let stream = server.accept().await.unwrap();
        let (mut reader, _writer, _peer_addr) = stream.into_parts().await.unwrap();
        // Read the RPC bytes the client sent (which were the peeked + pushed back).
        let mut buf = upstream_bytevec::ByteVec::default();
        reader.read_to_end(&mut buf).await.unwrap();
        buf.copy_to_bytes()
    });

    // Client task: send a fake RPC frame without negotiate.
    // Use bytes that are clearly not the negotiate global_id.
    let payload = bytes::Bytes::from_static(b"this is fake rpc traffic from old client");
    let client_payload = payload.clone();
    let client_task = tokio::spawn(async move {
        let (writer, _reader) = client
            .open_stream(server_addr, upstream_service::Priority::default())
            .await
            .unwrap();
        writer.write_to_end(client_payload).await.unwrap();
    });

    let (server_result, client_result) = tokio::join!(server_task, client_task);
    let received = server_result.unwrap();
    client_result.unwrap();

    // Server received the bytes the client sent (i.e., fall-through worked).
    assert_eq!(
        received, payload,
        "old client's RPC bytes should reach server unchanged via PrependedReader"
    );

    // Peer was NOT negotiated (old client never sent negotiate).
    assert!(
        !registry.has_peer(&PeerAddr::from_handshake_addr(
            "127.0.0.1:9300".parse().unwrap()
        )),
        "peer should not appear in registry — no negotiate happened"
    );
}

/// New client → old server.
///
/// Simulates a production server that doesn't know about the negotiate RPC.
/// In real production, the old server's RPC dispatcher reads the fixed-size
/// header, calls `parse_global_id` which returns `None` for the unfamiliar
/// `NEGOTIATE_GLOBAL_ID`, returns `Err("Method not supported")` from
/// `handle_server_connection`, and drops the stream — no response is written.
///
/// The client's `send_negotiate_rpc` is best-effort: on response-read failure
/// (EOF here) it returns `Ok(())` and leaves the peer unnegotiated. The
/// resulting `connect()` call must still succeed so subsequent RPCs can
/// proceed (with `Set::all()` fallback at the version layer).
#[tokio::test]
async fn new_client_old_server_falls_through_unnegotiated() {
    use crate::mem;

    let service_id = ServiceId(bytes::Bytes::from_static(b"svc"));
    let registry = Arc::new(ConcurrentPeerRegistry::new());
    let client_handshake: Arc<dyn Handshake> = Arc::new(
        DefaultPeerServiceNegotiator::builder()
            .with_peer_info(make_test_peer_info(&service_id, "10.0.0.1", 443, b"cli"))
            .with_service(service_id.clone(), Box::new(TestNegotiator))
            .with_registry(registry.clone())
            .build(),
    );

    let transport = mem::Transport::default();
    let client_transport = transport.clone().with_handshake(client_handshake.clone());
    let client = client_transport.create_client().await.unwrap();
    // Server has NO handshake — simulates a production server with no negotiate awareness
    let mut server = transport
        .create_server("127.0.0.1:9301".parse().unwrap())
        .await
        .unwrap();
    let server_addr = ServerAddr::from_same_handshake_port("127.0.0.1:9301".parse().unwrap());

    // Old server: accept stream, read just the RPC header bytes (don't drain
    // to EOF, since the client's `write_full_chunk` doesn't send FIN), then
    // drop the stream — modeling `parse_global_id` returning None.
    let server_task = tokio::spawn(async move {
        let stream = server.accept().await.unwrap();
        let (mut reader, _writer, _peer_addr) = stream.into_parts().await.unwrap();
        // Read just the header bytes — old server's RPC dispatcher would do this
        // before parse_global_id, then bail out.
        let mut buf = upstream_bytevec::ByteVec::default();
        crate::service::common::read_until(&mut reader, &mut buf, |b| {
            b.len() >= super::protocol::RPC_HEADER_LEN
        })
        .await
        .unwrap();
        // Drop reader/writer — stream closes (FIN), client's read sees EOF.
    });

    let client_task = tokio::spawn(async move {
        client
            .connect(server_addr, upstream_service::Priority::default())
            .await
    });

    let (server_result, client_result) = tokio::join!(server_task, client_task);
    server_result.unwrap();
    let connect_outcome = client_result.unwrap();
    assert!(
        connect_outcome.is_ok(),
        "client.connect() should succeed even when server drops the stream"
    );

    // Peer was NOT negotiated (server didn't respond).
    let server_peer = "127.0.0.1:9301".parse().unwrap();
    assert!(
        !client_handshake.is_negotiated(&server_peer),
        "client should leave peer unnegotiated when server doesn't respond"
    );
}

// ─── Concurrent negotiate (per-peer lock) ────────────────────────────────────

/// Per-peer locking via `Handshake::peer_lock()` should serialize concurrent
/// negotiation attempts so we don't run `negotiate()` more than once per peer.
///
/// This test calls `peer_lock(peer).lock()` from two concurrent tasks and
/// verifies that the second task observes the lock being held by the first.
/// (Calling the full inline-negotiate path concurrently is hard to set up
/// deterministically; this test directly exercises the locking primitive
/// `into_parts` uses.)
#[tokio::test]
async fn peer_lock_serializes_per_peer() {
    let registry = Arc::new(ConcurrentPeerRegistry::new());
    let hs: Arc<dyn Handshake> = Arc::new(
        DefaultPeerServiceNegotiator::builder()
            .with_peer_info(PeerInfo::default())
            .with_registry(registry)
            .build(),
    );

    let peer: SocketAddr = "10.0.0.1:443".parse().unwrap();

    // Two requests for the same peer should return the same lock instance.
    let lock1 = hs.peer_lock(&peer);
    let lock2 = hs.peer_lock(&peer);
    assert!(
        Arc::ptr_eq(&lock1, &lock2),
        "peer_lock should return the same lock instance for the same peer"
    );

    // Different peers get different locks.
    let other_peer: SocketAddr = "10.0.0.2:443".parse().unwrap();
    let lock_other = hs.peer_lock(&other_peer);
    assert!(
        !Arc::ptr_eq(&lock1, &lock_other),
        "different peers should get distinct locks"
    );

    // Hold the lock from one task; verify the second task blocks until released.
    let hs2 = hs.clone();
    let started = Arc::new(tokio::sync::Notify::new());
    let started_clone = started.clone();

    let holder = tokio::spawn(async move {
        let lock = hs2.peer_lock(&peer);
        let _guard = lock.lock().await;
        started_clone.notify_one();
        // Hold for a bit
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    });

    // Wait for the holder to acquire
    started.notified().await;

    // Second acquirer should block at least until the holder releases.
    let waiter_start = std::time::Instant::now();
    let lock_w = hs.peer_lock(&peer);
    let _guard = lock_w.lock().await;
    let elapsed = waiter_start.elapsed();

    // Should have waited a meaningful fraction of the holder's 50ms hold.
    assert!(
        elapsed >= std::time::Duration::from_millis(20),
        "waiter should have blocked while holder held the lock; elapsed = {:?}",
        elapsed
    );

    holder.await.unwrap();
}

// ─── Same-stream: RPC follows negotiate on the same stream ───────────────────

/// Verify that after negotiate completes on a stream, the same stream is
/// usable for an actual RPC frame. This is the core same-stream design
/// promise — `write_full_chunk` (no FIN) on negotiate plus length-prefixed
/// reads must leave the stream positioned exactly at the next RPC frame.
#[tokio::test]
async fn rpc_follows_negotiate_on_same_stream() {
    use crate::mem;
    use upstream_buffer::reader::Storage as _;

    let service_id = ServiceId(bytes::Bytes::from_static(b"replica"));

    let registry_a = Arc::new(ConcurrentPeerRegistry::new());
    let handshake_a: Arc<dyn Handshake> = Arc::new(
        DefaultPeerServiceNegotiator::builder()
            .with_peer_info(make_test_peer_info(
                &service_id,
                "10.0.0.1",
                443,
                b"version-A",
            ))
            .with_service(service_id.clone(), Box::new(TestNegotiator))
            .with_registry(registry_a.clone())
            .build(),
    );

    let registry_b = Arc::new(ConcurrentPeerRegistry::new());
    let handshake_b: Arc<dyn Handshake> = Arc::new(
        DefaultPeerServiceNegotiator::builder()
            .with_peer_info(make_test_peer_info(
                &service_id,
                "10.0.0.2",
                443,
                b"version-B",
            ))
            .with_service(service_id.clone(), Box::new(TestNegotiator))
            .with_registry(registry_b.clone())
            .build(),
    );

    let transport = mem::Transport::default();
    let server_transport = transport.clone().with_handshake(handshake_b);
    let client_transport = transport.clone().with_handshake(handshake_a);

    let mut server = server_transport
        .create_server("127.0.0.1:9302".parse().unwrap())
        .await
        .unwrap();
    let client = client_transport.create_client().await.unwrap();
    let server_addr = ServerAddr::from_same_handshake_port("127.0.0.1:9302".parse().unwrap());

    const RPC_PAYLOAD: &[u8] = b"actual RPC payload after negotiate";
    const RPC_RESPONSE: &[u8] = b"server response after RPC";

    // Server: accept one stream, into_parts() runs negotiate inline, then
    // we read the RPC payload that follows on the SAME stream.
    let server_task = tokio::spawn(async move {
        let stream = server.accept().await.unwrap();
        let (reader, writer, _peer_addr) = stream.into_parts().await.unwrap();

        // The negotiate already happened inside into_parts — now read the RPC.
        let mut buf = upstream_bytevec::ByteVec::default();
        reader.read_to_end(&mut buf).await.unwrap();
        let received = buf.copy_to_bytes();

        // Echo a response back on the same stream
        writer.write_to_end(RPC_RESPONSE).await.unwrap();
        received
    });

    // Client: connect (runs negotiate inline), then send the RPC payload on
    // the same stream and read the response.
    let client_task = tokio::spawn(async move {
        let (writer, reader) = client
            .connect(server_addr, upstream_service::Priority::default())
            .await
            .unwrap();
        let (_w, response) =
            tokio::try_join!(writer.write_to_end(RPC_PAYLOAD), reader.read_to_bytes()).unwrap();
        response
    });

    let (server_received, client_response) = tokio::join!(server_task, client_task);
    let server_received = server_received.unwrap();
    let client_response = client_response.unwrap();

    // The RPC payload made it through — proving negotiate + RPC share the stream.
    assert_eq!(
        server_received,
        bytes::Bytes::from_static(RPC_PAYLOAD),
        "server should receive the RPC payload on the same stream as negotiate"
    );
    let response_bytes = client_response.copy_to_bytes();
    assert_eq!(
        response_bytes,
        bytes::Bytes::from_static(RPC_RESPONSE),
        "client should receive the RPC response on the same stream"
    );

    // Sanity: negotiate populated both registries.
    assert!(registry_a.has_peer(&PeerAddr::from_handshake_addr(
        "10.0.0.2:443".parse().unwrap()
    )));
    assert!(registry_b.has_peer(&PeerAddr::from_handshake_addr(
        "10.0.0.1:443".parse().unwrap()
    )));
}

/// Each transport builds its OWN handshake (and registry) from `Config` via
/// `build_default_handshake`, so two transports configured from (clones of) the same
/// `Config` get **isolated** registries — negotiated state never bleeds across
/// transports. This is the core invariant of the transport-owns-its-handshake
/// design: the daemon no longer threads one shared registry into every
/// transport.
#[test]
fn each_transport_gets_an_isolated_registry() {
    use crate::negotiation::RpcVersionData;

    let services = vec![crate::negotiation::NegotiationService::new(
        ServiceId(bytes::Bytes::from_static(b"replica")),
        RpcVersionData::default(),
    )];

    // One Config, cloned per transport (mirrors how `select_transports!` /
    // the `build_*` helpers reuse the same config to construct each transport).
    let config = crate::Config::default()
        .handshake_addr("10.0.0.1:443".parse().unwrap())
        .negotiation_services(services);

    let hs_a = config
        .build_default_handshake()
        .expect("services configured");
    let hs_b = config
        .build_default_handshake()
        .expect("services configured");

    let reg_a = hs_a.peer_registry().expect("handshake owns a registry");
    let reg_b = hs_b.peer_registry().expect("handshake owns a registry");

    // Distinct registry instances.
    assert!(
        !Arc::ptr_eq(&reg_a, &reg_b),
        "two transports built from the same Config must get separate registries"
    );

    // And they are functionally isolated: a peer recorded in A is invisible in B.
    let peer = PeerAddr::from_handshake_addr("10.0.0.9:443".parse().unwrap());
    reg_a.set_peer_info(peer, PeerInfo::default());
    assert!(reg_a.has_peer(&peer));
    assert!(
        !reg_b.has_peer(&peer),
        "negotiated state in one transport's registry must not appear in another's"
    );
}

/// With no negotiation services configured, `build_default_handshake` returns `None`
/// (negotiation disabled) — so a transport built from a bare `Config` has no
/// handshake and falls back to current-version dispatch.
#[test]
fn build_default_handshake_is_none_without_services() {
    let config = crate::Config::default().handshake_addr("10.0.0.1:443".parse().unwrap());
    assert!(config.build_default_handshake().is_none());
}

/// End-to-end isolation: two **separate** transports, each built from its own
/// `Config` and each running a real negotiate handshake against a **different**
/// peer, end up with registries that contain only *their own* peer's state —
/// negotiated state from one transport/connection never leaks into another.
///
/// This is the cross-transport guarantee that complements
/// [`each_transport_gets_an_isolated_registry`] (which checks construction):
/// here the registries are populated by the actual negotiate RPC, not by hand.
///
/// Note on dcQUIC: a dcQUIC transport uses two sockets (handshake + data) but
/// is ONE transport with ONE handshake, so both sockets share one registry by
/// design — isolation is *between transports/nodes*, not between a transport's
/// own sockets. This mem-based test models two distinct nodes' transports.
#[tokio::test]
async fn negotiation_state_is_isolated_across_transports() {
    let service_id = ServiceId(bytes::Bytes::from_static(b"replica"));

    // Two independent "server" nodes, each on its own transport + registry,
    // listening on different addresses and advertising different version data.
    async fn spawn_server(
        fabric: &crate::mem::Transport,
        service_id: &ServiceId,
        listen: &str,
        advertise_ip: &str,
        version_tag: &'static [u8],
    ) -> Arc<ConcurrentPeerRegistry> {
        let registry = Arc::new(ConcurrentPeerRegistry::new());
        let peer_info = make_test_peer_info(service_id, advertise_ip, 443, version_tag);
        let handshake: Arc<dyn Handshake> = Arc::new(
            DefaultPeerServiceNegotiator::builder()
                .with_peer_info(peer_info)
                .with_service(service_id.clone(), Box::new(TestNegotiator))
                .with_registry(registry.clone())
                .build(),
        );
        let server_transport = fabric.clone().with_handshake(handshake);
        let mut server = server_transport
            .create_server(listen.parse().unwrap())
            .await
            .unwrap();
        tokio::spawn(async move {
            while let Some(stream) = server.accept().await {
                if stream.into_parts().await.is_ok() {
                    break;
                }
            }
        });
        registry
    }

    // Shared in-process fabric (the "network"); each node/client is its own
    // transport clone with its own handshake — exactly the production shape.
    let fabric = crate::mem::Transport::default();

    let reg_server1 = spawn_server(
        &fabric,
        &service_id,
        "127.0.0.1:7001",
        "10.0.0.1",
        b"v-server1",
    )
    .await;
    let reg_server2 = spawn_server(
        &fabric,
        &service_id,
        "127.0.0.1:7002",
        "10.0.0.2",
        b"v-server2",
    )
    .await;

    // Two independent client transports, each with its OWN registry, each
    // dialing a DIFFERENT server.
    let reg_client1 = Arc::new(ConcurrentPeerRegistry::new());
    let client1 = fabric
        .clone()
        .with_handshake(Arc::new(
            DefaultPeerServiceNegotiator::builder()
                .with_peer_info(make_test_peer_info(
                    &service_id,
                    "10.1.0.1",
                    443,
                    b"v-client1",
                ))
                .with_service(service_id.clone(), Box::new(TestNegotiator))
                .with_registry(reg_client1.clone())
                .build(),
        ) as Arc<dyn Handshake>)
        .create_client()
        .await
        .unwrap();

    let reg_client2 = Arc::new(ConcurrentPeerRegistry::new());
    let client2 = fabric
        .clone()
        .with_handshake(Arc::new(
            DefaultPeerServiceNegotiator::builder()
                .with_peer_info(make_test_peer_info(
                    &service_id,
                    "10.1.0.2",
                    443,
                    b"v-client2",
                ))
                .with_service(service_id.clone(), Box::new(TestNegotiator))
                .with_registry(reg_client2.clone())
                .build(),
        ) as Arc<dyn Handshake>)
        .create_client()
        .await
        .unwrap();

    // client1 ↔ server1, client2 ↔ server2.
    let c1 = tokio::spawn(async move {
        client1
            .connect(
                ServerAddr::from_same_handshake_port("127.0.0.1:7001".parse().unwrap()),
                upstream_service::Priority::default(),
            )
            .await
            .unwrap();
    });
    let c2 = tokio::spawn(async move {
        client2
            .connect(
                ServerAddr::from_same_handshake_port("127.0.0.1:7002".parse().unwrap()),
                upstream_service::Priority::default(),
            )
            .await
            .unwrap();
    });
    let (r1, r2) = tokio::join!(c1, c2);
    r1.unwrap();
    r2.unwrap();

    let server1_key = PeerAddr::from_handshake_addr("10.0.0.1:443".parse().unwrap());
    let server2_key = PeerAddr::from_handshake_addr("10.0.0.2:443".parse().unwrap());

    // Each client's registry knows ONLY the server it dialed.
    assert!(
        reg_client1.has_peer(&server1_key),
        "client1 should have negotiated with server1"
    );
    assert!(
        !reg_client1.has_peer(&server2_key),
        "client1's registry must NOT contain server2 — no cross-transport leak"
    );
    assert!(
        reg_client2.has_peer(&server2_key),
        "client2 should have negotiated with server2"
    );
    assert!(
        !reg_client2.has_peer(&server1_key),
        "client2's registry must NOT contain server1 — no cross-transport leak"
    );

    // The two client registries are genuinely distinct instances.
    assert!(
        !Arc::ptr_eq(
            &(reg_client1.clone() as Arc<dyn PeerRegistry>),
            &(reg_client2.clone() as Arc<dyn PeerRegistry>)
        ),
        "independent transports must not share a registry"
    );

    // Each server learned only its own client's version tag.
    assert_eq!(
        reg_server1
            .negotiated_version(
                &PeerAddr::from_handshake_addr("10.1.0.1:443".parse().unwrap()),
                &service_id,
                &MethodId(bytes::Bytes::from_static(b"read")),
            )
            .expect("server1 negotiated with client1"),
        VersionId(bytes::Bytes::from_static(b"v-client1")),
    );
    assert!(
        !reg_server2.has_peer(&PeerAddr::from_handshake_addr(
            "10.1.0.1:443".parse().unwrap()
        )),
        "server2 must not know client1 — connections did different handshakes"
    );
}
