// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Peer negotiation and service discovery.
//!
//! This module implements the application-level handshake protocol described in
//! `docs/designs/peer-negotiation-and-service-discovery.md`. It provides:
//!
//! - [`Handshake`] — the application-level negotiation trait
//! - [`ServiceNegotiator`] — per-service negotiation algorithm
//! - [`PeerRegistry`] — stores per-peer negotiation results
//! - [`DefaultPeerServiceNegotiator`] — builder-based `Handshake` implementation
//! - [`send_negotiate_rpc`] / [`handle_negotiate_rpc`] — RPC-based negotiate protocol

mod negotiator;
mod peer_info;
mod protocol;
mod registry;
pub mod rpc_version_negotiator;
mod traits;
mod types;

#[cfg(test)]
mod tests;

// ─── Re-exports ──────────────────────────────────────────────────────────────

/// Reserved `GlobalId` that identifies the negotiate RPC.
///
/// The dispatcher in `GenericStream::into_parts()` intercepts requests with
/// this method id and runs the negotiate protocol. Servers that don't know
/// about it return "Method not supported" — clients gracefully fall back.
///
/// Computed at compile time from a fixed string so it's stable across builds
/// and won't collide with any real method (real methods include service +
/// method names in their hash).
pub const NEGOTIATE_GLOBAL_ID: upstream_service::GlobalId = upstream_service::Hasher::new()
    .update(b"__upstream_negotiate__\0")
    .finalize_u64();

pub use negotiator::{DefaultPeerServiceNegotiator, DefaultPeerServiceNegotiatorBuilder};
pub(crate) use protocol::RPC_HEADER_LEN;
pub use protocol::{handle_negotiate_rpc, send_negotiate_rpc};
pub use registry::ConcurrentPeerRegistry;
pub use rpc_version_negotiator::{
    MethodVersionData, RpcVersionData, RpcVersionNegotiator, VersionEntry,
};
pub use traits::{Handshake, PeerRegistry, ServiceNegotiator};
pub use types::{
    MethodId, NegotiatedMethods, NegotiationService, PeerAddr, PeerInfo, ServiceAddr, ServiceEntry,
    ServiceId, VersionId,
};
