use crate::{Reader, Writer};
use core::sync::atomic::{AtomicUsize, Ordering};
use upstream_bytevec::{Builder, ByteVec};
use upstream_runtime::{Env, Error, Result};
use upstream_service::{
    Span,
    stream::{self, Stream},
};
use upstream_wire::{Deserialize, Serialize};
use std::{
    future::poll_fn,
    io,
    pin::{Pin, pin},
    task::Poll,
};

use super::stall::{
    Counted, End, StallPhase, StallPolicy, StallTimer, report_kill, send_stall_error,
};

pub const STREAM_BATCH_SIZE: usize = 1 << 17;

/// Per-frame instrumentation for the framed-stream WRITE path
/// ([`write_framed_stream_at`]).
///
/// The framing layer is shared by several RPC shapes (bidi request pump, bidi
/// response, plain stream-request/response). Each shape instruments frames
/// differently, but the framing loop is identical — so instead of duplicating
/// the loop per shape, callers pass an implementor of this trait and the loop
/// drives it: `on_item_serialize` per stream item (timing + size of the
/// serialized frame) and `on_chunk_transmit` per flushed chunk (transmit
/// timing). The default `()` impl is a no-op, so uninstrumented callers pay
/// nothing.
///
/// Only `Send` is required, not `'static`: the inline consumer stream-request
/// path passes a *borrowing* bridge (`&mut span`). Call sites that spawn the
/// write pump (bidi) get `'static` from the spawn closure's own bound, not from
/// this trait.
pub trait WriteInstrument<T>: Send {
    fn on_item_serialize<E: Env>(&mut self, env: &E, item: &T) -> impl Span<usize> {
        let _ = (env, item);
    }

    fn on_chunk_transmit<E: Env>(
        &mut self,
        env: &E,
        chunk_size: usize,
    ) -> impl Span<Result<(), Error>> {
        let _ = (env, chunk_size);
    }
}

impl<T> WriteInstrument<T> for () {}

/// Per-frame instrumentation for the framed-stream READ path
/// ([`read_framed_stream_at`]).
///
/// Mirror of [`WriteInstrument`]: `on_chunk_receive` times the wire read of a
/// frame's bytes and `on_item_deserialize` times the decode (sized by the
/// frame, exited with the decoded `Result<T>`). The `()` no-op default keeps
/// uninstrumented callers zero-cost. Only `Send` is required (see
/// [`WriteInstrument`]); `'static`, when needed, comes from the call site.
pub trait ReadInstrument<T>: Send {
    fn on_chunk_receive<E: Env>(&mut self, env: &E) -> impl Span<Result<usize, Error>> {
        let _ = env;
    }

    fn on_item_deserialize<E: Env>(
        &mut self,
        env: &E,
        chunk_size: usize,
    ) -> impl Span<Result<T, Error>> {
        let _ = (env, chunk_size);
    }
}

impl<T> ReadInstrument<T> for () {}

/// Reads a length prefix from a stream
pub async fn read_len_prefix(
    stream: &mut impl Reader,
    buffer: &mut ByteVec,
) -> Result<Option<usize>> {
    // Read until we have enough bytes for the message length
    read_until(stream, buffer, |buf| {
        buf.len() >= std::mem::size_of::<u64>()
    })
    .await?;

    if buffer.is_empty() {
        return Ok(None);
    }

    // Get the message length
    let message_len = u64::deserialize(buffer)?;
    Ok(Some(message_len as usize))
}

/// Reads a message by buffering at least the provided `len`. Production paths
/// call the `_at` variant directly; this non-versioned form is kept for tests
/// and external callers.
#[allow(dead_code)]
pub async fn read_msg_with_len<Msg: Deserialize>(
    stream: &mut impl Reader,
    buffer: &mut ByteVec,
    len: usize,
) -> Result<Msg> {
    read_msg_with_len_at(stream, buffer, len, None).await
}

/// Version-aware variant of [`read_msg_with_len`]. When `version_idx` is
/// `Some`, decodes via `Msg::deserialize_at(buf, idx)`; when `None`, falls
/// through to `Msg::deserialize` (current version).
pub async fn read_msg_with_len_at<Msg: Deserialize>(
    stream: &mut impl Reader,
    buffer: &mut ByteVec,
    len: usize,
    version_idx: Option<u32>,
) -> Result<Msg> {
    // Read until we have the full message
    read_until(stream, buffer, |buf| buf.len() >= len).await?;

    // Deserialize the message at the requested version, or current.
    let message = match version_idx {
        Some(idx) => Msg::deserialize_at(buffer, idx)?,
        None => Msg::deserialize(buffer)?,
    };
    Ok(message)
}

pub async fn read_msg_with_len_prefix<Msg: Deserialize>(
    stream: &mut impl Reader,
    buffer: &mut ByteVec,
) -> Result<Msg> {
    read_msg_with_len_prefix_at(stream, buffer, None).await
}

/// Version-aware variant of [`read_msg_with_len_prefix`].
pub async fn read_msg_with_len_prefix_at<Msg: Deserialize>(
    stream: &mut impl Reader,
    buffer: &mut ByteVec,
    version_idx: Option<u32>,
) -> Result<Msg> {
    let len = read_len_prefix(stream, buffer)
        .await?
        .ok_or_else(|| Error::Io(io::ErrorKind::UnexpectedEof.into()))?;
    read_msg_with_len_at(stream, buffer, len, version_idx).await
}

/// Helper function to read a stream of length-prefixed frames at the current
/// (latest) version, uninstrumented. Thin wrapper over
/// [`read_framed_stream_at`] with `version_idx = None` and a no-op
/// [`ReadInstrument`] under a throwaway env. Kept for tests and external
/// callers that don't have an `Env`/span to thread.
#[allow(dead_code)]
pub async fn read_framed_stream<T: Deserialize, R: Reader>(
    stream: R,
    in_buf: ByteVec,
) -> Result<impl Stream<Item = Result<T>>> {
    read_framed_stream_at(stream, in_buf, None, throwaway_env(), ()).await
}

/// Version-aware, instrumented variant of [`read_framed_stream`].
///
/// When `version_idx` is `Some`, each frame is decoded via
/// `T::deserialize_at(buf, idx)` (the default `deserialize_at` ignores `idx`
/// for non-versioned types, so `Some` is always safe). `instrument` is driven
/// per frame — `on_chunk_receive` around the wire read and
/// `on_item_deserialize` around the decode — so streaming RPCs can time the
/// receive/decode legs of their span; pass `()` for no instrumentation.
pub async fn read_framed_stream_at<T, R, E, I>(
    stream: R,
    in_buf: ByteVec,
    version_idx: Option<u32>,
    env: E,
    instrument: I,
) -> Result<impl Stream<Item = Result<T>>>
where
    T: Deserialize,
    R: Reader,
    E: Env,
    I: ReadInstrument<T>,
{
    // Create a stream that reads length-prefixed frames
    let stream = stream::unfold(
        (Some(stream), in_buf, version_idx, env, instrument),
        |(read_half, mut in_buf, version_idx, env, mut instrument)| async move {
            let mut read_half = read_half?;

            // Receive leg: buffer the frame's length prefix (and, transitively,
            // its bytes on the next read). Timed by `on_chunk_receive`.
            let receive_span = instrument.on_chunk_receive(&env);
            let frame_len = match read_len_prefix(&mut read_half, &mut in_buf).await {
                Ok(Some(len)) => {
                    receive_span.exit(&env, &Ok(len));
                    len
                }
                Ok(None) => {
                    // Clean end-of-stream: drop the receive sub-span WITHOUT
                    // exiting (this poll read no frame, so a zero-byte sample
                    // would skew the receive distribution).
                    drop(receive_span);
                    return None;
                }
                Err(e) => {
                    let e = upstream_service::consumer::span::exit_err(&env, receive_span, e);
                    return Some((Err(e), (None, in_buf, version_idx, env, instrument)));
                }
            };

            // Decode leg: read the remaining body and deserialize, sized by the
            // frame. Timed + outcome-recorded by `on_item_deserialize`. Each
            // frame is a `Result<T>` on the wire (the writer wraps every item in
            // `Result`), so decode as `Result<T>` and flatten with
            // `and_then(|v| v)` — decoding as bare `T` would skip the `Ok` tag
            // byte and desync the stream. Build the flattened `Result` once,
            // borrow it for the span exit, then move it out (no `T: Clone`).
            let deserialize_span = instrument.on_item_deserialize(&env, frame_len);
            let item = read_msg_with_len_at::<Result<T>>(
                &mut read_half,
                &mut in_buf,
                frame_len,
                version_idx,
            )
            .await
            .and_then(|v| v);
            deserialize_span.exit(&env, &item);
            match item {
                Ok(value) => Some((
                    Ok(value),
                    (Some(read_half), in_buf, version_idx, env, instrument),
                )),
                Err(e) => Some((Err(e), (None, in_buf, version_idx, env, instrument))),
            }
        },
    );

    Ok(stream)
}

/// Helper function to write a stream of length-prefixed frames at the current
/// (latest) version, uninstrumented. Thin wrapper over
/// [`write_framed_stream_at`] with `version_idx = None` and a no-op
/// [`WriteInstrument`] under a throwaway env. Kept for tests and external
/// callers that don't have an `Env`/span to thread.
#[allow(dead_code)]
pub async fn write_framed_stream<T: Serialize, W: Writer>(
    stream: W,
    value_stream: impl Stream<Item = Result<T>> + Send,
    out: Builder,
) -> Result<()> {
    write_framed_stream_at(stream, value_stream, out, None, &throwaway_env(), (), None).await
}

/// Version-aware, instrumented variant of [`write_framed_stream`].
///
/// When `version_idx` is `Some`, each frame is encoded via
/// `T::serialize_with_len_prefix_at(out, idx)` (the default `serialize_at`
/// falls through to plain `serialize`, so `Some` is always safe). `instrument`
/// is driven per frame — `on_item_serialize` around each item's encode (sized
/// by the bytes it adds) and `on_chunk_transmit` around each flush — so
/// streaming RPCs can time the serialize/transmit legs of their span; pass `()`
/// for no instrumentation.
///
/// `stall` bounds how long the response may go without the progress [`StallPolicy`]
/// defines; `None` applies no bound. At the bound the body is dropped, a pending
/// write is left to complete, and the stream ends in one error frame (see
/// [`StallPolicy`] for what the client sees and what a dead peer still holds).
/// However the body ends, it is dropped before the final write.
pub async fn write_framed_stream_at<T, W, E, I>(
    mut stream: W,
    value_stream: impl Stream<Item = Result<T>> + Send,
    mut out: Builder,
    version_idx: Option<u32>,
    env: &E,
    mut instrument: I,
    stall: Option<StallPolicy>,
) -> Result<()>
where
    T: Serialize,
    W: Writer,
    E: Env,
    I: WriteInstrument<T>,
{
    stream.keep_alive(true);

    let mut stall_timer = pin!(StallTimer::new(stall.as_ref()));
    // The body lives only inside this block, so whatever it holds is released
    // before any terminal write.
    let end = {
        let body = pin!(Some(value_stream));
        pump(
            &mut stream,
            body,
            &mut out,
            version_idx,
            env,
            &mut instrument,
            stall_timer.as_mut(),
        )
        .await?
    };

    match end {
        End::Drained => {
            let transmit_span = instrument.on_chunk_transmit(env, out.len());
            match stream.write_to_end(out).await {
                Ok(_) => {
                    transmit_span.exit(env, &Ok(()));
                    Ok(())
                }
                Err(e) => Err(upstream_service::consumer::span::exit_err(
                    env,
                    transmit_span,
                    Error::Io(e),
                )),
            }
        }
        End::Stalled => send_stall_error::<T, W>(stream, out, version_idx).await,
    }
}

/// Drains `body` into `stream` until the body ends or stalls.
async fn pump<T, W, E, I, S>(
    stream: &mut W,
    mut body: Pin<&mut Option<S>>,
    out: &mut Builder,
    version_idx: Option<u32>,
    env: &E,
    instrument: &mut I,
    mut stall_timer: Pin<&mut StallTimer<'_, E::Timer>>,
) -> Result<End>
where
    T: Serialize,
    W: Writer,
    E: Env,
    I: WriteInstrument<T>,
    S: Stream<Item = Result<T>>,
{
    let mut is_done = false;
    // The response's own progress, compared for change only: bytes its chunk
    // writes handed the transport, a mark at the end of each write that took
    // some, and one per batch the body made the loop wait for.
    let own = AtomicUsize::new(0);

    loop {
        // The stall timer drops the body when it fires during a write; the stream
        // ends once that write completes and what it left has drained to under a
        // batch.
        let Some(mut live) = body.as_mut().as_pin_mut() else {
            return Ok(End::Stalled);
        };
        // Gather as many ready upstream values as we can (up to the batch size), serializing them
        // into `out`. Returns Ready when there is data to flush or the stream is done; Pending only
        // while `out` is empty and upstream isn't ready.
        //
        // Polling `body` is what drives the response's backing work — for a shuffle read
        // response that is the per-block disk reads. So we race the gather against
        // `peer_cancellation`: the instant the consumer goes away (STOP_SENDING / reset / peer
        // death), we abandon the response instead of continuing to read blocks from disk for a peer
        // that will never consume them. This is the over-read brake — without it an idle producer
        // only discovers the cancellation on its next `write_chunk`, after the reads are already
        // issued. `peer_cancellation` touches only the writer's control/completion channels and
        // `gather` touches only `body`/`out`, so the two `select!` arms borrow disjoint
        // state; cancel-safe because a dropped `gather` re-polls cleanly (its state lives in the
        // outer scope) and `peer_cancellation` registers a waker but holds no per-poll state.
        // The stall timer is the third arm: a response with no progress for the whole
        // timeout ends.
        let mut waited = false;
        let gather = poll_fn(|cx| {
            loop {
                match live.as_mut().poll_next(cx) {
                    Poll::Ready(Some(value)) => {
                        is_done |= value.is_err();
                        // Time each item's serialization, sized by the bytes it
                        // adds to `out`. `on_item_serialize` borrows the item
                        // (for custom metrics) before it is consumed by encode.
                        // Errors carry no `T` to observe, so only `Ok` items are
                        // instrumented; both are still written to the wire.
                        match value {
                            Ok(item) => {
                                let serialize_span = instrument.on_item_serialize(env, &item);
                                let before = out.len();
                                serialize_frame(Ok::<T, Error>(item), out, version_idx);
                                serialize_span.exit(env, &(out.len() - before));
                            }
                            err => serialize_frame(err, out, version_idx),
                        }
                        // If done or we've hit the batch size, go flush
                        if is_done || out.len() >= STREAM_BATCH_SIZE {
                            return Poll::Ready(());
                        }
                        // Try to gather more without blocking
                        continue;
                    }
                    Poll::Ready(None) => {
                        is_done = true;
                        return Poll::Ready(());
                    }
                    Poll::Pending => {
                        // If we have buffered data, go ahead and flush it
                        if !out.is_empty() {
                            return Poll::Ready(());
                        }
                        // Otherwise wait for upstream
                        waited = true;
                        return Poll::Pending;
                    }
                }
            }
        });
        let stalled = tokio::select! {
            biased;
            () = gather => None,
            err = stream.peer_cancellation() => return Err(err.into()),
            (policy, idle) = stall_timer.as_mut().stalled(env, &own) => Some((policy, idle)),
        };
        if let Some((policy, idle)) = stalled {
            body.set(None);
            report_kill(policy, idle, StallPhase::Idle);
            return Ok(End::Stalled);
        }

        if is_done {
            return Ok(End::Drained);
        }
        // A batch the body made the loop wait for is progress its write would not
        // mark; the stall timer sees it when the response next waits.
        if waited {
            own.store(own.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
        }

        // Flush to the transport until less than a batch is left, so no write is
        // handed more than a batch and the item that crossed it, whatever a call
        // takes. A peer cancellation surfaces here as a write error, so the flush
        // needs no separate liveness race. The stall timer never abandons a write:
        // at the bound it drops the body and the write runs on.
        let transmit_span = instrument.on_chunk_transmit(env, out.len());
        loop {
            let before = out.len();
            let written = match stall_timer.policy() {
                None => stream.write_chunk(out).await,
                Some(_) => {
                    let mut payload = Counted::new(out, &own);
                    let mut write = pin!(stream.write_chunk(&mut payload));
                    loop {
                        tokio::select! {
                            biased;
                            written = &mut write => break written,
                            (policy, idle) = stall_timer.as_mut().stalled(env, &own), if body.is_some() => {
                                body.set(None);
                                report_kill(policy, idle, StallPhase::MidWrite);
                            }
                        }
                    }
                }
            };
            if let Err(e) = written {
                return Err(upstream_service::consumer::span::exit_err(
                    env,
                    transmit_span,
                    e.into(),
                ));
            }
            // A completion that took nothing (stable dcQUIC reports bytes it took on
            // an earlier call before taking more) is retried. It is no progress, so
            // the stall timer is polled here, where neither select would reach it,
            // and the task yields so a writer that never takes cannot monopolize it.
            if out.len() == before {
                tokio::select! {
                    biased;
                    (policy, idle) = stall_timer.as_mut().stalled(env, &own), if body.is_some() => {
                        body.set(None);
                        report_kill(policy, idle, StallPhase::MidWrite);
                    }
                    () = tokio::task::yield_now() => {}
                }
            }
            if out.len() < STREAM_BATCH_SIZE {
                break;
            }
        }
        transmit_span.exit(env, &Ok(()));
    }
}

/// One length-prefixed frame, at `version_idx` when the client negotiated one.
pub(crate) fn serialize_frame<T: Serialize>(
    frame: Result<T>,
    out: &mut Builder,
    version_idx: Option<u32>,
) {
    match version_idx {
        Some(idx) => frame.serialize_with_len_prefix_at(out, idx),
        None => frame.serialize_with_len_prefix(out),
    }
}

/// A throwaway [`Env`] for the uninstrumented framing wrappers. The no-op
/// `()` instrument never reads any env field, so its concrete values are
/// irrelevant; this exists only to satisfy the `E: Env` bound.
pub(crate) fn throwaway_env() -> upstream_runtime::tokio::Env<()> {
    upstream_runtime::tokio::Env::new(upstream_runtime::Trace::new([0; 16]), 0u64, ())
}

pub async fn read_until(
    stream: &mut impl Reader,
    buffer: &mut ByteVec,
    cond: impl Fn(&ByteVec) -> bool,
) -> io::Result<()> {
    while !cond(buffer) {
        let len = stream.read_chunk(buffer).await?;

        // the condition wasn't satisfied but we didn't get any data so the stream is finished
        if len == 0 {
            break;
        }
    }

    Ok(())
}

pub async fn read_to_end(stream: impl Reader, buffer: &mut ByteVec) -> io::Result<()> {
    stream.read_to_end(buffer).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Client as _, PendingStream as _, Rpc, Server as _, mem};
    use upstream_service::stream::StreamExt;
    use std::pin::pin;

    // Helper function to send ByteVec chunks
    async fn send_all(stream: &mut impl Writer, data: ByteVec) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }

        stream.write_full_chunk(data).await?;

        Ok(())
    }

    async fn test_transport<T: Rpc>(transport: T) -> Result<()> {
        upstream_testing::init_tracing();
        tracing::info!("Starting test");

        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        tracing::info!(?addr, "Server listening");

        let client = transport.create_client().await?;

        // Spawn server task
        let server_task = tokio::spawn(async move {
            tracing::info!("Server waiting for connection");
            let (reader, mut writer, _) =
                server.accept().await.unwrap().into_parts().await.unwrap();
            tracing::info!("Server accepted connection");

            let buffer = ByteVec::new();

            // Read the incoming stream
            tracing::info!("Server creating framed stream");
            let stream = read_framed_stream::<u32, _>(reader, buffer).await?;
            let mut stream = pin!(stream);
            let mut sum = 0;

            tracing::info!("Server reading values");
            while let Some(value) = stream.next().await {
                let value = value?;
                tracing::info!(?value, sum, "Server received value");
                sum += value;
            }

            // Write back the sum
            tracing::info!(sum, "Server writing sum back");
            let mut out = Builder::default();
            sum.serialize_with_len_prefix(&mut out);
            send_all(&mut writer, out.finish()).await?;
            tracing::info!("Server finished");

            Result::<_>::Ok(())
        });

        // Client sends numbers and receives sum
        tracing::info!("Client connecting");
        let (writer, mut reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;
        tracing::info!("Client connected");

        let values = vec![1u32, 2, 3, 4, 5];
        let stream = stream::iter(values.into_iter().map(Ok));

        tracing::info!("Client writing values");
        write_framed_stream(writer, stream, Builder::default()).await?;
        tracing::info!("Client finished writing");

        tracing::info!("Client reading response");
        let mut buffer = ByteVec::new();
        let sum: u32 = read_msg_with_len_prefix(&mut reader, &mut buffer).await?;
        tracing::info!(?sum, "Client received sum");
        assert_eq!(sum, 15);

        tracing::info!("Waiting for server task");
        server_task.await.unwrap()?;
        tracing::info!("Test complete");
        Ok(())
    }

    #[tokio::test]
    async fn test_mem_transport() -> Result<()> {
        test_transport(mem::Transport::default()).await
    }

    #[tokio::test]
    #[cfg(feature = "quic")]
    async fn test_quic_transport() -> Result<()> {
        test_transport(crate::quic::Transport::default()).await
    }

    #[tokio::test]
    #[cfg(feature = "tcp")]
    async fn test_tcp_transport() -> Result<()> {
        test_transport(crate::tcp::Transport::default()).await
    }

    #[tokio::test]
    #[cfg(feature = "dcquic")]
    async fn test_dcquic_transport() -> Result<()> {
        test_transport(
            crate::dcquic::Transport::new(crate::Config::testing())
                .await
                .unwrap(),
        )
        .await
    }

    // Additional edge case tests

    #[tokio::test]
    async fn test_empty_stream() -> Result<()> {
        upstream_testing::init_tracing();

        let transport = mem::Transport::default();
        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        let client = transport.create_client().await?;

        let server_task = tokio::spawn(async move {
            let (reader, mut writer, _) =
                server.accept().await.unwrap().into_parts().await.unwrap();

            // Read empty stream
            let stream = read_framed_stream::<u32, _>(reader, ByteVec::new()).await?;
            let mut stream = pin!(stream);

            // Should get no values
            assert!(stream.next().await.is_none());

            // Write back a response
            let mut out = Builder::default();
            42u32.serialize_with_len_prefix(&mut out);
            send_all(&mut writer, out.finish()).await?;

            Result::<_>::Ok(())
        });

        let (writer, mut reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;

        // Send empty stream
        let stream = stream::iter(Vec::<Result<u32>>::new());
        write_framed_stream(writer, stream, Builder::default()).await?;

        // Read response
        let mut buffer = ByteVec::new();
        let response: u32 = read_msg_with_len_prefix(&mut reader, &mut buffer).await?;
        assert_eq!(response, 42);

        server_task.await.unwrap()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_single_item_stream() -> Result<()> {
        upstream_testing::init_tracing();

        let transport = mem::Transport::default();
        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        let client = transport.create_client().await?;

        let server_task = tokio::spawn(async move {
            let (reader, mut writer, _) =
                server.accept().await.unwrap().into_parts().await.unwrap();

            let stream = read_framed_stream::<u32, _>(reader, ByteVec::new()).await?;
            let mut stream = pin!(stream);

            let value = stream.next().await.unwrap()?;
            assert_eq!(value, 123);
            assert!(stream.next().await.is_none());

            let mut out = Builder::default();
            value.serialize_with_len_prefix(&mut out);
            send_all(&mut writer, out.finish()).await?;

            Result::<_>::Ok(())
        });

        let (writer, mut reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;

        // Send single item
        let stream = stream::iter(vec![Ok(123u32)]);
        write_framed_stream(writer, stream, Builder::default()).await?;

        let mut buffer = ByteVec::new();
        let response: u32 = read_msg_with_len_prefix(&mut reader, &mut buffer).await?;
        assert_eq!(response, 123);

        server_task.await.unwrap()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_large_stream_batching() -> Result<()> {
        upstream_testing::init_tracing();

        let transport = mem::Transport::default();
        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        let client = transport.create_client().await?;

        let server_task = tokio::spawn(async move {
            let (reader, mut writer, _) =
                server.accept().await.unwrap().into_parts().await.unwrap();

            let stream = read_framed_stream::<Vec<u8>, _>(reader, ByteVec::new()).await?;
            let mut stream = pin!(stream);

            let mut count = 0;
            while let Some(value) = stream.next().await {
                let value = value?;
                assert_eq!(value.len(), 10000); // Each item is 10KB
                count += 1;
            }

            assert_eq!(count, 10); // Should receive all 10 items

            let mut out = Builder::default();
            count.serialize_with_len_prefix(&mut out);
            send_all(&mut writer, out.finish()).await?;

            Result::<_>::Ok(())
        });

        let (writer, mut reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;

        // Send 10 items of 10KB each (100KB total, exceeding STREAM_BATCH_SIZE)
        let large_data = vec![0u8; 10000];
        let items: Vec<_> = (0..10).map(|_| Ok(large_data.clone())).collect();
        let stream = stream::iter(items);
        write_framed_stream(writer, stream, Builder::default()).await?;

        let mut buffer = ByteVec::new();
        let count: u32 = read_msg_with_len_prefix(&mut reader, &mut buffer).await?;
        assert_eq!(count, 10);

        server_task.await.unwrap()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_stream_with_error_at_start() -> Result<()> {
        upstream_testing::init_tracing();

        let transport = mem::Transport::default();
        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        let client = transport.create_client().await?;

        let server_task = tokio::spawn(async move {
            let (reader, _writer, _) = server.accept().await.unwrap().into_parts().await.unwrap();

            let stream = read_framed_stream::<u32, _>(reader, ByteVec::new()).await?;
            let mut stream = pin!(stream);

            // Should get error
            let result = stream.next().await.unwrap();
            assert!(result.is_err());

            Result::<_>::Ok(())
        });

        let (writer, _reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;

        // Send stream with immediate error
        let stream = stream::iter(vec![Err::<u32, _>(Error::Io(io::Error::new(
            io::ErrorKind::Other,
            "test error",
        )))]);

        // this only returns an error if the reader is closed, not if the stream returns an error
        write_framed_stream(writer, stream, Builder::default()).await?;

        server_task.await.unwrap()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_stream_with_error_in_middle() -> Result<()> {
        upstream_testing::init_tracing();

        let transport = mem::Transport::default();
        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        let client = transport.create_client().await?;

        let server_task = tokio::spawn(async move {
            let (reader, _writer, _) = server.accept().await.unwrap().into_parts().await.unwrap();

            let stream = read_framed_stream::<u32, _>(reader, ByteVec::new()).await?;
            let mut stream = pin!(stream);

            // Should get first value
            let value = stream.next().await.unwrap()?;
            assert_eq!(value, 1);

            // Should get error
            let result = stream.next().await.unwrap();
            assert!(result.is_err());

            let result = stream.next().await;
            assert!(result.is_none());

            Result::<_>::Ok(())
        });

        let (writer, _reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;

        // Send stream with error in middle
        let stream = stream::iter(vec![
            Ok(1u32),
            Err(Error::Io(io::Error::new(
                io::ErrorKind::Other,
                "test error",
            ))),
            Ok(3u32), // This should never be sent
        ]);

        // this only returns an error if the reader is closed, not if the stream returns an error
        write_framed_stream(writer, stream, Builder::default()).await?;

        server_task.await.unwrap()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_read_partial_length_prefix() -> Result<()> {
        upstream_testing::init_tracing();

        let transport = mem::Transport::default();
        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        let client = transport.create_client().await?;

        let server_task = tokio::spawn(async move {
            let (mut reader, _writer, _) =
                server.accept().await.unwrap().into_parts().await.unwrap();

            let mut buffer = ByteVec::new();
            let result = read_len_prefix(&mut reader, &mut buffer).await?;
            Result::<_>::Ok(result)
        });

        let (mut writer, _reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;

        // Send length prefix byte by byte
        let mut out = Builder::default();
        42u64.serialize(&mut out);
        let mut bytes = out.finish();

        // Send data in single-byte chunks
        while let Ok(chunk) = bytes.split_to(1) {
            writer.write_full_chunk(chunk).await?;
            tokio::task::yield_now().await;
        }

        let result = server_task.await.unwrap()?;
        assert_eq!(result, Some(42));

        Ok(())
    }

    // TODO make the mem transport behave the same
    #[tokio::test]
    #[cfg(feature = "quic")]
    async fn test_read_empty_stream() -> Result<()> {
        upstream_testing::init_tracing();

        let transport = crate::quic::Transport::default();
        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        let client = transport.create_client().await?;

        let server_task = tokio::spawn(async move {
            let (mut reader, _writer, _) =
                server.accept().await.unwrap().into_parts().await.unwrap();

            let mut buffer = ByteVec::new();
            let result = read_len_prefix(&mut reader, &mut buffer).await?;
            Result::<_>::Ok(result)
        });

        let (writer, _reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;

        // Close the writer immediately to signal end of stream
        writer.finish().await?;

        let result = server_task.await.unwrap()?;
        assert_eq!(result, None);

        Ok(())
    }

    #[tokio::test]
    async fn test_write_with_initial_buffer_data() -> Result<()> {
        upstream_testing::init_tracing();

        let transport = mem::Transport::default();
        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        let client = transport.create_client().await?;

        let server_task = tokio::spawn(async move {
            let (reader, _writer, _) = server.accept().await.unwrap().into_parts().await.unwrap();

            let stream = read_framed_stream::<u32, _>(reader, ByteVec::new()).await?;
            let mut stream = pin!(stream);

            let mut values = vec![];
            while let Some(value) = stream.next().await {
                values.push(value?);
            }

            assert_eq!(values, vec![99, 1, 2, 3]);

            Result::<_>::Ok(())
        });

        let (writer, _reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;

        // Start with data already in the buffer
        let mut out = Builder::default();
        <Result<_>>::Ok(99u32).serialize_with_len_prefix(&mut out);

        let stream = stream::iter(vec![Ok(1u32), Ok(2u32), Ok(3u32)]);
        write_framed_stream(writer, stream, out).await?;

        server_task.await.unwrap()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_stream_bulk_transfer() -> Result<()> {
        upstream_testing::init_tracing();

        let transport = mem::Transport::default();
        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        let client = transport.create_client().await?;

        let chunk_size = 1024;
        let num_chunks = 1024;
        let expected_total = chunk_size * num_chunks;

        let server_task = tokio::spawn(async move {
            let (reader, _writer, _) = server.accept().await.unwrap().into_parts().await.unwrap();

            let stream = read_framed_stream::<Vec<u8>, _>(reader, ByteVec::new()).await?;
            let mut stream = pin!(stream);

            let mut total_bytes = 0;
            while let Some(value) = stream.next().await {
                let value = value?;
                total_bytes += value.len();
            }

            assert_eq!(total_bytes, expected_total);

            Result::<_>::Ok(())
        });

        let (writer, _reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;

        let chunk = vec![0u8; chunk_size];
        let chunk = bytes::Bytes::from(chunk);

        let items = (0..num_chunks).map(|_| Ok(chunk.clone()));
        let stream = stream::iter(items);
        write_framed_stream(writer, stream, Builder::default()).await?;

        server_task.await.unwrap()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_read_message_boundary_conditions() -> Result<()> {
        upstream_testing::init_tracing();

        let transport = mem::Transport::default();
        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        let client = transport.create_client().await?;

        let server_task = tokio::spawn(async move {
            let (mut reader, _writer, _) =
                server.accept().await.unwrap().into_parts().await.unwrap();

            let mut buffer = ByteVec::new();

            // First read the length prefix
            let len = read_len_prefix(&mut reader, &mut buffer).await?.unwrap();

            // Then read the message
            let msg: u32 = read_msg_with_len(&mut reader, &mut buffer, len).await?;

            Result::<_>::Ok(msg)
        });

        let (mut writer, _reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;

        // Send data in small chunks
        let mut out = Builder::default();
        12345u32.serialize_with_len_prefix(&mut out);
        let mut bytes = out.finish();

        // Send in 3 byte chunks
        while let Ok(chunk) = bytes.split_to(3) {
            writer.write_full_chunk(chunk).await?;
        }
        if !bytes.is_empty() {
            writer.write_full_chunk(bytes).await?;
        }

        let result = server_task.await.unwrap()?;
        assert_eq!(result, 12345);

        Ok(())
    }

    // ─── Version-aware framing tests ─────────────────────────────────────
    //
    // These verify that the `_at` variants of the framing helpers actually
    // dispatch through `serialize_at` / `deserialize_at` on a versioned
    // type. The streaming RPC code in `service/client.rs` and
    // `service/server.rs` relies on this — without it, streaming RPCs
    // would silently ignore negotiated method versions.

    /// A two-version wire type: v1 has just `a`, v2 adds `b: Option<u32>`.
    /// `default = None` makes v1 readers tolerate v2 bytes (and vice
    /// versa), but the serializer's choice of layout is observable: v1
    /// bytes are shorter than v2 bytes for a `b == Some(_)` value, and
    /// the deserialized `b` field reflects what was actually on the wire.
    #[derive(Debug, Clone, PartialEq, Eq, upstream_wire::Wire)]
    #[wire(crate = upstream_wire)]
    struct TwoVersionFrame {
        #[wire(tag = 1.1)]
        a: u64,
        #[wire(tag = 2.1, default = None)]
        b: Option<u32>,
    }

    // NOTE: the wire-layer semantics of `serialize_at` / `deserialize_at`
    // (per-version layouts, Result/Option propagation, byte-balance) are
    // covered in core-wire's `tests::versioned` module. The tests below
    // cover the TRANSPORT framing layer — `read_framed_stream_at`,
    // `write_framed_stream_at`, `read_msg_with_len_prefix_at` — over a real
    // mem transport, which is what the streaming RPC paths actually call.

    /// Single-frame v1↔v1 roundtrip via `read_msg_with_len_prefix_at` —
    /// writer and reader both use `version_idx = 0`. Asserts the v1 layout
    /// (which omits `b`) round-trips correctly. Single-frame so we don't
    /// rely on framing-buffer behavior across multiple frames; that's
    /// covered end-to-end by the cluster-level negotiation tests.
    #[tokio::test]
    async fn versioned_unary_v1_roundtrip() -> Result<()> {
        upstream_testing::init_tracing();

        let transport = mem::Transport::default();
        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        let client = transport.create_client().await?;

        const V1_IDX: u32 = 0;

        let server_task = tokio::spawn(async move {
            let (mut reader, _writer, _) =
                server.accept().await.unwrap().into_parts().await.unwrap();
            let mut buf = ByteVec::new();
            let msg: TwoVersionFrame =
                read_msg_with_len_prefix_at(&mut reader, &mut buf, Some(V1_IDX)).await?;
            Result::<_>::Ok(msg)
        });

        let (mut writer, _reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;
        let mut out = Builder::default();
        // Sender at v1: `b` would be dropped on the wire if it were Some.
        // Send it as None to make the test independent of forward-compat
        // tag-skipping in the deserializer.
        TwoVersionFrame { a: 42, b: None }.serialize_with_len_prefix_at(&mut out, V1_IDX);
        writer.write_full_chunk(out.finish()).await?;

        let got = server_task.await.unwrap()?;
        assert_eq!(got.a, 42);
        assert_eq!(got.b, None);
        Ok(())
    }

    /// Same as above, but at v2 — both writer and reader use the v2
    /// layout. `b` round-trips end-to-end. Single-frame to keep the test
    /// orthogonal to multi-frame buffer behavior.
    #[tokio::test]
    async fn versioned_unary_v2_roundtrip() -> Result<()> {
        upstream_testing::init_tracing();

        let transport = mem::Transport::default();
        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        let client = transport.create_client().await?;

        const V2_IDX: u32 = 1;

        let server_task = tokio::spawn(async move {
            let (mut reader, _writer, _) =
                server.accept().await.unwrap().into_parts().await.unwrap();
            let mut buf = ByteVec::new();
            let msg: TwoVersionFrame =
                read_msg_with_len_prefix_at(&mut reader, &mut buf, Some(V2_IDX)).await?;
            Result::<_>::Ok(msg)
        });

        let (mut writer, _reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;
        let mut out = Builder::default();
        TwoVersionFrame { a: 42, b: Some(7) }.serialize_with_len_prefix_at(&mut out, V2_IDX);
        writer.write_full_chunk(out.finish()).await?;

        let got = server_task.await.unwrap()?;
        assert_eq!(got.a, 42);
        assert_eq!(got.b, Some(7));
        Ok(())
    }

    /// Sanity: passing `None` to the `_at` helpers behaves identically to
    /// the non-`_at` versions. Guards the wrapper-equivalence claim made
    /// in the function docs.
    #[tokio::test]
    async fn _at_helpers_with_none_match_plain_helpers() -> Result<()> {
        upstream_testing::init_tracing();

        let transport = mem::Transport::default();
        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        let client = transport.create_client().await?;

        let server_task = tokio::spawn(async move {
            let (reader, _writer, _) = server.accept().await.unwrap().into_parts().await.unwrap();
            let stream = read_framed_stream_at::<u32, _, _, _>(
                reader,
                ByteVec::new(),
                None,
                throwaway_env(),
                (),
            )
            .await?;
            let mut stream = pin!(stream);
            let mut got = vec![];
            while let Some(value) = stream.next().await {
                got.push(value?);
            }
            Result::<_>::Ok(got)
        });

        let (writer, _reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;
        let stream = stream::iter(vec![Ok(1u32), Ok(2u32), Ok(3u32)]);
        write_framed_stream_at(
            writer,
            stream,
            Builder::default(),
            None,
            &throwaway_env(),
            (),
            None,
        )
        .await?;

        let got = server_task.await.unwrap()?;
        assert_eq!(got, vec![1, 2, 3]);
        Ok(())
    }

    // ─── How the framing layer actually decodes a frame ──────────────────
    //
    // KEY FACT that the next several tests rely on: `read_framed_stream_at::<T>`
    // returns `Stream<Item = Result<T>>`, and `stream::unfold` makes the
    // per-frame value the stream Item — so the type handed to
    // `read_msg_with_len_at` is `Result<T>`, NOT bare `T`. The writer also
    // serializes the stream item `Result<T>` (an `Ok`-tag byte + the inner
    // `T`). So BOTH sides go through `Result<T, _>`'s wire impl:
    //
    //   write:  Result<T>::serialize_with_len_prefix_at(idx)
    //   read:   Result<T>::deserialize_at(idx)
    //
    // This is why `Result<T, E>` MUST override BOTH `serialize_at` and
    // `deserialize_at` to forward `idx` to the inner `T`. If either override
    // is missing, the default falls through to the non-versioned path and
    // the inner `T` is (de)serialized at the current layout regardless of
    // the negotiated version — silently corrupting multi-frame streams.

    /// Documents the per-frame wire layout: framing serializes the stream
    /// item `Result<T>`, so each frame is `[u64 len][Ok-tag=0u8][T@idx]`.
    /// Decoding the SAME bytes as `Result<T>` round-trips; decoding as bare
    /// `T` (skipping the Ok-tag) would shift every byte and corrupt the value.
    #[test]
    fn frame_item_is_result_t_not_bare_t() {
        // Frame a Result<u32> exactly as write_framed_stream does.
        let mut out = Builder::default();
        let item: Result<u32> = Ok(0x0102_0304);
        item.serialize_with_len_prefix(&mut out);
        let mut framed = out.finish();

        // Peel the u64 length prefix (what read_len_prefix does).
        let inner_len = u64::deserialize(&mut framed).unwrap() as usize;
        assert_eq!(
            inner_len, 5,
            "inner frame = 1 Ok-tag byte + 4 u32 bytes = 5; if this is 4 the writer is \
             not wrapping in Result and the read side must match"
        );

        // Decoding as Result<u32> (what read_msg_with_len_at actually does)
        // recovers the value exactly and consumes the whole frame.
        let decoded = <Result<u32>>::deserialize(&mut framed).unwrap().unwrap();
        assert_eq!(decoded, 0x0102_0304);
        assert_eq!(
            framed.len(),
            0,
            "Result<u32> decode consumes the full frame"
        );
    }

    /// The bug this whole fix closes: a MULTI-frame v1 stream. Before
    /// `Result<T,E>::deserialize_at` was added, the writer wrote v1 frames
    /// (8-byte `a`, no `b`) but `Result<TwoVersionFrame>::deserialize_at`
    /// fell through to the default → `TwoVersionFrame::deserialize` (v2),
    /// which tried to read the absent `b` field, over-read into the next
    /// frame's length prefix, and failed with `InvalidLength { ty: u64 }` on
    /// the SECOND frame. Single-frame tests cannot catch this — the desync
    /// only manifests when a second frame must be read from the shared buffer.
    #[tokio::test]
    async fn multiframe_v1_stream_roundtrip() -> Result<()> {
        upstream_testing::init_tracing();
        const V1_IDX: u32 = 0;

        let transport = mem::Transport::default();
        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        let client = transport.create_client().await?;

        let server_task = tokio::spawn(async move {
            let (reader, _writer, _) = server.accept().await.unwrap().into_parts().await.unwrap();
            let stream = read_framed_stream_at::<TwoVersionFrame, _, _, _>(
                reader,
                ByteVec::new(),
                Some(V1_IDX),
                throwaway_env(),
                (),
            )
            .await?;
            let mut stream = pin!(stream);
            let mut got = vec![];
            while let Some(value) = stream.next().await {
                got.push(value?);
            }
            Result::<_>::Ok(got)
        });

        let (writer, _reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;
        // 4 frames so a 1-byte desync on frame N corrupts frame N+1.
        let items: Vec<_> = (0..4)
            .map(|i| {
                Ok(TwoVersionFrame {
                    a: 0xAABB_CCDD_0000_0000 + i as u64,
                    b: None, // v1 layout: b is not on the wire
                })
            })
            .collect();
        write_framed_stream_at(
            writer,
            stream::iter(items),
            Builder::default(),
            Some(V1_IDX),
            &throwaway_env(),
            (),
            None,
        )
        .await?;

        let got = server_task.await.unwrap()?;
        assert_eq!(got.len(), 4, "all 4 v1 frames must decode without desync");
        for (i, f) in got.iter().enumerate() {
            assert_eq!(f.a, 0xAABB_CCDD_0000_0000 + i as u64, "frame {i} a");
            assert_eq!(f.b, None, "frame {i} b stays None at v1");
        }
        Ok(())
    }

    /// Multi-frame v2 stream — the `b` field IS on the wire and must
    /// round-trip on every frame.
    #[tokio::test]
    async fn multiframe_v2_stream_roundtrip() -> Result<()> {
        upstream_testing::init_tracing();
        const V2_IDX: u32 = 1;

        let transport = mem::Transport::default();
        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        let client = transport.create_client().await?;

        let server_task = tokio::spawn(async move {
            let (reader, _writer, _) = server.accept().await.unwrap().into_parts().await.unwrap();
            let stream = read_framed_stream_at::<TwoVersionFrame, _, _, _>(
                reader,
                ByteVec::new(),
                Some(V2_IDX),
                throwaway_env(),
                (),
            )
            .await?;
            let mut stream = pin!(stream);
            let mut got = vec![];
            while let Some(value) = stream.next().await {
                got.push(value?);
            }
            Result::<_>::Ok(got)
        });

        let (writer, _reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;
        let items: Vec<_> = (0..4)
            .map(|i| {
                Ok(TwoVersionFrame {
                    a: i as u64,
                    b: Some(1000 + i as u32),
                })
            })
            .collect();
        write_framed_stream_at(
            writer,
            stream::iter(items),
            Builder::default(),
            Some(V2_IDX),
            &throwaway_env(),
            (),
            None,
        )
        .await?;

        let got = server_task.await.unwrap()?;
        assert_eq!(got.len(), 4);
        for (i, f) in got.iter().enumerate() {
            assert_eq!(f.a, i as u64, "frame {i} a");
            assert_eq!(f.b, Some(1000 + i as u32), "frame {i} b");
        }
        Ok(())
    }

    // ─── Per-frame instrumentation hooks ─────────────────────────────────
    //
    // The bidi paxos-read metrics ride on the `WriteInstrument` /
    // `ReadInstrument` hooks threaded through the framing layer. These verify
    // the framing loop actually drives those hooks once per frame on both the
    // write (serialize + transmit) and read (receive + deserialize) sides — the
    // wiring the generated `Bidi` span depends on to emit anything at all.

    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(Clone, Default)]
    struct WriteCounts {
        serialize: Arc<AtomicUsize>,
        transmit: Arc<AtomicUsize>,
    }

    impl WriteInstrument<u32> for WriteCounts {
        fn on_item_serialize<E: upstream_runtime::Env>(
            &mut self,
            _env: &E,
            _item: &u32,
        ) -> impl Span<usize> {
            self.serialize.fetch_add(1, Ordering::Relaxed);
        }

        fn on_chunk_transmit<E: upstream_runtime::Env>(
            &mut self,
            _env: &E,
            _chunk_size: usize,
        ) -> impl Span<Result<(), Error>> {
            self.transmit.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[derive(Clone, Default)]
    struct ReadCounts {
        receive: Arc<AtomicUsize>,
        deserialize: Arc<AtomicUsize>,
    }

    impl ReadInstrument<u32> for ReadCounts {
        fn on_chunk_receive<E: upstream_runtime::Env>(
            &mut self,
            _env: &E,
        ) -> impl Span<Result<usize, Error>> {
            self.receive.fetch_add(1, Ordering::Relaxed);
        }

        fn on_item_deserialize<E: upstream_runtime::Env>(
            &mut self,
            _env: &E,
            _chunk_size: usize,
        ) -> impl Span<Result<u32, Error>> {
            self.deserialize.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The framing loop drives the write instrument's `on_item_serialize` once
    /// per item and the read instrument's `on_item_deserialize` once per frame,
    /// with at least one transmit and receive. This is the hook contract the
    /// generated bidi span relies on to emit per-frame metrics.
    #[tokio::test]
    async fn framing_drives_instrument_hooks_per_frame() -> Result<()> {
        upstream_testing::init_tracing();

        let transport = mem::Transport::default();
        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        let client = transport.create_client().await?;

        let read_counts = ReadCounts::default();
        let server_read = read_counts.clone();
        let server_task = tokio::spawn(async move {
            let (reader, _writer, _) = server.accept().await.unwrap().into_parts().await.unwrap();
            let stream = read_framed_stream_at::<u32, _, _, _>(
                reader,
                ByteVec::new(),
                None,
                throwaway_env(),
                server_read,
            )
            .await?;
            let mut stream = pin!(stream);
            let mut got = vec![];
            while let Some(value) = stream.next().await {
                got.push(value?);
            }
            Result::<_>::Ok(got)
        });

        let (writer, _reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;
        let write_counts = WriteCounts::default();
        let stream = stream::iter(vec![Ok(1u32), Ok(2u32), Ok(3u32)]);
        write_framed_stream_at(
            writer,
            stream,
            Builder::default(),
            None,
            &throwaway_env(),
            write_counts.clone(),
            None,
        )
        .await?;

        let got = server_task.await.unwrap()?;
        assert_eq!(got, vec![1, 2, 3]);

        // One serialize per item written.
        assert_eq!(
            write_counts.serialize.load(Ordering::Relaxed),
            3,
            "on_item_serialize once per item"
        );
        // At least one flush transmitted the buffered frames.
        assert!(
            write_counts.transmit.load(Ordering::Relaxed) >= 1,
            "on_chunk_transmit at least once"
        );
        // One deserialize per frame received.
        assert_eq!(
            read_counts.deserialize.load(Ordering::Relaxed),
            3,
            "on_item_deserialize once per frame"
        );
        // Receive is polled at least once per frame (plus the terminal poll).
        assert!(
            read_counts.receive.load(Ordering::Relaxed) >= 3,
            "on_chunk_receive at least once per frame"
        );
        Ok(())
    }
}
