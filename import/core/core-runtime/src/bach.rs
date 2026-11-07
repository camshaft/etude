// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Bach runtime implementation for upstream
//!
//! This crate provides a concrete implementation of the runtime traits using bach.

use crate::{Caller, Env as EnvTrait, Error, Trace, WithSubscriber, trace};
use bach::{task::JoinHandle, time::Instant as BachInstant};
use futures::TryFutureExt as _;
use upstream_time::{Instant, SystemTime, Timeout};
use std::{future::Future, sync::Arc, time::Duration};
use tracing::{Instrument as _, info_span};

/// A bach-based environment implementation.
#[derive(Clone)]
pub struct Env<Sub> {
    /// The trace context for this environment
    trace: Trace,
    inner: Arc<Inner<Sub>>,
}

struct Inner<Sub> {
    /// The span ID for this environment
    span_id: u64,

    /// When this current environment was created
    start_time: BachInstant,

    /// Event subscriber
    subscriber: Sub,
}

impl<Sub: Clone> Inner<Sub> {
    fn new(span_id: u64, subscriber: Sub) -> Self {
        let start_time = BachInstant::now();

        Self {
            span_id,
            start_time,
            subscriber,
        }
    }
}

impl<Sub: Clone> Env<Sub> {
    /// Creates a new bach environment.
    pub fn new(trace: Trace, span_id: u64, subscriber: Sub) -> Self {
        let inner = Arc::new(Inner::new(span_id, subscriber));
        Self { trace, inner }
    }

    /// Returns a reference to the subscriber
    pub fn subscriber(&self) -> &Sub {
        &self.inner.subscriber
    }
}

impl<Sub, Namespace> WithSubscriber<Namespace> for Env<Sub>
where
    Sub: WithSubscriber<Namespace>,
{
    type Subscriber = Sub::Subscriber;

    fn subscriber(&self) -> &Self::Subscriber {
        self.inner.subscriber.subscriber()
    }
}

impl<Sub: Send + Sync + Clone + 'static> EnvTrait for Env<Sub> {
    type JoinHandle<T: Send> = JoinHandle<T>;
    type ComputeJoinHandle<T: Send> = JoinHandle<T>;
    type Timer = Timer;

    fn span_id(&self) -> trace::Span {
        trace::Span::from(self.inner.span_id)
    }

    fn trace(&self) -> &Trace {
        &self.trace
    }

    fn trace_flags(&self) -> trace::Flags {
        // Always sample traces in the bach runtime
        trace::Flags::SAMPLED
    }

    fn now(&self) -> Instant {
        let nanos = BachInstant::now().elapsed_since_start();
        Instant::from_nanos(nanos.as_nanos() as _)
    }

    fn span_start(&self) -> Instant {
        let v = self.inner.start_time.elapsed_since_start();
        Instant::from_nanos(v.as_nanos() as _)
    }

    #[track_caller]
    fn spawn<T: crate::task_monitor::TaskClass<Self>, F>(
        &self,
        _class: T,
        future: F,
    ) -> Self::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let task = Caller::capture();
        bach::spawn(future.instrument(info_span!("task", task = %task)))
    }

    #[track_caller]
    fn spawn_blocking<F, R>(&self, f: F) -> Self::JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let task = Caller::capture();
        let f = async move { f() };
        let f = f.instrument(info_span!("blocking_task", task = %task));
        bach::spawn(f)
    }

    fn is_simulation(&self) -> bool {
        true
    }

    fn timer(&self, initial_duration: Duration) -> Self::Timer {
        Timer::new(initial_duration)
    }

    async fn sleep(&self, duration: Duration) {
        bach::time::sleep(duration).await
    }

    async fn yield_now(&self) {
        bach::task::yield_now().await
    }

    async fn consume_budget(&self) {
        // bach has no cooperative-budget primitive; cooperation in simulation
        // is driven by the runtime's own coop mode, so this is a no-op.
    }

    fn poll_consume_budget(&self, _cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        // bach has no cooperative-budget primitive (see `consume_budget`); always
        // report budget remaining so manual `poll` loops make progress in sim.
        std::task::Poll::Ready(())
    }

    async fn resolve_dns(&self, host: &str) -> Result<Vec<std::net::SocketAddr>, Error> {
        bach::net::lookup_host(host)
            .await
            .map(|v| v.collect())
            .map_err(Error::Io)
    }

    fn accept_trace(&self, trace: trace::Header) -> Self {
        // TODO create a new span_id since this is a new environment
        Self {
            trace: trace.trace,
            inner: self.inner.clone(),
        }
    }

    fn timeout<F>(
        &self,
        duration: Duration,
        future: F,
    ) -> impl std::future::Future<Output = Result<F::Output, Timeout>> + Send
    where
        F: Future + Send,
        F::Output: Send,
    {
        bach::time::timeout(duration, future).map_err(|_| Timeout)
    }

    fn system_time(&self) -> SystemTime {
        let nanos = BachInstant::now().elapsed_since_start();
        SystemTime::from_nanos(nanos.as_nanos() as _)
    }

    #[track_caller]
    #[allow(clippy::async_yields_async)] // The ComputeJoinHandle implements Future and clippy doesn't like that
    fn spawn_compute<F, R>(&self, f: F) -> impl Future<Output = Self::ComputeJoinHandle<R>>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let task = Caller::capture();
        let f = async move { f() };
        let f = f.instrument(info_span!("compute_task", task = %task));
        async { bach::spawn(f) }
    }
}

impl<T: Send> crate::JoinHandle<T> for JoinHandle<T> {
    async fn join(self) -> crate::Result<T> {
        self.await
            .map_err(|e| crate::Error::Io(std::io::Error::other(format!("{e:?}"))))
    }

    fn abort(&mut self) {
        Self::abort(self)
    }
}

pub struct Timer(bach::time::Sleep);

impl Timer {
    pub fn new(duration: Duration) -> Self {
        Self(bach::time::sleep(duration))
    }
}

impl upstream_time::Timer for Timer {
    fn now(&self) -> Instant {
        Instant::from_nanos(bach::time::Instant::now().elapsed_since_start().as_nanos() as _)
    }

    fn reset(mut self: core::pin::Pin<&mut Self>, duration: Duration) {
        self.0.reset(bach::time::Instant::now() + duration);
    }
}

impl Future for Timer {
    type Output = Timeout;

    fn poll(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        use futures::FutureExt;
        self.0.poll_unpin(cx).map(|_| Timeout)
    }
}
