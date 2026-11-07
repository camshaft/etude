// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! The framing loop's cost per batch with no bound (`off`) and under a one-hour
//! bound (`on`), on writers shaped like the real transports.
//!
//! ```sh
//! cargo bench -p amzn-upstream-transport-rpc --features bench --bench write_framed_stream
//! ```
//!
//! Each response is `BATCHES` batches: 128 KiB items fill one batch each, 4 KiB
//! items fill one batch per 32. A writer that takes the chunk whole flushes once per
//! batch; one that takes a segment per call flushes the first batch segment by
//! segment and then one item per pass. The `write_framed_stream` groups run a body that is
//! always ready on a writer that takes every chunk whole; `write_framed_stream_waits`
//! runs one the loop has to wait for once per batch, which is when the stall timer
//! is polled; `write_framed_stream_segment` and `write_framed_stream_mtu` run the
//! ready body on a writer that takes one segment per call (TCP, mux, in-memory) and
//! one that copies MTU-sized pieces (QUIC). Throughput counts payload bytes.

use amzn_upstream_transport_rpc::{
    Writer,
    service::{Progress, StallPolicy, StallTimeout, write_framed_stream_at},
};
use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use upstream_bytevec::{Builder, ByteVec};
use upstream_runtime::{Env, Error, Result};
use upstream_service::stream::{self, Stream};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

const BATCHES: usize = 64;
const STREAM_BATCH_SIZE: usize = 1 << 17;

/// Takes every chunk whole and discards it.
struct NullWriter;

impl Writer for NullWriter {
    fn write_chunk<Payload>(
        &mut self,
        payload: &mut Payload,
    ) -> impl Future<Output = io::Result<usize>> + Send
    where
        Payload: upstream_buffer::reader::storage::Infallible + Send,
    {
        let mut len = 0;
        while !payload.buffer_is_empty() {
            len += payload.infallible_read_chunk(usize::MAX).len();
        }
        async move { Ok(len) }
    }

    fn keep_alive(&mut self, _enabled: bool) {}

    fn finish(self) -> impl Future<Output = io::Result<()>> + Send {
        async { Ok(()) }
    }
}

/// Takes one segment per call and discards it, like the TCP, mux and in-memory
/// writers.
struct SegmentWriter;

impl Writer for SegmentWriter {
    fn write_chunk<Payload>(
        &mut self,
        payload: &mut Payload,
    ) -> impl Future<Output = io::Result<usize>> + Send
    where
        Payload: upstream_buffer::reader::storage::Infallible + Send,
    {
        let len = payload.infallible_read_chunk(usize::MAX).len();
        async move { Ok(len) }
    }

    fn keep_alive(&mut self, _enabled: bool) {}

    fn finish(self) -> impl Future<Output = io::Result<()>> + Send {
        async { Ok(()) }
    }
}

/// Copies the chunk out in MTU-sized pieces and discards them, like a QUIC writer
/// paced by credit.
struct MtuWriter;

impl Writer for MtuWriter {
    fn write_chunk<Payload>(
        &mut self,
        payload: &mut Payload,
    ) -> impl Future<Output = io::Result<usize>> + Send
    where
        Payload: upstream_buffer::reader::storage::Infallible + Send,
    {
        use upstream_buffer::writer::{Storage as _, storage::Discard};
        let len = payload.buffered_len();
        while !payload.buffer_is_empty() {
            payload.infallible_copy_into(&mut Discard.with_write_limit(1200));
        }
        async move { Ok(len) }
    }

    fn keep_alive(&mut self, _enabled: bool) {}

    fn finish(self) -> impl Future<Output = io::Result<()>> + Send {
        async { Ok(()) }
    }
}

fn one_hour() -> StallPolicy {
    StallPolicy {
        timeout: StallTimeout::new(Duration::from_secs(3600)).unwrap(),
        method: "bench",
        progress: Some(Arc::new(Progress::default())),
    }
}

/// `inner`, returning `Pending` with an immediate wake before every `every` items:
/// a body the loop has to wait for once per batch.
struct Yielding<S> {
    inner: S,
    every: usize,
    left: usize,
}

impl<S: Stream + Unpin> Stream for Yielding<S> {
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<S::Item>> {
        if self.left == 0 {
            self.left = self.every;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        self.left -= 1;
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

fn bench_write_framed_stream(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let env = upstream_runtime::tokio::Env::new(upstream_runtime::Trace::new([0; 16]), 0u64, ());

    for (name, item_len, items_per_batch) in [("128KiB", 128 * 1024, 1), ("4KiB", 4 * 1024, 32)] {
        let items: Vec<ByteVec> = (0..BATCHES * items_per_batch)
            .map(|_| ByteVec::from(vec![0u8; item_len]))
            .collect();
        let bytes = (items.len() * item_len) as u64;
        let ready = || stream::iter(items.clone().into_iter().map(Ok::<_, Error>));
        let waits = || Yielding {
            inner: ready(),
            every: items_per_batch,
            left: 0,
        };
        bench_loops(
            c,
            &format!("write_framed_stream/{name}"),
            bytes,
            &rt,
            &env,
            || NullWriter,
            ready,
        );
        bench_loops(
            c,
            &format!("write_framed_stream_waits/{name}"),
            bytes,
            &rt,
            &env,
            || NullWriter,
            waits,
        );
        bench_loops(
            c,
            &format!("write_framed_stream_segment/{name}"),
            bytes,
            &rt,
            &env,
            || SegmentWriter,
            ready,
        );
        bench_loops(
            c,
            &format!("write_framed_stream_mtu/{name}"),
            bytes,
            &rt,
            &env,
            || MtuWriter,
            ready,
        );
    }
}

/// `off` and `on` over one body and one writer.
fn bench_loops<E, W, B>(
    c: &mut Criterion,
    group: &str,
    bytes: u64,
    rt: &tokio::runtime::Runtime,
    env: &E,
    writer: impl Fn() -> W + Copy,
    body: impl Fn() -> B + Copy,
) where
    E: Env,
    W: Writer,
    B: Stream<Item = Result<ByteVec>> + Send,
{
    let mut group = c.benchmark_group(group);
    group.throughput(Throughput::Bytes(bytes));
    group.bench_function("off", |b| {
        b.iter_batched(
            body,
            |body| {
                rt.block_on(write_framed_stream_at(
                    writer(),
                    body,
                    Builder::default(),
                    None,
                    env,
                    (),
                    None,
                ))
                .unwrap()
            },
            BatchSize::SmallInput,
        )
    });
    group.bench_function("on", |b| {
        b.iter_batched(
            || (body(), one_hour()),
            |(body, policy)| {
                rt.block_on(write_framed_stream_at(
                    writer(),
                    body,
                    Builder::default(),
                    None,
                    env,
                    (),
                    Some(policy),
                ))
                .unwrap()
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

criterion_group!(benches, bench_write_framed_stream);
criterion_main!(benches);
