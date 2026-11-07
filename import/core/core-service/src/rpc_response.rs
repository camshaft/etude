/// Transport-agnostic trait for RPC response handling with async backpressure.
///
/// This provides an async `provide_storage()` hook that transports call between
/// reads, allowing implementations to apply backpressure by awaiting before
/// providing more storage. Transport implementations bridge to their native
/// backpressure mechanisms (e.g., QUIC flow control) in their
/// `Client::request_response_into` impl.
use upstream_buffer::writer;
use std::{future::Future, io};

pub trait Response {
    type Storage: writer::Storage;
    type Output;

    /// Provides storage for the next chunk of response data.
    ///
    /// Implementations can `.await` here to apply backpressure — the transport
    /// will not read more data until storage is provided. The returned storage
    /// must have capacity of at least 1, or the operation will be aborted.
    fn provide_storage(&mut self) -> impl Future<Output = io::Result<&mut Self::Storage>> + Send;

    /// Called when the peer has transmitted the entire response.
    fn finish(self) -> impl Future<Output = io::Result<Self::Output>> + Send;
}

/// Trivial implementation that always provides the same storage buffer.
/// No backpressure — `provide_storage()` returns immediately.
pub struct InMemoryResponse<S>(S);

impl<S> From<S> for InMemoryResponse<S>
where
    S: writer::Storage,
{
    fn from(value: S) -> Self {
        InMemoryResponse(value)
    }
}

impl<S> Response for InMemoryResponse<S>
where
    S: writer::Storage + Send,
{
    type Storage = S;
    type Output = S;

    async fn provide_storage(&mut self) -> io::Result<&mut Self::Storage> {
        Ok(&mut self.0)
    }

    async fn finish(self) -> io::Result<Self::Output> {
        Ok(self.0)
    }
}
