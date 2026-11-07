// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use super::{
    traits::PeerRegistry,
    types::{MethodId, NegotiatedMethods, PeerAddr, PeerInfo, ServiceAddr, ServiceId, VersionId},
};
use dashmap::DashMap;
use std::{collections::BTreeMap, sync::Arc};

/// Per-peer state stored in the registry.
#[derive(Clone, Debug, Default)]
struct PeerState {
    info: Option<PeerInfo>,
    negotiated: BTreeMap<ServiceId, NegotiatedMethods>,
}

/// A concurrent hash map backed `PeerRegistry` implementation.
#[derive(Clone, Debug, Default)]
pub struct ConcurrentPeerRegistry {
    peers: Arc<DashMap<PeerAddr, PeerState>>,
}

impl ConcurrentPeerRegistry {
    pub fn new() -> Self {
        Self {
            peers: Arc::new(DashMap::new()),
        }
    }
}

impl PeerRegistry for ConcurrentPeerRegistry {
    fn has_peer(&self, peer: &PeerAddr) -> bool {
        self.peers.contains_key(peer)
    }

    fn set_peer_info(&self, peer: PeerAddr, info: PeerInfo) {
        self.peers.entry(peer).or_default().info = Some(info);
    }

    fn set_negotiated(&self, peer: PeerAddr, service: &ServiceId, methods: NegotiatedMethods) {
        self.peers
            .entry(peer)
            .or_default()
            .negotiated
            .insert(service.clone(), methods);
    }

    fn negotiated_version(
        &self,
        peer: &PeerAddr,
        service: &ServiceId,
        method: &MethodId,
    ) -> Option<VersionId> {
        let entry = self.peers.get(peer)?;
        let negotiated = entry.negotiated.get(service)?;
        negotiated.methods.get(method).cloned()
    }

    fn resolve_service(&self, peer: &PeerAddr, service: &ServiceId) -> Option<Vec<ServiceAddr>> {
        let entry = self.peers.get(peer)?;
        let info = entry.info.as_ref()?;
        let service_entry = info.services.get(service)?;
        Some(service_entry.addrs.clone())
    }

    fn peer_info(&self, peer: &PeerAddr) -> Option<PeerInfo> {
        self.peers.get(peer)?.info.clone()
    }

    fn len(&self) -> usize {
        self.peers.len()
    }
}
