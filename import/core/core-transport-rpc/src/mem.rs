//! In-memory RPC transport over a bach virtual network.
//!
//! The byte-stream substrate ([`upstream_bach_net::BachDuplexStream`]), the
//! address-keyed virtual network ([`upstream_bach_net::Network`]), and the
//! impairment/latency fault-injection model live in `amzn-upstream-bach-net`;
//! this module layers the RPC [`crate::Rpc`]/[`crate::Client`]/[`crate::Server`]
//! traits, the negotiation handshake, and stream framing on top of that
//! network so both this transport and the HTTP stack share one bach network.

use upstream_bach_net::Network;
use std::{collections::VecDeque, sync::Arc};

pub use client::Client;
pub use upstream_bach_net::{
    ImpairGuard, ImpairmentState, set_connect_impairment_enabled, set_latency_in_millis,
    set_max_listener_capacity, set_max_stream_capacity, set_message_impairment_enabled,
};
pub use server::Server;

#[derive(Clone)]
pub struct Transport {
    net: Network,
    handshake: Option<Arc<dyn crate::negotiation::Handshake>>,
}

impl Default for Transport {
    /// The mem transport joins the ambient (per-sim) bach network rather than a fresh isolated one, so every
    /// transport built within a sim — cluster nodes, the gateway, and clients constructed independently from
    /// config alike — dials the same virtual network. That lets a config-built client reach a running cluster
    /// by address, the same way the production gossip path dials real seeds, with no shared transport handle
    /// to thread through. There is one network per sim (the `sim` harness resets the ambient network per run),
    /// and no test runs more than one cluster, so a single ambient network per sim is unambiguous.
    fn default() -> Self {
        Self {
            net: upstream_bach_net::current(),
            handshake: None,
        }
    }
}

impl Transport {
    /// Build a mem transport, constructing its handshake (and a fresh peer
    /// registry) from `config` when negotiation services are configured.
    pub fn new(config: crate::Config) -> Self {
        Self {
            handshake: config.build_default_handshake(),
            ..Default::default()
        }
    }

    /// Inject a pre-built handshake directly. Retained as an inherent method
    /// for tests that construct a `DefaultPeerServiceNegotiator` by hand rather
    /// than going through `Config::build_default_handshake`.
    pub fn with_handshake(
        mut self,
        handshake: std::sync::Arc<dyn crate::negotiation::Handshake>,
    ) -> Self {
        self.handshake = Some(handshake);
        self
    }

    /// Impairs network connectivity between two addresses.
    /// Returns a guard that will restore connectivity when dropped.
    pub fn impair_between<S, D>(&self, source: S, dest: D) -> ImpairGuard
    where
        (S, u16): upstream_bach_net::ToSocketAddrs,
        (D, u16): upstream_bach_net::ToSocketAddrs,
    {
        self.net.impair_between(source, dest)
    }

    /// Impairs all network connectivity to/from the specified address.
    /// Returns a guard that will restore connectivity when dropped.
    pub fn impair<A>(&self, addr: A) -> ImpairGuard
    where
        (A, u16): upstream_bach_net::ToSocketAddrs,
    {
        self.net.impair(addr)
    }

    /// Impairs network connectivity between two addresses with given pattern.
    /// Returns a guard that will restore connectivity immediately when dropped.
    pub fn preset_impair_between<S, D>(
        &self,
        source: S,
        dest: D,
        default: bool,
        connect_presets: VecDeque<bool>,
        message_presets: VecDeque<bool>,
    ) -> ImpairGuard
    where
        (S, u16): upstream_bach_net::ToSocketAddrs,
        (D, u16): upstream_bach_net::ToSocketAddrs,
    {
        self.net
            .preset_impair_between(source, dest, default, connect_presets, message_presets)
    }

    /// Impairs all network connectivity to/from the specified address with given pattern.
    /// Returns a guard that will restore connectivity immediately when dropped.
    pub fn preset_impair<A>(
        &self,
        addr: A,
        default: bool,
        connect_presets: VecDeque<bool>,
        message_presets: VecDeque<bool>,
    ) -> ImpairGuard
    where
        (A, u16): upstream_bach_net::ToSocketAddrs,
    {
        self.net
            .preset_impair(addr, default, connect_presets, message_presets)
    }

    /// Randomly impairs network connectivity between two addresses.
    /// Returns a guard that will restore connectivity immediately when dropped.
    pub fn random_impair_between<S, D>(&self, source: S, dest: D) -> ImpairGuard
    where
        (S, u16): upstream_bach_net::ToSocketAddrs,
        (D, u16): upstream_bach_net::ToSocketAddrs,
    {
        self.net.random_impair_between(source, dest)
    }

    /// Impairs all network connectivity to/from the specified address with given pattern.
    /// Returns a guard that will restore connectivity immediately when dropped.
    pub fn random_impair<A>(&self, addr: A) -> ImpairGuard
    where
        (A, u16): upstream_bach_net::ToSocketAddrs,
    {
        self.net.random_impair(addr)
    }

    /// Builds a client that dials from an explicit source address (resolved
    /// through bach's DNS), for tests that need a specific client identity to
    /// impair. Prefer [`create_client`](crate::Rpc::create_client), whose source
    /// bach assigns on the calling task's group.
    #[inline]
    pub async fn create_client_with(
        &self,
        addr: impl upstream_bach_net::ToSocketAddrs,
    ) -> std::io::Result<Client> {
        let local = upstream_bach_net::resolve(addr).await?;
        Ok(Client::new(
            self.net.clone(),
            Some(local),
            self.handshake.clone(),
        ))
    }

    /// Returns the configured handshake, if any.
    pub fn handshake(&self) -> Option<&Arc<dyn crate::negotiation::Handshake>> {
        self.handshake.as_ref()
    }
}

impl crate::Rpc for Transport {
    type Client = Client;
    type Server = Server;

    type Handshake = std::sync::Arc<dyn crate::negotiation::Handshake>;
    const NAME: &'static str = "mem";

    fn handshake(&self) -> Option<std::sync::Arc<dyn crate::negotiation::Handshake>> {
        self.handshake.clone()
    }

    #[inline]
    async fn create_client(&self) -> std::io::Result<Self::Client> {
        Ok(Client::new(self.net.clone(), None, self.handshake.clone()))
    }

    #[inline]
    async fn create_server(
        &self,
        listen_addr: std::net::SocketAddr,
    ) -> std::io::Result<Self::Server> {
        let acceptor = self.net.bind(listen_addr).await?;
        Ok(Server::new(acceptor, self.handshake.clone()))
    }
}

mod client {
    use super::*;
    use crate::async_io;
    use upstream_bach_net::{BachDuplexStream, Network};
    use upstream_service::ServerAddr;
    use std::{future::Future, net::SocketAddr};
    use tokio::io;

    #[derive(Clone)]
    pub struct Client {
        net: Network,
        /// Explicit source address, when the caller pinned one via
        /// `create_client_with`; otherwise bach assigns the source on the
        /// calling task's group at connect time.
        local_addr: Option<SocketAddr>,
        /// Off bach (no group to source from), the client's source is pinned
        /// once here on its first connect and reused, so the client keeps one
        /// stable identity across reconnects rather than churning a fresh source
        /// each call. Shared across clones (a clone is the same logical client).
        /// On bach this stays unset — the group supplies the source per connect.
        off_bach_source: Arc<std::sync::OnceLock<SocketAddr>>,
        hs: Option<Arc<dyn crate::negotiation::Handshake>>,
    }

    impl Client {
        pub(super) fn new(
            net: Network,
            local_addr: Option<SocketAddr>,
            hs: Option<Arc<dyn crate::negotiation::Handshake>>,
        ) -> Self {
            Self {
                net,
                local_addr,
                off_bach_source: Arc::new(std::sync::OnceLock::new()),
                hs,
            }
        }

        #[inline]
        async fn connect(&self, addr: SocketAddr) -> io::Result<BachDuplexStream> {
            if let Some(local) = self.local_addr {
                return self.net.connect_from(local, addr).await;
            }
            // Off bach, pin one source per client (not per connect) so identity
            // is stable and cannot churn; on bach the group assigns per connect.
            if !upstream_bach_net::is_active() {
                let local = *self
                    .off_bach_source
                    .get_or_init(upstream_bach_net::ephemeral_source_off_bach);
                return self.net.connect_from(local, addr).await;
            }
            self.net.connect(addr).await
        }
    }

    impl crate::Client for Client {
        type Reader = async_io::AsyncReader<io::ReadHalf<BachDuplexStream>>;
        type Writer = async_io::AsyncWriter<io::WriteHalf<BachDuplexStream>>;

        type Handshake = std::sync::Arc<dyn crate::negotiation::Handshake>;
        fn open_stream(
            &self,
            addr: ServerAddr,
            _priority: upstream_service::Priority,
        ) -> impl Future<Output = io::Result<(Self::Writer, Self::Reader)>> + Send + 'static
        {
            let addr = addr.acceptor_addr();
            let client = self.clone();
            async move {
                let stream = client.connect(addr).await?;
                let (reader, writer) = io::split(stream);
                Ok((
                    async_io::AsyncWriter::new(writer),
                    async_io::AsyncReader::new(reader),
                ))
            }
        }

        fn handshake(&self) -> Option<&Arc<dyn crate::negotiation::Handshake>> {
            self.hs.as_ref()
        }
    }
}

mod server {
    use super::*;
    use crate::async_io;
    use upstream_bach_net::{Acceptor, BachDuplexStream};
    use upstream_service::ServerAddr;
    use tokio::io;

    #[derive(Clone)]
    pub struct Server {
        acceptor: Acceptor,
        hs: Option<Arc<dyn crate::negotiation::Handshake>>,
    }

    impl Server {
        #[inline]
        pub(super) fn new(
            acceptor: Acceptor,
            hs: Option<Arc<dyn crate::negotiation::Handshake>>,
        ) -> Self {
            Self { acceptor, hs }
        }
    }

    impl crate::Server for Server {
        type Stream = crate::GenericStream<
            async_io::AsyncReader<io::ReadHalf<BachDuplexStream>>,
            async_io::AsyncWriter<io::WriteHalf<BachDuplexStream>>,
        >;

        type Handshake = std::sync::Arc<dyn crate::negotiation::Handshake>;
        fn handshake(&self) -> Option<&Self::Handshake> {
            self.hs.as_ref()
        }
        async fn accept(&mut self) -> Option<Self::Stream> {
            let (stream, addr) = self.acceptor.accept().await?;
            let (reader, writer) = io::split(stream);

            Some(crate::GenericStream::new(
                async_io::AsyncReader::new(reader),
                async_io::AsyncWriter::new(writer),
                addr,
                self.hs.clone(),
            ))
        }

        fn local_addr(&self) -> ServerAddr {
            // mem has no separate handshake socket. If a negotiation
            // handshake is attached, advertise its `local_handshake_port`
            // so leaders looking up peers by `addr.handshake_port` match
            // what the negotiator stores; otherwise fall back to the
            // bound port. dcQUIC reports its bound handshake socket port
            // for the same reason.
            ServerAddr::from_local_addr(
                self.acceptor.local_addr(),
                self.hs.as_ref().map(|hs| hs.local_handshake_port()),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Client as _, Reader, Rpc as _, Server as _, Writer};
    use bytes::BytesMut;
    use upstream_service::ServerAddr;
    use upstream_testing::{PrimaryExt, SpawnExt, sim_coop_exhaustive};
    use std::{io, net::SocketAddr};
    use tokio::task::JoinSet;

    test_rpc!(super::Transport);

    #[tokio::test]
    async fn multiple_servers_test() {
        let transport = Transport::default();

        let addr = ServerAddr::from_same_handshake_port("127.0.0.1:443".parse().unwrap());

        let server_count = 4u64;

        for idx in 0..server_count {
            let mut server = transport.create_server(addr.acceptor_addr()).await.unwrap();
            tokio::spawn(async move {
                // store the request queue to reduce allocations
                let mut buffer: Vec<BytesMut> = vec![];
                while let Some(stream) = server.accept().await {
                    let (request, response, _addr) =
                        (stream.reader, stream.writer, stream.peer_addr);
                    let task = async {
                        buffer.clear();

                        request.read_to_end(&mut buffer).await?;

                        // Do something with the request here - here we add some artificial delay
                        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

                        // respond with the server ID
                        response.write_to_end(&idx.to_be_bytes()[..]).await?;

                        <io::Result<_>>::Ok(())
                    };

                    match task.await {
                        Ok(_) => {}
                        Err(err) => {
                            tracing::error!("error processing request: {err}");
                        }
                    }
                }
            });
        }

        let client = transport.create_client().await.unwrap();
        let mut requests = JoinSet::new();
        for _ in 0..20 {
            let client = client.clone();
            requests.spawn(async move {
                let (request, response) = client
                    .connect(addr, upstream_service::Priority::default())
                    .await
                    .unwrap();
                request.write_to_end(&b"hello!"[..]).await.unwrap();
                let mut payload: Vec<u8> = vec![];
                response.read_to_end(&mut payload).await.unwrap();
                assert_eq!(payload.len(), 8);
                u64::from_be_bytes(payload.try_into().unwrap())
            });
        }

        // make sure we get responses from everyone
        let mut pending_seen: std::collections::HashSet<u64> = (0..server_count).collect();
        while let Some(res) = requests.join_next().await {
            pending_seen.remove(&res.unwrap());
        }
        assert!(pending_seen.is_empty());
    }

    #[tokio::test]
    async fn test_impair() {
        let transport = Transport::default();
        let addr = ServerAddr::from_same_handshake_port("127.0.0.1:443".parse().unwrap());

        // Create a server
        let _server = transport.create_server(addr.acceptor_addr()).await.unwrap();
        let client = transport.create_client().await.unwrap();

        // Connection should work initially
        client
            .connect(addr, upstream_service::Priority::default())
            .await
            .unwrap();

        // Impair the connection
        let guard = transport.impair(addr.ip());
        assert!(
            client
                .connect(addr, upstream_service::Priority::default())
                .await
                .is_err()
        );

        // Drop the guard to restore connectivity
        drop(guard);
        client
            .connect(addr, upstream_service::Priority::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_impair_between() {
        let transport = Transport::default();

        // Use different IPs for each server
        let server1_addr = ServerAddr::from_same_handshake_port("127.0.0.2:443".parse().unwrap());
        let server2_addr = ServerAddr::from_same_handshake_port("127.0.0.3:443".parse().unwrap());
        let client_addr: SocketAddr = "127.0.0.4:443".parse().unwrap();

        // Create two servers
        let _server1 = transport
            .create_server(server1_addr.acceptor_addr())
            .await
            .unwrap();
        let _server2 = transport
            .create_server(server2_addr.acceptor_addr())
            .await
            .unwrap();

        // Create client with specific address
        let client = transport.create_client_with(client_addr).await.unwrap();

        // Both connections should work initially
        client
            .connect(server1_addr, upstream_service::Priority::default())
            .await
            .unwrap();
        client
            .connect(server2_addr, upstream_service::Priority::default())
            .await
            .unwrap();

        // Impair connection between the servers
        let guard = transport.impair_between(server1_addr.ip(), server2_addr.ip());

        // Client should still be able to connect to both servers
        client
            .connect(server1_addr, upstream_service::Priority::default())
            .await
            .unwrap();
        client
            .connect(server2_addr, upstream_service::Priority::default())
            .await
            .unwrap();

        // Create clients with server addresses
        let client1 = transport
            .create_client_with(server1_addr.acceptor_addr())
            .await
            .unwrap();
        let client2 = transport
            .create_client_with(server2_addr.acceptor_addr())
            .await
            .unwrap();

        // Connection between servers should be impaired
        assert!(
            client1
                .connect(server2_addr, upstream_service::Priority::default())
                .await
                .is_err()
        );
        assert!(
            client2
                .connect(server1_addr, upstream_service::Priority::default())
                .await
                .is_err()
        );

        // Drop the guard to restore connectivity
        drop(guard);
        client1
            .connect(server2_addr, upstream_service::Priority::default())
            .await
            .unwrap();
        client2
            .connect(server1_addr, upstream_service::Priority::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_impair_write_after_established_connection() {
        let transport = Transport::default();
        let addr = ServerAddr::from_same_handshake_port("127.0.0.1:443".parse().unwrap());

        // Create a server
        let server = transport.create_server(addr.acceptor_addr()).await.unwrap();
        let mut server_clone = server.clone();
        let server_idx: u64 = 0;

        tokio::spawn(async move {
            while let Some(stream) = server_clone.accept().await {
                let (_request, response, _addr) = (stream.reader, stream.writer, stream.peer_addr);
                let _ = response.write_to_end(&server_idx.to_be_bytes()[..]).await;
            }
        });

        let client = transport.create_client().await.unwrap();
        {
            // Connection should work initially
            let (request, response) = client
                .connect(addr, upstream_service::Priority::default())
                .await
                .unwrap();
            // Call write and confirm it works
            request.write_to_end(&b"hello!"[..]).await.unwrap();
            let mut payload: Vec<u8> = vec![];
            response.read_to_end(&mut payload).await.unwrap();
            assert_eq!(payload.len(), 8);
            assert_eq!(server_idx, u64::from_be_bytes(payload.try_into().unwrap()));
        }

        let (mut request, mut _response) = client
            .connect(addr, upstream_service::Priority::default())
            .await
            .unwrap();
        request.write_chunk(&mut &b"hello!"[..]).await.unwrap();
        // Impair the connection
        let guard = transport.impair(addr.ip());
        // Call write again and confirm it throws error since it is now impaired.
        assert!(request.write_chunk(&mut &b"hello!"[..]).await.is_err());
        // Drop the guard to restore connectivity
        drop(guard);

        // After impairment is cleared, verify it works again.
        let (request, response) = client
            .connect(addr, upstream_service::Priority::default())
            .await
            .unwrap();
        request.write_to_end(&b"hello!"[..]).await.unwrap();
        let mut payload: Vec<u8> = vec![];
        response.read_to_end(&mut payload).await.unwrap();
        assert_eq!(payload.len(), 8);
        assert_eq!(server_idx, u64::from_be_bytes(payload.try_into().unwrap()));
    }

    #[tokio::test]
    async fn test_impair_read_after_established_connection() {
        let transport = Transport::default();
        let addr = ServerAddr::from_same_handshake_port("127.0.0.1:443".parse().unwrap());

        // Create a server that sends data immediately
        let server = transport.create_server(addr.acceptor_addr()).await.unwrap();
        let mut server_clone = server.clone();
        let server_idx: u64 = 0;

        tokio::spawn(async move {
            while let Some(stream) = server_clone.accept().await {
                let (_request, mut response, _addr) =
                    (stream.reader, stream.writer, stream.peer_addr);
                let _ = response
                    .write_chunk(&mut &server_idx.to_be_bytes()[..])
                    .await;
            }
        });

        let client = transport.create_client().await.unwrap();

        let (_request, mut response) = client
            .connect(addr, upstream_service::Priority::default())
            .await
            .unwrap();
        // Read successfully before impairment
        let mut payload: Vec<u8> = vec![];
        response.read_chunk(&mut payload).await.unwrap();
        assert_eq!(payload.len(), 8);

        // Impair the connection
        let guard = transport.impair(addr.ip());

        // After impair, read should fail.
        let mut payload2: Vec<u8> = vec![];
        assert!(response.read_chunk(&mut payload2).await.is_err());

        // Drop the guard to restore connectivity
        drop(guard);

        // After impairment is cleared, verify it works again.
        {
            let (_request, mut response) = client
                .connect(addr, upstream_service::Priority::default())
                .await
                .unwrap();
            let mut payload: Vec<u8> = vec![];
            response.read_chunk(&mut payload).await.unwrap();
            assert_eq!(payload.len(), 8);
        }
    }

    #[tokio::test]
    async fn test_preset_connect_only() {
        set_connect_impairment_enabled(true);
        set_message_impairment_enabled(false);

        let transport = Transport::default();
        let server_addr = ServerAddr::from_same_handshake_port("127.0.0.2:443".parse().unwrap());
        let client_addr: SocketAddr = "127.0.0.3:443".parse().unwrap();

        let _server = transport
            .create_server(server_addr.acceptor_addr())
            .await
            .unwrap();

        // Preset: allow 2 connections, no message control
        // Impair specific pair to avoid double-counting when source==dest
        let _guard = transport.preset_impair_between(
            client_addr.ip(),
            server_addr.ip(),
            true,
            VecDeque::from([false, false]),
            VecDeque::new(),
        );

        let client = transport.create_client_with(client_addr).await.unwrap();

        // Connection 1: should succeed (uses connect token #1)
        let (request1, _response1) = client
            .connect(server_addr, upstream_service::Priority::default())
            .await
            .unwrap();
        request1.write_to_end(&b"test1"[..]).await.unwrap();

        // Connection 2: should succeed (uses connect token #2)
        let (request2, _response2) = client
            .connect(server_addr, upstream_service::Priority::default())
            .await
            .unwrap();
        request2.write_to_end(&b"test2"[..]).await.unwrap();

        // Connection 3: should fail (connect tokens exhausted, default=true blocks)
        assert!(
            client
                .connect(server_addr, upstream_service::Priority::default())
                .await
                .is_err()
        );

        // Reset for cleanup
        set_message_impairment_enabled(true);
    }

    #[tokio::test]
    async fn test_preset_message_only() {
        // Reset flags at start
        set_connect_impairment_enabled(false); // Disable connection checking for this test
        set_message_impairment_enabled(true); // Enable message checking

        let transport = Transport::default();
        let server_addr = ServerAddr::from_same_handshake_port("127.0.0.2:443".parse().unwrap());
        let client_addr: SocketAddr = "127.0.0.3:443".parse().unwrap();

        const MESSAGE_TOKEN_COUNT: usize = 10;

        // Preset: no connection control, control message count
        let _guard = transport.preset_impair_between(
            client_addr.ip(),
            server_addr.ip(),
            true,
            VecDeque::new(),
            VecDeque::from([false; MESSAGE_TOKEN_COUNT]),
        );

        let _server = transport
            .create_server(server_addr.acceptor_addr())
            .await
            .unwrap();
        let client = transport.create_client_with(client_addr).await.unwrap();

        // Connection should succeed
        let (mut request, _response) = client
            .connect(server_addr, upstream_service::Priority::default())
            .await
            .unwrap();

        // First 10 should succeed
        for i in 0..MESSAGE_TOKEN_COUNT {
            request
                .write_chunk(&mut &[i as u8][..])
                .await
                .expect(&format!(
                    "Write #{} should succeed (have {} tokens)",
                    i + 1,
                    MESSAGE_TOKEN_COUNT
                ));
        }

        // Then it should fail
        assert!(
            request
                .write_chunk(&mut &[MESSAGE_TOKEN_COUNT as u8][..])
                .await
                .is_err(),
            "Write #{} should fail (tokens exhausted)",
            MESSAGE_TOKEN_COUNT + 1
        );

        // Reset
        set_connect_impairment_enabled(true);
    }

    #[tokio::test]
    async fn test_permanent_impair_ignores_preset_flags() {
        let transport = Transport::default();
        let addr = ServerAddr::from_same_handshake_port("127.0.0.1:443".parse().unwrap());

        let _server = transport.create_server(addr.acceptor_addr()).await.unwrap();
        let _guard = transport.impair(addr.ip());
        let client = transport.create_client().await.unwrap();

        // Test with flags enabled - should block
        set_connect_impairment_enabled(true);
        set_message_impairment_enabled(true);
        assert!(
            client
                .connect(addr, upstream_service::Priority::default())
                .await
                .is_err(),
            "Permanent should block with flags enabled"
        );

        // Test with flags disabled - should still block
        set_connect_impairment_enabled(false);
        set_message_impairment_enabled(false);
        assert!(
            client
                .connect(addr, upstream_service::Priority::default())
                .await
                .is_err(),
            "Permanent should block even with flags disabled"
        );

        // Reset
        set_connect_impairment_enabled(true);
        set_message_impairment_enabled(true);
    }

    #[tokio::test]
    async fn test_random_impair_ignores_preset_flags() {
        let transport = Transport::default();
        let addr = ServerAddr::from_same_handshake_port("127.0.0.1:443".parse().unwrap());

        let _server = transport.create_server(addr.acceptor_addr()).await.unwrap();
        let _guard = transport.random_impair(addr.ip());
        let client = transport.create_client().await.unwrap();

        // Test with flags enabled
        set_connect_impairment_enabled(true);
        set_message_impairment_enabled(true);

        let mut had_success = false;
        let mut had_failure = false;
        for _ in 0..50 {
            if client
                .connect(addr, upstream_service::Priority::default())
                .await
                .is_err()
            {
                had_failure = true;
            } else {
                had_success = true;
            }
            if had_success && had_failure {
                break;
            }
        }
        assert!(
            had_success && had_failure,
            "Random should cause BOTH successes and failures with flags enabled"
        );

        // Test with flags disabled - behavior should be the same
        set_connect_impairment_enabled(false);
        set_message_impairment_enabled(false);

        let mut had_success_disabled = false;
        let mut had_failure_disabled = false;
        for _ in 0..50 {
            if client
                .connect(addr, upstream_service::Priority::default())
                .await
                .is_err()
            {
                had_failure_disabled = true;
            } else {
                had_success_disabled = true;
            }
            if had_success_disabled && had_failure_disabled {
                break;
            }
        }
        assert!(
            had_success_disabled && had_failure_disabled,
            "Random should cause BOTH successes and failures even with flags disabled"
        );

        set_connect_impairment_enabled(true);
        set_message_impairment_enabled(true);
    }

    #[test]
    fn bach_coop_test_mem_transport() {
        static LOG: upstream_testing::Log = upstream_testing::Log::new();

        bolero::check!().exhaustive().run(sim_coop_exhaustive(|| {
            LOG.push("START");
            set_latency_in_millis(1, 2);
            let transport = Transport::default();
            let addr = ServerAddr::from_same_handshake_port("127.0.0.1:443".parse().unwrap());

            let transport_clone = transport.clone();
            async move {
                // Create a server
                let mut server = transport_clone
                    .create_server(addr.acceptor_addr())
                    .await
                    .unwrap();

                bach::spawn(async move {
                    while let Some(stream) = server.accept().await {
                        let (request, response, _addr) =
                            (stream.reader, stream.writer, stream.peer_addr);
                        let task = async {
                            let mut buffer = request.read_to_bytes().await?;
                            LOG.push(format_args!("request: {buffer:?}"));
                            let front = buffer.split_to("hello ".len()).unwrap();
                            assert_eq!(front, "hello ".as_bytes());
                            buffer.push_front("goodbye ".into());
                            response.write_to_end(buffer).await?;
                            <io::Result<_>>::Ok(())
                        };

                        match task.await {
                            Ok(_) => {}
                            Err(err) => {
                                tracing::error!("error processing request: {err}");
                            }
                        }
                    }
                });
            }
            .primary()
            .spawn_named("server");

            for idx in 0..2 {
                let transport_clone = transport.clone();
                async move {
                    let client = transport_clone.create_client().await.unwrap();

                    let (request, response) = client
                        .connect(addr, upstream_service::Priority::default())
                        .await
                        .unwrap();
                    request
                        .write_to_end(format!("hello {idx}").as_bytes())
                        .await
                        .unwrap();
                    let payload = response.read_to_bytes().await.unwrap();
                    assert_eq!(payload, format!("goodbye {idx}").as_str());
                }
                .primary()
                .spawn_named("clients");
            }
        }));

        insta::assert_snapshot!(LOG);
    }
}
