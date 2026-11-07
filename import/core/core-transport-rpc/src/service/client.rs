use super::common::{
    ReadInstrument, WriteInstrument, read_framed_stream_at, read_len_prefix, read_msg_with_len_at,
    read_msg_with_len_prefix, read_to_end, write_framed_stream_at,
};
use crate::{
    Client, Reader, Writer,
    negotiation::{Handshake as _, MethodId, PeerAddr, ServiceId},
};
use bytes::Bytes;
use upstream_bytevec::{Builder, ByteVec};
use upstream_runtime::{Env, Error, Result};
use upstream_service::{
    Methods, RequestStream, ResponseStream, ServerAddr, Service, Span, SupportedSet,
    consumer::{self, Method, Peer, Registry, SupportedRegistry, span::SetupSpan as _},
    stream,
};
use upstream_wire::{Deserialize, Serialize};
use std::future::Future;

pub struct MethodHandle<C, S>
where
    C: Client,
    S: Service,
{
    client: C,
    method: S::Methods,
    supported: bool,
    addr: ServerAddr,
    /// The negotiated version entry for this method, if the peer negotiated
    /// a non-current version. `None` means the method uses the current
    /// (latest) version — the default path with zero overhead.
    negotiated_version: Option<upstream_service::MethodVersionEntry>,
}

impl<C, S> Clone for MethodHandle<C, S>
where
    C: Client,
    S: Service,
{
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            method: self.method.clone(),
            supported: self.supported,
            addr: self.addr,
            negotiated_version: self.negotiated_version,
        }
    }
}

impl<C, S> MethodHandle<C, S>
where
    C: Client,
    S: Service,
{
    /// Writes the `[trace::Header][global_id][optional ReqH]` prefix that
    /// every streaming-input RPC sends before the first chunk. Returns the
    /// buffer plus the `input_version_idx` to thread into the framing
    /// layer so subsequent chunks serialize at the negotiated version.
    ///
    /// When `negotiated_version` is `None` (no peer registry, or the peer
    /// supports the current version), writes the current `global_id` and
    /// returns `None` for the version idx — the framing layer falls
    /// through to plain `serialize_with_len_prefix`.
    fn write_stream_request_header<E: Env, ReqH>(
        &self,
        env: &E,
        header: Option<ReqH>,
    ) -> (Builder, Option<u32>)
    where
        ReqH: consumer::RequestHeader<S>,
    {
        let mut out = Builder::default();
        env.trace_header().serialize(&mut out);
        if let Some(ref entry) = self.negotiated_version {
            entry.global_id.serialize(&mut out);
        } else {
            self.method.info().global_id.serialize(&mut out);
        }
        if let Some(header) = header {
            header.serialize_with_len_prefix(&mut out);
        }
        (out, self.negotiated_version.map(|e| e.input_version_idx))
    }
}

/// Bridges the framing write pump's [`WriteInstrument`] to a consumer
/// bidi-stream request span half. The request half writes `Req` frames, so its
/// `on_item_serialize`/`on_chunk_transmit` map directly onto the pump's
/// per-frame serialize/transmit legs.
struct WriteBidiRequest<Sp>(Sp);

impl<Req, Sp> WriteInstrument<Req> for WriteBidiRequest<Sp>
where
    Sp: consumer::span::bidi::Request<Req>,
{
    fn on_item_serialize<E: Env>(
        &mut self,
        env: &E,
        item: &Req,
    ) -> impl upstream_service::Span<usize> {
        self.0.on_item_serialize(env, item)
    }

    fn on_chunk_transmit<E: Env>(
        &mut self,
        env: &E,
        chunk_size: usize,
    ) -> impl upstream_service::Span<Result<(), Error>> {
        self.0.on_chunk_transmit(env, chunk_size)
    }
}

/// Bridges the framing write pump's [`WriteInstrument`] to a consumer
/// `StreamRequest` span (streamed request → unary response). Unlike bidi, the
/// span is a single un-split value driven inline, so this borrows `&mut span`
/// (the framing loop runs to completion before the span is used for the unary
/// response). `WriteInstrument` only requires `Send`, so the borrow is fine.
/// `Res` is carried in `PhantomData` because the span trait is parameterized by
/// it even though the write-side hooks don't use it.
struct WriteStreamRequest<'a, Sp, Res>(&'a mut Sp, core::marker::PhantomData<fn() -> Res>);

impl<Req, Res, Sp> WriteInstrument<Req> for WriteStreamRequest<'_, Sp, Res>
where
    Res: Send,
    Sp: consumer::span::StreamRequest<Req, Res>,
{
    fn on_item_serialize<E: Env>(
        &mut self,
        env: &E,
        item: &Req,
    ) -> impl upstream_service::Span<usize> {
        self.0.on_item_serialize(env, item)
    }

    fn on_chunk_transmit<E: Env>(
        &mut self,
        env: &E,
        chunk_size: usize,
    ) -> impl upstream_service::Span<Result<(), Error>> {
        self.0.on_chunk_transmit(env, chunk_size)
    }
}

/// Bridges the framing reader's [`ReadInstrument`] to a consumer bidi-stream
/// response span half. The response half reads `Res` frames, so its
/// `on_chunk_receive`/`on_item_deserialize` map onto the reader's per-frame
/// receive/deserialize legs.
struct ReadBidiResponse<Sp>(Sp);

impl<Res, Sp> ReadInstrument<Res> for ReadBidiResponse<Sp>
where
    Sp: consumer::span::bidi::Response<Res>,
{
    fn on_chunk_receive<E: Env>(
        &mut self,
        env: &E,
    ) -> impl upstream_service::Span<Result<usize, Error>> {
        self.0.on_chunk_receive(env)
    }

    fn on_item_deserialize<E: Env>(
        &mut self,
        env: &E,
        chunk_size: usize,
    ) -> impl upstream_service::Span<Result<Res, Error>> {
        self.0.on_item_deserialize(env, chunk_size)
    }
}

#[derive(Clone, Debug)]
pub struct RawRequest(ByteVec);

impl upstream_service::consumer::RawRequest for RawRequest {
    fn serialized_len(&self) -> usize {
        self.0.len()
    }
}

/// Decode a unary `Result<(ResH, Res)>` response, applying the negotiated
/// output `version_idx` to the body `Res` only.
///
/// Mirrors the server's `write_with_header` split. The joint
/// `<Result<(ResH, Res)>>::deserialize_at` would route the index through the
/// tuple's default `deserialize_at`, which does NOT forward it to `Res`
/// (tuples carry no per-field version mapping). Headers are never versioned,
/// so we decode the `Ok`/`Err` tag, then the header at the current layout,
/// then the body at `version_idx`. With `None` this is byte-for-byte the old
/// joint decode.
fn deserialize_unary_with_header_at<ResH, Res>(
    bytes: &mut ByteVec,
    version_idx: Option<u32>,
) -> Result<(ResH, Res)>
where
    ResH: Deserialize,
    Res: Deserialize,
{
    let Some(idx) = version_idx else {
        return <Result<(ResH, Res)>>::deserialize(bytes)
            .map_err(Error::Wire)
            .and_then(|v| v);
    };

    // `Ok`(0)/`Err`(1) tag matches `Result<(ResH, Res)>::serialize`.
    let tag = u8::deserialize(bytes).map_err(Error::Wire)?;
    match tag {
        0 => {
            let header = ResH::deserialize(bytes).map_err(Error::Wire)?;
            let response = Res::deserialize_at(bytes, idx).map_err(Error::Wire)?;
            Ok((header, response))
        }
        // Errors are not version-negotiated.
        1 => Err(Error::deserialize(bytes).map_err(Error::Wire)?),
        _ => Err(Error::Wire(upstream_wire::de::Error::new(
            upstream_wire::de::ErrorKind::InvalidVariant,
            upstream_wire::type_of::<ResH>(),
        ))),
    }
}

/// Reads a length-prefixed response stream off the wire, instrumenting the
/// streaming-response span per frame and exiting it when the stream drains.
///
/// This is the consumer mirror of the server's `write_stream` path: each frame
/// is timed as a receive step (`on_chunk_receive`, the wire read) followed by a
/// decode step (`on_item_deserialize`, sized by the frame). The main `span`
/// outlives the setup and is exited exactly once — with success after a clean
/// end-of-stream, or with the first error encountered. Frames decode at the
/// negotiated `version_idx`; each is a `Result<Res>` (`read_msg_with_len_at`
/// forwards the index to the inner `Ok(Res)`), matching `read_framed_stream_at`.
///
/// Both `send_stream_response` and its with-header sibling funnel through here,
/// so the per-frame instrumentation and the end-of-stream exit live in one
/// place rather than being copy-pasted per method and per transport.
fn read_response_stream<E, Req, Res, Sp, R>(
    env: E,
    span: Sp,
    reader: R,
    in_buf: ByteVec,
    version_idx: Option<u32>,
) -> impl ResponseStream<Res>
where
    E: Env,
    Res: Deserialize + Send + 'static,
    Sp: consumer::span::StreamResponse<Req, Res> + Send + 'static,
    R: Reader,
{
    stream::unfold(
        (env, Some(span), Some(reader), in_buf, version_idx),
        move |(env, mut span, reader, mut in_buf, version_idx)| async move {
            // `reader == None` is the terminal state: a prior frame errored, so
            // the span is already exited and we stop yielding.
            let mut reader = reader?;

            // Receive step: buffer enough bytes for the next length-prefixed
            // frame. `on_chunk_receive` times the wire read.
            let receive_span = span.as_mut().map(|s| s.on_chunk_receive(&env));
            let frame_len = match read_len_prefix(&mut reader, &mut in_buf).await {
                Ok(Some(len)) => {
                    receive_span.exit(&env, &Ok(len));
                    len
                }
                Ok(None) => {
                    // Clean end-of-stream. Drop the receive sub-span WITHOUT
                    // exiting it: this poll read no frame, so recording a
                    // zero-byte `receive_chunk.duration` sample would skew the
                    // distribution with one near-zero entry per stream. The
                    // sub-span's only side effect is in `exit`, so dropping it
                    // records nothing. The main span still exits as a success.
                    drop(receive_span);
                    if let Some(span) = span.take() {
                        span.exit(&env, &Ok(()));
                    }
                    return None;
                }
                Err(err) => {
                    // The byte read itself failed: fail the receive sub-span
                    // and the main span with the same (moved) error, yield it,
                    // and terminate (reader dropped → None next round).
                    let err = consumer::span::exit_err(&env, receive_span, err);
                    let err = match span.take() {
                        Some(span) => consumer::span::exit_err::<_, _, ()>(&env, span, err),
                        None => err,
                    };
                    return Some((Err(err), (env, None, None, in_buf, version_idx)));
                }
            };

            // Deserialize step: decode the buffered frame as `Result<Res>` at
            // the negotiated version. `on_item_deserialize` is sized by the
            // frame and records the decode outcome.
            let deserialize_span = span
                .as_mut()
                .map(|s| s.on_item_deserialize(&env, frame_len));
            let item = read_msg_with_len_at::<Result<Res>>(
                &mut reader,
                &mut in_buf,
                frame_len,
                version_idx,
            )
            .await
            .and_then(|v| v);
            deserialize_span.exit(&env, &item);

            match item {
                Ok(value) => Some((Ok(value), (env, span, Some(reader), in_buf, version_idx))),
                Err(err) => {
                    // Decode failure ends the stream: fail the main span and
                    // forward the error, then terminate.
                    let err = match span.take() {
                        Some(span) => consumer::span::exit_err::<_, _, ()>(&env, span, err),
                        None => err,
                    };
                    Some((Err(err), (env, None, None, in_buf, version_idx)))
                }
            }
        },
    )
}

impl<C, S> Method<S> for MethodHandle<C, S>
where
    C: Client,
    S: Service,
{
    type RawRequest = RawRequest;

    fn is_supported(&self) -> bool {
        self.supported
    }

    fn create_raw_request<E: Env, H, I>(
        &self,
        env: &E,
        header: Option<H>,
        input: I,
    ) -> Self::RawRequest
    where
        H: consumer::RequestHeader<S>,
        I: consumer::Request<S>,
    {
        let mut out = Builder::default();

        // Write trace header
        env.trace_header().serialize(&mut out);

        // Write method ID — use the negotiated global_id if present, otherwise the current one
        if let Some(ref entry) = self.negotiated_version {
            entry.global_id.serialize(&mut out);
        } else {
            self.method.info().global_id.serialize(&mut out);
        }

        // Write request header and input — use version-aware serializer if negotiated
        if let Some(header) = header {
            if let Some(ref entry) = self.negotiated_version {
                header.serialize(&mut out);
                input.serialize_at(&mut out, entry.input_version_idx);
            } else {
                (header, input).serialize(&mut out);
            }
        } else if let Some(ref entry) = self.negotiated_version {
            input.serialize_at(&mut out, entry.input_version_idx);
        } else {
            input.serialize(&mut out);
        }

        RawRequest(out.finish())
    }

    fn send<E: Env, Req, Res, Span>(
        &self,
        env: &E,
        request: Self::RawRequest,
        mut span: Span,
    ) -> impl Future<Output = Result<Res>> + Send
    where
        Res: consumer::Response<S>,
        Span: consumer::span::Unary<Req, Res>,
    {
        let client = self.client.clone();
        let addr = self.addr;
        let priority = self.method.info().priority;
        let output_version_idx = self.negotiated_version.map(|e| e.output_version_idx);
        async move {
            let req_span = span.on_execute(env, request.0.len());
            let response = client.request_response(addr, request.0, priority).await;

            let mut response = match response {
                Ok(response) => {
                    req_span.exit(env, &Ok(response.len()));
                    response
                }
                Err(err) => {
                    let res = <Result<usize>>::Err(err.into());
                    req_span.exit(env, &res);
                    let res = unsafe { Err(res.unwrap_err_unchecked()) };
                    span.exit(env, &res);
                    res?;
                    unreachable!()
                }
            };

            let response = {
                let deserialize_span = span.on_deserialize(env, response.len());
                // Decode at the negotiated output version (if any).
                // `Result<Res>::deserialize_at` forwards the index to the
                // inner `Ok(Res)`; errors are never version-negotiated.
                let response = match output_version_idx {
                    Some(idx) => <Result<Res>>::deserialize_at(&mut response, idx),
                    None => <Result<Res>>::deserialize(&mut response),
                }
                .map_err(Error::Wire)
                .and_then(|v| v);
                deserialize_span.exit(env, &response);
                response
            };

            span.exit(env, &response);

            response
        }
    }

    fn send_unidirectional<E: Env, Req, Span>(
        &self,
        env: &E,
        request: Self::RawRequest,
        mut span: Span,
    ) -> impl Future<Output = Result> + Send
    where
        Span: consumer::span::Unidirectional<Req>,
    {
        let client = self.client.clone();
        let addr = self.addr;
        let priority = self.method.info().priority;
        async move {
            let tx_span = span.on_transmit(env, request.0.len());
            let (writer, _reader) = client.connect(addr, priority).await?;
            let res = writer
                .write_to_end(request.0)
                .await
                .map(|_| ())
                .map_err(Error::Io);
            tx_span.exit(env, &res);
            span.exit(env, &res);
            res
        }
    }

    fn send_with_response_header<E: Env, Req, ResH, Res, Span>(
        &self,
        env: &E,
        request: Self::RawRequest,
        mut span: Span,
    ) -> impl Future<Output = Result<(ResH, Res)>> + Send
    where
        ResH: consumer::ResponseHeader<S>,
        Res: consumer::Response<S>,
        Span: consumer::span::Unary<Req, (ResH, Res)>,
    {
        let client = self.client.clone();
        let addr = self.addr;
        let priority = self.method.info().priority;
        let output_version_idx = self.negotiated_version.map(|e| e.output_version_idx);
        async move {
            let req_span = span.on_execute(env, request.0.len());
            let response = client.request_response(addr, request.0, priority).await;

            let mut response = match response {
                Ok(response) => {
                    req_span.exit(env, &Ok(response.len()));
                    response
                }
                Err(err) => {
                    let res = <Result<usize>>::Err(err.into());
                    req_span.exit(env, &res);
                    let res = unsafe { Err(res.unwrap_err_unchecked()) };
                    span.exit(env, &res);
                    res?;
                    unreachable!()
                }
            };

            let response = {
                let deserialize_span = span.on_deserialize(env, response.len());
                // The header is decoded at the current layout; only the body
                // `Res` honors the negotiated output version.
                let response = deserialize_unary_with_header_at::<ResH, Res>(
                    &mut response,
                    output_version_idx,
                );
                deserialize_span.exit(env, &response);
                response
            };

            span.exit(env, &response);

            response
        }
    }

    fn send_stream_bidi<T, E: Env, ReqH, Req, Res, Span>(
        self,
        env: &E,
        header: Option<ReqH>,
        input: impl RequestStream<Req>,
        mut span: Span,
    ) -> impl Future<Output = Result<impl ResponseStream<Res>>> + Send + 'static
    where
        T: upstream_runtime::task_monitor::TaskClass<E> + Default,
        ReqH: consumer::RequestHeader<S>,
        Req: consumer::Request<S>,
        Res: consumer::Response<S>,
        Span: consumer::span::bidi::Stream<Req, Res>,
    {
        let env = env.clone();
        async move {
            // Write header (trace + negotiated-or-current global_id)
            let (out, version_idx) = self.write_stream_request_header(&env, header);
            let output_version_idx = self.negotiated_version.map(|e| e.output_version_idx);
            let priority = self.method.info().priority;

            // Time the connect leg on the main span.
            let connect_span = span.on_connect(&env);
            let (writer, reader) = match self.client.connect(self.addr, priority).await {
                Ok(pair) => {
                    connect_span.exit(&env, &Ok(()));
                    pair
                }
                Err(e) => {
                    let e = consumer::span::exit_err(&env, connect_span, Error::Io(e));
                    return Err(consumer::span::exit_err::<_, _, ()>(&env, span, e));
                }
            };

            // Split the main span into request (write) and response (read)
            // halves so each leg instruments its own frames.
            let (req_span, res_span) = span.split();

            // Spawn the request-write pump under the per-method client task class
            // `T` (supplied by the generated consumer, where the crate publisher
            // is in scope), so its scheduling/poll metrics attribute to this
            // method on the client side. `WriteBidiRequest` bridges the pump's
            // per-frame serialize/transmit to the request-side span.
            let pump_env = env.clone();
            let _join_handle = env.spawn(T::default(), async move {
                let _ = write_framed_stream_at(
                    writer,
                    input,
                    out,
                    version_idx,
                    &pump_env,
                    WriteBidiRequest(req_span),
                    None,
                )
                .await;
            });

            // Response stream decoded at the negotiated output version. Each
            // frame is a `Result<Res>`; `read_framed_stream_at` forwards the
            // index through to the inner `Ok(Res)`. The response-side span
            // instruments per-frame receive/deserialize.
            read_framed_stream_at(
                reader,
                ByteVec::default(),
                output_version_idx,
                env,
                ReadBidiResponse(res_span),
            )
            .await
        }
    }

    fn send_stream_bidi_with_response_header<T, E: Env, ReqH, Req, ResH, Res, Span>(
        self,
        env: &E,
        header: Option<ReqH>,
        input: impl RequestStream<Req>,
        mut span: Span,
    ) -> impl Future<Output = Result<(ResH, impl ResponseStream<Res>)>> + Send + 'static
    where
        T: upstream_runtime::task_monitor::TaskClass<E> + Default,
        ReqH: consumer::RequestHeader<S>,
        Req: consumer::Request<S>,
        ResH: consumer::ResponseHeader<S>,
        Res: consumer::Response<S>,
        Span: consumer::span::bidi::Stream<Req, Res>,
    {
        let env = env.clone();
        async move {
            let (out, version_idx) = self.write_stream_request_header(&env, header);
            let output_version_idx = self.negotiated_version.map(|e| e.output_version_idx);
            let priority = self.method.info().priority;

            // Time the connect leg on the main span.
            let connect_span = span.on_connect(&env);
            let (writer, mut reader) = match self.client.connect(self.addr, priority).await {
                Ok(pair) => {
                    connect_span.exit(&env, &Ok(()));
                    pair
                }
                Err(e) => {
                    let e = consumer::span::exit_err(&env, connect_span, Error::Io(e));
                    return Err(consumer::span::exit_err::<_, _, ()>(&env, span, e));
                }
            };

            let (req_span, res_span) = span.split();

            // Spawn the request-write pump under the per-method client task class
            // `T` (supplied by the generated consumer, where the crate publisher
            // is in scope), so its scheduling/poll metrics attribute to this
            // method on the client side.
            let pump_env = env.clone();
            let _join_handle = env.spawn(T::default(), async move {
                let _ = write_framed_stream_at(
                    writer,
                    input,
                    out,
                    version_idx,
                    &pump_env,
                    WriteBidiRequest(req_span),
                    None,
                )
                .await;
            });

            // First read the response header (always at the current layout —
            // headers are not version-negotiated).
            let mut in_buf = ByteVec::default();
            let header =
                read_msg_with_len_prefix::<Result<ResH>>(&mut reader, &mut in_buf).await??;

            // Then read the response stream at the negotiated output version.
            let response_stream = read_framed_stream_at(
                reader,
                in_buf,
                output_version_idx,
                env,
                ReadBidiResponse(res_span),
            )
            .await?;

            Ok((header, response_stream))
        }
    }

    fn send_stream_request<E: Env, ReqH, Req, Res, Span>(
        &self,
        env: &E,
        header: Option<ReqH>,
        input: impl RequestStream<Req>,
        mut span: Span,
    ) -> impl Future<Output = Result<Res>> + Send
    where
        ReqH: consumer::RequestHeader<S>,
        Req: consumer::Request<S>,
        Res: consumer::Response<S>,
        Span: consumer::span::StreamRequest<Req, Res>,
    {
        // Clone self so the header (which captures env.trace_header()) can be
        // built inside the async block — sampling the trace at first poll
        // matches the _bidi siblings and the pre-diff behavior.
        let this = self.clone();
        let env = env.clone();
        async move {
            let (out, version_idx) = this.write_stream_request_header(&env, header);
            let output_version_idx = this.negotiated_version.map(|e| e.output_version_idx);
            let priority = this.method.info().priority;

            // Time the connect leg on the main span.
            let connect_span = span.on_connect(&env);
            let (writer, reader) = match this.client.connect(this.addr, priority).await {
                Ok(pair) => {
                    connect_span.exit(&env, &Ok(()));
                    pair
                }
                Err(e) => {
                    let e = consumer::span::exit_err(&env, connect_span, Error::Io(e));
                    return Err(consumer::span::exit_err::<_, _, Res>(&env, span, e));
                }
            };

            // Write the request stream, instrumenting per-frame serialize +
            // transmit via a borrowing bridge (the pump runs inline, so `span`
            // is free again once it returns). A write error fails the main span.
            let write = write_framed_stream_at(
                writer,
                input,
                out,
                version_idx,
                &env,
                WriteStreamRequest(&mut span, core::marker::PhantomData),
                None,
            )
            .await;
            if let Err(e) = write {
                return Err(consumer::span::exit_err::<_, _, Res>(&env, span, e));
            }

            // Read the unary response at the negotiated output version.
            let mut response = ByteVec::new();
            let receive_span = span.on_receive(&env);
            let recv = read_to_end(reader, &mut response)
                .await
                .map(|_| response.len())
                .map_err(Error::Io);
            receive_span.exit(&env, &recv);
            if let Err(e) = recv {
                return Err(consumer::span::exit_err::<_, _, Res>(&env, span, e));
            }

            let deserialize_span = span.on_deserialize(&env, response.len());
            let response = match output_version_idx {
                Some(idx) => <Result<Res>>::deserialize_at(&mut response, idx),
                None => <Result<Res>>::deserialize(&mut response),
            }
            .map_err(Error::Wire)
            .and_then(|v| v);
            deserialize_span.exit(&env, &response);

            span.exit(&env, &response);
            response
        }
    }

    fn send_stream_request_with_response_header<E: Env, ReqH, Req, ResH, Res, Span>(
        &self,
        env: &E,
        header: Option<ReqH>,
        input: impl RequestStream<Req>,
        mut span: Span,
    ) -> impl Future<Output = Result<(ResH, Res)>> + Send
    where
        ReqH: consumer::RequestHeader<S>,
        Req: consumer::Request<S>,
        ResH: consumer::ResponseHeader<S>,
        Res: consumer::Response<S>,
        Span: consumer::span::StreamRequest<Req, (ResH, Res)>,
    {
        let this = self.clone();
        let env = env.clone();
        async move {
            let (out, version_idx) = this.write_stream_request_header(&env, header);
            let output_version_idx = this.negotiated_version.map(|e| e.output_version_idx);
            let priority = this.method.info().priority;

            let connect_span = span.on_connect(&env);
            let (writer, reader) = match this.client.connect(this.addr, priority).await {
                Ok(pair) => {
                    connect_span.exit(&env, &Ok(()));
                    pair
                }
                Err(e) => {
                    let e = consumer::span::exit_err(&env, connect_span, Error::Io(e));
                    return Err(consumer::span::exit_err::<_, _, (ResH, Res)>(&env, span, e));
                }
            };

            // Write the request stream, instrumenting per-frame serialize +
            // transmit. The span's response type is `(ResH, Res)`.
            let write = write_framed_stream_at(
                writer,
                input,
                out,
                version_idx,
                &env,
                WriteStreamRequest(&mut span, core::marker::PhantomData),
                None,
            )
            .await;
            if let Err(e) = write {
                return Err(consumer::span::exit_err::<_, _, (ResH, Res)>(&env, span, e));
            }

            // Read the unary response header + body. The header is at the
            // current layout; only the body honors the negotiated version.
            let mut response = ByteVec::new();
            let receive_span = span.on_receive(&env);
            let recv = read_to_end(reader, &mut response)
                .await
                .map(|_| response.len())
                .map_err(Error::Io);
            receive_span.exit(&env, &recv);
            if let Err(e) = recv {
                return Err(consumer::span::exit_err::<_, _, (ResH, Res)>(&env, span, e));
            }

            let deserialize_span = span.on_deserialize(&env, response.len());
            let decoded =
                deserialize_unary_with_header_at::<ResH, Res>(&mut response, output_version_idx);
            deserialize_span.exit(&env, &decoded);

            span.exit(&env, &decoded);
            decoded
        }
    }

    fn send_stream_response<E: Env, Req, Res, Span>(
        self,
        env: &E,
        request: Self::RawRequest,
        mut span: Span,
    ) -> impl Future<Output = Result<impl ResponseStream<Res>>> + Send + 'static
    where
        Res: consumer::Response<S>,
        Span: consumer::span::StreamResponse<Req, Res>,
    {
        let env = env.clone();
        async move {
            let output_version_idx = self.negotiated_version.map(|e| e.output_version_idx);
            let priority = self.method.info().priority;

            // Setup: connect + send the request. `on_transmit` times this
            // single step; the main span lives on until the stream drains.
            let request_size = request.0.len();
            let transmit_span = span.on_transmit(&env, request_size);
            let setup = async {
                let (writer, reader) = self.client.connect(self.addr, priority).await?;
                // Request already serialized at the negotiated input version
                // by create_raw_request.
                writer.write_to_end(request.0).await.map_err(Error::Io)?;
                Ok(reader)
            }
            .await;
            let reader = transmit_span.exit_with(&env, setup);
            let (span, reader) = span.keep_or_exit(&env, reader)?;

            // Response stream decoded at the negotiated output version, with
            // the main span exited when it drains.
            Ok(read_response_stream::<E, Req, Res, _, _>(
                env,
                span,
                reader,
                ByteVec::default(),
                output_version_idx,
            ))
        }
    }

    fn send_stream_response_with_response_header<E: Env, Req, ResH, Res, Span>(
        self,
        env: &E,
        request: Self::RawRequest,
        mut span: Span,
    ) -> impl Future<Output = Result<(ResH, impl ResponseStream<Res>)>> + Send + 'static
    where
        ResH: consumer::ResponseHeader<S>,
        Res: consumer::Response<S>,
        Span: consumer::span::StreamResponse<Req, Res>,
    {
        let env = env.clone();
        async move {
            let output_version_idx = self.negotiated_version.map(|e| e.output_version_idx);
            let priority = self.method.info().priority;

            // Setup: connect + send the request. `on_transmit` times the
            // request transmission; the header read that follows is part of
            // setup (a failure fails the main span) but the streamed body's
            // drain is what finally exits it.
            let transmit_span = span.on_transmit(&env, request.0.len());
            let connect = async {
                let (writer, reader) = self.client.connect(self.addr, priority).await?;
                // Request already serialized at the negotiated input version
                // by create_raw_request.
                writer.write_to_end(request.0).await.map_err(Error::Io)?;
                Ok(reader)
            }
            .await;
            let reader = transmit_span.exit_with(&env, connect);

            // Read response header (always at the current layout — headers are
            // not version-negotiated), then gate the streaming phase on it.
            let setup = async {
                let mut reader = reader?;
                let mut in_buf = ByteVec::default();
                let header =
                    read_msg_with_len_prefix::<Result<ResH>>(&mut reader, &mut in_buf).await??;
                Ok((header, reader, in_buf))
            }
            .await;
            let (span, (header, reader, in_buf)) = span.keep_or_exit(&env, setup)?;

            // Response stream decoded at the negotiated output version, with
            // the main span exited when it drains.
            let response_stream = read_response_stream::<E, Req, Res, _, _>(
                env,
                span,
                reader,
                in_buf,
                output_version_idx,
            );
            Ok((header, response_stream))
        }
    }

    fn send_raw<E: Env, Req, Res, Span>(
        &self,
        env: &E,
        request: Self::RawRequest,
        response: Res,
        mut span: Span,
    ) -> impl Future<Output = Result<Res::Output>> + Send
    where
        Res: upstream_service::rpc_response::Response + Send,
        Res::Storage: Send,
        Res::Output: Send,
        Span: consumer::span::Unary<Req, usize>,
    {
        let client = self.client.clone();
        let addr = self.addr;
        let priority = self.method.info().priority;
        async move {
            let req_span = span.on_execute(env, request.0.len());
            let result = client
                .request_response_into(addr, request.0, response, priority)
                .await;

            match result {
                Ok(output) => {
                    req_span.exit(env, &Ok(0usize));
                    span.exit(env, &Ok(0usize));
                    Ok(output)
                }
                Err(err) => {
                    let res = <Result<usize>>::Err(err.into());
                    req_span.exit(env, &res);
                    let res = unsafe { Err(res.unwrap_err_unchecked()) };
                    span.exit(env, &res);
                    res?;
                    unreachable!()
                }
            }
        }
    }
}

pub struct ClientPeer<C, S>
where
    C: Client,
    S: Service,
{
    client: C,
    supported_set: S::SupportedSet,
    addr: ServerAddr,
}

impl<C, S> Peer<S> for ClientPeer<C, S>
where
    C: Client,
    S: Service,
{
    type Method = MethodHandle<C, S>;

    fn resolve(&self, method: S::Methods) -> Self::Method {
        let negotiated_version = self.supported_set.version_entry(&method);
        MethodHandle {
            client: self.client.clone(),
            method,
            supported: self.supported_set.contains(&method),
            addr: self.addr,
            negotiated_version,
        }
    }
}

pub struct ClientRegistry<C, R = TransportNegotiated<C>>
where
    C: Client,
{
    client: C,
    supported_registry: R,
}

impl<C> ClientRegistry<C, TransportNegotiated<C>>
where
    C: Client,
{
    /// Build a registry that derives per-peer negotiated versions from the
    /// client's own transport handshake (`client.handshake().peer_registry()`).
    /// This is the default for negotiated consumers — there is no externally
    /// supplied registry to thread; the transport is the single source.
    pub fn from_transport(client: C) -> Self {
        let supported_registry = TransportNegotiated::new(client.clone());
        Self {
            client,
            supported_registry,
        }
    }
}

impl<C, R> ClientRegistry<C, R>
where
    C: Client,
{
    /// Build a registry with an explicit [`SupportedRegistry`] (e.g.
    /// [`StaticSupportedRegistry`](consumer::StaticSupportedRegistry)) for
    /// consumers that pin a fixed `SupportedSet` rather than deriving it from
    /// the transport handshake.
    ///
    /// Negotiated consumers should prefer [`Self::from_transport`].
    pub fn new(client: C, supported_registry: R) -> Self {
        Self {
            client,
            supported_registry,
        }
    }
}

impl<C, R, S> Registry<S, ServerAddr> for ClientRegistry<C, R>
where
    C: Client,
    R: SupportedRegistry<S, ServerAddr>,
    S: Service,
{
    type Peer = ClientPeer<C, S>;

    async fn resolve_peer(&self, peer_addr: &ServerAddr) -> Result<Self::Peer> {
        let supported_set = self
            .supported_registry
            .resolve_supported_set(peer_addr)
            .await?;
        // Service discovery: if the peer advertised the address service `S`
        // accepts connections on (in its negotiated `PeerInfo`), dial that
        // instead of the supplied `peer_addr`. Falls back to `peer_addr` when
        // nothing is advertised — so callers that already know the address
        // (e.g. from gossip) are unaffected, and old peers that don't advertise
        // service addresses keep working. The peer's stable handshake identity
        // is preserved so negotiation lookups still key consistently.
        let addr = resolve_service_addr::<C, S>(&self.client, peer_addr).unwrap_or(*peer_addr);
        Ok(ClientPeer {
            client: self.client.clone(),
            supported_set,
            addr,
        })
    }
}

/// Resolve the address to dial for service `S` from the peer's advertised
/// service map (`PeerInfo.services[S].addrs`), sourced from the client's own
/// transport handshake registry. Returns `None` when there is no handshake /
/// registry, the peer is absent, or the peer advertised no socket address for
/// `S` — in which case the caller falls back to the dialed `ServerAddr`.
fn resolve_service_addr<C, S>(client: &C, peer_addr: &ServerAddr) -> Option<ServerAddr>
where
    C: Client,
    S: Service,
{
    let registry = client.handshake().and_then(|hs| hs.peer_registry())?;
    let pa = PeerAddr::new(peer_addr.ip(), peer_addr.handshake_port);
    let service_id = ServiceId::for_service::<S>();
    let addrs = registry.resolve_service(&pa, &service_id)?;
    addrs.into_iter().find_map(|a| match a {
        crate::negotiation::ServiceAddr::Socket { ip, port } => Some(ServerAddr {
            ip,
            acceptor_port: port,
            // Keep the peer's stable handshake identity for negotiation lookups.
            handshake_port: peer_addr.handshake_port,
        }),
        // Non-socket (fabric) addresses aren't dialable as a `ServerAddr`.
        crate::negotiation::ServiceAddr::Fabric { .. } => None,
    })
}

impl<C, R> Clone for ClientRegistry<C, R>
where
    C: Client,
    R: Clone,
{
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
            supported_registry: self.supported_registry.clone(),
        }
    }
}

// ─── SupportedRegistry Implementations ────────────────────────────────────────

/// A per-peer-aware [`SupportedRegistry`] that sources the negotiated state
/// from the client's **own transport handshake** —
/// `client.handshake().peer_registry()`. The transport that built the
/// handshake owns that registry, so there is no separate registry to thread
/// down from the daemon; the consumer derives it from the transport it talks
/// over.
///
/// For peers present in the registry: builds a `SupportedSet` from their
/// negotiated method versions. When there is no handshake / no peer registry,
/// or the peer is absent: returns `SupportedSet::all()` (current-version
/// dispatch, backward compatible with peers that never negotiated).
pub struct TransportNegotiated<C> {
    client: C,
}

impl<C> TransportNegotiated<C> {
    /// Wrap a client so per-peer `SupportedSet` resolution is sourced from the
    /// client's own transport handshake (`client.handshake().peer_registry()`).
    pub fn new(client: C) -> Self {
        Self { client }
    }
}

impl<C: Clone> Clone for TransportNegotiated<C> {
    fn clone(&self) -> Self {
        Self {
            client: self.client.clone(),
        }
    }
}

impl<C, S> SupportedRegistry<S, ServerAddr> for TransportNegotiated<C>
where
    C: Client,
    S: Service,
    S::SupportedSet: SupportedSet,
{
    fn resolve_supported_set(
        &self,
        peer_addr: &ServerAddr,
    ) -> impl Future<Output = core::result::Result<S::SupportedSet, Error>> + Send {
        let peer_addr = *peer_addr;
        // Source the registry from the transport's handshake. The borrow is
        // released before the async block (we only clone out the cheap `Arc`).
        let peer_registry = self.client.handshake().and_then(|hs| hs.peer_registry());

        async move {
            let Some(peer_registry) = peer_registry else {
                // No handshake / negotiation disabled — current-version dispatch.
                return Ok(S::SupportedSet::all());
            };
            let service_id = ServiceId::for_service::<S>();

            let pa = PeerAddr::new(peer_addr.ip(), peer_addr.handshake_port);
            if !peer_registry.has_peer(&pa) {
                // Peer never negotiated — old binary. Fall back to Set::all()
                // with current-version serializers.
                return Ok(S::SupportedSet::all());
            }

            // Build global_id list from negotiated method versions.
            let mut global_ids = Vec::new();
            for method in S::methods() {
                let method_id = MethodId(Bytes::copy_from_slice(method.name.as_bytes()));
                if let Some(version_id) =
                    peer_registry.negotiated_version(&pa, &service_id, &method_id)
                {
                    // Parse u64 global_id from VersionId bytes (stored as big-endian u64).
                    if version_id.0.len() >= 8 {
                        let id =
                            u64::from_be_bytes(version_id.0[..8].try_into().unwrap_or_default());
                        global_ids.push(id);
                    }
                }
            }

            // `has_peer == true` but no method on THIS service has a
            // negotiated version. Three real cases produce this state:
            //   1. Service mismatch — peer advertises a different service
            //      list, so `set_peer_info` fired on its side but the loop
            //      that calls `set_negotiated` for OUR service never ran.
            //   2. Race — first RPC dispatched between `set_peer_info`
            //      and the per-service `set_negotiated` loop in
            //      `RpcVersionNegotiator::negotiate`.
            //   3. No common version — `negotiate` returned `None`
            //      because no method has any common global_id.
            // Feeding `from_negotiated(methods, &[])` here would mark
            // every method `Unsupported`. That's byte-safe today
            // because `MethodHandle::create_raw_request` falls through
            // to the current global_id either way, but it conflates
            // these three "we don't know what the peer speaks" states
            // with a legitimate "peer offers nothing" outcome. Fall
            // back to `Set::all()` to match the `has_peer == false`
            // branch's semantics — same current-version dispatch,
            // explicit "unknown" status.
            if global_ids.is_empty() {
                return Ok(S::SupportedSet::all());
            }

            Ok(S::SupportedSet::from_negotiated(S::methods(), &global_ids))
        }
    }
}
