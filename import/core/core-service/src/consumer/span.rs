// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use upstream_runtime::{Env, Error};

/// Exit `span` recording a failure and hand the error back, unchanged.
///
/// This is the one primitive that makes streaming-span instrumentation clean.
/// A span's [`exit`](crate::Span::exit) borrows its result, but [`Error`] is not
/// `Clone`, so the same error often has to drive several exits (a sub-span, the
/// main span) and still be returned to the caller. `exit_err` moves the error
/// into a typed `Result` to borrow it for one `exit`, then moves it back out —
/// no clone, no allocation. It is generic over the span's result type `T`, so
/// it works for the `usize` receive sub-span, the `Res` deserialize sub-span,
/// and the unit-typed main span alike.
pub fn exit_err<E, Sp, T>(env: &E, span: Sp, err: Error) -> Error
where
    E: Env,
    Sp: crate::Span<Result<T, Error>>,
{
    let outcome: Result<T, Error> = Err(err);
    span.exit(env, &outcome);
    // Move the error back out without requiring `T: Debug` (which
    // `Result::unwrap_err` would impose).
    match outcome {
        Err(err) => err,
        // SAFETY-equivalent: `outcome` was constructed as `Err` just above and
        // `exit` only borrows it, so the `Ok` arm is unreachable.
        Ok(_) => unreachable!("outcome was constructed as Err"),
    }
}

/// Helpers for instrumenting streaming-response RPCs, shared by every transport.
///
/// A streaming consumer span ([`StreamResponse`]) measures "time until the
/// response stream is fully consumed": its sub-spans (`on_transmit`,
/// `on_chunk_receive`, …) are exited inline as each step completes, and the
/// main span is exited once the response stream drains. These helpers (built on
/// [`exit_err`]) are the only places that logic lives, so transports compose
/// them instead of repeating the match-on-result at every call site.
pub trait SetupSpan: crate::Span<Result<(), Error>> + Sized {
    /// Exit the (sub-)span recording the success/failure of `result`, then
    /// return `result` unchanged so the caller can `?` it or keep the value.
    /// Used for the inline `on_transmit` / `on_connect` sub-spans, each of
    /// which wraps a single fallible setup step.
    fn exit_with<E: Env, T>(self, env: &E, result: Result<T, Error>) -> Result<T, Error> {
        match result {
            Ok(value) => {
                self.exit(env, &Ok(()));
                Ok(value)
            }
            Err(err) => Err(exit_err::<_, _, ()>(env, self, err)),
        }
    }

    /// Gate the streaming phase on a `setup` result. On `Ok`, return the span
    /// paired with the setup value so the caller can move the span into
    /// [`exit_on_drain`]. On `Err`, exit the span as a failure and propagate
    /// the error (so the surrounding `?` returns it from the RPC future).
    fn keep_or_exit<E: Env, T>(self, env: &E, setup: Result<T, Error>) -> Result<(Self, T), Error> {
        match setup {
            Ok(value) => Ok((self, value)),
            Err(err) => Err(exit_err::<_, _, ()>(env, self, err)),
        }
    }
}

impl<Sp> SetupSpan for Sp where Sp: crate::Span<Result<(), Error>> {}

/// Wrap `stream` so `span` exits exactly once when the stream finishes
/// draining: with success after a clean end, or with the first `Err` the
/// stream yields. The yielded items are forwarded unchanged.
///
/// This gives the time-to-fully-consume semantics for streaming-response spans.
/// Transports whose response stream arrives pre-deserialized (e.g. the
/// in-memory transport) use this directly. Transports that read bytes off the
/// wire instead wire the per-frame `on_chunk_receive` / `on_item_deserialize`
/// sub-spans inline and fold the same end-of-stream exit into their own reader,
/// since the per-frame sub-spans must be created from the same `&mut` main span.
pub fn exit_on_drain<E, Res, Sp, St>(
    env: E,
    span: Sp,
    stream: St,
) -> impl crate::stream::Stream<Item = Result<Res, Error>> + Send
where
    E: Env,
    Res: Send + 'static,
    Sp: crate::Span<Result<(), Error>> + Send + 'static,
    St: crate::stream::Stream<Item = Result<Res, Error>> + Send + 'static,
{
    crate::stream::unfold(
        (env, Some(span), Box::pin(stream)),
        |(env, mut span, mut stream)| async move {
            use crate::stream::StreamExt as _;
            match stream.next().await {
                Some(Ok(value)) => Some((Ok(value), (env, span, stream))),
                Some(Err(err)) => {
                    // First error: fail the span and forward the error, then
                    // short-circuit (span taken → no further exit).
                    let item = match span.take() {
                        Some(span) => Err(exit_err::<_, _, ()>(&env, span, err)),
                        None => Err(err),
                    };
                    Some((item, (env, None, stream)))
                }
                None => {
                    if let Some(span) = span.take() {
                        span.exit(&env, &Ok(()));
                    }
                    None
                }
            }
        },
    )
}

/// Span for unidirectional operations (no response)
pub trait Unidirectional<Req>: crate::Span<Result<(), Error>> {
    /// Create a span for serializing the request
    ///
    /// The span will be exited when serialization is complete, with the size of the serialized payload
    fn on_serialize<E: Env>(&mut self, env: &E, request: &Req) -> impl crate::Span<usize> {
        let _ = env;
        let _ = request;
    }

    /// Called when the request was already serialized for multicast
    fn on_serialize_multicast<E: Env>(&mut self, env: &E, len: usize) {
        let _ = env;
        let _ = len;
    }

    /// Create a span for transmitting the request
    ///
    /// The span will be exited when transmission is complete, with the result of the transmission
    fn on_transmit<E: Env>(
        &mut self,
        env: &E,
        request_size: usize,
    ) -> impl crate::Span<Result<(), Error>> {
        let _ = env;
        let _ = request_size;
    }
}

impl<Req> Unidirectional<Req> for () {}

impl<Req, T> Unidirectional<Req> for Option<T>
where
    T: Unidirectional<Req>,
{
    fn on_serialize<E: Env>(&mut self, env: &E, request: &Req) -> impl crate::Span<usize> {
        self.as_mut().map(|v| v.on_serialize(env, request))
    }

    fn on_serialize_multicast<E: Env>(&mut self, env: &E, len: usize) {
        if let Some(v) = self.as_mut() {
            v.on_serialize_multicast(env, len)
        }
    }

    fn on_transmit<E: Env>(
        &mut self,
        env: &E,
        request_size: usize,
    ) -> impl crate::Span<Result<(), Error>> {
        self.as_mut().map(|v| v.on_transmit(env, request_size))
    }
}

impl<Req, A, B> Unidirectional<Req> for (A, B)
where
    A: Unidirectional<Req>,
    B: Unidirectional<Req>,
{
    fn on_serialize<E: Env>(&mut self, env: &E, request: &Req) -> impl crate::Span<usize> {
        let a = self.0.on_serialize(env, request);
        let b = self.1.on_serialize(env, request);
        (a, b)
    }

    fn on_serialize_multicast<E: Env>(&mut self, env: &E, len: usize) {
        self.0.on_serialize_multicast(env, len);
        self.1.on_serialize_multicast(env, len);
    }

    fn on_transmit<E: Env>(
        &mut self,
        env: &E,
        request_size: usize,
    ) -> impl crate::Span<Result<(), Error>> {
        let a = self.0.on_transmit(env, request_size);
        let b = self.1.on_transmit(env, request_size);
        (a, b)
    }
}

/// Span for unary request/response operations
pub trait Unary<Req, Res>: crate::Span<Result<Res, Error>> {
    /// Create a span for serializing the request
    ///
    /// The span will be exited when serialization is complete, with the size of the serialized payload
    fn on_serialize<E: Env>(&mut self, env: &E, request: &Req) -> impl crate::Span<usize> {
        let _ = env;
        let _ = request;
    }

    /// Called when the request was already serialized for multicast
    fn on_serialize_multicast<E: Env>(&mut self, env: &E, len: usize) {
        let _ = env;
        let _ = len;
    }

    /// Create a span for transmitting the request and receiving the response
    ///
    /// The span will be exited when transmission is complete, with the result of the transmission
    fn on_execute<E: Env>(
        &mut self,
        env: &E,
        request_size: usize,
    ) -> impl crate::Span<Result<usize, Error>> {
        let _ = env;
        let _ = request_size;
    }

    /// Create a span for deserializing the response
    ///
    /// The span will be exited when deserialization is complete, with the deserialized result
    fn on_deserialize<E: Env>(
        &mut self,
        env: &E,
        response_size: usize,
    ) -> impl crate::Span<Result<Res, Error>> {
        let _ = env;
        let _ = response_size;
    }
}

impl<Req, Res> Unary<Req, Res> for () {}

impl<Req, Res, T> Unary<Req, Res> for Option<T>
where
    T: Unary<Req, Res>,
{
    fn on_serialize<E: Env>(&mut self, env: &E, request: &Req) -> impl crate::Span<usize> {
        self.as_mut().map(|v| v.on_serialize(env, request))
    }

    fn on_serialize_multicast<E: Env>(&mut self, env: &E, len: usize) {
        if let Some(v) = self.as_mut() {
            v.on_serialize_multicast(env, len)
        }
    }

    fn on_execute<E: Env>(
        &mut self,
        env: &E,
        request_size: usize,
    ) -> impl crate::Span<Result<usize, Error>> {
        self.as_mut().map(|v| v.on_execute(env, request_size))
    }

    fn on_deserialize<E: Env>(
        &mut self,
        env: &E,
        response_size: usize,
    ) -> impl crate::Span<Result<Res, Error>> {
        self.as_mut().map(|v| v.on_deserialize(env, response_size))
    }
}

impl<Req, Res, A, B> Unary<Req, Res> for (A, B)
where
    A: Unary<Req, Res>,
    B: Unary<Req, Res>,
{
    fn on_serialize<E: Env>(&mut self, env: &E, request: &Req) -> impl crate::Span<usize> {
        let a = self.0.on_serialize(env, request);
        let b = self.1.on_serialize(env, request);
        (a, b)
    }

    fn on_serialize_multicast<E: Env>(&mut self, env: &E, len: usize) {
        self.0.on_serialize_multicast(env, len);
        self.1.on_serialize_multicast(env, len);
    }

    fn on_execute<E: Env>(
        &mut self,
        env: &E,
        request_size: usize,
    ) -> impl crate::Span<Result<usize, Error>> {
        let a = self.0.on_execute(env, request_size);
        let b = self.1.on_execute(env, request_size);
        (a, b)
    }

    fn on_deserialize<E: Env>(
        &mut self,
        env: &E,
        response_size: usize,
    ) -> impl crate::Span<Result<Res, Error>> {
        let a = self.0.on_deserialize(env, response_size);
        let b = self.1.on_deserialize(env, response_size);
        (a, b)
    }
}

/// Span for client-to-server streaming operations
pub trait StreamRequest<Req, Res>: crate::Span<Result<Res, Error>> {
    /// Create a span for establishing the connection
    ///
    /// The span will be exited when the connection is established, with the result of the connection attempt
    fn on_connect<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<(), Error>> {
        let _ = env;
    }

    /// Create a span for serializing an item
    ///
    /// The span will be exited when serialization is complete, with the size of the serialized item
    fn on_item_serialize<E: Env>(&mut self, env: &E, item: &Req) -> impl crate::Span<usize> {
        let _ = env;
        let _ = item;
    }

    /// Create a span for sending a chunk
    ///
    /// The span will be exited when the chunk is sent, with the result of the send operation
    fn on_chunk_transmit<E: Env>(
        &mut self,
        env: &E,
        chunk_size: usize,
    ) -> impl crate::Span<Result<(), Error>> {
        let _ = env;
        let _ = chunk_size;
    }

    /// Create a span for receiving the response
    ///
    /// The span will be exited when the response is fully received, with the size of the response
    fn on_receive<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<usize, Error>> {
        let _ = env;
    }

    /// Create a span for deserializing the response
    ///
    /// The span will be exited when deserialization is complete, with the deserialized result
    fn on_deserialize<E: Env>(
        &mut self,
        env: &E,
        response_size: usize,
    ) -> impl crate::Span<Result<Res, Error>> {
        let _ = env;
        let _ = response_size;
    }
}

impl<Req, Res> StreamRequest<Req, Res> for () {}

impl<Req, Res, T> StreamRequest<Req, Res> for Option<T>
where
    T: StreamRequest<Req, Res>,
{
    fn on_connect<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<(), Error>> {
        self.as_mut().map(|v| v.on_connect(env))
    }

    fn on_item_serialize<E: Env>(&mut self, env: &E, item: &Req) -> impl crate::Span<usize> {
        self.as_mut().map(|v| v.on_item_serialize(env, item))
    }

    fn on_chunk_transmit<E: Env>(
        &mut self,
        env: &E,
        chunk_size: usize,
    ) -> impl crate::Span<Result<(), Error>> {
        self.as_mut().map(|v| v.on_chunk_transmit(env, chunk_size))
    }

    fn on_receive<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<usize, Error>> {
        self.as_mut().map(|v| v.on_receive(env))
    }

    fn on_deserialize<E: Env>(
        &mut self,
        env: &E,
        response_size: usize,
    ) -> impl crate::Span<Result<Res, Error>> {
        self.as_mut().map(|v| v.on_deserialize(env, response_size))
    }
}

impl<Req, Res, A, B> StreamRequest<Req, Res> for (A, B)
where
    A: StreamRequest<Req, Res>,
    B: StreamRequest<Req, Res>,
{
    fn on_connect<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<(), Error>> {
        let a = self.0.on_connect(env);
        let b = self.1.on_connect(env);
        (a, b)
    }

    fn on_item_serialize<E: Env>(&mut self, env: &E, item: &Req) -> impl crate::Span<usize> {
        let a = self.0.on_item_serialize(env, item);
        let b = self.1.on_item_serialize(env, item);
        (a, b)
    }

    fn on_chunk_transmit<E: Env>(
        &mut self,
        env: &E,
        chunk_size: usize,
    ) -> impl crate::Span<Result<(), Error>> {
        let a = self.0.on_chunk_transmit(env, chunk_size);
        let b = self.1.on_chunk_transmit(env, chunk_size);
        (a, b)
    }

    fn on_receive<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<usize, Error>> {
        let a = self.0.on_receive(env);
        let b = self.1.on_receive(env);
        (a, b)
    }

    fn on_deserialize<E: Env>(
        &mut self,
        env: &E,
        response_size: usize,
    ) -> impl crate::Span<Result<Res, Error>> {
        let a = self.0.on_deserialize(env, response_size);
        let b = self.1.on_deserialize(env, response_size);
        (a, b)
    }
}

/// Span for server-to-client streaming operations
pub trait StreamResponse<Req, Res>: crate::Span<Result<(), Error>> {
    /// Create a span for serializing the initial request
    ///
    /// The span will be exited when serialization is complete, with the size of the serialized payload
    fn on_serialize<E: Env>(&mut self, env: &E, request: &Req) -> impl crate::Span<usize> {
        let _ = env;
        let _ = request;
    }

    /// Called when the request was already serialized for multicast
    fn on_serialize_multicast<E: Env>(&mut self, env: &E, len: usize) {
        let _ = env;
        let _ = len;
    }

    /// Create a span for transmitting the initial request
    ///
    /// The span will be exited when transmission is complete, with the result of the transmission
    fn on_transmit<E: Env>(
        &mut self,
        env: &E,
        request_size: usize,
    ) -> impl crate::Span<Result<(), Error>> {
        let _ = env;
        let _ = request_size;
    }

    /// Create a span for receiving a chunk
    ///
    /// The span will be exited when the chunk is received, with the size of the chunk
    fn on_chunk_receive<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<usize, Error>> {
        let _ = env;
    }

    /// Create a span for deserializing a chunk
    ///
    /// The span will be exited when deserialization is complete, with the result of the deserialization
    fn on_item_deserialize<E: Env>(
        &mut self,
        env: &E,
        chunk_size: usize,
    ) -> impl crate::Span<Result<Res, Error>> {
        let _ = env;
        let _ = chunk_size;
    }
}

impl<Req, Res> StreamResponse<Req, Res> for () {}

impl<Req, Res, T> StreamResponse<Req, Res> for Option<T>
where
    T: StreamResponse<Req, Res>,
{
    fn on_serialize<E: Env>(&mut self, env: &E, request: &Req) -> impl crate::Span<usize> {
        self.as_mut().map(|v| v.on_serialize(env, request))
    }

    fn on_serialize_multicast<E: Env>(&mut self, env: &E, len: usize) {
        if let Some(v) = self.as_mut() {
            v.on_serialize_multicast(env, len)
        }
    }

    fn on_transmit<E: Env>(
        &mut self,
        env: &E,
        request_size: usize,
    ) -> impl crate::Span<Result<(), Error>> {
        self.as_mut().map(|v| v.on_transmit(env, request_size))
    }

    fn on_chunk_receive<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<usize, Error>> {
        self.as_mut().map(|v| v.on_chunk_receive(env))
    }

    fn on_item_deserialize<E: Env>(
        &mut self,
        env: &E,
        chunk_size: usize,
    ) -> impl crate::Span<Result<Res, Error>> {
        self.as_mut()
            .map(|v| v.on_item_deserialize(env, chunk_size))
    }
}

impl<Req, Res, A, B> StreamResponse<Req, Res> for (A, B)
where
    A: StreamResponse<Req, Res>,
    B: StreamResponse<Req, Res>,
{
    fn on_serialize<E: Env>(&mut self, env: &E, request: &Req) -> impl crate::Span<usize> {
        let a = self.0.on_serialize(env, request);
        let b = self.1.on_serialize(env, request);
        (a, b)
    }

    fn on_serialize_multicast<E: Env>(&mut self, env: &E, len: usize) {
        self.0.on_serialize_multicast(env, len);
        self.1.on_serialize_multicast(env, len);
    }

    fn on_transmit<E: Env>(
        &mut self,
        env: &E,
        request_size: usize,
    ) -> impl crate::Span<Result<(), Error>> {
        let a = self.0.on_transmit(env, request_size);
        let b = self.1.on_transmit(env, request_size);
        (a, b)
    }

    fn on_chunk_receive<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<usize, Error>> {
        let a = self.0.on_chunk_receive(env);
        let b = self.1.on_chunk_receive(env);
        (a, b)
    }

    fn on_item_deserialize<E: Env>(
        &mut self,
        env: &E,
        chunk_size: usize,
    ) -> impl crate::Span<Result<Res, Error>> {
        let a = self.0.on_item_deserialize(env, chunk_size);
        let b = self.1.on_item_deserialize(env, chunk_size);
        (a, b)
    }
}

pub mod bidi {
    use super::*;

    /// Request side of split span for bidirectional streaming
    pub trait Request<Req>: Send + 'static {
        /// Create a span for serializing an item
        ///
        /// The span will be exited when serialization is complete, with the size of the serialized item
        fn on_item_serialize<E: Env>(&mut self, env: &E, item: &Req) -> impl crate::Span<usize> {
            let _ = env;
            let _ = item;
        }

        /// Create a span for sending a chunk
        ///
        /// The span will be exited when the chunk is sent, with the result of the send operation
        fn on_chunk_transmit<E: Env>(
            &mut self,
            env: &E,
            chunk_size: usize,
        ) -> impl crate::Span<Result<(), Error>> {
            let _ = env;
            let _ = chunk_size;
        }
    }

    impl<Req> Request<Req> for () {}

    impl<Req, T> Request<Req> for Option<T>
    where
        T: Request<Req>,
    {
        fn on_item_serialize<E: Env>(&mut self, env: &E, item: &Req) -> impl crate::Span<usize> {
            self.as_mut().map(|v| v.on_item_serialize(env, item))
        }

        fn on_chunk_transmit<E: Env>(
            &mut self,
            env: &E,
            chunk_size: usize,
        ) -> impl crate::Span<Result<(), Error>> {
            self.as_mut().map(|v| v.on_chunk_transmit(env, chunk_size))
        }
    }

    /// Response side of split span for bidirectional streaming
    pub trait Response<Res>: crate::Span<Result<(), Error>> {
        /// Create a span for receiving a chunk
        ///
        /// The span will be exited when the chunk is received, with the size of the chunk
        fn on_chunk_receive<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<usize, Error>> {
            let _ = env;
        }

        /// Create a span for deserializing a chunk
        ///
        /// The span will be exited when deserialization is complete, with the result of the deserialization
        fn on_item_deserialize<E: Env>(
            &mut self,
            env: &E,
            chunk_size: usize,
        ) -> impl crate::Span<Result<Res, Error>> {
            let _ = env;
            let _ = chunk_size;
        }
    }

    impl<Res> Response<Res> for () {}

    impl<Res, T> Response<Res> for Option<T>
    where
        T: Response<Res>,
    {
        fn on_chunk_receive<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<usize, Error>> {
            self.as_mut().map(|v| v.on_chunk_receive(env))
        }

        fn on_item_deserialize<E: Env>(
            &mut self,
            env: &E,
            chunk_size: usize,
        ) -> impl crate::Span<Result<Res, Error>> {
            self.as_mut()
                .map(|v| v.on_item_deserialize(env, chunk_size))
        }
    }

    /// Main span that splits into request/response spans
    pub trait Stream<Req, Res>: crate::Span<Result<(), Error>> {
        type Request: Request<Req>;
        type Response: Response<Res>;

        /// Create a span for establishing the connection
        ///
        /// The span will be exited when the connection is established, with the result of the connection attempt
        fn on_connect<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<(), Error>> {
            let _ = env;
        }

        /// Split this span into separate request and response spans
        fn split(&mut self) -> (Self::Request, Self::Response);
    }

    impl<Req, Res> Stream<Req, Res> for () {
        type Request = ();
        type Response = ();

        fn split(&mut self) -> (Self::Request, Self::Response) {
            ((), ())
        }
    }

    impl<Req, Res, T> Stream<Req, Res> for Option<T>
    where
        T: Stream<Req, Res>,
    {
        type Request = Option<T::Request>;
        type Response = Option<T::Response>;

        fn on_connect<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<(), Error>> {
            self.as_mut().map(|v| v.on_connect(env))
        }

        fn split(&mut self) -> (Self::Request, Self::Response) {
            match self.as_mut() {
                Some(v) => {
                    let (req, res) = v.split();
                    (Some(req), Some(res))
                }
                None => (None, None),
            }
        }
    }

    impl<Req, A, B> Request<Req> for (A, B)
    where
        A: Request<Req>,
        B: Request<Req>,
    {
        fn on_item_serialize<E: Env>(&mut self, env: &E, item: &Req) -> impl crate::Span<usize> {
            let a = self.0.on_item_serialize(env, item);
            let b = self.1.on_item_serialize(env, item);
            (a, b)
        }

        fn on_chunk_transmit<E: Env>(
            &mut self,
            env: &E,
            chunk_size: usize,
        ) -> impl crate::Span<Result<(), Error>> {
            let a = self.0.on_chunk_transmit(env, chunk_size);
            let b = self.1.on_chunk_transmit(env, chunk_size);
            (a, b)
        }
    }

    impl<Res, A, B> Response<Res> for (A, B)
    where
        A: Response<Res>,
        B: Response<Res>,
    {
        fn on_chunk_receive<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<usize, Error>> {
            let a = self.0.on_chunk_receive(env);
            let b = self.1.on_chunk_receive(env);
            (a, b)
        }

        fn on_item_deserialize<E: Env>(
            &mut self,
            env: &E,
            chunk_size: usize,
        ) -> impl crate::Span<Result<Res, Error>> {
            let a = self.0.on_item_deserialize(env, chunk_size);
            let b = self.1.on_item_deserialize(env, chunk_size);
            (a, b)
        }
    }

    impl<Req, Res, A, B> Stream<Req, Res> for (A, B)
    where
        A: Stream<Req, Res>,
        B: Stream<Req, Res>,
    {
        type Request = (A::Request, B::Request);
        type Response = (A::Response, B::Response);

        fn on_connect<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<(), Error>> {
            let a = self.0.on_connect(env);
            let b = self.1.on_connect(env);
            (a, b)
        }

        fn split(&mut self) -> (Self::Request, Self::Response) {
            let (req_a, res_a) = self.0.split();
            let (req_b, res_b) = self.1.split();
            ((req_a, req_b), (res_a, res_b))
        }
    }
}
