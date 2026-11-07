use super::{Client, PendingStream, Reader, Rpc, Server, Writer};
use anyhow::Result;
use upstream_buffer::{reader::Storage as _, testing::Buffer as Data};
use std::time::Duration;
use tokio::task::JoinSet;
use tracing::{Instrument, info_span};

macro_rules! test_rpc {
    ($ty:path) => {
        test_rpc!(|| <$ty>::default());
    };
    (|| $init:expr) => {
        #[tokio::test]
        async fn echo_test() {
            upstream_testing::init_tracing();
            let rpc = $init;
            crate::testing::stress_test(rpc, 1, 1000, 1000).await;
        }

        #[tokio::test]
        async fn stress_test_1k() {
            upstream_testing::init_tracing();
            let rpc = $init;
            crate::testing::stress_test(rpc, 16, 1000, 1000).await;
        }

        #[tokio::test]
        async fn stress_test_100k() {
            upstream_testing::init_tracing();
            let rpc = $init;
            crate::testing::stress_test(rpc, 16, 100, 100_000).await;
        }

        #[tokio::test]
        #[ignore = "flaky under load in CI build fleet with 1MB payloads"]
        async fn stress_test_1m() {
            upstream_testing::init_tracing();
            let rpc = $init;
            crate::testing::stress_test(rpc, 16, 100, 1_000_000).await;
        }

        #[tokio::test]
        async fn streaming_echo_test() {
            upstream_testing::init_tracing();
            let rpc = $init;
            crate::testing::streaming_stress_test(rpc, 1, 100, 10_000).await;
        }

        #[tokio::test]
        async fn streaming_stress_test() {
            upstream_testing::init_tracing();
            let rpc = $init;
            crate::testing::streaming_stress_test(rpc, 8, 50, 100_000).await;
        }

        #[tokio::test]
        async fn request_response_test() {
            upstream_testing::init_tracing();
            let rpc = $init;
            crate::testing::request_response_test(rpc, 4, 100, 1000).await;
        }

        #[tokio::test]
        async fn request_response_large_test() {
            upstream_testing::init_tracing();
            let rpc = $init;
            crate::testing::request_response_test(rpc, 4, 10, 100_000).await;
        }

        #[tokio::test]
        async fn bidirectional_streaming_test() {
            upstream_testing::init_tracing();
            let rpc = $init;
            crate::testing::bidirectional_streaming_test(rpc, 4, 50, 10_000).await;
        }

        #[tokio::test]
        async fn data_integrity_1k() {
            upstream_testing::init_tracing();
            let rpc = $init;
            crate::testing::data_integrity_test(rpc, 1000).await;
        }

        #[tokio::test]
        async fn data_integrity_100k() {
            upstream_testing::init_tracing();
            let rpc = $init;
            crate::testing::data_integrity_test(rpc, 100_000).await;
        }

        #[tokio::test]
        async fn connection_reuse_test() {
            upstream_testing::init_tracing();
            let rpc = $init;
            crate::testing::connection_reuse_test(rpc, 500, 1000).await;
        }

        #[tokio::test]
        async fn concurrent_streams_test() {
            upstream_testing::init_tracing();
            let rpc = $init;
            crate::testing::concurrent_streams_test(rpc, 256, 1000).await;
        }

        #[tokio::test]
        async fn writer_drop_no_finish_test() {
            upstream_testing::init_tracing();
            let rpc = $init;
            crate::testing::writer_drop_without_finish_test(rpc).await;
        }

        #[tokio::test]
        async fn empty_payload_test() {
            upstream_testing::init_tracing();
            let rpc = $init;
            crate::testing::empty_payload_test(rpc).await;
        }
    };
}

/// Tests the streaming write path: write_chunk called multiple times then finish().
///
/// This exercises the contract that write_chunk does NOT send FIN — only finish() does.
/// If write_chunk sends FIN (e.g. using write_all_from_fin internally), the second
/// write_chunk call will fail because the stream is already terminated.
pub async fn streaming_stress_test<R: Rpc>(
    rpc: R,
    workers: usize,
    requests: usize,
    payload_size: usize,
) {
    let server = rpc.create_server("[::1]:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();

    for worker in 0..workers {
        let mut server = server.clone();
        tokio::spawn(
            async move {
                while let Some(stream) = server.accept().await {
                    let Ok((mut request, mut response, _remote_addr)) = stream.into_parts().await
                    else {
                        continue;
                    };
                    let task = async move {
                        let mut buf = Data::new(payload_size as _);
                        let mut total_read = 0;
                        loop {
                            let n = request.read_chunk(&mut buf).await?;
                            if n == 0 {
                                break;
                            }
                            total_read += n;
                        }

                        let mut send_buf = Data::new(total_read as _);
                        while !send_buf.buffer_is_empty() {
                            let written = response.write_chunk(&mut send_buf).await?;
                            if written == 0 {
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::WriteZero,
                                    "write_chunk returned 0 before buffer was empty",
                                ));
                            }
                        }
                        response.finish().await?;

                        <std::io::Result<()>>::Ok(())
                    }
                    .instrument(info_span!("streaming_request"));

                    tokio::spawn(task);
                }
            }
            .instrument(info_span!("streaming_server", worker)),
        );
    }

    let client = rpc.create_client().await.unwrap();
    let mut tasks = JoinSet::new();
    for worker in 0..workers {
        let client = client.clone();
        tasks.spawn(
            async move {
                for request in 0..requests {
                    async {
                        let (mut writer, reader) = client
                            .connect(addr, upstream_service::Priority::default())
                            .await
                            .unwrap();

                        let mut send_buf = Data::new(payload_size as _);
                        while !send_buf.buffer_is_empty() {
                            let written = writer.write_chunk(&mut send_buf).await.unwrap();
                            assert!(written > 0, "write_chunk returned 0 before buffer empty");
                        }
                        writer.finish().await.unwrap();

                        let mut recv_buf = Data::new(payload_size as _);
                        reader.read_to_end(&mut recv_buf).await.unwrap();
                        assert!(
                            recv_buf.is_finished(),
                            "response did not contain expected payload_size={payload_size}"
                        );
                    }
                    .instrument(info_span!("streaming_client_request", request))
                    .await;
                }
            }
            .instrument(info_span!("streaming_client", worker)),
        );
    }

    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
}

/// Tests the Client::request_response trait method directly.
///
/// The experimental dcquic overrides this with a native QUIC RPC path (client.rpc()),
/// which is a completely different code path from connect + write_to_end + read_to_bytes.
pub async fn request_response_test<R: Rpc>(
    rpc: R,
    workers: usize,
    requests: usize,
    payload_size: usize,
) {
    let server = rpc.create_server("[::1]:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();

    for worker in 0..workers {
        let mut server = server.clone();
        tokio::spawn(
            async move {
                while let Some(stream) = server.accept().await {
                    let Ok((request, response, _)) = stream.into_parts().await else {
                        continue;
                    };
                    tokio::spawn(
                        async move {
                            let data = request.read_to_bytes().await?;
                            response.write_to_end(data).await?;
                            <std::io::Result<()>>::Ok(())
                        }
                        .instrument(info_span!("rr_handler")),
                    );
                }
            }
            .instrument(info_span!("rr_server", worker)),
        );
    }

    let client = rpc.create_client().await.unwrap();
    let mut tasks = JoinSet::new();
    for worker in 0..workers {
        let client = client.clone();
        tasks.spawn(
            async move {
                for i in 0..requests {
                    let payload = Data::new(payload_size as _);
                    let expected_len = payload.buffered_len();
                    let response = client
                        .request_response(addr, payload, upstream_service::Priority::default())
                        .await
                        .unwrap_or_else(|e| panic!("request_response #{i} failed: {e}"));
                    assert_eq!(
                        response.len(),
                        expected_len,
                        "request_response #{i} returned wrong size: {} vs {}",
                        response.len(),
                        expected_len
                    );
                }
            }
            .instrument(info_span!("rr_client", worker)),
        );
    }

    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
}

/// Tests concurrent read and write on the same stream.
///
/// The client writes while the server simultaneously reads and writes back.
/// Catches deadlocks or corruption under concurrent bidirectional I/O.
pub async fn bidirectional_streaming_test<R: Rpc>(
    rpc: R,
    workers: usize,
    requests: usize,
    payload_size: usize,
) {
    let server = rpc.create_server("[::1]:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();

    for worker in 0..workers {
        let mut server = server.clone();
        tokio::spawn(
            async move {
                while let Some(stream) = server.accept().await {
                    let Ok((mut request, mut response, _)) = stream.into_parts().await else {
                        continue;
                    };
                    tokio::spawn(
                        async move {
                            // Read full request then write response
                            // (we can't truly interleave with split halves in the trait,
                            // but we test that writing starts immediately after read completes)
                            let mut buf = Data::new(payload_size as _);
                            let mut total_read = 0;
                            loop {
                                let n = request.read_chunk(&mut buf).await?;
                                if n == 0 {
                                    break;
                                }
                                total_read += n;
                            }

                            let mut send_buf = Data::new(total_read as _);
                            while !send_buf.buffer_is_empty() {
                                let written = response.write_chunk(&mut send_buf).await?;
                                if written == 0 {
                                    break;
                                }
                            }
                            response.finish().await?;
                            <std::io::Result<()>>::Ok(())
                        }
                        .instrument(info_span!("bidi_handler")),
                    );
                }
            }
            .instrument(info_span!("bidi_server", worker)),
        );
    }

    let client = rpc.create_client().await.unwrap();
    let mut tasks = JoinSet::new();
    for worker in 0..workers {
        let client = client.clone();
        tasks.spawn(
            async move {
                for _request in 0..requests {
                    let (writer, reader) = client
                        .connect(addr, upstream_service::Priority::default())
                        .await
                        .unwrap();

                    // Client writes and reads concurrently via try_join
                    let write_fut = async {
                        let mut writer = writer;
                        let mut send_buf = Data::new(payload_size as _);
                        while !send_buf.buffer_is_empty() {
                            let written = writer.write_chunk(&mut send_buf).await?;
                            if written == 0 {
                                break;
                            }
                        }
                        writer.finish().await?;
                        <std::io::Result<()>>::Ok(())
                    };

                    let read_fut = async {
                        let mut recv_buf = Data::new(payload_size as _);
                        reader.read_to_end(&mut recv_buf).await?;
                        assert!(recv_buf.is_finished());
                        <std::io::Result<()>>::Ok(())
                    };

                    tokio::try_join!(write_fut, read_fut).unwrap();
                }
            }
            .instrument(info_span!("bidi_client", worker)),
        );
    }

    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
}

/// Verifies actual byte content integrity, not just payload size.
///
/// Existing tests only check `is_finished()` (size check). This test writes a
/// deterministic byte pattern and verifies exact content after round-trip.
pub async fn data_integrity_test<R: Rpc>(rpc: R, payload_size: usize) {
    let server = rpc.create_server("[::1]:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();

    let mut server_clone = server.clone();
    tokio::spawn(
        async move {
            while let Some(stream) = server_clone.accept().await {
                let Ok((request, response, _)) = stream.into_parts().await else {
                    continue;
                };
                tokio::spawn(async move {
                    let data = request.read_to_bytes().await?;
                    response.write_to_end(data).await?;
                    <std::io::Result<()>>::Ok(())
                });
            }
        }
        .instrument(info_span!("integrity_server")),
    );

    let client = rpc.create_client().await.unwrap();

    // Deterministic payload: prime modulus for good byte diversity
    let mut expected = vec![0u8; payload_size];
    for (i, byte) in expected.iter_mut().enumerate() {
        *byte = (i % 251) as u8;
    }

    let (writer, reader) = client
        .connect(addr, upstream_service::Priority::default())
        .await
        .unwrap();
    writer.write_to_end(&expected[..]).await.unwrap();
    let response = reader.read_to_bytes().await.unwrap();

    assert_eq!(response.len(), expected.len(), "size mismatch");
    assert!(response == expected[..], "data corruption detected");
}

/// Tests many sequential RPCs over the same client (connection/stream reuse).
///
/// Catches stream ID exhaustion, connection state corruption, or resource leaks
/// when a single client opens many streams over its lifetime.
pub async fn connection_reuse_test<R: Rpc>(
    rpc: R,
    sequential_requests: usize,
    payload_size: usize,
) {
    let server = rpc.create_server("[::1]:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();

    let mut server_clone = server.clone();
    tokio::spawn(
        async move {
            while let Some(stream) = server_clone.accept().await {
                let Ok((request, response, _)) = stream.into_parts().await else {
                    continue;
                };
                tokio::spawn(async move {
                    let data = request.read_to_bytes().await?;
                    response.write_to_end(data).await?;
                    <std::io::Result<()>>::Ok(())
                });
            }
        }
        .instrument(info_span!("reuse_server")),
    );

    let client = rpc.create_client().await.unwrap();
    for i in 0..sequential_requests {
        let payload = Data::new(payload_size as _);
        let response = client
            .request_response(addr, payload, upstream_service::Priority::default())
            .await
            .unwrap_or_else(|e| panic!("sequential request #{i} failed: {e}"));
        assert_eq!(
            response.len(),
            payload_size,
            "sequential request #{i} returned wrong size"
        );
    }
}

/// Tests burst traffic with many concurrent streams on the same endpoint.
///
/// Catches contention on shared state (Budgets, pools), deadlocks, or
/// acceptor queue overflow under high concurrency.
pub async fn concurrent_streams_test<R: Rpc>(rpc: R, concurrent: usize, payload_size: usize) {
    let server = rpc.create_server("[::1]:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();

    let mut server_clone = server.clone();
    tokio::spawn(
        async move {
            while let Some(stream) = server_clone.accept().await {
                let Ok((request, response, _)) = stream.into_parts().await else {
                    continue;
                };
                tokio::spawn(async move {
                    let data = request.read_to_bytes().await?;
                    response.write_to_end(data).await?;
                    <std::io::Result<()>>::Ok(())
                });
            }
        }
        .instrument(info_span!("concurrent_server")),
    );

    let client = rpc.create_client().await.unwrap();
    let mut tasks = JoinSet::new();

    for i in 0..concurrent {
        let client = client.clone();
        tasks.spawn(async move {
            let payload = Data::new(payload_size as _);
            let response = client
                .request_response(addr, payload, upstream_service::Priority::default())
                .await
                .unwrap_or_else(|e| panic!("concurrent request #{i} failed: {e}"));
            assert_eq!(response.len(), payload_size);
        });
    }

    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }
}

/// Tests that dropping a writer without calling finish() doesn't hang the reader.
///
/// If a server handler panics mid-write, the client reader should get an error
/// or EOF within a reasonable timeout, not hang forever.
pub async fn writer_drop_without_finish_test<R: Rpc>(rpc: R) {
    let server = rpc.create_server("[::1]:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();

    let mut server_clone = server.clone();
    tokio::spawn(
        async move {
            while let Some(stream) = server_clone.accept().await {
                let Ok((mut request, response, _)) = stream.into_parts().await else {
                    continue;
                };
                // Read request, then drop response writer without calling finish()
                tokio::spawn(async move {
                    let mut buf = upstream_bytevec::Builder::default();
                    let _ = request.read_chunk(&mut buf).await;
                    drop(response);
                });
            }
        }
        .instrument(info_span!("drop_server")),
    );

    let client = rpc.create_client().await.unwrap();
    let (writer, reader) = client
        .connect(addr, upstream_service::Priority::default())
        .await
        .unwrap();
    writer.write_to_end(&b"hello"[..]).await.unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), reader.read_to_bytes()).await;

    match result {
        Ok(Ok(bytes)) => {
            // EOF with empty response is acceptable
            assert!(
                bytes.is_empty(),
                "expected empty response on server drop, got {} bytes",
                bytes.len()
            );
        }
        Ok(Err(_)) => {
            // IO error is acceptable (stream reset)
        }
        Err(_) => {
            panic!("reader hung for >5s after server dropped writer without finish()");
        }
    }
}

/// Tests zero-length request/response edge case.
///
/// If the transport sends FIN without any data, the reader must correctly
/// return 0 from read_chunk / return empty from read_to_bytes.
pub async fn empty_payload_test<R: Rpc>(rpc: R) {
    let server = rpc.create_server("[::1]:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();

    let mut server_clone = server.clone();
    tokio::spawn(
        async move {
            while let Some(stream) = server_clone.accept().await {
                let Ok((request, response, _)) = stream.into_parts().await else {
                    continue;
                };
                tokio::spawn(async move {
                    let data = request.read_to_bytes().await?;
                    assert!(
                        data.is_empty(),
                        "expected empty request, got {} bytes",
                        data.len()
                    );
                    // Echo back empty response
                    response.write_to_end(&[][..]).await?;
                    <std::io::Result<()>>::Ok(())
                });
            }
        }
        .instrument(info_span!("empty_server")),
    );

    let client = rpc.create_client().await.unwrap();
    let (writer, reader) = client
        .connect(addr, upstream_service::Priority::default())
        .await
        .unwrap();

    writer.write_to_end(&[][..]).await.unwrap();
    let response = reader.read_to_bytes().await.unwrap();
    assert!(
        response.is_empty(),
        "expected empty response, got {} bytes",
        response.len()
    );
}

pub async fn stress_test<R: Rpc>(rpc: R, workers: usize, requests: usize, payload_size: usize) {
    let server = rpc.create_server("[::1]:0".parse().unwrap()).await.unwrap();
    let addr = server.local_addr();

    for worker in 0..workers {
        let mut server = server.clone();
        tokio::spawn(
            async move {
                let mut request_id = 0;
                while let Some(stream) = server.accept().await {
                    let Ok((request, response, remote_addr)) = stream.into_parts().await else {
                        continue;
                    };
                    let task = async move {
                        tracing::info!("request accepted");
                        let request = request.read_to_bytes().await?;
                        tracing::info!(request_len = request.len(), "request received");
                        response.write_to_end(request).await?;
                        tracing::info!("response sent");

                        <Result<()>>::Ok(())
                    }
                    .instrument(info_span!(
                        "request",
                        request_id,
                        ?remote_addr
                    ));

                    request_id += 1;

                    tokio::spawn(task);
                }
            }
            .instrument(info_span!("server", worker)),
        );
    }

    let client = rpc.create_client().await.unwrap();
    let mut tasks = JoinSet::new();
    for worker in 0..workers {
        let client = client.clone();
        tasks.spawn(
            async move {
                for request in 0..requests {
                    async {
                        tracing::info!(?addr, payload_size, "creating request");
                        let (request, response) = client
                            .connect(addr, upstream_service::Priority::default())
                            .await
                            .unwrap();
                        request
                            .write_to_end(Data::new(payload_size as _))
                            .await
                            .unwrap();
                        tracing::info!("reading response");
                        let mut res = Data::new(payload_size as _);
                        response.read_to_end(&mut res).await.unwrap();
                        tracing::info!("response finished");
                        assert!(res.is_finished());
                    }
                    .instrument(info_span!("request", request))
                    .await;
                }
            }
            .instrument(info_span!("client", worker)),
        );
    }

    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(_) => result.unwrap(),
            Err(_) => {
                if payload_size > 1024 * 1024 * 1024 {
                    tracing::info!(
                        "expected error that message size {} is larger than 1GB.",
                        payload_size
                    );
                } else {
                    panic!("Task failed");
                }
            }
        };
    }
}
