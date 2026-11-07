// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use upstream_runtime::{Env, Error};

pub trait FullRequest<Req>: Send {
    /// Create a span for receiving the request
    ///
    /// The span will be exited when the request is fully received, with the size of the request
    fn on_receive<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<usize, Error>> {
        let _ = env;
    }

    /// Create a span for deserializing the request
    ///
    /// The span will be exited when deserialization is complete, with the result of the deserialization
    fn on_deserialize<E: Env>(
        &mut self,
        env: &E,
        payload_size: usize,
    ) -> impl crate::Span<Result<Req, Error>> {
        let _ = env;
        let _ = payload_size;
    }
}

impl<Req> FullRequest<Req> for () {}

impl<Req, T> FullRequest<Req> for Option<T>
where
    T: FullRequest<Req>,
{
    fn on_receive<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<usize, Error>> {
        self.as_mut().map(|v| v.on_receive(env))
    }

    fn on_deserialize<E: Env>(
        &mut self,
        env: &E,
        payload_size: usize,
    ) -> impl crate::Span<Result<Req, Error>> {
        self.as_mut().map(|v| v.on_deserialize(env, payload_size))
    }
}

impl<Req, A, B> FullRequest<Req> for (A, B)
where
    A: FullRequest<Req>,
    B: FullRequest<Req>,
{
    fn on_receive<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<usize, Error>> {
        let a = self.0.on_receive(env);
        let b = self.1.on_receive(env);
        (a, b)
    }

    fn on_deserialize<E: Env>(
        &mut self,
        env: &E,
        payload_size: usize,
    ) -> impl crate::Span<Result<Req, Error>> {
        let a = self.0.on_deserialize(env, payload_size);
        let b = self.1.on_deserialize(env, payload_size);
        (a, b)
    }
}

pub trait StreamingRequest<Req>: Send {
    /// Create a span for receiving a chunk
    ///
    /// The span will be exited when the chunk is received, with the size of the chunk
    fn on_chunk_receive<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<usize, Error>> {
        let _ = env;
    }

    /// Create a span for deserializing a chunk
    ///
    /// The span will be exited when deserialization is complete, with the result of the deserialization
    fn on_chunk_deserialize<E: Env>(
        &mut self,
        env: &E,
        payload_size: usize,
    ) -> impl crate::Span<Result<Req, Error>> {
        let _ = env;
        let _ = payload_size;
    }
}

impl<Req> StreamingRequest<Req> for () {}

impl<Req, T> StreamingRequest<Req> for Option<T>
where
    T: StreamingRequest<Req>,
{
    fn on_chunk_receive<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<usize, Error>> {
        self.as_mut().map(|v| v.on_chunk_receive(env))
    }

    fn on_chunk_deserialize<E: Env>(
        &mut self,
        env: &E,
        payload_size: usize,
    ) -> impl crate::Span<Result<Req, Error>> {
        self.as_mut()
            .map(|v| v.on_chunk_deserialize(env, payload_size))
    }
}

impl<Req, A, B> StreamingRequest<Req> for (A, B)
where
    A: StreamingRequest<Req>,
    B: StreamingRequest<Req>,
{
    fn on_chunk_receive<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<usize, Error>> {
        let a = self.0.on_chunk_receive(env);
        let b = self.1.on_chunk_receive(env);
        (a, b)
    }

    fn on_chunk_deserialize<E: Env>(
        &mut self,
        env: &E,
        payload_size: usize,
    ) -> impl crate::Span<Result<Req, Error>> {
        let a = self.0.on_chunk_deserialize(env, payload_size);
        let b = self.1.on_chunk_deserialize(env, payload_size);
        (a, b)
    }
}

pub trait FullResponse<Res>: crate::Span<Result<(), Error>> {
    /// Create a span for serializing the response
    ///
    /// The span will be exited when serialization is complete, with the size of the serialized response
    fn on_serialize<E: Env>(&mut self, env: &E) -> impl crate::Span<usize> {
        let _ = env;
    }

    /// Create a span for sending the response
    ///
    /// The span will be exited when the response is sent
    fn on_transmit<E: Env>(
        &mut self,
        env: &E,
        response_size: usize,
    ) -> impl crate::Span<Result<(), Error>> {
        let _ = env;
        let _ = response_size;
    }
}

impl<Res> FullResponse<Res> for () {}

impl<Res, T> FullResponse<Res> for Option<T>
where
    T: FullResponse<Res>,
{
    fn on_serialize<E: Env>(&mut self, env: &E) -> impl crate::Span<usize> {
        self.as_mut().map(|v| v.on_serialize(env))
    }

    fn on_transmit<E: Env>(
        &mut self,
        env: &E,
        response_size: usize,
    ) -> impl crate::Span<Result<(), Error>> {
        self.as_mut().map(|v| v.on_transmit(env, response_size))
    }
}

impl<Res, A, B> FullResponse<Res> for (A, B)
where
    A: FullResponse<Res>,
    B: FullResponse<Res>,
{
    fn on_serialize<E: Env>(&mut self, env: &E) -> impl crate::Span<usize> {
        let a = self.0.on_serialize(env);
        let b = self.1.on_serialize(env);
        (a, b)
    }

    fn on_transmit<E: Env>(
        &mut self,
        env: &E,
        response_size: usize,
    ) -> impl crate::Span<Result<(), Error>> {
        let a = self.0.on_transmit(env, response_size);
        let b = self.1.on_transmit(env, response_size);
        (a, b)
    }
}

pub trait StreamingResponse<Res>: crate::Span<Result<(), Error>> {
    /// Produce an owned per-frame writer for the response stream.
    ///
    /// Mirror of [`StreamRequest::request_frames`]: the framing layer holds the
    /// per-frame instrument for the duration of the write, so it must be an
    /// owned value sharing this span's metric handles rather than a borrow —
    /// the main span is retained afterward to exit it with the drain result.
    fn response_frames(&mut self) -> impl StreamingResponse<Res> + Send + 'static {}

    /// Create a span for serializing a chunk
    ///
    /// The span will be exited when serialization is complete, with the size of the serialized chunk
    fn on_item_serialize<E: Env>(&mut self, env: &E) -> impl crate::Span<usize> {
        let _ = env;
    }

    /// Create a span for sending a chunk
    ///
    /// The span will be exited when the chunk is sent
    fn on_item_transmit<E: Env>(
        &mut self,
        env: &E,
        response_size: usize,
    ) -> impl crate::Span<Result<(), Error>> {
        let _ = env;
        let _ = response_size;
    }
}

impl<Res> StreamingResponse<Res> for () {}

impl<Res, T> StreamingResponse<Res> for Option<T>
where
    T: StreamingResponse<Res>,
{
    fn response_frames(&mut self) -> impl StreamingResponse<Res> + Send + 'static {
        self.as_mut().map(|v| v.response_frames())
    }

    fn on_item_serialize<E: Env>(&mut self, env: &E) -> impl crate::Span<usize> {
        self.as_mut().map(|v| v.on_item_serialize(env))
    }

    fn on_item_transmit<E: Env>(
        &mut self,
        env: &E,
        response_size: usize,
    ) -> impl crate::Span<Result<(), Error>> {
        self.as_mut()
            .map(|v| v.on_item_transmit(env, response_size))
    }
}

impl<Res, A, B> StreamingResponse<Res> for (A, B)
where
    A: StreamingResponse<Res>,
    B: StreamingResponse<Res>,
{
    fn response_frames(&mut self) -> impl StreamingResponse<Res> + Send + 'static {
        // A tuple of two owned carriers is itself a `StreamingResponse` (via the
        // `(A, B)` blanket impl) and is `Send + 'static` when both halves are —
        // so composed subscribers get per-frame instrumentation from both.
        (self.0.response_frames(), self.1.response_frames())
    }

    fn on_item_serialize<E: Env>(&mut self, env: &E) -> impl crate::Span<usize> {
        let a = self.0.on_item_serialize(env);
        let b = self.1.on_item_serialize(env);
        (a, b)
    }

    fn on_item_transmit<E: Env>(
        &mut self,
        env: &E,
        response_size: usize,
    ) -> impl crate::Span<Result<(), Error>> {
        let a = self.0.on_item_transmit(env, response_size);
        let b = self.1.on_item_transmit(env, response_size);
        (a, b)
    }
}

/// Span for unidirectional operations (no response)
pub trait Unidirectional<Req>: crate::Span<Result<(), Error>> + FullRequest<Req> {
    /// Create a span for processing the request
    ///
    /// The span will be exited when processing is complete
    fn on_process<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<(), Error>> {
        let _ = env;
    }
}

impl<Req> Unidirectional<Req> for () {}

impl<Req, T> Unidirectional<Req> for Option<T>
where
    T: Unidirectional<Req>,
{
    fn on_process<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<(), Error>> {
        self.as_mut().map(|v| v.on_process(env))
    }
}

impl<Req, A, B> Unidirectional<Req> for (A, B)
where
    A: Unidirectional<Req>,
    B: Unidirectional<Req>,
{
    fn on_process<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<(), Error>> {
        let a = self.0.on_process(env);
        let b = self.1.on_process(env);
        (a, b)
    }
}

/// Span for unary request/response operations
pub trait Unary<Req, Res>: FullRequest<Req> + FullResponse<Res> {
    /// Create a span for processing the request
    ///
    /// The span will be exited when processing is complete, with the result
    fn on_process<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<Res, Error>> {
        let _ = env;
    }
}

impl<Req, Res> Unary<Req, Res> for () {}

impl<Req, Res, T> Unary<Req, Res> for Option<T>
where
    T: Unary<Req, Res>,
{
    fn on_process<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<Res, Error>> {
        self.as_mut().map(|v| v.on_process(env))
    }
}

impl<Req, Res, A, B> Unary<Req, Res> for (A, B)
where
    A: Unary<Req, Res>,
    B: Unary<Req, Res>,
{
    fn on_process<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<Res, Error>> {
        let a = self.0.on_process(env);
        let b = self.1.on_process(env);
        (a, b)
    }
}

/// Span for server-side streaming request handling operations.
///
/// The request stream is read per-frame via an owned reader from
/// [`request_frames`](Self::request_frames) (not via a `StreamingRequest`
/// supertrait on the span), because the reader outlives `on_process` and the
/// response write. The unary response goes through the [`FullResponse`]
/// supertrait.
pub trait StreamRequest<Req, Res>: FullResponse<Res> {
    /// Produce an owned per-frame reader for the request stream.
    ///
    /// The provider reads the request stream *before* `on_process` and the
    /// response write, and the returned `RequestStream` outlives those steps
    /// (it is consumed lazily inside the provider handler). So the reader must
    /// be an owned value that shares this span's metric handles — it cannot
    /// borrow or consume the main span.
    ///
    /// Generic over the frame type `F` because a with-header method's span
    /// carries `Req = (ReqHeader, Body)` while the streamed frames are the bare
    /// `Body`. The reader emits the standard per-frame receive/deserialize
    /// timing + size metrics (with a no-op custom-metrics handler). Defaults to
    /// `()` (no instrumentation).
    fn request_frames<F: 'static>(&mut self) -> impl StreamingRequest<F> + 'static {}

    /// Create a span for processing the request
    ///
    /// The span will be exited when processing is complete, with the result
    fn on_process<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<Res, Error>> {
        let _ = env;
    }
}

impl<Req, Res> StreamRequest<Req, Res> for () {}

impl<Req, Res, T> StreamRequest<Req, Res> for Option<T>
where
    T: StreamRequest<Req, Res>,
{
    fn request_frames<F: 'static>(&mut self) -> impl StreamingRequest<F> + 'static {
        self.as_mut().map(|v| v.request_frames())
    }

    fn on_process<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<Res, Error>> {
        self.as_mut().map(|v| v.on_process(env))
    }
}

impl<Req, Res, A, B> StreamRequest<Req, Res> for (A, B)
where
    A: StreamRequest<Req, Res>,
    B: StreamRequest<Req, Res>,
{
    fn request_frames<F: 'static>(&mut self) -> impl StreamingRequest<F> + 'static {
        // A tuple of two owned readers is itself a `StreamingRequest<F>` (via
        // the `(A, B)` blanket impl), so both composed subscribers instrument.
        (self.0.request_frames::<F>(), self.1.request_frames::<F>())
    }

    fn on_process<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<Res, Error>> {
        let a = self.0.on_process(env);
        let b = self.1.on_process(env);
        (a, b)
    }
}

/// Span for server-side streaming response handling operations
pub trait StreamResponse<Req, Res>: FullRequest<Req> + StreamingResponse<Res> {
    /// Create a span for processing the request
    ///
    /// The span will be exited when processing is complete
    fn on_process<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<(), Error>> {
        let _ = env;
    }
}

impl<Req, Res> StreamResponse<Req, Res> for () {}

impl<Req, Res, T> StreamResponse<Req, Res> for Option<T>
where
    T: StreamResponse<Req, Res>,
{
    fn on_process<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<(), Error>> {
        self.as_mut().map(|v| v.on_process(env))
    }
}

impl<Req, Res, A, B> StreamResponse<Req, Res> for (A, B)
where
    A: StreamResponse<Req, Res>,
    B: StreamResponse<Req, Res>,
{
    fn on_process<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<(), Error>> {
        let a = self.0.on_process(env);
        let b = self.1.on_process(env);
        (a, b)
    }
}

pub mod bidi {
    use super::*;

    /// Main span for a bidirectional-streaming provider handler.
    ///
    /// The provider reads the request stream (per-frame, via an owned reader
    /// from [`request_frames`](Stream::request_frames)) and writes the response
    /// stream. The response side reuses the [`StreamingResponse`] hooks on the
    /// span itself (the main span instruments its own written frames and exits
    /// when the stream drains), so there is no separate response half.
    pub trait Stream<Req, Res>: StreamingResponse<Res> {
        /// Produce an owned per-frame reader for the request stream. See
        /// [`super::StreamRequest::request_frames`] — the reader outlives
        /// `on_process` and the response write, so it must be owned, and it is
        /// generic over the frame type `F` to accommodate with-header spans.
        fn request_frames<F: 'static>(&mut self) -> impl StreamingRequest<F> + 'static {}

        /// Create a span for processing the request
        ///
        /// The span will be exited when processing is complete
        fn on_process<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<(), Error>> {
            let _ = env;
        }
    }

    impl<Req, Res> Stream<Req, Res> for () {}

    impl<Req, Res, T> Stream<Req, Res> for Option<T>
    where
        T: Stream<Req, Res>,
    {
        fn request_frames<F: 'static>(&mut self) -> impl StreamingRequest<F> + 'static {
            self.as_mut().map(|v| v.request_frames())
        }

        fn on_process<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<(), Error>> {
            self.as_mut().map(|v| v.on_process(env))
        }
    }

    impl<Req, Res, A, B> Stream<Req, Res> for (A, B)
    where
        A: Stream<Req, Res>,
        B: Stream<Req, Res>,
    {
        fn request_frames<F: 'static>(&mut self) -> impl StreamingRequest<F> + 'static {
            (self.0.request_frames::<F>(), self.1.request_frames::<F>())
        }

        fn on_process<E: Env>(&mut self, env: &E) -> impl crate::Span<Result<(), Error>> {
            let a = self.0.on_process(env);
            let b = self.1.on_process(env);
            (a, b)
        }
    }
}
