use super::{
    common::{
        ReadInstrument, WriteInstrument, read_framed_stream_at, read_msg_with_len_prefix,
        read_to_end, read_until, write_framed_stream_at,
    },
    stall::{Progress, StallPolicy, StallTimeout},
};
use crate::{PendingStream, Reader, Server, Writer};
use upstream_bytevec::{Builder, ByteVec};
use upstream_runtime::{Env, Error, Result, trace};
use upstream_service::{
    GlobalId, Methods as _, RequestStream, ResponseStream, Service, Span,
    provider::{self, Acceptor, RequestProvider, ResponseProvider},
};
use upstream_wire::{Deserialize, Serialize};
use std::{io, mem::size_of, net::SocketAddr, sync::Arc};
use tracing::{Instrument, info_span};

/// Bridges the framing reader's [`ReadInstrument`] to a provider
/// streaming-request span. The provider reads `Req` frames, so its
/// `on_chunk_receive`/`on_chunk_deserialize` map onto the reader's per-frame
/// receive/deserialize legs. Every frame also counts as progress of the
/// response, when it runs under a stall bound.
struct ReadStreamRequest<Sp> {
    span: Sp,
    progress: Option<Arc<Progress>>,
}

impl<Req, Sp> ReadInstrument<Req> for ReadStreamRequest<Sp>
where
    Sp: provider::span::StreamingRequest<Req> + Send,
{
    fn on_chunk_receive<E: Env>(&mut self, env: &E) -> impl Span<Result<usize, Error>> {
        self.span.on_chunk_receive(env)
    }

    fn on_item_deserialize<E: Env>(
        &mut self,
        env: &E,
        chunk_size: usize,
    ) -> impl Span<Result<Req, Error>> {
        if let Some(progress) = &self.progress {
            progress.bump();
        }
        self.span.on_chunk_deserialize(env, chunk_size)
    }
}

/// Bridges the framing writer's [`WriteInstrument`] to a provider
/// streaming-response span. The provider writes `Res` frames; its
/// `on_item_serialize` takes no item (unlike the consumer side), so the item
/// ref is dropped, and `on_item_transmit` maps onto the writer's transmit leg.
struct WriteStreamResponse<Sp>(Sp);

impl<Res, Sp> WriteInstrument<Res> for WriteStreamResponse<Sp>
where
    Sp: provider::span::StreamingResponse<Res> + Send,
{
    fn on_item_serialize<E: Env>(&mut self, env: &E, _item: &Res) -> impl Span<usize> {
        self.0.on_item_serialize(env)
    }

    fn on_chunk_transmit<E: Env>(
        &mut self,
        env: &E,
        chunk_size: usize,
    ) -> impl Span<Result<(), Error>> {
        self.0.on_item_transmit(env, chunk_size)
    }
}

pub struct ServerAcceptor<S, A, E>
where
    S: Server,
    A: Acceptor<E, SocketAddr> + Send + Sync + 'static,
    E: Env + Clone + Send + 'static,
{
    server: S,
    acceptor: Arc<A>,
    env: E,
    stall: Option<StallTimeout>,
}

impl<S, A, E> ServerAcceptor<S, A, E>
where
    S: Server,
    A: Acceptor<E, SocketAddr> + Send + Sync + 'static,
    E: Env + Clone + Send + 'static,
{
    pub fn new(server: S, acceptor: A, env: E) -> Self {
        Self {
            server,
            acceptor: Arc::new(acceptor),
            env,
            stall: None,
        }
    }

    /// Ends every streamed response that makes none of the progress [`StallPolicy`]
    /// defines for `timeout`: its body is dropped at the bound, a pending write
    /// completes and what it left goes out with no further bound, then one error
    /// frame ends the stream.
    pub fn response_stall_timeout(mut self, timeout: StallTimeout) -> Self {
        self.stall = Some(timeout);
        self
    }

    pub async fn run(mut self) {
        // TODO race this with each acceptor level
        let _while_open = self.acceptor.while_open();

        let mut id = 0u64;

        // The accept loop only exits when `accept()` returns `None`
        // (listener closed). Per-stream failures (validation timeout,
        // negotiation) live entirely inside the spawned task's
        // `into_parts()` and cannot stall or kill this loop.
        while let Some(stream) = self.server.accept().await {
            let peer_addr = stream.remote_addr();
            let task = {
                let acceptor = self.acceptor.clone();
                let env = self.env.clone();
                let stall = self.stall;
                async move {
                    if let Err(error) =
                        handle_server_connection(stream, acceptor, env, stall).await
                    {
                        tracing::error!(
                            %error,
                            namespaces = ?upstream_debug::fmt::from_iter(|| <A::Service as Service>::namespaces()),
                            "error processing request"
                        );
                    }
                }
                // `trace_id` and `method` are declared as Empty here and
                // populated by `handle_server_connection` once the wire
                // `trace::Header` and the method id have been parsed.
                // Pre-declaring as span fields means CloudWatch sees them on
                // every event/child span emitted for this request.
                .instrument(info_span!(
                    "request",
                    %peer_addr,
                    request_id = id,
                    trace_id = tracing::field::Empty,
                    parent_span = tracing::field::Empty,
                    method = tracing::field::Empty,
                ))
            };

            id += 1;

            #[cfg(test)]
            assert_static_send(&task);

            // Spawn the connection task with the acceptor's per-service task
            // class (the generated `Instance` overrides `spawn_connection` to use
            // `<Service>Service`); the transport layer itself names no crate
            // `Publisher`, so `E` here stays an unbounded `Env`.
            let _ = self.acceptor.spawn_connection(&self.env, task);
        }
    }
}

#[cfg(test)]
fn assert_static_send<T: 'static + Send>(_: &T) {}

async fn handle_server_connection<S, A, E>(
    stream: S,
    acceptor: Arc<A>,
    env: E,
    stall: Option<StallTimeout>,
) -> Result
where
    S: PendingStream,
    A: Acceptor<E, SocketAddr>,
    E: Env,
{
    // Validation and negotiation run here, off the accept hot path.
    // `into_parts()` handles both stream validation (e.g., dcquic) and
    // inline negotiate (e.g., TCP magic-byte exchange) internally.
    let (mut reader, writer, peer_addr) = stream.into_parts().await?;

    let Some(mut in_buf) = read_request_header(&mut reader).await? else {
        tracing::debug!("peer closed before sending a request");
        return Ok(());
    };

    let trace = Deserialize::deserialize(&mut in_buf)?;
    let method_id = GlobalId::deserialize(&mut in_buf)?;

    // Create environment with the trace
    let env = env.accept_trace(trace);

    // Now that we know the trace id, populate the `trace_id` field on the
    // surrounding `request` span (declared as Empty above) so all events and
    // child spans emitted while processing this RPC are queryable by trace.
    let span = tracing::Span::current();
    span.record("trace_id", tracing::field::display(env.current_trace()));
    span.record("parent_span", tracing::field::display(env.parent_span()));

    // Look up the method by its global ID
    let method =
        match <<A as Acceptor<E, SocketAddr>>::Service as Service>::parse_global_id(method_id) {
            Some(method) => method,
            None => {
                return Err(Error::Io(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Method not supported: {}", method_id),
                )));
            }
        };
    span.record("method", tracing::field::display(method.info().name));

    // If the wire global_id differs from the method's current global_id, the
    // client sent a non-current layout. Look up the MethodVersionEntry to get
    // the negotiated (input, output) version indices: the request body is
    // deserialized at `input_version_idx`, and the response is serialized at
    // `output_version_idx` so the client decodes it at the same layout.
    let version_entry = if method_id != method.info().global_id {
        method.info().versions.get(method_id)
    } else {
        None
    };
    let input_version_idx = version_entry.map(|entry| entry.input_version_idx);
    let output_version_idx = version_entry.map(|entry| entry.output_version_idx);

    // Create request and response providers. Only a streamed response runs under
    // the stall bound, and only a streamed request feeds it frames.
    let flags = &method.info().flags;
    let stall = stall
        .filter(|_| flags.stream_response)
        .map(|timeout| StallPolicy {
            timeout,
            method: method.info().name,
            progress: flags.stream_request.then(Arc::default),
        });
    let req_provider = RequestContext {
        reader,
        in_buf,
        version_idx: input_version_idx,
        progress: stall.as_ref().and_then(|policy| policy.progress.clone()),
    };
    let res_provider = ResponseContext {
        writer,
        version_idx: output_version_idx,
        stall,
    };

    // Dispatch the request to the acceptor
    acceptor
        .dispatch(&env, &peer_addr, method, req_provider, res_provider)
        .await;

    Ok(())
}

/// Reads the wire header (trace + method id). Returns `Ok(None)` on EOF or
/// `ConnectionReset` before any header byte arrived.
pub(crate) async fn read_request_header(reader: &mut impl Reader) -> Result<Option<ByteVec>> {
    let header_len = trace::Header::WIRE_SIZE + size_of::<GlobalId>();
    let mut in_buf = ByteVec::with_capacity(header_len);
    let result = read_until(reader, &mut in_buf, |in_buf| in_buf.len() >= header_len).await;

    if in_buf.is_empty() {
        match &result {
            Ok(()) => return Ok(None),
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => return Ok(None),
            Err(_) => {}
        }
    }
    result?;

    if in_buf.len() < header_len {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "could not read wire header - expected {header_len}, got {}",
                in_buf.len()
            ),
        )
        .into());
    }

    Ok(Some(in_buf))
}

struct RequestContext<R: Reader> {
    reader: R,
    in_buf: ByteVec,
    /// If present, the client sent a non-current version layout. The server
    /// uses this index to call `Req::deserialize_at(bytes, version_idx)` instead
    /// of `Req::deserialize(bytes)`, selecting the correct monolithic inlined
    /// deserializer for the negotiated wire layout.
    version_idx: Option<u32>,
    /// The response's stall progress, when it runs under a stall bound.
    progress: Option<Arc<Progress>>,
}

impl<R, Svc> RequestProvider<Svc> for RequestContext<R>
where
    R: Reader,
    Svc: Service,
{
    async fn read<E, Req, Span>(self, env: &E, span: &mut Span) -> Result<Req, Error>
    where
        E: Env,
        Req: provider::Request<Svc>,
        Span: provider::span::FullRequest<Req>,
    {
        let mut in_buf = self.in_buf;

        let read_span = span.on_receive(env);
        let result = read_to_end(self.reader, &mut in_buf).await;
        let result = result.map(|_| in_buf.len()).map_err(Error::Io);
        read_span.exit(env, &result);
        result?;

        let deserialize_span = span.on_deserialize(env, in_buf.len());
        let value = if let Some(idx) = self.version_idx {
            Req::deserialize_at(&mut in_buf, idx)
        } else {
            Req::deserialize(&mut in_buf)
        };
        let value = value.map_err(Error::Wire);
        deserialize_span.exit(env, &value);

        value
    }

    async fn read_with_header<E, ReqH, Req, Span>(
        self,
        env: &E,
        span: &mut Span,
    ) -> Result<(ReqH, Req), Error>
    where
        E: Env,
        ReqH: provider::RequestHeader<Svc>,
        Req: provider::Request<Svc>,
        Span: provider::span::FullRequest<(ReqH, Req)>,
    {
        let mut in_buf = self.in_buf;
        let read_span = span.on_receive(env);
        let result = read_to_end(self.reader, &mut in_buf).await;
        let result = result.map(|_| in_buf.len()).map_err(Error::Io);
        read_span.exit(env, &result);

        // Decode the header and request body separately so the versioned
        // body can dispatch via `Req::deserialize_at(idx)` while the header
        // (always at the current layout) uses plain `deserialize`. The
        // earlier `<(ReqH, Req)>::deserialize` would silently use the tuple
        // default and skip version-aware decoding for `Req`.
        let deserialize_span = span.on_deserialize(env, in_buf.len());
        let value = ReqH::deserialize(&mut in_buf).and_then(|header| {
            let req = match self.version_idx {
                Some(idx) => Req::deserialize_at(&mut in_buf, idx)?,
                None => Req::deserialize(&mut in_buf)?,
            };
            Ok((header, req))
        });
        let value = value.map_err(Error::Wire);
        deserialize_span.exit(env, &value);

        value
    }

    async fn read_stream<E, Req, I>(
        self,
        env: &E,
        instrument: I,
    ) -> Result<impl RequestStream<Req>, Error>
    where
        E: Env,
        Req: provider::Request<Svc>,
        I: provider::span::StreamingRequest<Req> + 'static,
    {
        // Each chunk decodes at the negotiated input version when set;
        // `ReadStreamRequest` times per-frame receive + deserialize on the span.
        read_framed_stream_at(
            self.reader,
            self.in_buf,
            self.version_idx,
            env.clone(),
            ReadStreamRequest {
                span: instrument,
                progress: self.progress,
            },
        )
        .await
    }

    async fn read_stream_with_header<E, H, Req, I>(
        self,
        env: &E,
        instrument: I,
    ) -> Result<(H, impl RequestStream<Req>), Error>
    where
        E: Env,
        H: provider::RequestHeader<Svc>,
        Req: provider::Request<Svc>,
        I: provider::span::StreamingRequest<Req> + 'static,
    {
        // First read the header (always at the current layout — headers are
        // not versioned through `MethodVersionEntry`).
        let mut reader = self.reader;
        let mut in_buf = self.in_buf;
        let header = read_msg_with_len_prefix(&mut reader, &mut in_buf).await?;

        // Then read the stream of requests at the negotiated input version.
        let request_stream = read_framed_stream_at(
            reader,
            in_buf,
            self.version_idx,
            env.clone(),
            ReadStreamRequest {
                span: instrument,
                progress: self.progress,
            },
        )
        .await?;

        Ok((header, request_stream))
    }
}

struct ResponseContext<W: Writer> {
    writer: W,
    /// If present, the negotiated output layout differs from the current
    /// version. The server serializes the response via
    /// `ResponseType::serialize_at(bytes, version_idx)` so the client (which
    /// negotiated the same global_id) can decode it at the matching layout.
    version_idx: Option<u32>,
    /// Bound on a streamed response's time without progress; `None` is unbounded.
    stall: Option<StallPolicy>,
}

impl<W, Svc> ResponseProvider<Svc> for ResponseContext<W>
where
    W: Writer,
    Svc: Service,
{
    async fn finish_unidirectional<E, Sp>(self, env: &E, span: Sp)
    where
        E: Env,
        Sp: Span<Result>,
    {
        // For unidirectional methods, we don't send anything
        let result = self.writer.finish().await.map_err(Error::Io);

        span.exit(env, &result);
    }

    async fn write<E, Res, Span>(self, env: &E, response: Res, mut span: Span)
    where
        E: Env,
        Res: provider::Response<Svc>,
        Span: provider::span::FullResponse<Res>,
    {
        // Serialize the response at the negotiated output version (or current
        // if none was negotiated). `Result<T, E>::serialize_at` forwards the
        // index to the inner `Ok(T)`.
        let serialize_span = span.on_serialize(env);
        let mut out = Builder::default();
        match self.version_idx {
            Some(idx) => <Result<_>>::Ok(response).serialize_at(&mut out, idx),
            None => <Result<_>>::Ok(response).serialize(&mut out),
        }
        let response = out.finish();
        serialize_span.exit(env, &response.len());

        // Write the response
        let transmit_span = span.on_transmit(env, response.len());
        let result = self.writer.write_to_end(response).await;
        let result = result.map(|_| ()).map_err(Error::Io);
        transmit_span.exit(env, &result);

        span.exit(env, &result);
    }

    async fn write_with_header<E, ResH, Res, Span>(
        self,
        env: &E,
        header: ResH,
        response: Res,
        mut span: Span,
    ) where
        E: Env,
        ResH: provider::ResponseHeader<Svc>,
        Res: provider::Response<Svc>,
        Span: provider::span::FullResponse<(ResH, Res)>,
    {
        // Serialize header and response. The header is never versioned, but
        // the response body is, so encode them separately: the joint
        // `(ResH, Res)::serialize` (and its tuple `serialize_at` default)
        // would not forward the version index to `Res`. We write the same
        // wire bytes as the tuple path — `Ok` tag, then header, then body —
        // but the body goes through `serialize_at` when negotiated.
        let serialize_span = span.on_serialize(env);
        let mut out = Builder::default();
        match self.version_idx {
            Some(idx) => {
                // `Ok` tag (0u8) matches `Result<(ResH, Res)>::serialize`,
                // then header (current layout), then body at the negotiated
                // output version.
                0u8.serialize(&mut out);
                header.serialize(&mut out);
                response.serialize_at(&mut out, idx);
            }
            None => <Result<_>>::Ok((header, response)).serialize(&mut out),
        }
        let response = out.finish();
        serialize_span.exit(env, &response.len());

        // Write the response
        let transmit_span = span.on_transmit(env, response.len());
        let result = self.writer.write_to_end(response).await;
        let result = result.map(|_| ()).map_err(Error::Io);
        transmit_span.exit(env, &result);

        span.exit(env, &result);
    }

    async fn write_raw<E, Res, Span>(self, env: &E, response: Res, mut span: Span)
    where
        E: Env,
        Res: upstream_buffer::reader::storage::Infallible + Send,
        Span: provider::span::FullResponse<usize>,
    {
        // Write the raw response directly
        let transmit_span = span.on_transmit(env, response.buffered_len());
        let result = self.writer.write_to_end(response).await;
        let result = result.map(|_| ()).map_err(Error::Io);
        transmit_span.exit(env, &result);

        span.exit(env, &result);
    }

    async fn write_stream<E, Res, Sp>(
        self,
        env: &E,
        response_stream: impl ResponseStream<Res>,
        mut span: Sp,
    ) where
        E: Env,
        Res: provider::Response<Svc>,
        Sp: provider::span::StreamingResponse<Res>,
    {
        // Each frame is serialized at the negotiated output version (the
        // framing wraps every item as `Result<Res>`, whose `serialize_at`
        // forwards the index to the inner `Ok(Res)`). `WriteStreamResponse`
        // borrows the span to time per-frame serialize + transmit; the whole-
        // stream `span` is then exited with the drain result.
        let frames = span.response_frames();
        let result = write_framed_stream_at(
            self.writer,
            response_stream,
            Builder::default(),
            self.version_idx,
            env,
            WriteStreamResponse(frames),
            self.stall,
        )
        .await;
        span.exit(env, &result);
    }

    async fn write_stream_with_header<E, ResH, Res, Sp>(
        self,
        env: &E,
        header: ResH,
        response_stream: impl ResponseStream<Res>,
        mut span: Sp,
    ) where
        E: Env,
        ResH: provider::ResponseHeader<Svc>,
        Res: provider::Response<Svc>,
        Sp: provider::span::StreamingResponse<Res>,
    {
        // Write the header first. Headers are never versioned, so this
        // always uses the current layout.
        let mut out = Builder::default();
        <Result<_>>::Ok(header).serialize_with_len_prefix(&mut out);

        // Then write the stream at the negotiated output version - the stream
        // will try to write any stream items that are ready as well.
        // `WriteStreamResponse` wraps an owned per-frame writer for timing.
        let frames = span.response_frames();
        let result = write_framed_stream_at(
            self.writer,
            response_stream,
            out,
            self.version_idx,
            env,
            WriteStreamResponse(frames),
            self.stall,
        )
        .await;
        span.exit(env, &result);
    }

    async fn write_error<E, Sp>(self, env: &E, err: Error, span: Sp)
    where
        E: Env,
        Sp: Span<Result<(), Error>>,
    {
        // Serialize the error response
        let mut out = Builder::default();
        <Result>::Err(err).serialize(&mut out);
        let response = out.finish();

        // Write the error response
        let result = self.writer.write_to_end(response).await;
        let result = result.map(|_| ()).map_err(Error::Io);
        span.exit(env, &result);
    }

    async fn write_stream_header_error<E, Sp>(self, env: &E, err: Error, span: Sp)
    where
        E: Env,
        Sp: Span<Result<(), Error>>,
    {
        // For streaming responses with headers, we need to use length-prefix serialization
        // to match the wire format of success responses (write_stream_with_header)
        let mut out = Builder::default();
        <Result>::Err(err).serialize_with_len_prefix(&mut out);
        let response = out.finish();

        let result = self.writer.write_to_end(response).await;
        let result = result.map(|_| ()).map_err(Error::Io);
        span.exit(env, &result);
    }

    async fn write_raw_error<E, Sp>(self, env: &E, err: Error, span: Sp)
    where
        E: Env,
        Sp: Span<Result<(), Error>>,
    {
        // TODO add support for raw error responses in the transport trait
        span.exit(env, &Err(err));
    }
}

#[cfg(test)]
mod tests {
    use super::{Progress, ReadStreamRequest};
    use crate::{
        Client as _, PendingStream as _, Rpc as _, Server as _, mem,
        service::common::{read_framed_stream_at, throwaway_env, write_framed_stream},
    };
    use upstream_bytevec::{Builder, ByteVec};
    use upstream_runtime::Result;
    use upstream_service::stream::{self, StreamExt as _};
    use std::{pin::pin, sync::Arc};

    #[tokio::test]
    async fn every_request_frame_read_counts_as_response_progress() -> Result<()> {
        let transport = mem::Transport::default();
        let mut server = transport.create_server("[::1]:0".parse().unwrap()).await?;
        let addr = server.local_addr();
        let client = transport.create_client().await?;
        let progress = Arc::new(Progress::default());

        let server_task = tokio::spawn({
            let progress = progress.clone();
            async move {
                let (reader, _writer, _) = server.accept().await.unwrap().into_parts().await?;
                let frames = read_framed_stream_at::<u32, _, _, _>(
                    reader,
                    ByteVec::new(),
                    None,
                    throwaway_env(),
                    ReadStreamRequest {
                        span: (),
                        progress: Some(progress.clone()),
                    },
                )
                .await?;
                let mut frames = pin!(frames);
                let mut read = vec![];
                while let Some(frame) = frames.next().await {
                    read.push((frame?, progress.count()));
                }
                Result::<_>::Ok(read)
            }
        });

        let (writer, _reader) = client
            .connect(addr, upstream_service::Priority::default())
            .await?;
        write_framed_stream(
            writer,
            stream::iter([Ok(1u32), Ok(2), Ok(3)]),
            Builder::default(),
        )
        .await?;

        assert_eq!(server_task.await.unwrap()?, vec![(1, 1), (2, 2), (3, 3)]);
        Ok(())
    }
}
