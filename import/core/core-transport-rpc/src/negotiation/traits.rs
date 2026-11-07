// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use super::types::{
    MethodId, NegotiatedMethods, PeerAddr, PeerInfo, ServiceAddr, ServiceEntry, ServiceId,
    VersionId,
};
use bytes::Bytes;
use std::{io, net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;

/// Application-level negotiation trait. Transport-agnostic and direction-agnostic.
///
/// The transport calls these methods during connection setup:
/// - `local_payload()` to get the bytes to send
/// - `negotiate()` when remote bytes arrive
/// - `is_negotiated()` to check if a peer has already been negotiated
pub trait Handshake: Send + Sync {
    /// The local peer's serialized negotiation payload.
    fn local_payload(&self) -> Bytes;

    /// The handshake port advertised in the local peer's `PeerInfo`. The
    /// transport's `Server::local_addr()` should report this as
    /// `handshake_port` so leaders that key peer-registry lookups by
    /// `addr.handshake_port` find the entry the negotiator will store
    /// under the same port. dcQUIC binds its handshake socket on this
    /// port directly; in-process / TCP transports that don't have a
    /// separate handshake socket should still report this value so the
    /// canonical peer identity is consistent across transports.
    fn local_handshake_port(&self) -> u16;

    /// Deserialize the remote peer's payload, run per-service negotiation,
    /// and cache the results in the PeerRegistry.
    ///
    /// The `peer` is the transport-layer source address (may be ephemeral).
    /// Implementations SHOULD derive the stable `ServerAddr` from the remote's
    /// `PeerInfo` and use that as the registry key.
    fn negotiate(&self, peer: SocketAddr, remote_payload: Bytes) -> io::Result<()>;

    /// Whether negotiation has already completed for this peer.
    fn is_negotiated(&self, peer: &SocketAddr) -> bool;

    /// Get or create the per-peer negotiation lock.
    ///
    /// Used by callers to serialize concurrent negotiation attempts to the same
    /// peer. The default returns a fresh uncontested mutex (no cross-stream
    /// coordination). Implementations with shared state should return from a
    /// persistent map so all streams targeting the same peer share one lock.
    fn peer_lock(&self, _peer: &SocketAddr) -> Arc<Mutex<()>> {
        Arc::new(Mutex::new(()))
    }

    /// The peer registry this handshake stores negotiation results in, if any.
    ///
    /// This is the single source of negotiated state for the transport that
    /// owns this handshake: consumers derive their `SupportedSet` resolution
    /// from `client.handshake().peer_registry()` rather than receiving a
    /// registry threaded down from the daemon. The default returns `None`
    /// (no negotiation configured).
    fn peer_registry(&self) -> Option<Arc<dyn PeerRegistry>> {
        None
    }
}

impl Handshake for Arc<dyn Handshake> {
    fn local_payload(&self) -> Bytes {
        (**self).local_payload()
    }

    fn local_handshake_port(&self) -> u16 {
        (**self).local_handshake_port()
    }

    fn negotiate(&self, peer: SocketAddr, remote_payload: Bytes) -> io::Result<()> {
        (**self).negotiate(peer, remote_payload)
    }

    fn is_negotiated(&self, peer: &SocketAddr) -> bool {
        (**self).is_negotiated(peer)
    }

    fn peer_lock(&self, peer: &SocketAddr) -> Arc<Mutex<()>> {
        (**self).peer_lock(peer)
    }

    fn peer_registry(&self) -> Option<Arc<dyn PeerRegistry>> {
        (**self).peer_registry()
    }
}

/// Pluggable per-service negotiation algorithm.
///
/// The `DefaultPeerServiceNegotiator` invokes one `ServiceNegotiator` per service.
/// Each implementation receives the local and remote `ServiceEntry` and returns
/// `NegotiatedMethods` if negotiation succeeds.
pub trait ServiceNegotiator: Send + Sync {
    fn negotiate(
        &self,
        peer: SocketAddr,
        local: &ServiceEntry,
        remote: &ServiceEntry,
    ) -> Option<NegotiatedMethods>;
}

/// Stores per-peer negotiation results.
///
/// Keyed by `PeerAddr` — the remote peer's stable identity (IP + handshake_port).
/// Populated once per peer during negotiation, queried on every outbound RPC call.
pub trait PeerRegistry: Send + Sync {
    fn has_peer(&self, peer: &PeerAddr) -> bool;
    fn set_peer_info(&self, peer: PeerAddr, info: PeerInfo);
    fn set_negotiated(&self, peer: PeerAddr, service: &ServiceId, methods: NegotiatedMethods);
    fn negotiated_version(
        &self,
        peer: &PeerAddr,
        service: &ServiceId,
        method: &MethodId,
    ) -> Option<VersionId>;
    fn resolve_service(&self, peer: &PeerAddr, service: &ServiceId) -> Option<Vec<ServiceAddr>>;
    fn peer_info(&self, peer: &PeerAddr) -> Option<PeerInfo>;

    /// Number of peers currently recorded. Implementations should make this
    /// O(1) where possible; primarily used by tests to assert a negotiate
    /// path actually fired without enumerating addresses.
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
