// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Tokio runtime implementation for upstream
//!
//! This crate provides a concrete implementation of the runtime traits using tokio.

use crate::{Env as EnvTrait, Error, Trace, WithSubscriber, trace};
use bolero_generator::{TypeGenerator, ValueGenerator};
use futures::TryFutureExt;
use upstream_time::{Instant, SystemTime, Timeout};
use rand::{prelude::*, rng};
use std::{
    future::Future,
    sync::Arc,
    time::{Duration, SystemTime as StdSystemTime},
};
use tokio::time::Instant as TokioInstant;

static ROOT_START_TIME: std::sync::OnceLock<TokioInstant> = std::sync::OnceLock::new();

fn root_start_time() -> TokioInstant {
    *ROOT_START_TIME.get_or_init(tokio::time::Instant::now)
}

tokio::task_local! {
    /// Per-request override of the trace header that should be serialized on
    /// outbound RPC frames. Set by HTTP ingress middleware via [`scope_request_trace`]
    /// and read by [`Env::trace_header`]. Survives `env.spawn(...)` because
    /// [`Env::spawn`] / `spawn_compute` / `spawn_blocking` re-scope the value
    /// inside the spawned task.
    pub static REQUEST_TRACE: trace::Header;
}

/// Run `future` with `header` installed as the current task's request-trace
/// override. Calls to `env.trace_header()` inside `future` will return `header`
/// regardless of which `Env` instance is used (handler's, leader's, etc.).
pub async fn scope_request_trace<F, R>(header: trace::Header, future: F) -> R
where
    F: Future<Output = R>,
{
    REQUEST_TRACE.scope(header, future).await
}

/// Returns the current task's request-trace override, if any was installed by
/// an outer [`scope_request_trace`] call.
fn current_request_trace() -> Option<trace::Header> {
    REQUEST_TRACE.try_with(|h| *h).ok()
}

/// Wrap `future` so that, when polled, it sees the current request trace
/// installed in its task-local slot. Used to propagate the trace across
/// `tokio::spawn` boundaries (which do not auto-propagate task-locals).
///
/// Currently re-installs the **same** `trace::Header` (i.e. the same
/// `span_id`) inside the spawned task. This is sufficient for log correlation
/// — every event/span in the spawned task records the originating request's
/// `trace_id`. For X-Ray-style hierarchical traces we'd want each spawned
/// task to mint its own `span_id` so concurrent fan-out work (e.g. parallel
/// Paxos prepares) shows up as sibling spans rather than collapsing onto the
/// parent — see `distributed-tracing-flow.md` for the followup.
fn with_request_trace<F>(future: F) -> impl Future<Output = F::Output>
where
    F: Future,
{
    let header = current_request_trace();
    async move {
        match header {
            Some(h) => REQUEST_TRACE.scope(h, future).await,
            None => future.await,
        }
    }
}

/// Re-export for use in generated code.
pub type RuntimeHandle = tokio::runtime::Handle;

/// Time-sliced cooperative-yield budget.
///
/// `tokio::task::coop::consume_budget` gates yielding on a fixed 128-operation
/// counter — a *proxy* for "this task has run long enough, let others in." That
/// proxy assumes every charged op costs roughly the same, which holds for
/// uniform socket/timer work but breaks badly for our dynamic CPU loops: a
/// snapshot-teardown element drop ranges from sub-microsecond (a flat `Unit`) to
/// hundreds of microseconds (a spilled block whose refcount release retires a
/// disk block via epoch GC). At ~0.3ms/op the 128-op budget lets a single poll
/// hold a worker for ~40ms, starving latency-sensitive tasks (proven on
/// strss-1-dcquic: delete bursts spiked `task_queue_latency` to 0.5–1.9s).
///
/// This wrapper keeps tokio's counter (so genuine resource ops still drive
/// fairness and the counter remains a hard upper bound) but adds a *wall-clock*
/// trigger: if the current run-slice has exceeded [`coop_slice`], yield
/// regardless of the op count. Time directly bounds the quantum no matter what
/// each op actually costs, so it self-tunes across workloads.
///
/// The slice is anchored at poll-start by [`crate::task_monitor::Instrumented`]
/// (which wraps every `env.spawn`ed task), so it resets whenever the task
/// yielded for ANY reason and was re-polled — we don't have to model awaits on
/// sockets/locks/timers ourselves, and a yield from tokio's own budget below
/// also re-anchors on the next poll.
mod coop {
    use super::*;

    /// Wall-clock budget for a single uninterrupted run-slice. Sourced once from
    /// `UPSTREAM_SERVER_COOP_SLICE_US` (default 500µs) so it can be swept
    /// on-cluster without a rebuild. `consume_budget` runs hundreds of thousands
    /// of times per second, so this is cached rather than read per call.
    fn coop_slice() -> core::time::Duration {
        static SLICE: std::sync::OnceLock<core::time::Duration> = std::sync::OnceLock::new();
        *SLICE.get_or_init(|| {
            let us = crate::config::env_value_with_default("SERVER", "COOP_SLICE_US", 500u64)
                .unwrap_or(500);
            core::time::Duration::from_micros(us)
        })
    }

    /// Time-sliced replacement for `tokio::task::coop::consume_budget`. Charges
    /// tokio's op budget, then — only if tokio did *not* already yield — yields
    /// if this run-slice has exceeded the wall-clock budget.
    pub(super) async fn consume_budget() {
        use std::{future::poll_fn, task::Poll};

        // Drive tokio's coop future ourselves so we can observe whether it
        // yielded. It returns `Pending` exactly when the 128-op budget is
        // exhausted (i.e. it decided to yield); we propagate that `Pending` so
        // the task actually yields, and remember it. Charging tokio's counter
        // preserves resource-driven coop and keeps 128 ops as a hard ceiling.
        let mut budget = std::pin::pin!(tokio::task::coop::consume_budget());
        let mut tokio_yielded = false;
        poll_fn(|cx| {
            if budget.as_mut().poll(cx).is_pending() {
                tokio_yielded = true;
                return Poll::Pending;
            }
            Poll::Ready(())
        })
        .await;

        // If tokio already yielded, the task was descheduled and re-polled — its
        // slice anchor was re-stamped by `Instrumented::poll`, so there is no
        // point checking elapsed time (and nothing to gain by yielding again).
        if tokio_yielded {
            return;
        }

        // Otherwise yield if this task has held the worker past the slice.
        // `slice_elapsed` is `None` for tasks not spawned via `env.spawn`
        // (no poll-start anchor) — those fall back to tokio's budget alone.
        if let Some(elapsed) = crate::task_monitor::slice_elapsed() {
            if elapsed >= coop_slice() {
                tokio::task::yield_now().await;
            }
        }
    }

    /// Synchronous, `poll`-context sibling of [`consume_budget`], for hand-rolled
    /// `Future`/`Stream` impls that pump work in a manual `poll` loop and cannot
    /// `.await` (e.g. the duplex-stream coupling loop). Returns `Poll::Pending`
    /// after arranging a wake exactly when the async version would have yielded —
    /// the caller must propagate that `Pending` out of its own `poll` so the task
    /// is descheduled and re-polled (its slice anchor re-stamped) rather than
    /// spinning. Returns `Poll::Ready(())` while there is still budget to keep
    /// making progress within this poll.
    pub(super) fn poll_consume_budget(cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        use std::task::Poll;

        // Charge tokio's op budget. `poll_proceed` returns `Pending` exactly when
        // the 128-op budget is exhausted; on `Ready` it hands back a guard that we
        // must `made_progress()` to actually consume one unit (otherwise the
        // charge is rolled back on drop).
        match tokio::task::coop::poll_proceed(cx) {
            Poll::Ready(guard) => guard.made_progress(),
            Poll::Pending => {
                // tokio wants us to yield; it already registered the wake.
                return Poll::Pending;
            }
        }

        // Otherwise yield if this task has held the worker past the wall-clock
        // slice. `slice_elapsed` is `None` for tasks not spawned via `env.spawn`
        // (no poll-start anchor) — those fall back to tokio's op budget alone.
        if let Some(elapsed) = crate::task_monitor::slice_elapsed() {
            if elapsed >= coop_slice() {
                // No `.await` here, so arrange our own wake before returning
                // `Pending` — the task re-polls immediately (behind the rest of
                // the run queue), re-anchoring its slice via `Instrumented::poll`.
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
        }

        Poll::Ready(())
    }
}

/// A tokio-based environment implementation.
#[derive(Clone)]
pub struct Env<Sub> {
    /// The trace context for this environment
    trace: Trace,
    trace_flags: trace::Flags,
    inner: Arc<Inner<Sub>>,
}

struct Inner<Sub> {
    /// The span ID for this environment
    span_id: u64,

    /// The sender's span_id from the inbound wire header (our parent).
    /// `None` for root environments or locally-originated work.
    parent_span: Option<trace::Span>,

    /// When the root environment was created
    root_start_time: TokioInstant,

    /// When this current environment was created
    start_time: TokioInstant,

    /// Event subscriber
    subscriber: Sub,
}

impl<Sub: Clone> Inner<Sub> {
    fn new(span_id: u64, subscriber: Sub) -> Self {
        let root_start_time = root_start_time();
        let start_time = tokio::time::Instant::now();

        Self {
            span_id,
            parent_span: None,
            root_start_time,
            start_time,
            subscriber,
        }
    }
}

impl<Sub: Clone> Env<Sub> {
    /// Creates a new tokio environment.
    pub fn new(trace: Trace, span_id: impl Into<trace::Span>, subscriber: Sub) -> Self {
        let inner = Arc::new(Inner::new(span_id.into().into_u64(), subscriber));
        let trace_flags = trace::Flags::default();
        Self {
            trace,
            trace_flags,
            inner,
        }
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

impl<Sub> EnvTrait for Env<Sub>
where
    Sub: Send + Sync + Clone + 'static,
{
    type JoinHandle<T: Send> = tokio::task::JoinHandle<T>;
    type ComputeJoinHandle<T: Send> = tokio::sync::oneshot::Receiver<T>;
    type Timer = Timer;

    fn span_id(&self) -> trace::Span {
        // Honor the task-local override so codegen-generated `tracing::span!`
        // and `tracing::event!` calls (which read `env.span_id()` directly)
        // record the originating request's span id rather than the startup
        // default. Same task-local that `trace_header()` consults.
        if let Some(header) = current_request_trace() {
            return trace::Span::from(header.span_id);
        }
        trace::Span::from(self.inner.span_id)
    }

    fn parent_span(&self) -> trace::ParentSpan {
        trace::ParentSpan(self.inner.parent_span)
    }

    fn trace(&self) -> &Trace {
        // Returns the env's stored trace, NOT the task-local override.
        // Codegen / instrumentation sites that need the override should call
        // `current_trace()`.
        &self.trace
    }

    fn current_trace(&self) -> Trace {
        // Mirror of `span_id()` — honor the task-local override so
        // `env.current_trace()` (called by codegen `tracing::span!` /
        // `tracing::event!` sites) returns the originating request's trace.
        if let Some(header) = current_request_trace() {
            return header.trace;
        }
        self.trace
    }

    fn trace_flags(&self) -> crate::trace::Flags {
        if let Some(header) = current_request_trace() {
            return header.flags;
        }
        self.trace_flags
    }

    /// Returns the trace header to serialize on outbound RPC frames.
    ///
    /// Prefers a [`REQUEST_TRACE`] task-local override (installed by HTTP
    /// ingress middleware via [`scope_request_trace`]) when present. This is
    /// what makes wire-level trace propagation work end-to-end without
    /// threading a per-request `Env` through every handler — every async task
    /// spawned via `Env::spawn` re-installs the override, so any deeply-nested
    /// `env.trace_header()` call (e.g. inside Paxos `leader.prepare`) reads
    /// the originating request's trace.
    fn trace_header(&self) -> trace::Header {
        if let Some(header) = current_request_trace() {
            return header;
        }
        trace::Header {
            trace: self.trace,
            flags: self.trace_flags,
            span_id: self.inner.span_id,
        }
    }

    fn now(&self) -> Instant {
        let now = TokioInstant::now();
        let v = now.duration_since(self.inner.root_start_time);
        Instant::from_nanos(v.as_nanos() as _)
    }

    fn span_start(&self) -> Instant {
        let v = self
            .inner
            .start_time
            .duration_since(self.inner.root_start_time);
        Instant::from_nanos(v.as_nanos() as _)
    }

    fn spawn<T: crate::task_monitor::TaskClass<Self>, F>(
        &self,
        _class: T,
        future: F,
    ) -> Self::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let instrumented = crate::task_monitor::instrument::<T, _, _>(self.clone(), future);
        tokio::spawn(with_request_trace(instrumented))
    }

    async fn spawn_compute<F, R>(&self, f: F) -> Self::ComputeJoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let (send, recv) = tokio::sync::oneshot::channel();
        // Capture the current request-trace into the rayon worker. rayon
        // worker threads are not tokio tasks, so the override is captured
        // into a local variable rather than via task-local re-scoping.
        let header = current_request_trace();
        rayon::spawn(move || {
            // We cannot install a tokio task-local from a rayon thread, but
            // any tokio futures created here would get the override via
            // their own subsequent `env.spawn` calls. This thread runs
            // synchronous work; if it needs the trace it should pass `header`
            // explicitly.
            let _ = header;
            let result = f();
            let _ = send.send(result);
        });
        recv
    }

    fn spawn_blocking<F, R>(&self, f: F) -> Self::JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        // `spawn_blocking` runs `f` on tokio's blocking pool. Since `f` is a
        // sync `FnOnce` and not a future, we can't re-scope a task-local
        // around it; trace propagation here would require capturing the
        // header into `f`'s closure if the work needs it. For RPC sites that
        // call `env.trace_header()` inside an async chain, those chains use
        // `spawn` (covered above), not `spawn_blocking`.
        tokio::task::spawn_blocking(f)
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await
    }

    async fn yield_now(&self) {
        tokio::task::yield_now().await
    }

    async fn consume_budget(&self) {
        coop::consume_budget().await
    }

    fn poll_consume_budget(&self, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        coop::poll_consume_budget(cx)
    }

    async fn resolve_dns(&self, host: &str) -> Result<Vec<std::net::SocketAddr>, Error> {
        // Use tokio's DNS resolver for actual network resolution
        tokio::net::lookup_host(host)
            .await
            .map(|addrs| addrs.collect())
            .map_err(Error::Io)
    }

    fn accept_trace(&self, header: trace::Header) -> Self {
        // Per W3C trace context: the wire `header.span_id` is the SENDER's
        // span — i.e. THIS hop's parent — and we must mint a FRESH span_id
        // for ourselves so any outbound RPC we make propagates our own id
        // (becoming the next hop's parent) rather than re-sending the
        // grandparent's. Resolves the long-standing TODO.
        let inner = Inner {
            span_id: self.random::<u64>(),
            parent_span: Some(trace::Span::from(header.span_id)),
            root_start_time: self.inner.root_start_time,
            start_time: tokio::time::Instant::now(),
            subscriber: self.inner.subscriber.clone(),
        };
        Self {
            trace: header.trace,
            trace_flags: header.flags,
            inner: Arc::new(inner),
        }
    }

    fn random<T: TypeGenerator>(&self) -> T {
        self.random_with(&bolero_generator::produce::<T>())
    }

    fn random_with<V: ValueGenerator + 'static>(&self, g: &V) -> V::Output {
        use bolero_generator::driver::Rng;
        let options = bolero_generator::driver::Options::default()
            .with_max_len(usize::MAX)
            .with_max_depth(10);
        let mut driver = Rng::new(rng(), &options);
        g.generate(&mut driver).unwrap()
    }

    fn pick<'a, T>(&self, slice: &'a [T]) -> Option<&'a T> {
        slice.choose(&mut rng())
    }

    fn pick_multiple<T, I>(&self, iter: I, n: usize) -> Vec<T>
    where
        I: IntoIterator<Item = T>,
    {
        let mut items: Vec<T> = iter.into_iter().collect();
        items.shuffle(&mut rng());
        items.truncate(n);
        items
    }

    fn shuffle<T>(&self, items: &mut [T]) {
        items.shuffle(&mut rng());
    }

    fn timer(&self, initial_duration: Duration) -> Self::Timer {
        Timer::new(initial_duration)
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
        tokio::time::timeout(duration, future).map_err(|_| Timeout)
    }

    fn system_time(&self) -> SystemTime {
        let now = StdSystemTime::now()
            .duration_since(StdSystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        SystemTime::from_nanos(now as _)
    }
}

impl<T: Send> crate::JoinHandle<T> for tokio::task::JoinHandle<T> {
    async fn join(self) -> crate::Result<T> {
        self.await
            .map_err(|e| crate::Error::Io(std::io::Error::other(format!("{e:?}"))))
    }

    fn abort(&mut self) {
        Self::abort(self)
    }
}

impl<T: Send> crate::JoinHandle<T> for tokio::sync::oneshot::Receiver<T> {
    async fn join(self) -> crate::Result<T> {
        self.await
            .map_err(|e| crate::Error::Io(std::io::Error::other(format!("{e:?}"))))
    }

    fn abort(&mut self) {
        self.close();
    }
}

pin_project_lite::pin_project! {
    pub struct Timer {
        #[pin]
        inner: tokio::time::Sleep,
    }
}

impl Timer {
    pub fn new(duration: Duration) -> Self {
        let inner = tokio::time::sleep(duration);
        Self { inner }
    }
}

impl upstream_time::Timer for Timer {
    fn now(&self) -> Instant {
        Instant::from_nanos(root_start_time().elapsed().as_nanos() as _)
    }

    fn reset(self: core::pin::Pin<&mut Self>, duration: Duration) {
        self.project().inner.set(tokio::time::sleep(duration));
    }
}

impl Future for Timer {
    type Output = Timeout;

    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        self.project().inner.poll(cx).map(|_| Timeout)
    }
}

/// An environment wrapper that spawns tasks on a dedicated tokio runtime.
///
/// All methods delegate to the inner env, except `spawn` and
/// `spawn_blocking` which use the provided runtime handle.
#[derive(Clone)]
pub struct RuntimeEnv<E> {
    inner: E,
    handle: tokio::runtime::Handle,
}

impl<E> RuntimeEnv<E> {
    pub fn new(inner: E, handle: tokio::runtime::Handle) -> Self {
        Self { inner, handle }
    }
}

impl<E> EnvTrait for RuntimeEnv<E>
where
    E: EnvTrait,
{
    type JoinHandle<T: Send> = tokio::task::JoinHandle<T>;
    type ComputeJoinHandle<T: Send> = E::ComputeJoinHandle<T>;
    type Timer = E::Timer;

    fn span_id(&self) -> trace::Span {
        // Inner impl already honors the task-local override.
        self.inner.span_id()
    }
    fn parent_span(&self) -> trace::ParentSpan {
        self.inner.parent_span()
    }
    fn trace(&self) -> &Trace {
        self.inner.trace()
    }
    fn current_trace(&self) -> Trace {
        // Inner impl already honors the task-local override.
        self.inner.current_trace()
    }
    fn trace_flags(&self) -> crate::trace::Flags {
        self.inner.trace_flags()
    }
    /// See [`Env::trace_header`] in the underlying tokio impl — checks the
    /// task-local override first so a `RuntimeEnv` wrapper still serializes
    /// the request's trace on the wire.
    fn trace_header(&self) -> trace::Header {
        if let Some(header) = current_request_trace() {
            return header;
        }
        self.inner.trace_header()
    }
    fn now(&self) -> Instant {
        self.inner.now()
    }
    fn span_start(&self) -> Instant {
        self.inner.span_start()
    }
    fn system_time(&self) -> SystemTime {
        self.inner.system_time()
    }
    fn timer(&self, d: Duration) -> Self::Timer {
        self.inner.timer(d)
    }

    fn accept_trace(&self, header: trace::Header) -> Self {
        Self {
            inner: self.inner.accept_trace(header),
            handle: self.handle.clone(),
        }
    }

    fn spawn<T: crate::task_monitor::TaskClass<Self>, F>(
        &self,
        _class: T,
        future: F,
    ) -> Self::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let instrumented = crate::task_monitor::instrument::<T, _, _>(self.clone(), future);
        self.handle.spawn(with_request_trace(instrumented))
    }

    async fn spawn_compute<F, R>(&self, f: F) -> Self::ComputeJoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.inner.spawn_compute(f).await
    }

    fn spawn_blocking<F, R>(&self, f: F) -> Self::JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.handle.spawn_blocking(f)
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await
    }

    async fn yield_now(&self) {
        tokio::task::yield_now().await
    }

    async fn consume_budget(&self) {
        coop::consume_budget().await
    }

    fn poll_consume_budget(&self, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        coop::poll_consume_budget(cx)
    }

    async fn resolve_dns(&self, host: &str) -> Result<Vec<std::net::SocketAddr>, Error> {
        tokio::net::lookup_host(host)
            .await
            .map(|addrs| addrs.collect())
            .map_err(Error::Io)
    }

    fn random<T: TypeGenerator>(&self) -> T {
        self.inner.random()
    }

    fn random_with<V: ValueGenerator + 'static>(&self, g: &V) -> V::Output {
        self.inner.random_with(g)
    }

    fn pick<'a, T>(&self, slice: &'a [T]) -> Option<&'a T> {
        self.inner.pick(slice)
    }

    fn pick_multiple<T, I>(&self, iter: I, n: usize) -> Vec<T>
    where
        I: IntoIterator<Item = T>,
    {
        self.inner.pick_multiple(iter, n)
    }

    fn shuffle<T>(&self, items: &mut [T]) {
        self.inner.shuffle(items)
    }

    fn timeout<F>(
        &self,
        duration: Duration,
        future: F,
    ) -> impl Future<Output = Result<F::Output, Timeout>> + Send
    where
        F: Future + Send,
        F::Output: Send,
    {
        tokio::time::timeout(duration, future).map_err(|_| Timeout)
    }
}

impl<E, Namespace> WithSubscriber<Namespace> for RuntimeEnv<E>
where
    E: WithSubscriber<Namespace>,
{
    type Subscriber = E::Subscriber;

    fn subscriber(&self) -> &Self::Subscriber {
        self.inner.subscriber()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nonzero_header() -> trace::Header {
        trace::Header {
            trace: Trace::new([
                0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
                0x77, 0x88,
            ]),
            flags: trace::Flags::SAMPLED,
            span_id: 0xdeadbeefcafebabe,
        }
    }

    fn make_env() -> Env<()> {
        Env::new(Trace::empty(), 0, ())
    }

    #[tokio::test]
    async fn trace_header_returns_default_outside_scope() {
        let env = make_env();
        let header = env.trace_header();
        assert_eq!(header.trace, Trace::empty());
        assert_eq!(header.span_id, 0);
    }

    #[tokio::test]
    async fn trace_header_returns_override_inside_scope() {
        let env = make_env();
        let h = nonzero_header();
        scope_request_trace(h, async {
            let observed = env.trace_header();
            assert_eq!(observed.trace, h.trace);
            assert_eq!(observed.span_id, h.span_id);
            assert!(observed.flags.is_sampled());
        })
        .await;
        // Override should NOT leak outside the scope.
        let after = env.trace_header();
        assert_eq!(after.trace, Trace::empty());
    }

    #[tokio::test]
    async fn trace_header_propagates_through_env_spawn() {
        // This is the critical test: the request-trace override must survive
        // a `tokio::spawn` boundary so deeply nested `env.trace_header()` calls
        // (e.g. inside Paxos `Transaction::commit` that's spawned at
        // `client/tx.rs:594`) still see it.
        let env = make_env();
        let h = nonzero_header();

        let observed = scope_request_trace(h, async {
            let env_for_spawn = env.clone();
            let join = env.spawn(crate::task_monitor::Test, async move {
                // We're now in a brand-new tokio task that did not inherit
                // the parent's task-locals. The override must have been
                // re-installed by `Env::spawn`.
                env_for_spawn.trace_header()
            });
            join.await.unwrap()
        })
        .await;

        assert_eq!(observed.trace, h.trace);
        assert_eq!(observed.span_id, h.span_id);
        assert!(observed.flags.is_sampled());
    }

    #[tokio::test]
    async fn trace_header_propagates_through_nested_env_spawn() {
        // Two-level deep spawn — mirrors `tx.rs:594` which itself runs
        // inside a connection-handling task and spawns the commit task.
        let env = make_env();
        let h = nonzero_header();

        let observed = scope_request_trace(h, async {
            let outer_env = env.clone();
            let outer = env.spawn(crate::task_monitor::Test, async move {
                let inner_env = outer_env.clone();
                let inner = outer_env.spawn(crate::task_monitor::Test, async move {
                    inner_env.trace_header()
                });
                inner.await.unwrap()
            });
            outer.await.unwrap()
        })
        .await;

        assert_eq!(observed.trace, h.trace);
        assert_eq!(observed.span_id, h.span_id);
    }

    #[tokio::test]
    async fn trace_header_no_override_when_outside_any_scope_in_spawn() {
        // Spawning without an outer scope_request_trace should NOT install
        // a fake override — the env's own struct fields are returned.
        let env = make_env();
        let env_for_spawn = env.clone();
        let observed = env
            .spawn(crate::task_monitor::Test, async move {
                env_for_spawn.trace_header()
            })
            .await
            .unwrap();
        assert_eq!(observed.trace, Trace::empty());
        assert_eq!(observed.span_id, 0);
    }

    #[tokio::test]
    async fn accept_trace_stores_parent_span() {
        let env = make_env();
        // Before accept_trace, no parent.
        assert_eq!(env.parent_span(), trace::ParentSpan(None));

        let h = nonzero_header();
        let accepted = env.accept_trace(h);

        // After accept_trace, parent_span is the sender's span_id.
        assert_eq!(
            accepted.parent_span(),
            trace::ParentSpan(Some(trace::Span::from(h.span_id)))
        );
        // trace_id is propagated.
        assert_eq!(accepted.current_trace(), h.trace);
        // span_id is fresh (not the sender's).
        assert_ne!(accepted.span_id(), trace::Span::from(h.span_id));
    }
}
