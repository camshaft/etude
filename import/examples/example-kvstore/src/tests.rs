use crate::{
    env, events, server,
    service::{
        self,
        kvstore::consumer::{MulticastConsumer, Registry as _},
    },
};
use bach::ext::*;
use upstream_core::{
    runtime::{self, Error, Trace},
    service::{
        consumer::{self, Operation as _},
        stream::{self, StreamExt, TryStreamExt},
    },
    transport::rpc::{self, Server},
};
use std::pin::pin;

fn sim(f: impl FnOnce()) {
    let mut rt = bach::environment::default::Runtime::new();
    rt.run(f);
}

fn create_kvstore_provider<PeerAddr>() -> (
    server::KVStore,
    service::kvstore::provider::Instance<server::KVStore, PeerAddr>,
)
where
    PeerAddr: upstream_core::service::consumer::PeerAddr,
{
    let provider = server::KVStore::default();
    let acceptor = service::kvstore::provider::Instance::new(provider.clone());
    (provider, acceptor)
}

#[test]
fn sim_example() {
    sim(|| {
        let env = env::bach_env();
        run_rpc_example(env, rpc::mem::Transport::default())
            .primary()
            .spawn();
    });
}

#[test]
fn sim_multicast_example() {
    sim(|| {
        let env = env::bach_env();
        run_multicast_example(env, rpc::mem::Transport::default())
            .primary()
            .spawn();
    });
}

#[tokio::test]
async fn dcquic_example() {
    let env = env::tokio_env();
    run_rpc_example(
        env,
        rpc::dcquic::Transport::new(rpc::Config::testing())
            .await
            .unwrap(),
    )
    .await;
}

#[tokio::test]
async fn dcquic_multicast_example() {
    let env = env::tokio_env();
    run_multicast_example(
        env,
        rpc::dcquic::Transport::new(rpc::Config::testing())
            .await
            .unwrap(),
    )
    .await;
}

#[tokio::test]
async fn memory_example() {
    let env = env::tokio_env();
    run_rpc_example(env.clone(), rpc::mem::Transport::default()).await;
}

#[tokio::test]
async fn memory_multicast_example() {
    let env = env::tokio_env();
    run_multicast_example(env, rpc::mem::Transport::default()).await;
}

#[tokio::test]
async fn quic_example() {
    let env = env::tokio_env();
    run_rpc_example(env.clone(), rpc::quic::Transport::default()).await;
}

#[tokio::test]
async fn quic_multicast_example() {
    let env = env::tokio_env();
    run_multicast_example(env, rpc::quic::Transport::default()).await;
}

#[tokio::test]
async fn tcp_example() {
    let env = env::tokio_env();
    run_rpc_example(env.clone(), rpc::tcp::Transport::default()).await;
}

#[tokio::test]
async fn tcp_multicast_example() {
    let env = env::tokio_env();
    run_multicast_example(env, rpc::tcp::Transport::default()).await;
}

async fn run_rpc_example(env: impl crate::Env, transport: impl rpc::Rpc) {
    upstream_core::testing::init_tracing();
    use upstream_core::transport::rpc;

    // Create memory transport for the bridge
    let (_, acceptor) = create_kvstore_provider();
    let server = transport
        .create_server("[::1]:0".parse().unwrap())
        .await
        .unwrap();
    let server_addr = server.local_addr();
    let server = rpc::service::ServerAcceptor::new(server, acceptor, env.clone());

    // Start the server
    let _ = env.spawn(upstream_core::runtime::task_monitor::Test, server.run());

    let client = transport.create_client().await.unwrap();
    let client = rpc::service::ClientRegistry::from_transport(client);

    // Create and connect the client
    let client = client.resolve(&server_addr).await.unwrap();

    // Run the client example with streaming enabled
    client_example(env, client).await;
}

async fn run_multicast_example(env: impl crate::Env, transport: impl rpc::Rpc) {
    upstream_core::testing::init_tracing();
    use upstream_core::transport::rpc;

    // Create two servers
    let server1 = transport
        .create_server("[::1]:0".parse().unwrap())
        .await
        .unwrap();
    let server2 = transport
        .create_server("[::1]:0".parse().unwrap())
        .await
        .unwrap();
    let server_addr1 = server1.local_addr();
    let server_addr2 = server2.local_addr();

    // Create KVStore providers
    let (_, acceptor1) = create_kvstore_provider();
    let (_, acceptor2) = create_kvstore_provider();

    // Start both servers
    let server1 = rpc::service::ServerAcceptor::new(server1, acceptor1, env.clone());
    let server2 = rpc::service::ServerAcceptor::new(server2, acceptor2, env.clone());
    let _ = env.spawn(upstream_core::runtime::task_monitor::Test, server1.run());
    let _ = env.spawn(upstream_core::runtime::task_monitor::Test, server2.run());

    // Create client for multicast
    let client = transport.create_client().await.unwrap();
    let client = rpc::service::ClientRegistry::from_transport(client);

    // Create multicast client targeting both replicas
    let multicast = client
        .multicast([server_addr1, server_addr2].into_iter())
        .await
        .unwrap();

    multicast_example(env, multicast).await;
}

async fn client_example(env: impl crate::Env, client: impl service::kvstore::Consumer) {
    let key = bytes::Bytes::from_static(b"hello");
    let value = bytes::Bytes::from_static(b"world");

    match client.fail(&env, key.clone()).await {
        Ok((header, _stream)) => {
            unreachable!("request should fail - got header: {header:?}");
        }
        Err(err) => {
            dbg!(err);
        }
    }

    // Test regular set and get
    dbg!(client.set(&env, key.clone(), value.clone()).await.unwrap());
    let res = dbg!(client.get(&env, key.clone()).await).unwrap().unwrap();
    assert_eq!(res, value);

    client.del(&env, key.clone()).await.unwrap();
    let res = client.get(&env, key.clone()).await.unwrap();
    assert_eq!(res, None);

    // Test unidirectional set_no_wait
    let key2 = bytes::Bytes::from_static(b"hello2");
    let value2 = bytes::Bytes::from_static(b"world2");

    // Use set_no_wait which doesn't wait for a response
    dbg!(
        client
            .set_no_wait(&env, key2.clone(), value2.clone())
            .await
            .unwrap()
    );

    // Give some time for the unidirectional message to be processed
    // In a real application, you wouldn't need this sleep, but for testing
    // we need to ensure the message is processed before we check the result
    env.sleep(100.ms()).await;

    // Verify the value was set
    let res = dbg!(client.get(&env, key2.clone()).await).unwrap().unwrap();
    assert_eq!(res, value2);

    // Test streaming operations
    {
        // Test server-side streaming with keys()
        let (count, keys) = client.keys(&env).await.unwrap();
        let keys: Result<Vec<_>, Error> = keys.try_collect().await;
        let keys = keys.unwrap();
        assert_eq!(count, keys.len());
        // Only key2 should be present since key was deleted
        assert!(
            !keys.contains(&key),
            "key should not be present since it was deleted"
        );
        assert!(keys.contains(&key2), "key2 should be present");
    }

    {
        // Test client-side streaming with bulk_set
        let pairs = vec![
            (
                bytes::Bytes::from_static(b"bulk1"),
                bytes::Bytes::from_static(b"value1"),
            ),
            (
                bytes::Bytes::from_static(b"bulk2"),
                bytes::Bytes::from_static(b"value2"),
            ),
        ];
        let bulk_stream = stream::iter(pairs.into_iter().map(Ok));
        client.bulk_set(&env, bulk_stream).await.unwrap();
    }

    // Test bidirectional streaming with bulk_get
    {
        let keys = vec![
            bytes::Bytes::from_static(b"bulk1"),
            bytes::Bytes::from_static(b"bulk2"),
            bytes::Bytes::from_static(b"nonexistent"),
        ];
        let keys = stream::iter(keys.into_iter().map(Ok));
        let values: Result<Vec<_>, Error> = client
            .bulk_get(&env, keys)
            .await
            .unwrap()
            .try_collect()
            .await;
        let values = values.unwrap();

        assert_eq!(values.len(), 3);

        let mut values = values.into_iter();

        assert_eq!(values.next().unwrap().unwrap(), &b"value1"[..]);
        assert_eq!(values.next().unwrap().unwrap(), &b"value2"[..]);
        assert_eq!(values.next().unwrap(), None);
    }

    {
        // Tests slow streams
        let (count, keys) = client.slow_keys(&env, 1.s()).await.unwrap();
        let mut keys = pin!(keys);
        let mut actual = 0;
        let mut start = env.now();
        while let Some(key) = keys.next().await {
            let _key = key.unwrap();
            let end = env.now();
            let _v = end.duration_since(start);
            // assert!((1.s() - 100.ms()..1.s() + 100.ms()).contains(&v));
            start = end;
            actual += 1;
        }
        assert_eq!(count, actual);
    }
}

async fn multicast_example(env: impl crate::Env, client: impl MulticastConsumer) {
    // Test key/value
    let key = bytes::Bytes::from_static(b"multicast-test");
    let value = bytes::Bytes::from_static(b"hello world");

    // Multicast set to both replicas
    for op in client.set(&env, key.clone(), value.clone()) {
        op.send(&env).await.unwrap();
    }

    // Verify value was set on both replicas
    for op in client.get(&env, key.clone()) {
        assert_eq!(op.send(&env).await.unwrap(), Some(value.clone()));
    }

    // Test multicast delete
    for op in client.del(&env, key.clone()) {
        op.send(&env).await.unwrap();
    }

    // Verify value was deleted from both replicas
    for op in client.get(&env, key.clone()) {
        assert_eq!(op.send(&env).await.unwrap(), None);
    }
}

#[test]
fn supported_set() {
    let empty = service::kvstore::schema::supported::Set::NONE;
    assert_eq!(empty.iter().next(), None);

    let all = service::kvstore::schema::supported::Set::all();
    let methods = all.iter().collect::<Vec<_>>();
    assert_eq!(methods.len(), service::kvstore::schema::METHODS.len());
}

/// Regression test for streaming-response consumer metrics.
///
/// `keys()` is a `#[response(stream, header)]` method, so it dispatches through
/// `send_stream_response_with_response_header`. Before the span was wired into
/// the response stream, the `StreamResponse` consumer span was dropped without
/// `exit()` being called, so its `duration` timer (and the per-frame
/// `receive_chunk` / `item_deserialize` sub-spans) never recorded. This test
/// drives a real `keys()` call against a metrics-capturing registry and asserts
/// those timers fire — guarding the consumer side of the fix end to end.
#[tokio::test]
async fn stream_response_consumer_metrics_are_emitted() {
    use crate::service::kvstore::Consumer as _;
    use upstream_core::{
        core_events, metrics::registry::testing::TestRegistry, runtime::Env as _,
        transport::rpc::Rpc as _,
    };

    upstream_core::testing::init_tracing();

    // A capturing registry; clones share the same record buffer (it is an
    // Arc<Mutex<_>> inside), so we can read what the metrics subscriber wrote.
    let registry = TestRegistry::default();
    let recorder = registry.recorder.clone();

    // Build an env whose kvstore subscriber records into `registry`. The
    // tracing and core-events arms are left default; only the metrics arm is
    // swapped for the capturing one.
    let subscriber = events::Composed {
        crate_sub: (events::Tracing::default(), events::Metrics::new(registry)),
        core_events: core_events::events::DefaultSubscriber::default(),
    };
    let env = runtime::tokio::Env::new(Trace::new([0; 16]), 0u64, subscriber);

    // Stand up a server and a connected client over the in-memory transport.
    let transport = rpc::mem::Transport::default();
    let (_, acceptor) = create_kvstore_provider();
    let server = transport
        .create_server("[::1]:0".parse().unwrap())
        .await
        .unwrap();
    let server_addr = server.local_addr();
    let server = rpc::service::ServerAcceptor::new(server, acceptor, env.clone());
    let _ = env.spawn(upstream_core::runtime::task_monitor::Test, server.run());

    let client = transport.create_client().await.unwrap();
    let client = rpc::service::ClientRegistry::from_transport(client);
    let client = client.resolve(&server_addr).await.unwrap();

    // Seed a key so the response stream yields at least one frame, then run the
    // streaming-response RPC and fully drain it (the span exits on drain).
    client
        .set(
            &env,
            bytes::Bytes::from_static(b"k"),
            bytes::Bytes::from_static(b"v"),
        )
        .await
        .unwrap();

    let (count, keys) = client.keys(&env).await.unwrap();
    let keys: Result<Vec<_>, Error> = keys.try_collect().await;
    let keys = keys.unwrap();
    assert_eq!(count, keys.len());
    assert_eq!(keys.len(), 1);

    // Assert the streaming-response span and its per-frame sub-spans recorded.
    let recorded: Vec<String> = recorder
        .record_calls
        .lock()
        .unwrap()
        .iter()
        .map(|c| c.name.clone())
        .collect();
    let recorded_one = |name: &str| {
        assert!(
            recorded.iter().any(|n| n == name),
            "expected metric `{name}` to be recorded, got: {recorded:#?}"
        );
    };
    // The main span duration — the metric that was silently never emitted.
    recorded_one("kvstore.kvstore.consumer.keys.duration");
    // Per-frame sub-spans wired into the response stream reader.
    recorded_one("kvstore.kvstore.consumer.keys.receive_chunk.duration");
    recorded_one("kvstore.kvstore.consumer.keys.item_deserialize.duration");
}
