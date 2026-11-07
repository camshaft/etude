use crate::{RequestStream, ResponseStream, Service};
use core::{future::Future, marker::Send};
use upstream_runtime::{Env, Error};
use upstream_wire::{Deserialize, Serialize};

pub mod span;
mod tuple;

/// A trait for request types that can be deserialized and converted to a service's request type
pub trait Request<S: Service>:
    'static + Deserialize + TryFrom<S::Request, Error = Error> + Send
{
}

pub trait RequestHeader<S: Service>:
    'static + Deserialize + TryFrom<S::RequestHeader, Error = Error> + Send
{
}

pub trait ResponseHeader<S: Service>: 'static + Serialize + Into<S::ResponseHeader> + Send {}

/// A trait for response types that can be serialized and converted from a service's response type
pub trait Response<S: Service>: 'static + Serialize + Into<S::Response> + Send {}

// Implement the traits for any types that satisfy the bounds
impl<S: Service, T> Request<S> for T where
    T: 'static + Deserialize + TryFrom<S::Request, Error = Error> + Send
{
}

impl<S: Service, T> RequestHeader<S> for T where
    T: 'static + Deserialize + TryFrom<S::RequestHeader, Error = Error> + Send
{
}

impl<S: Service, T> ResponseHeader<S> for T where
    T: 'static + Serialize + Into<S::ResponseHeader> + Send
{
}

impl<S: Service, T> Response<S> for T where T: 'static + Serialize + Into<S::Response> + Send {}

/// A trait provided by the transport to drive requests
pub trait RequestProvider<S: Service>: Send {
    /// Read a request with 1 entry
    fn read<E, Req, Span>(
        self,
        env: &E,
        span: &mut Span,
    ) -> impl Future<Output = Result<Req, Error>> + Send
    where
        E: Env,
        Req: Request<S>,
        Span: span::FullRequest<Req>;

    /// Read a request with a header
    fn read_with_header<E, ReqH, Req, Span>(
        self,
        env: &E,
        span: &mut Span,
    ) -> impl Future<Output = Result<(ReqH, Req), Error>> + Send
    where
        E: Env,
        ReqH: RequestHeader<S>,
        Req: Request<S>,
        Span: span::FullRequest<(ReqH, Req)>;

    /// Read a stream of requests, instrumenting each frame via `instrument`
    /// (`on_chunk_receive` around the wire read, `on_chunk_deserialize` around
    /// the decode). Pass `()` for no instrumentation.
    fn read_stream<E, Req, I>(
        self,
        env: &E,
        instrument: I,
    ) -> impl Future<Output = Result<impl RequestStream<Req>, Error>> + Send
    where
        E: Env,
        Req: Request<S>,
        I: span::StreamingRequest<Req> + 'static;

    /// Header variant of [`read_stream`](Self::read_stream); `instrument`
    /// applies to the streamed request frames after the header.
    fn read_stream_with_header<E, H, Req, I>(
        self,
        env: &E,
        instrument: I,
    ) -> impl Future<Output = Result<(H, impl RequestStream<Req>), Error>> + Send
    where
        E: Env,
        H: RequestHeader<S>,
        Req: Request<S>,
        I: span::StreamingRequest<Req> + 'static;
}

pub trait ResponseProvider<S: Service>: Send {
    fn finish_unidirectional<E, Span>(self, env: &E, span: Span) -> impl Future<Output = ()> + Send
    where
        E: Env,
        Span: crate::Span<Result<(), Error>>;

    /// Write a response with 1 entry
    fn write<E, Res, Span>(
        self,
        env: &E,
        response: Res,
        span: Span,
    ) -> impl Future<Output = ()> + Send
    where
        E: Env,
        Res: Response<S>,
        Span: span::FullResponse<Res>;

    /// Write a response with a header
    fn write_with_header<E, ResH, Res, Span>(
        self,
        env: &E,
        header: ResH,
        response: Res,
        span: Span,
    ) -> impl Future<Output = ()> + Send
    where
        E: Env,
        ResH: ResponseHeader<S>,
        Res: Response<S>,
        Span: span::FullResponse<(ResH, Res)>;

    /// Write a raw response
    fn write_raw<E, Res, Span>(
        self,
        env: &E,
        response: Res,
        span: Span,
    ) -> impl Future<Output = ()> + Send
    where
        E: Env,
        Res: upstream_buffer::reader::storage::Infallible + Send,
        Span: span::FullResponse<usize>;

    /// Write a stream of responses, instrumenting each frame via `span`
    /// (`on_item_serialize` around the encode, `on_item_transmit` around the
    /// flush) and exiting `span` when the stream drains. Pass `()` for no
    /// instrumentation.
    fn write_stream<E, Res, Span>(
        self,
        env: &E,
        response_stream: impl ResponseStream<Res>,
        span: Span,
    ) -> impl Future<Output = ()> + Send
    where
        E: Env,
        Res: Response<S>,
        Span: span::StreamingResponse<Res>;

    /// Header variant of [`write_stream`](Self::write_stream); `span`
    /// instruments the streamed response frames after the header.
    fn write_stream_with_header<E, ResH, Res, Span>(
        self,
        env: &E,
        header: ResH,
        response_stream: impl ResponseStream<Res>,
        span: Span,
    ) -> impl Future<Output = ()> + Send
    where
        E: Env,
        ResH: ResponseHeader<S>,
        Res: Response<S>,
        Span: span::StreamingResponse<Res>;

    fn write_error<E, Span>(
        self,
        env: &E,
        error: Error,
        span: Span,
    ) -> impl Future<Output = ()> + Send
    where
        E: Env,
        Span: crate::Span<Result<(), Error>>;

    fn write_stream_header_error<E, Span>(
        self,
        env: &E,
        error: Error,
        span: Span,
    ) -> impl Future<Output = ()> + Send
    where
        E: Env,
        Span: crate::Span<Result<(), Error>>;

    fn write_raw_error<E, Span>(
        self,
        env: &E,
        error: Error,
        span: Span,
    ) -> impl Future<Output = ()> + Send
    where
        E: Env,
        Span: crate::Span<Result<(), Error>>;
}

pub trait Acceptor<E: Env, PeerAddr>: 'static + Send + Sync {
    type Service: Service;

    /// Process a request
    fn dispatch<Req, Res>(
        &self,
        env: &E,
        peer_addr: &PeerAddr,
        method: <Self::Service as Service>::Methods,
        request: Req,
        response: Res,
    ) -> impl Future<Output = ()> + Send
    where
        Req: RequestProvider<Self::Service>,
        Res: ResponseProvider<Self::Service>;

    /// Spawns the connection-handling task with this acceptor's per-service task
    /// class, so connection-level scheduling/poll metrics roll up per service.
    ///
    /// The generated `Instance` overrides this to use the `<Service>Service`
    /// class (its impl has the crate `Publisher` in scope). The default classes
    /// the task `Unlabeled` — used by acceptors with no generated publisher
    /// (e.g. in tests) so the transport layer needs no publisher bound on `E`.
    fn spawn_connection<F>(&self, env: &E, future: F) -> E::JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        env.spawn(upstream_runtime::task_monitor::Unlabeled, future)
    }

    /// A future that only returns once a Provider implementation indicates it's shutting down
    fn while_open(&self) -> impl Future<Output = ()> + Send;
}
