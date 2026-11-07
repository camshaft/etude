// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use bytes::Bytes;
use upstream_wire::{Deserialize, TaggedWire, Wire};
use std::{collections::BTreeMap, net::IpAddr};

/// Identifies a peer node by its IP and handshake port.
///
/// This is the canonical key for the `PeerRegistry`. Each node has one
/// handshake port where it accepts PSK handshakes. Services on the node
/// may listen on different acceptor ports, but the handshake port is
/// the node's stable identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeerAddr {
    pub ip: IpAddr,
    pub handshake_port: u16,
}

impl PeerAddr {
    pub const fn new(ip: IpAddr, handshake_port: u16) -> Self {
        Self { ip, handshake_port }
    }

    /// Construct from a `SocketAddr` (assumes it's the handshake address).
    pub const fn from_handshake_addr(addr: std::net::SocketAddr) -> Self {
        Self {
            ip: addr.ip(),
            handshake_port: addr.port(),
        }
    }
}

/// Identifies a service within a peer's advertised service set.
///
/// The canonical convention is the service's namespace name (e.g. `b"paxos"`,
/// `b"gossip"`). Use [`ServiceId::for_service`] to derive one from a
/// `Service` type at compile time so server and client sides are guaranteed
/// to agree without hand-maintaining string constants.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServiceId(pub Bytes);

impl ServiceId {
    /// Derive the canonical `ServiceId` from a `Service` type, using the
    /// first namespace declared by the service. The resulting `Bytes` is
    /// backed by a static slice — no allocation.
    ///
    /// # Panics
    /// If the service declares no namespaces. Codegen always declares one,
    /// so this only fires for hand-rolled `Service` impls that misbehave.
    pub fn for_service<S: upstream_service::Service>() -> Self {
        let name = S::namespaces()
            .next()
            .expect("Service has no namespaces — codegen invariant violated");
        // The Str dereferences without the trailing NUL, so `as_bytes`
        // here gives us a clean service name.
        Self(Bytes::from_static(name.as_bytes()))
    }
}

/// Identifies a method within a service.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MethodId(pub Bytes);

/// Identifies a negotiated wire layout version for a method.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VersionId(pub Bytes);

/// Address where a service can be reached.
#[derive(TaggedWire, Clone, Debug, PartialEq, Eq)]
#[wire(crate = upstream_wire)]
pub enum ServiceAddr {
    #[wire(tag = 1.1)]
    Socket { ip: std::net::IpAddr, port: u16 },
    #[wire(tag = 1.2)]
    Fabric { addr: Bytes },
}

/// Entry for a single service in the PeerInfo structure.
#[derive(TaggedWire, Clone, Debug, PartialEq, Eq, Default)]
#[wire(crate = upstream_wire)]
pub struct ServiceEntry {
    /// Addresses where this service accepts connections.
    #[wire(tag = 1.1)]
    pub addrs: Vec<ServiceAddr>,
    /// Opaque per-service negotiation data (e.g., RpcVersionInfo).
    #[wire(tag = 1.2)]
    pub service_data: Bytes,
}

/// A service this node advertises during negotiation — used to build the
/// outgoing [`PeerInfo`]. Not a wire type; it's the construction-time input to
/// [`crate::Config::build_default_handshake`].
///
/// `addrs` are the addresses at which this service accepts connections; they
/// populate the advertised [`ServiceEntry::addrs`] so peers can discover where
/// to reach the service. Leave `addrs` empty to advertise version data only
/// (the peer then falls back to the address it already dialed).
#[derive(Clone, Debug)]
pub struct NegotiationService {
    pub service_id: ServiceId,
    pub version_data: super::RpcVersionData,
    pub addrs: Vec<ServiceAddr>,
}

impl NegotiationService {
    /// A service advertised with version data but no addresses (peers fall
    /// back to the dialed address).
    pub fn new(service_id: ServiceId, version_data: super::RpcVersionData) -> Self {
        Self {
            service_id,
            version_data,
            addrs: Vec::new(),
        }
    }

    /// Advertise the address(es) at which this service accepts connections.
    pub fn with_addrs(mut self, addrs: Vec<ServiceAddr>) -> Self {
        self.addrs = addrs;
        self
    }
}

/// The top-level structure exchanged between peers during negotiation.
/// Each peer builds this to advertise its services.
#[derive(TaggedWire, Clone, Debug, Default, PartialEq, Eq)]
#[wire(crate = upstream_wire)]
pub struct PeerInfo {
    #[wire(tag = 1.1)]
    pub services: BTreeMap<ServiceId, ServiceEntry>,
    /// The peer's IP address. `None` if not advertised.
    #[wire(tag = 1.2)]
    pub peer_ip: Option<std::net::IpAddr>,
    /// The peer's handshake port (where PSK handshakes are accepted).
    #[wire(tag = 1.3)]
    pub peer_handshake_port: u16,
}

/// The result of per-service negotiation: a map from method to negotiated version.
#[derive(Clone, Debug, Default)]
pub struct NegotiatedMethods {
    pub methods: BTreeMap<MethodId, VersionId>,
}

impl Default for ServiceAddr {
    fn default() -> Self {
        ServiceAddr::Socket {
            ip: std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            port: 0,
        }
    }
}

// ServiceId needs Serialize/Deserialize for BTreeMap<ServiceId, ServiceEntry>
impl upstream_wire::Wire for ServiceId {
    const TYPE: upstream_wire::Type = <Bytes as upstream_wire::Wire>::TYPE;
}

impl upstream_wire::Serialize for ServiceId {
    fn serialize<B: upstream_bytevec::Behavior>(self, out: &mut upstream_bytevec::Builder<B>) {
        upstream_wire::Serialize::serialize(self.0, out);
    }
}

impl upstream_wire::Deserialize for ServiceId {
    fn deserialize(
        bytes: &mut upstream_bytevec::ByteVec,
    ) -> Result<Self, upstream_wire::de::Error> {
        Ok(ServiceId(Bytes::deserialize(bytes)?))
    }
}
