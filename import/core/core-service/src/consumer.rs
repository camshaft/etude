use crate::{RequestStream, ResponseStream, Service};
use upstream_runtime::{Env, Error};
use upstream_wire::{Deserialize, Serialize};
use std::{convert::TryFrom, future::Future};

pub mod span;

/// A trait for request types that can be serialized and converted to a service's request type
pub trait Request<S: Service>: 'static + Serialize + Into<S::Request> + Send {}

pub trait RequestHeader<S: Service>: 'static + Serialize + Into<S::RequestHeader> + Send {}

/// A trait for response types that can be deserialized and converted from a service's response type
pub trait Response<S: Service>:
    'static + Deserialize + TryFrom<S::Response, Error = Error> + Send
{
}

pub trait ResponseHeader<S: Service>:
    'static + Deserialize + TryFrom<S::ResponseHeader, Error = Error> + Send
{
}

// Implement the traits for any types that satisfy the bounds
impl<S: Service, T> Request<S> for T where T: 'static + Serialize + Into<S::Request> + Send {}
impl<S: Service, T> RequestHeader<S> for T where
    T: 'static + Serialize + Into<S::RequestHeader> + Send
{
}
impl<S: Service, T> Response<S> for T where
    T: 'static + Deserialize + TryFrom<S::Response, Error = Error> + Send
{
}
impl<S: Service, T> ResponseHeader<S> for T where
    T: 'static + Deserialize + TryFrom<S::ResponseHeader, Error = Error> + Send
{
}

/// Base operation trait for all operations
pub trait Operation<E: Env>: Clone + Send + 'static {
    type Output: Send;
    type PeerAddr: PeerAddr;

    fn peer_addr(&self) -> &Self::PeerAddr;

    /// Execute the operation and return the result
    fn send(self, env: &E) -> impl Future<Output = Result<Self::Output, Error>> + Send;
}

/// Operation that returns a value with a header
pub trait OperationWithHeader<E: Env>: Clone + Send + 'static {
    type Output: Send;
    type Header: Send;
    type PeerAddr: PeerAddr;

    fn peer_addr(&self) -> &Self::PeerAddr;

    /// Execute the operation and return the result with header
    fn send(
        self,
        env: &E,
    ) -> impl Future<Output = Result<(Self::Header, Self::Output), Error>> + Send;
}

/// Operation that returns a stream of values
pub trait OperationStream<E: Env>: Clone + Send + 'static {
    type Output: 'static + Send;
    type PeerAddr: PeerAddr;

    fn peer_addr(&self) -> &Self::PeerAddr;

    /// Execute the operation and return a stream of results
    fn send(
        self,
        env: &E,
    ) -> impl Future<Output = Result<impl ResponseStream<Self::Output>, Error>> + Send;
}

/// Operation that returns a stream of values with a header
pub trait OperationStreamWithHeader<E: Env>: Clone + Send + 'static {
    type Output: 'static + Send;
    type Header: Send;
    type PeerAddr: PeerAddr;

    fn peer_addr(&self) -> &Self::PeerAddr;

    /// Execute the operation and return a stream of results with header
    fn send(
        self,
        env: &E,
    ) -> impl Future<Output = Result<(Self::Header, impl ResponseStream<Self::Output>), Error>> + Send;
}

pub trait PeerAddr: 'static + Clone + Send + Sync {}

impl<T: 'static + Clone + Send + Sync> PeerAddr for T {}

pub trait RawRequest: 'static + Clone + Send + Sync {
    fn serialized_len(&self) -> usize;
}

pub trait Method<S: Service>: Sized + Send + Sync + Clone + 'static {
    type RawRequest: RawRequest;

    /// Returns true if the method is supported by the peer
    fn is_supported(&self) -> bool;

    /// Serializes a request for submission
    fn create_raw_request<E: Env, H, I>(
        &self,
        env: &E,
        header: Option<H>,
        input: I,
    ) -> Self::RawRequest
    where
        H: RequestHeader<S>,
        I: Request<S>;

    /// Sends a request and receives a response
    fn send<E: Env, Req, Res, Span>(
        &self,
        env: &E,
        request: Self::RawRequest,
        span: Span,
    ) -> impl Future<Output = Result<Res, Error>> + Send
    where
        Span: span::Unary<Req, Res>,
        Res: Response<S>;

    /// Sends a request without expecting a response (unidirectional)
    fn send_unidirectional<E: Env, Req, Span>(
        &self,
        env: &E,
        request: Self::RawRequest,
        span: Span,
    ) -> impl Future<Output = Result<(), Error>> + Send
    where
        Span: span::Unidirectional<Req>;

    /// Sends a request with a header and receives a response
    fn send_with_response_header<E: Env, Req, ResH, Res, Span>(
        &self,
        env: &E,
        request: Self::RawRequest,
        span: Span,
    ) -> impl Future<Output = Result<(ResH, Res), Error>> + Send
    where
        ResH: ResponseHeader<S>,
        Res: Response<S>,
        Span: span::Unary<Req, (ResH, Res)>;

    /// Sends a stream of requests and receives a stream of responses.
    ///
    /// `T` is the task class for the spawned request-write pump, supplied by the
    /// generated consumer (a per-method client class) so the pump's
    /// scheduling/poll metrics are attributed to this method on the client side.
    fn send_stream_bidi<T, E: Env, ReqH, Req, Res, Span>(
        self,
        env: &E,
        header: Option<ReqH>,
        input: impl RequestStream<Req>,
        span: Span,
    ) -> impl Future<Output = Result<impl ResponseStream<Res>, Error>> + Send + 'static
    where
        T: upstream_runtime::task_monitor::TaskClass<E> + Default,
        ReqH: RequestHeader<S>,
        Req: Request<S>,
        Res: Response<S>,
        Span: span::bidi::Stream<Req, Res>;

    /// Sends a stream of requests and receives a stream of responses with headers.
    ///
    /// `T` is the per-method client task class for the spawned write pump (see
    /// [`send_stream_bidi`](Self::send_stream_bidi)).
    fn send_stream_bidi_with_response_header<T, E: Env, ReqH, Req, ResH, Res, Span>(
        self,
        env: &E,
        header: Option<ReqH>,
        input: impl RequestStream<Req>,
        span: Span,
    ) -> impl Future<Output = Result<(ResH, impl ResponseStream<Res>), Error>> + Send + 'static
    where
        T: upstream_runtime::task_monitor::TaskClass<E> + Default,
        ReqH: RequestHeader<S>,
        Req: Request<S>,
        ResH: ResponseHeader<S>,
        Res: Response<S>,
        Span: span::bidi::Stream<Req, Res>;

    /// Sends a stream of requests and receives a single response
    fn send_stream_request<E: Env, ReqH, Req, Res, Span>(
        &self,
        env: &E,
        header: Option<ReqH>,
        input: impl RequestStream<Req>,
        span: Span,
    ) -> impl Future<Output = Result<Res, Error>> + Send
    where
        ReqH: RequestHeader<S>,
        Req: Request<S>,
        Res: Response<S>,
        Span: span::StreamRequest<Req, Res>;

    /// Sends a stream of requests and receives a single response with header
    fn send_stream_request_with_response_header<E: Env, ReqH, Req, ResH, Res, Span>(
        &self,
        env: &E,
        header: Option<ReqH>,
        input: impl RequestStream<Req>,
        span: Span,
    ) -> impl Future<Output = Result<(ResH, Res), Error>> + Send
    where
        ReqH: RequestHeader<S>,
        Req: Request<S>,
        ResH: ResponseHeader<S>,
        Res: Response<S>,
        Span: span::StreamRequest<Req, (ResH, Res)>;

    /// Sends a single request and receives a stream of responses
    fn send_stream_response<E: Env, Req, Res, Span>(
        self,
        env: &E,
        request: Self::RawRequest,
        span: Span,
    ) -> impl Future<Output = Result<impl ResponseStream<Res>, Error>> + Send + 'static
    where
        Res: Response<S>,
        Span: span::StreamResponse<Req, Res>;

    /// Sends a single request and receives a stream of responses with header
    fn send_stream_response_with_response_header<E: Env, Req, ResH, Res, Span>(
        self,
        env: &E,
        request: Self::RawRequest,
        span: Span,
    ) -> impl Future<Output = Result<(ResH, impl ResponseStream<Res>), Error>> + Send + 'static
    where
        ResH: ResponseHeader<S>,
        Res: Response<S>,
        Span: span::StreamResponse<Req, Res>;

    /// Sends a request and receives a raw response via a `rpc_response::Response`
    /// that supports async backpressure via `provide_storage()`.
    fn send_raw<E: Env, Req, Res, Span>(
        &self,
        env: &E,
        request: Self::RawRequest,
        response: Res,
        span: Span,
    ) -> impl Future<Output = Result<Res::Output, Error>> + Send
    where
        Res: crate::rpc_response::Response + Send,
        Res::Storage: Send,
        Res::Output: Send,
        Span: span::Unary<Req, usize>;
}

pub trait Peer<S: Service>: 'static + Send + Sync {
    type Method: Method<S>;

    fn resolve(&self, method: S::Methods) -> Self::Method;
}

pub trait Registry<S: Service, Addr: PeerAddr>: 'static + Send + Sync + Clone {
    type Peer: Peer<S>;

    /// Resolve a single peer
    fn resolve_peer(
        &self,
        peer_addr: &Addr,
    ) -> impl Future<Output = Result<Self::Peer, Error>> + Send;

    /// Resolve a multicast group
    fn resolve_multicast(
        &self,
        addrs: impl Iterator<Item = Addr> + Send,
    ) -> impl Future<Output = Result<Vec<Self::Peer>, Error>> + Send {
        let registry = self.clone();
        async move {
            let cap = addrs.size_hint().1.unwrap_or(1);
            let mut peers = Vec::with_capacity(cap);
            for addr in addrs {
                let peer = registry.resolve_peer(&addr).await?;
                peers.push(peer);
            }
            Ok(peers)
        }
    }
}

pub trait SupportedRegistry<S: Service, PeerAddr>: 'static + Send + Sync + Clone {
    fn resolve_supported_set(
        &self,
        peer_addr: &PeerAddr,
    ) -> impl Future<Output = Result<S::SupportedSet, Error>> + Send;
}

/// A registry that returns the same `SupportedSet` for every peer.
///
/// Constructors:
/// - `StaticSupportedRegistry::all()` — pre-negotiation, all methods at current version.
/// - `StaticSupportedRegistry::new(set)` — inject a pre-computed set.
/// - `StaticSupportedRegistry::from_negotiated(peer_global_ids)` — build from negotiated ids.
pub struct StaticSupportedRegistry<S: Service>(S::SupportedSet);

impl<S: Service> StaticSupportedRegistry<S> {
    /// Construct from a pre-computed `SupportedSet`.
    pub const fn new(set: S::SupportedSet) -> Self {
        Self(set)
    }

    /// All methods supported at the current version (pre-negotiation path).
    pub fn all() -> Self
    where
        S::SupportedSet: crate::SupportedSet,
    {
        Self(<S::SupportedSet as crate::SupportedSet>::all())
    }

    /// Build from negotiated per-peer `global_id`s.
    pub fn from_negotiated(peer_global_ids: &[crate::GlobalId]) -> Self
    where
        S::SupportedSet: crate::SupportedSet,
    {
        Self(<S::SupportedSet as crate::SupportedSet>::from_negotiated(
            S::methods(),
            peer_global_ids,
        ))
    }
}

impl<S: Service> Clone for StaticSupportedRegistry<S> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<S: Service, PeerAddr> SupportedRegistry<S, PeerAddr> for StaticSupportedRegistry<S> {
    fn resolve_supported_set(
        &self,
        _peer_addr: &PeerAddr,
    ) -> impl Future<Output = Result<S::SupportedSet, Error>> + Send {
        let set = self.0.clone();
        async move { Ok(set) }
    }
}
