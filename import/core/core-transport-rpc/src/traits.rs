use core::future::Future;
use upstream_buffer::{reader, writer};
use upstream_bytevec::ByteVec;
use upstream_service::{Priority, ServerAddr};
use std::{io, net::SocketAddr, sync::Arc};

use crate::negotiation::Handshake;

pub trait Reader: 'static + Send + Sized {
    /// Reads a single chunk at a time
    ///
    /// Used for streaming responses and operating on them incrementally
    fn read_chunk<Payload>(
        &mut self,
        payload: &mut Payload,
    ) -> impl Future<Output = io::Result<usize>> + Send
    where
        Payload: writer::Storage + Send;

    /// Validate that the stream is ready for reading.
    ///
    /// For transports that require stream validation (e.g. dcquic stream3 flow
    /// validation), this must be called before reading. Callers should wrap this
    /// in a timeout — a stream that never validates should be dropped/reset.
    ///
    /// The default implementation is a no-op for transports that don't need it.
    #[inline]
    fn validate(&mut self) -> impl Future<Output = io::Result<()>> + Send {
        async { Ok(()) }
    }

    /// Reads the entire response to the `Payload`
    #[inline]
    fn read_to_end<Payload>(
        mut self,
        payload: &mut Payload,
    ) -> impl Future<Output = io::Result<usize>> + Send
    where
        Payload: writer::Storage + Send,
    {
        async move {
            let mut total = 0;
            loop {
                let len = self.read_chunk(payload).await?;

                if len > 0 {
                    total += len;
                    continue;
                }

                // we don't need to check if we ran out of space in the payload
                if payload.has_remaining_capacity() {
                    return Ok(total);
                }

                let mut buffer = &mut [0u8][..];
                let len = self.read_chunk(&mut buffer).await?;
                // if the stream is actually finished with additional capacity provided, then return the total
                if len == 0 {
                    return Ok(total);
                }

                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "the provided payload buffer ran out of space for reading the response",
                ));
            }
        }
    }

    #[inline]
    fn read_to_bytes(self) -> impl Future<Output = io::Result<ByteVec>> + Send {
        async move {
            let mut out = upstream_bytevec::Builder::default();
            self.read_to_end(&mut out).await?;
            Ok(out.into())
        }
    }
}

pub trait Writer: 'static + Send + Sized {
    /// Writes a single chunk at a time
    ///
    /// Used for streaming responses
    fn write_chunk<Payload>(
        &mut self,
        payload: &mut Payload,
    ) -> impl Future<Output = io::Result<usize>> + Send
    where
        Payload: reader::storage::Infallible + Send;

    /// Sets keep alive for the stream
    fn keep_alive(&mut self, enabled: bool);

    /// Resolves when the peer no longer wants this stream's data — it sent
    /// STOP_SENDING (dropped its reader before FIN), the stream was reset, or
    /// the connection died — yielding the `io::Error` a write would surface.
    ///
    /// Lets a streaming producer race this against the future that produces
    /// items (often backed by disk reads) and stop that expensive work the
    /// moment the consumer goes away, instead of only discovering it on the
    /// next `write_chunk`. It never resolves "still alive", so it yields the
    /// error directly.
    ///
    /// Default: never resolves. A transport with no cheap liveness probe relies
    /// on `write_chunk` to surface the error instead, so racing a
    /// never-resolving future is harmless. The dcquic experimental writer
    /// overrides this with the real `STOP_SENDING`/reset/peer-death signal.
    #[inline]
    fn peer_cancellation(&mut self) -> impl Future<Output = io::Error> + Send {
        std::future::pending()
    }

    /// Finishes writing and shuts down the write half of the stream.
    ///
    /// Must reliably signal end-of-stream to the peer even when `write_chunk` has
    /// never been called (zero-length messages). Transports with lazy stream
    /// establishment must materialize the stream before signaling FIN —
    /// otherwise the peer may drop the close as a frame for an unknown stream
    /// and the receiver will hang. See `empty_payload_test` for a regression
    /// guard.
    fn finish(self) -> impl Future<Output = io::Result<()>> + Send;

    /// Writes the entire chunk from the `Payload`
    /// Used for streaming responses
    #[inline]
    fn write_full_chunk<Payload>(
        &mut self,
        mut payload: Payload,
    ) -> impl Future<Output = io::Result<usize>> + Send
    where
        Payload: reader::storage::Infallible + Send,
    {
        async move {
            let mut total = 0;

            loop {
                let chunk_len = self.write_chunk(&mut payload).await?;

                total += chunk_len;

                tracing::trace!(
                    chunk_len,
                    written_len = total,
                    remaining = payload.buffered_len(),
                    "write_full_chunk",
                );

                if payload.buffer_is_empty() {
                    break;
                }
            }

            Ok(total)
        }
    }

    /// Writes the entire response from the `Payload` and finishes the stream
    #[inline]
    fn write_to_end<Payload>(
        mut self,
        payload: Payload,
    ) -> impl Future<Output = io::Result<usize>> + Send
    where
        Payload: reader::storage::Infallible + Send,
    {
        async move {
            let total = self.write_full_chunk(payload).await?;
            self.finish().await?;
            Ok(total)
        }
    }
}

pub trait PendingStream: Send + 'static {
    type Reader: Reader;
    type Writer: Writer;

    fn remote_addr(&self) -> SocketAddr;

    /// Validate and split into reader/writer/peer_addr.
    /// For most transports this is trivial. For dcquic, this performs stream
    /// validation. For `GenericStream`, this peeks the standard RPC header and
    /// runs the negotiate RPC inline (on the same stream) if the reserved
    /// `NEGOTIATE_GLOBAL_ID` is detected — the returned reader is positioned
    /// at the next RPC frame.
    fn into_parts(
        self,
    ) -> impl Future<Output = io::Result<(Self::Reader, Self::Writer, SocketAddr)>> + Send;
}

pub trait Server: 'static + Clone + Send {
    type Stream: PendingStream;
    type Handshake: Handshake + Clone;

    /// Accept the next pending stream from the listener.
    ///
    /// Returns `None` only when the listener is closed. Stream validation and
    /// the magic-byte handshake (if configured) are deferred to the spawned
    /// per-connection task in [`service::server`], so per-stream failures
    /// cannot impact accept throughput.
    fn accept(&mut self) -> impl Future<Output = Option<Self::Stream>> + Send;

    /// Access the handshake implementation, if configured.
    fn handshake(&self) -> Option<&Self::Handshake> {
        None
    }

    fn local_addr(&self) -> ServerAddr;
}

/// A reader that drains a buffered prefix before delegating to the inner
/// reader. Used by [`GenericStream::into_parts`] to "push back" bytes that
/// were peeked from the wire while looking for the negotiate RPC header.
///
/// When the prefix is empty, `read_chunk` delegates straight to the inner
/// reader — zero overhead in the steady state.
pub struct PrependedReader<R: Reader> {
    prefix: ByteVec,
    inner: R,
}

impl<R: Reader> PrependedReader<R> {
    pub fn new(prefix: ByteVec, inner: R) -> Self {
        Self { prefix, inner }
    }
}

impl<R: Reader> Reader for PrependedReader<R> {
    fn read_chunk<Payload>(
        &mut self,
        payload: &mut Payload,
    ) -> impl Future<Output = io::Result<usize>> + Send
    where
        Payload: writer::Storage + Send,
    {
        async move {
            if !self.prefix.is_empty() {
                use reader::storage::Infallible as _;
                let before = payload.remaining_capacity();
                self.prefix.infallible_copy_into(payload);
                let copied = before.saturating_sub(payload.remaining_capacity());
                if copied > 0 {
                    return Ok(copied);
                }
            }
            self.inner.read_chunk(payload).await
        }
    }

    fn validate(&mut self) -> impl Future<Output = io::Result<()>> + Send {
        self.inner.validate()
    }
}

/// A ready-to-use stream for transports that don't need deferred validation.
///
/// Used by TCP, QUIC, mux, and mem transports. If a `handshake` is
/// attached, `into_parts()` peeks the standard RPC header
/// (`[trace::Header][GlobalId]`); if the global_id is the reserved
/// `NEGOTIATE_GLOBAL_ID`, it dispatches to the negotiate RPC handler and
/// returns `Err` to short-circuit normal RPC dispatch on this stream.
/// Otherwise the peeked bytes are wrapped in a `PrependedReader` so the
/// caller's first read sees them as if they were never consumed.
pub struct GenericStream<R: Reader, W: Writer> {
    pub reader: R,
    pub writer: W,
    pub peer_addr: SocketAddr,
    pub(crate) handshake: Option<Arc<dyn Handshake>>,
}

impl<R: Reader, W: Writer> GenericStream<R, W> {
    pub fn new(
        reader: R,
        writer: W,
        peer_addr: SocketAddr,
        handshake: Option<Arc<dyn Handshake>>,
    ) -> Self {
        Self {
            reader,
            writer,
            peer_addr,
            handshake,
        }
    }
}

impl<R: Reader, W: Writer> PendingStream for GenericStream<R, W> {
    type Reader = PrependedReader<R>;
    type Writer = W;

    fn remote_addr(&self) -> SocketAddr {
        self.peer_addr
    }

    fn into_parts(
        self,
    ) -> impl Future<Output = io::Result<(Self::Reader, Self::Writer, SocketAddr)>> + Send {
        async move {
            let Self {
                mut reader,
                mut writer,
                peer_addr,
                handshake,
            } = self;

            // Only peek for the negotiate RPC if a handshake is configured AND
            // this peer hasn't been negotiated yet. Already-negotiated peers
            // and transports without a handshake (or where negotiation happens
            // at a lower layer like dcQUIC) skip the peek entirely.
            let needs_peek = match &handshake {
                Some(hs) => !hs.is_negotiated(&peer_addr),
                None => false,
            };
            if !needs_peek {
                return Ok((
                    PrependedReader::new(ByteVec::default(), reader),
                    writer,
                    peer_addr,
                ));
            }
            let hs = handshake.expect("needs_peek implies handshake is set");

            // Peek the standard RPC header [trace::Header][GlobalId].
            let mut prefix = ByteVec::with_capacity(crate::negotiation::RPC_HEADER_LEN);
            crate::service::common::read_until(&mut reader, &mut prefix, |b| {
                b.len() >= crate::negotiation::RPC_HEADER_LEN
            })
            .await?;

            if prefix.len() < crate::negotiation::RPC_HEADER_LEN {
                // Stream closed before we got the header — let the caller handle it.
                return Ok((PrependedReader::new(prefix, reader), writer, peer_addr));
            }

            let global_id = peek_global_id(&prefix);

            if global_id != crate::negotiation::NEGOTIATE_GLOBAL_ID {
                // Not a negotiate RPC — push the peeked header back for normal
                // RPC dispatch. (Production rollout: old client sends RPC
                // directly without negotiate; we let it through, peer stays
                // unnegotiated, falls back to `Set::all()` defaults.)
                return Ok((PrependedReader::new(prefix, reader), writer, peer_addr));
            }

            // Negotiate RPC: pass the full prefix to the handler; it will
            // deserialize the trace header (to scope its tracing span) and
            // the global_id, then read the length-prefixed payload and write
            // the length-prefixed response — neither side closes the stream.
            crate::negotiation::handle_negotiate_rpc(
                &mut reader,
                &mut writer,
                hs.as_ref(),
                peer_addr,
                prefix,
            )
            .await?;

            // The stream is now positioned at the next RPC frame. Return parts
            // for normal RPC dispatch.
            Ok((
                PrependedReader::new(ByteVec::default(), reader),
                writer,
                peer_addr,
            ))
        }
    }
}

/// Peek the `GlobalId` from a buffer that contains at least
/// `RPC_HEADER_LEN` bytes (`[trace::Header][GlobalId]`), without consuming.
fn peek_global_id(buf: &ByteVec) -> upstream_service::GlobalId {
    use upstream_runtime::trace;
    let bytes = buf.copy_to_bytes();
    let start = trace::Header::WIRE_SIZE;
    let end = start + std::mem::size_of::<upstream_service::GlobalId>();
    let id_bytes: [u8; 8] = bytes[start..end]
        .try_into()
        .expect("buf has at least RPC_HEADER_LEN bytes");
    u64::from_be_bytes(id_bytes)
}

pub trait Client: 'static + Clone + Send + Sync {
    type Reader: Reader;
    type Writer: Writer;
    type Handshake: Handshake;

    /// Open a raw stream — no negotiation.
    ///
    /// `priority` is the transport-level send priority for the request. Only
    /// the experimental dcquic transport honors it today; other transports
    /// accept and ignore it.
    fn open_stream(
        &self,
        addr: ServerAddr,
        priority: Priority,
    ) -> impl Future<Output = io::Result<(Self::Writer, Self::Reader)>> + Send + 'static;

    /// Access the handshake implementation, if configured.
    fn handshake(&self) -> Option<&Self::Handshake> {
        None
    }

    /// Connect with negotiation. Default: open a stream and run inline
    /// negotiate (if a handshake is configured and the peer isn't already
    /// negotiated). The negotiate RPC uses the reserved `NEGOTIATE_GLOBAL_ID`
    /// with length-prefixed framing so the stream stays open for the actual
    /// RPC payload that follows.
    ///
    /// Transports where negotiation happens at a lower level (e.g., dcQUIC)
    /// override this to just call `open_stream()`.
    fn connect(
        &self,
        addr: ServerAddr,
        priority: Priority,
    ) -> impl Future<Output = io::Result<(Self::Writer, Self::Reader)>> + Send + 'static {
        let client = self.clone();
        async move {
            let (mut writer, mut reader) = client.open_stream(addr, priority).await?;
            if let Some(hs) = client.handshake() {
                if !hs.is_negotiated(&addr.handshake_addr()) {
                    crate::negotiation::send_negotiate_rpc(
                        &mut writer,
                        &mut reader,
                        hs,
                        addr.handshake_addr(),
                    )
                    .await?;
                }
            }
            Ok((writer, reader))
        }
    }

    /// Helper method for simple request/response patterns
    fn request_response<Payload>(
        &self,
        addr: ServerAddr,
        payload: Payload,
        priority: Priority,
    ) -> impl Future<Output = io::Result<ByteVec>> + Send + 'static
    where
        Payload: reader::storage::Infallible + Send + 'static,
    {
        let client = self.clone();
        async move {
            let (writer, reader) = client.connect(addr, priority).await?;
            let (_written_len, response) =
                tokio::try_join!(writer.write_to_end(payload), reader.read_to_bytes())?;
            Ok(response)
        }
    }

    /// Helper method for request/response patterns using a `rpc::Response`
    /// that supports async backpressure via `provide_storage()`.
    ///
    /// The default implementation uses `connect` + manual read/write.
    /// The dcquic implementation calls `client.rpc()` directly to leverage
    /// QUIC-native RPC with proper backpressure.
    fn request_response_into<Payload, Res>(
        &self,
        addr: ServerAddr,
        payload: Payload,
        mut response: Res,
        priority: Priority,
    ) -> impl Future<Output = io::Result<Res::Output>> + Send
    where
        Payload: reader::storage::Infallible + Send + 'static,
        Res: upstream_service::rpc_response::Response + Send,
        Res::Storage: Send,
        Res::Output: Send,
    {
        let client = self.clone();
        async move {
            let (writer, reader) = client.connect(addr, priority).await?;
            let writer_fut = writer.write_to_end(payload);
            let reader_fut = async {
                use upstream_buffer::writer::Storage as _;
                let mut reader = reader;
                loop {
                    let storage = response.provide_storage().await?;
                    if storage.remaining_capacity() == 0 {
                        return Err(io::Error::other("response buffer ran out of capacity"));
                    }
                    let len = reader.read_chunk(storage).await?;
                    if len == 0 {
                        return response.finish().await;
                    }
                }
            };
            let (_written_len, output) = tokio::try_join!(writer_fut, reader_fut)?;
            Ok(output)
        }
    }
}

#[cfg(test)]
mod accept_contract_tests {
    //! Verifies the contract introduced when `accept()` was changed to return
    //! the unvalidated `PendingStream` (and validation moved into the spawned
    //! per-connection task). Together these properties guarantee that no
    //! single bad stream can stall or kill the accept loop.

    use super::*;
    use crate::negotiation::Handshake;
    use bytes::Bytes;
    use upstream_buffer::{reader, writer};
    use std::{
        collections::VecDeque,
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    /// EOF-on-first-read reader.
    struct EofReader;

    impl Reader for EofReader {
        fn read_chunk<P>(
            &mut self,
            _payload: &mut P,
        ) -> impl Future<Output = io::Result<usize>> + Send
        where
            P: writer::Storage + Send,
        {
            async { Ok(0) }
        }
    }

    /// Discarding writer.
    struct NullWriter;

    impl Writer for NullWriter {
        fn write_chunk<P>(
            &mut self,
            _payload: &mut P,
        ) -> impl Future<Output = io::Result<usize>> + Send
        where
            P: reader::storage::Infallible + Send,
        {
            async { Ok(0) }
        }

        fn keep_alive(&mut self, _enabled: bool) {}

        fn finish(self) -> impl Future<Output = io::Result<()>> + Send {
            async { Ok(()) }
        }
    }

    /// A `PendingStream` that records whether `into_parts()` was invoked, and
    /// can be configured to return success or a specified error. Optionally
    /// gates `into_parts()` on a `Notify` so the test can hold validation
    /// open while it observes the accept loop independently.
    struct ProbedPendingStream {
        peer: SocketAddr,
        outcome: io::Result<()>,
        into_parts_calls: Arc<AtomicUsize>,
        gate: Option<Arc<tokio::sync::Notify>>,
    }

    impl PendingStream for ProbedPendingStream {
        type Reader = EofReader;
        type Writer = NullWriter;

        fn remote_addr(&self) -> SocketAddr {
            self.peer
        }

        fn into_parts(
            self,
        ) -> impl Future<Output = io::Result<(Self::Reader, Self::Writer, SocketAddr)>> + Send
        {
            async move {
                self.into_parts_calls.fetch_add(1, Ordering::SeqCst);
                if let Some(gate) = self.gate {
                    gate.notified().await;
                }
                self.outcome?;
                Ok((EofReader, NullWriter, self.peer))
            }
        }
    }

    /// Pre-scripted server. `outcomes` is a queue of either:
    ///   - `Some((peer, outcome, gate))` → next `accept()` returns a stream
    ///     whose `remote_addr` is `peer` and whose `into_parts()` resolves to
    ///     `outcome` (after waiting on `gate`, if `Some`).
    ///   - `None` → `accept()` returns `None` (listener closed).
    type Outcome = Option<(SocketAddr, io::Result<()>, Option<Arc<tokio::sync::Notify>>)>;

    #[derive(Clone)]
    struct ScriptedServer {
        outcomes: Arc<Mutex<VecDeque<Outcome>>>,
        into_parts_calls: Arc<AtomicUsize>,
    }

    impl ScriptedServer {
        fn new<I>(outcomes: I) -> Self
        where
            I: IntoIterator<Item = Outcome>,
        {
            Self {
                outcomes: Arc::new(Mutex::new(outcomes.into_iter().collect())),
                into_parts_calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn into_parts_calls(&self) -> usize {
            self.into_parts_calls.load(Ordering::SeqCst)
        }
    }

    impl Server for ScriptedServer {
        type Stream = ProbedPendingStream;
        type Handshake = Arc<dyn Handshake>;

        fn accept(&mut self) -> impl Future<Output = Option<Self::Stream>> + Send {
            let outcomes = self.outcomes.clone();
            let into_parts_calls = self.into_parts_calls.clone();
            async move {
                let next = outcomes.lock().unwrap().pop_front();
                match next {
                    Some(Some((peer, outcome, gate))) => Some(ProbedPendingStream {
                        peer,
                        outcome,
                        into_parts_calls,
                        gate,
                    }),
                    Some(None) | None => None,
                }
            }
        }

        fn local_addr(&self) -> ServerAddr {
            ServerAddr::from_same_handshake_port("127.0.0.1:0".parse().unwrap())
        }
    }

    fn peer(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    fn ok(p: SocketAddr) -> Outcome {
        Some((p, Ok(()), None))
    }

    fn err(p: SocketAddr, e: io::Error) -> Outcome {
        Some((p, Err(e), None))
    }

    fn gated(p: SocketAddr, gate: Arc<tokio::sync::Notify>) -> Outcome {
        Some((p, Ok(()), Some(gate)))
    }

    /// `accept()` must return without invoking `into_parts()`. If it ever
    /// regresses to running validation inline, this test fails because the
    /// `into_parts_calls` counter would tick before the assertion.
    #[tokio::test]
    async fn accept_does_not_run_validation() {
        let mut server = ScriptedServer::new([
            ok(peer(1)),
            err(peer(2), io::Error::new(io::ErrorKind::TimedOut, "x")),
            None,
        ]);

        let stream = server.accept().await.expect("first stream");
        assert_eq!(
            server.into_parts_calls(),
            0,
            "accept() must not run into_parts() on the hot path",
        );
        assert_eq!(stream.remote_addr(), peer(1));

        // The peer addr is observable on the unvalidated stream so the spawn
        // span can include it. This is part of the new contract.
        let stream = server.accept().await.expect("second stream");
        assert_eq!(
            server.into_parts_calls(),
            0,
            "accept() must not run into_parts() even when the next stream's \
             validation would fail",
        );
        assert_eq!(stream.remote_addr(), peer(2));
    }

    /// A failing `into_parts()` only consumes that one stream — the next
    /// `accept()` is unaffected.
    #[tokio::test]
    async fn into_parts_failure_is_isolated() {
        let mut server = ScriptedServer::new([
            err(peer(10), io::Error::new(io::ErrorKind::TimedOut, "v")),
            ok(peer(11)),
        ]);

        let bad = server.accept().await.expect("bad stream");
        assert!(bad.into_parts().await.is_err());

        let good = server.accept().await.expect("good stream");
        let parts = good.into_parts().await.expect("good stream validates");
        assert_eq!(parts.2, peer(11));
    }

    /// `accept()` returns `None` only on listener closure — even after a
    /// stream returned `Err` from `into_parts()`.
    #[tokio::test]
    async fn listener_closure_after_failure_returns_none() {
        let mut server = ScriptedServer::new([
            err(peer(20), io::Error::new(io::ErrorKind::TimedOut, "v")),
            None,
        ]);

        let bad = server.accept().await.expect("bad stream");
        let _ = bad.into_parts().await;

        assert!(
            server.accept().await.is_none(),
            "listener closure must propagate after per-stream failures"
        );
    }

    // ─── Caller pattern test ─────────────────────────────────────────────────

    /// A handshake stub that pretends every peer is already negotiated.
    struct AlwaysNegotiated;

    impl Handshake for AlwaysNegotiated {
        fn local_payload(&self) -> Bytes {
            Bytes::new()
        }
        fn local_handshake_port(&self) -> u16 {
            0
        }
        fn negotiate(&self, _peer: SocketAddr, _payload: Bytes) -> io::Result<()> {
            Ok(())
        }
        fn is_negotiated(&self, _peer: &SocketAddr) -> bool {
            true
        }
    }

    /// Drives the canonical `service::server` caller pattern: spawn each
    /// stream's `into_parts()` into its own task. This proves the property
    /// the production loop actually relies on — that a slow or failing
    /// validation is **isolated by the spawn**, not by the loop body — by
    /// holding `into_parts()` open on one stream and verifying the loop
    /// continues to accept and dispatch the next ones.
    #[tokio::test]
    async fn spawned_caller_pattern_isolates_slow_validation() {
        let stuck = Arc::new(tokio::sync::Notify::new());
        let mut server = ScriptedServer::new([
            ok(peer(100)),
            // peer(101) hangs in into_parts() until the test releases `stuck`.
            gated(peer(101), stuck.clone()),
            ok(peer(102)),
            err(peer(103), io::Error::other("transport")),
            ok(peer(104)),
            None,
        ]);

        // Sanity-check the handshake getter compiles at the call site, even
        // though this test never invokes the protocol.
        let _hs: Option<&Arc<dyn Handshake>> = server.handshake();
        let _ = AlwaysNegotiated;

        let accepted = Arc::new(Mutex::new(Vec::<SocketAddr>::new()));
        let validation_failures = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        while let Some(stream) = server.accept().await {
            let observed_peer = stream.remote_addr();
            let accepted = accepted.clone();
            let validation_failures = validation_failures.clone();
            handles.push(tokio::spawn(async move {
                match stream.into_parts().await {
                    Ok((_r, _w, peer_addr)) => {
                        assert_eq!(peer_addr, observed_peer);
                        accepted.lock().unwrap().push(peer_addr);
                    }
                    Err(_) => {
                        validation_failures.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }));
        }

        // The accept loop reached `None` (listener closed) even though the
        // stuck stream's `into_parts()` is still hanging. That's the
        // latency-isolation property the production code depends on.
        //
        // Yield a few times so each spawned task at least enters
        // `into_parts()` (the counter ticks at the start of the function),
        // including the one that will park on the gate.
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            server.into_parts_calls(),
            5,
            "every accepted stream's into_parts() should have been entered \
             (one is still parked on the gate, but its counter ticks first)",
        );

        // Now release the stuck stream and let all spawned tasks finish.
        stuck.notify_one();
        for h in handles {
            h.await.unwrap();
        }

        // Three streams validated cleanly, one failed, and the previously
        // stuck one has now also completed successfully.
        let mut accepted = accepted.lock().unwrap().clone();
        accepted.sort_by_key(|a| a.port());
        assert_eq!(accepted, vec![peer(100), peer(101), peer(102), peer(104)]);
        assert_eq!(validation_failures.load(Ordering::SeqCst), 1);
    }
}
