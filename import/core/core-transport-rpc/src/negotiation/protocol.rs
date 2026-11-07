// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Inline RPC-based negotiate protocol.
//!
//! Negotiation is exposed as a regular RPC with a reserved `GlobalId`
//! ([`super::NEGOTIATE_GLOBAL_ID`]) and runs inline on the same stream that
//! will carry the actual RPC traffic. The wire format reuses the standard
//! RPC framing but with length-prefixed payloads (instead of read-to-EOF) so
//! the stream stays open after the negotiate exchange completes:
//!
//! ```text
//! Client → Server: [trace::Header][NEGOTIATE_GLOBAL_ID][len:u64][PeerInfo bytes]
//! Server → Client: [len:u64][Result<Bytes>::serialize bytes]
//! ... same stream continues with the next RPC frame ...
//! ```
//!
//! [`crate::GenericStream::into_parts`] peeks the global_id; if it matches
//! `NEGOTIATE_GLOBAL_ID`, it runs [`handle_negotiate_rpc`] and then continues
//! reading from the same stream for normal RPC dispatch.
//!
//! Old peers that don't recognize `NEGOTIATE_GLOBAL_ID` send RPC bytes whose
//! header global_id won't match — the dispatcher falls through to normal RPC
//! dispatch, leaving the peer unnegotiated (RPC falls back to `Set::all()`).

use super::Handshake;
use bytes::Bytes;
use upstream_bytevec::{Builder, ByteVec};
use upstream_runtime::{Error, trace};
use upstream_service::GlobalId;
use upstream_wire::{Deserialize, Serialize};
use std::{io, mem::size_of, net::SocketAddr};

/// Mint a fresh randomized trace header for the negotiate RPC.
///
/// Negotiate is a connection-setup operation logically independent of any
/// specific user RPC, so we treat it as its own trace context (matches how
/// TLS/QUIC handshakes are typically modeled in distributed tracing). The
/// random Trace ID lets server-side handler logs be correlated end-to-end
/// with the originating client.
fn fresh_negotiate_header() -> trace::Header {
    trace::Header {
        trace: trace::Trace::new(rand::random()),
        flags: trace::Flags::NONE,
        span_id: rand::random(),
    }
}

/// Send the negotiate RPC inline on the given stream and process the response.
///
/// Writes `[trace::Header][NEGOTIATE_GLOBAL_ID][len-prefixed PeerInfo]` using
/// `write_full_chunk` (no FIN) so the stream stays open. Reads the response
/// (also length-prefixed) so we can stop precisely at the response boundary
/// without consuming any bytes from the next RPC frame.
///
/// Best-effort: a malformed response or a response indicating the server
/// rejected the call leaves the peer unnegotiated and returns `Ok(())`.
pub async fn send_negotiate_rpc<W, R>(
    writer: &mut W,
    reader: &mut R,
    handshake: &dyn Handshake,
    peer: SocketAddr,
) -> io::Result<()>
where
    W: crate::Writer,
    R: crate::Reader,
{
    let local = handshake.local_payload();

    // Build the request frame with a fresh trace header so server-side
    // handler logs can be correlated to this client invocation.
    let mut out = Builder::default();
    fresh_negotiate_header().serialize(&mut out);
    super::NEGOTIATE_GLOBAL_ID.serialize(&mut out);
    Serialize::serialize_with_len_prefix(local, &mut out);
    let request = out.finish();

    // Write without closing the stream
    writer.write_full_chunk(request).await?;

    // Read the length-prefixed response without consuming past it
    let mut buf = ByteVec::default();
    let response_msg: core::result::Result<Bytes, Error> =
        match crate::service::common::read_msg_with_len_prefix::<
            core::result::Result<Bytes, Error>,
        >(reader, &mut buf)
        .await
        {
            Ok(v) => v,
            Err(error) => {
                tracing::debug!(%error, %peer, "negotiate response read failed; peer stays unnegotiated");
                return Ok(());
            }
        };
    let response = response_msg;

    match response {
        Ok(remote_payload) => handshake.negotiate(peer, remote_payload),
        Err(error) => {
            tracing::debug!(%error, %peer, "negotiate RPC rejected; peer stays unnegotiated");
            Ok(())
        }
    }
}

/// Server-side handler for the negotiate RPC, dispatched by
/// [`crate::GenericStream::into_parts`] when it sees `NEGOTIATE_GLOBAL_ID` on
/// the wire. The caller passes the already-peeked
/// `[trace::Header][NEGOTIATE_GLOBAL_ID]` bytes in `prefix`; we deserialize
/// them and use the trace header to scope a `negotiate` span so handler logs
/// are correlated with the originating client.
///
/// Then reads the length-prefixed `PeerInfo` payload, calls
/// `handshake.negotiate`, and writes the length-prefixed response. Does NOT
/// close the stream — the caller continues reading the next RPC frame on the
/// same stream.
pub async fn handle_negotiate_rpc<R, W>(
    reader: &mut R,
    writer: &mut W,
    handshake: &dyn Handshake,
    peer: SocketAddr,
    mut prefix: ByteVec,
) -> io::Result<()>
where
    R: crate::Reader,
    W: crate::Writer,
{
    use tracing::Instrument;

    // Consume the [trace::Header][GlobalId] from the prefix. The trace header
    // is used to scope the handler's tracing span.
    let header = trace::Header::deserialize(&mut prefix)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    let _global_id = GlobalId::deserialize(&mut prefix)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

    let span = tracing::info_span!(
        "negotiate",
        %peer,
        trace = %header.trace,
        span_id = %trace::Span::from(header.span_id),
    );

    async move {
        // Read the length-prefixed PeerInfo payload, starting from any bytes
        // already buffered in `prefix` (after the header).
        let mut buf = prefix;
        let remote_payload: Bytes =
            match crate::service::common::read_msg_with_len_prefix::<Bytes>(reader, &mut buf).await
            {
                Ok(v) => v,
                Err(error) => {
                    return write_negotiate_response(
                        writer,
                        Err(Error::Io(io::Error::new(
                            io::ErrorKind::InvalidData,
                            error.to_string(),
                        ))),
                    )
                    .await;
                }
            };

        // Per-peer lock + double-checked is_negotiated
        let lock = handshake.peer_lock(&peer);
        let _guard = lock.lock().await;

        if !handshake.is_negotiated(&peer) {
            if let Err(e) = handshake.negotiate(peer, remote_payload) {
                return write_negotiate_response(writer, Err(Error::Io(e))).await;
            }
        }

        let local = handshake.local_payload();
        write_negotiate_response(writer, Ok(local)).await
    }
    .instrument(span)
    .await
}

async fn write_negotiate_response<W: crate::Writer>(
    writer: &mut W,
    result: core::result::Result<Bytes, Error>,
) -> io::Result<()> {
    let mut out = Builder::default();
    Serialize::serialize_with_len_prefix(result, &mut out);
    writer.write_full_chunk(out.finish()).await.map(|_| ())
}

/// The fixed-size header prefix every RPC begins with: `[trace::Header][GlobalId]`.
pub(crate) const RPC_HEADER_LEN: usize = trace::Header::WIRE_SIZE + size_of::<GlobalId>();
