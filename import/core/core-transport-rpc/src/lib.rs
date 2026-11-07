// Task classes for this crate's spawn sites live in core_events (this crate
// records metrics directly against the registry and has no event pipeline of
// its own). Alias the dependency so spawn sites can name `core_events::events`.
extern crate upstream_core_events as core_events;

#[cfg(test)]
#[macro_use]
mod testing;

mod traits;
pub use traits::{Client, GenericStream, PendingStream, Reader, Server, Writer};

pub mod negotiation;

pub trait Rpc: 'static + Clone + Send + Sync {
    type Client: Client;
    type Server: Server;
    type Handshake: negotiation::Handshake;

    const NAME: &'static str;

    /// The handshake this transport built from its `Config`, if negotiation
    /// was configured. This is the single source of the transport's peer
    /// registry — consumers reach it via `Client::handshake().peer_registry()`.
    /// The default returns `None` (no negotiation).
    fn handshake(&self) -> Option<std::sync::Arc<dyn negotiation::Handshake>> {
        None
    }

    fn create_client(
        &self,
    ) -> impl std::future::Future<Output = std::io::Result<Self::Client>> + Send;

    fn create_server(
        &self,
        listen_addr: std::net::SocketAddr,
    ) -> impl std::future::Future<Output = std::io::Result<Self::Server>> + Send;
}

pub mod async_io;
pub mod config;
#[cfg(feature = "dcquic")]
pub mod dcquic;
pub mod mem;
pub mod mux;
#[cfg(any(test, feature = "testing"))]
pub mod mux_bach;
#[cfg(feature = "quic")]
pub mod quic;
pub mod service;
pub mod tcp;

pub use config::Config;

pub fn default_worker_count() -> usize {
    if cfg!(debug_assertions) && ::bach::is_active() {
        return 1;
    }

    use std::sync::OnceLock;

    static COUNT: OnceLock<usize> = OnceLock::new();

    let count = *COUNT.get_or_init(|| {
        if let Ok(v) = std::env::var("UPSTREAM_RPC_WORKER_COUNT") {
            return v
                .parse()
                .unwrap_or_else(|_| panic!("invalid core count: {v:?}"));
        }

        std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(1)
    });
    tracing::debug!("Using RPC worker count: {}", count);
    count
}
