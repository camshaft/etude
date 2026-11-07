// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use super::{
    traits::{Handshake, PeerRegistry, ServiceNegotiator},
    types::{PeerAddr, PeerInfo, ServiceId},
};
use bytes::Bytes;
use dashmap::DashMap;
use upstream_bytevec::ByteVec;
use std::{io, net::SocketAddr, sync::Arc};
use tokio::sync::Mutex;

/// Builder for [`DefaultPeerServiceNegotiator`].
pub struct DefaultPeerServiceNegotiatorBuilder {
    local_info: Option<PeerInfo>,
    negotiators: Vec<(ServiceId, Box<dyn ServiceNegotiator>)>,
    registry: Option<Arc<dyn PeerRegistry>>,
}

impl DefaultPeerServiceNegotiatorBuilder {
    pub fn new() -> Self {
        Self {
            local_info: None,
            negotiators: Vec::new(),
            registry: None,
        }
    }

    /// Set the local PeerInfo that will be advertised to remote peers.
    pub fn with_peer_info(mut self, info: PeerInfo) -> Self {
        self.local_info = Some(info);
        self
    }

    /// Register a per-service negotiator.
    pub fn with_service(
        mut self,
        service_id: ServiceId,
        negotiator: Box<dyn ServiceNegotiator>,
    ) -> Self {
        self.negotiators.push((service_id, negotiator));
        self
    }

    /// Set the peer registry where negotiation results are stored.
    pub fn with_registry(mut self, registry: Arc<dyn PeerRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Build the negotiator.
    ///
    /// # Panics
    /// Panics if `local_info` or `registry` is not set.
    pub fn build(self) -> DefaultPeerServiceNegotiator {
        DefaultPeerServiceNegotiator {
            local_info: self.local_info.expect("local_info is required"),
            negotiators: self.negotiators,
            registry: self.registry.expect("registry is required"),
            peer_locks: DashMap::new(),
        }
    }
}

/// Upstream's `Handshake` implementation.
///
/// Built via a builder pattern that registers per-service `ServiceNegotiator`s.
/// During negotiation, iterates over registered services, finds matching entries
/// in the remote PeerInfo, and invokes the per-service negotiator.
pub struct DefaultPeerServiceNegotiator {
    local_info: PeerInfo,
    negotiators: Vec<(ServiceId, Box<dyn ServiceNegotiator>)>,
    registry: Arc<dyn PeerRegistry>,
    peer_locks: DashMap<SocketAddr, Arc<Mutex<()>>>,
}

impl DefaultPeerServiceNegotiator {
    pub fn builder() -> DefaultPeerServiceNegotiatorBuilder {
        DefaultPeerServiceNegotiatorBuilder::new()
    }

    /// Run per-service negotiation between the local services and a remote
    /// peer's advertised services. Pure compute — does not touch the registry.
    ///
    /// `peer` is only forwarded to each `ServiceNegotiator` (some use it for
    /// logging); the result is the negotiated method versions per service.
    /// Shared by [`Handshake::negotiate`] (which stores into the registry) and
    /// the dcQUIC Entry-stored path (which stores the result on the connection).
    pub fn compute_negotiated(
        &self,
        peer: SocketAddr,
        remote_info: &PeerInfo,
    ) -> std::collections::BTreeMap<ServiceId, super::types::NegotiatedMethods> {
        // Prefer the caller-supplied peer address, but fall back to the peer's
        // own advertised handshake identity when it is unspecified. The dcQUIC
        // path computes negotiation off the connection `Entry` and has no
        // `SocketAddr` to hand us, so it passes `0.0.0.0:0`; without this
        // fallback every dcQUIC `NEGOTIATED` line would read `peer=0.0.0.0:0`
        // and links couldn't be told apart.
        let log_peer = if peer.ip().is_unspecified() && peer.port() == 0 {
            match remote_info.peer_ip {
                Some(ip) => SocketAddr::new(ip, remote_info.peer_handshake_port),
                None => peer,
            }
        } else {
            peer
        };

        let mut out = std::collections::BTreeMap::new();
        for (service_id, negotiator) in &self.negotiators {
            if let (Some(local), Some(remote)) = (
                self.local_info.services.get(service_id),
                remote_info.services.get(service_id),
            ) {
                if let Some(methods) = negotiator.negotiate(peer, local, remote) {
                    // Emit one negotiation-outcome line per method so a live
                    // cluster's logs show which wire version each link settled
                    // on. The full marker lives in the message string (not in
                    // structured fields) so downstream scrapers can match the
                    // contiguous `NEGOTIATED ...` text regardless of formatter.
                    let service = String::from_utf8_lossy(&service_id.0);
                    for (method_id, version_id) in &methods.methods {
                        let method = String::from_utf8_lossy(&method_id.0);
                        let global_id = version_id
                            .0
                            .as_ref()
                            .try_into()
                            .map(u64::from_be_bytes)
                            .unwrap_or_default();
                        tracing::info!(
                            target: "dc_negotiation",
                            "NEGOTIATED peer={log_peer} service={service} method={method} global_id={global_id:#018x}"
                        );
                    }
                    out.insert(service_id.clone(), methods);
                }
            }
        }
        out
    }

    /// Parse a remote peer's serialized payload and negotiate against it,
    /// returning the parsed `PeerInfo` plus the per-service negotiated methods.
    /// Used by the dcQUIC path to compute the result once at handshake time and
    /// store it on the connection's `Entry`.
    pub fn negotiate_payload(
        &self,
        peer: SocketAddr,
        remote_payload: &Bytes,
    ) -> io::Result<(
        PeerInfo,
        std::collections::BTreeMap<ServiceId, super::types::NegotiatedMethods>,
    )> {
        let remote_info = PeerInfo::from_bytes(ByteVec::from(remote_payload.clone()))?;
        let negotiated = self.compute_negotiated(peer, &remote_info);
        Ok((remote_info, negotiated))
    }
}

impl Handshake for DefaultPeerServiceNegotiator {
    fn local_payload(&self) -> Bytes {
        self.local_info.to_bytes()
    }

    fn local_handshake_port(&self) -> u16 {
        self.local_info.peer_handshake_port
    }

    fn negotiate(&self, _peer: SocketAddr, remote_payload: Bytes) -> io::Result<()> {
        let remote_info = PeerInfo::from_bytes(ByteVec::from(remote_payload))?;

        // Derive the key this peer's negotiated state is stored under.
        //
        // A peer that *accepts* connections (a server/node) advertises its
        // stable handshake identity in `peer_ip`/`peer_handshake_port`. We MUST
        // key by that advertised address — not the raw connection source —
        // because the source port is ephemeral and may sit on a different IP
        // (NAT, multi-ENI). When this node later *dials that peer back*, the
        // consumer looks the entry up by the dialed handshake `ServerAddr`
        // (`is_negotiated`, `resolve_supported_set`), so the stored key must
        // equal the dialed address.
        //
        // A peer that only *dials* and is never dialed back (a pure client) has
        // no handshake identity to advertise, so it sends `peer_handshake_port
        // = 0`. There is no stable address to key by — and none is needed,
        // since nobody dials it back — so we key by the connection address.
        // This entry is effectively write-only on the acceptor side: it records
        // "this client negotiated X" for diagnostics, never read by a dialed
        // lookup. This is an expected, first-class case, not a misconfiguration.
        let peer_addr = match (remote_info.peer_ip, remote_info.peer_handshake_port) {
            (Some(ip), port) if port != 0 => PeerAddr::new(ip, port),
            (advertised_ip, advertised_port) => {
                let conn_key = PeerAddr::from_handshake_addr(_peer);
                tracing::debug!(
                    target: "dc_negotiation",
                    connection_peer = %_peer,
                    ?advertised_ip,
                    advertised_port,
                    conn_key = ?conn_key,
                    "negotiate: peer advertised no handshake port (pure client); \
                     keying its negotiated state by the connection address."
                );
                conn_key
            }
        };

        // Always store PeerInfo, even if no common services.
        self.registry.set_peer_info(peer_addr, remote_info.clone());

        let socket = SocketAddr::new(peer_addr.ip, peer_addr.handshake_port);
        for (service_id, methods) in self.compute_negotiated(socket, &remote_info) {
            self.registry
                .set_negotiated(peer_addr, &service_id, methods);
        }

        Ok(())
    }

    fn is_negotiated(&self, peer: &SocketAddr) -> bool {
        let peer_addr = PeerAddr::from_handshake_addr(*peer);
        self.registry.has_peer(&peer_addr)
    }

    fn peer_lock(&self, peer: &SocketAddr) -> Arc<Mutex<()>> {
        self.peer_locks.entry(*peer).or_default().clone()
    }

    fn peer_registry(&self) -> Option<Arc<dyn PeerRegistry>> {
        Some(self.registry.clone())
    }
}
