mod client;
pub(crate) mod common;
mod server;
pub(crate) mod stall;
#[cfg(test)]
mod stall_tests;

pub use client::{ClientPeer, ClientRegistry, MethodHandle, TransportNegotiated};
#[cfg(feature = "bench")]
#[doc(hidden)]
pub use common::{WriteInstrument, write_framed_stream_at};
pub use server::ServerAcceptor;
#[cfg(test)]
pub(crate) use server::read_request_header;
pub use stall::StallTimeout;
#[cfg(feature = "bench")]
#[doc(hidden)]
pub use stall::{Progress, StallPolicy};
