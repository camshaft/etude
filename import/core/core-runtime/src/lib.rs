// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Runtime environment abstractions for upstream
//!
//! This crate defines the core traits and types for runtime environments.
//! Concrete implementations are provided by the upstream-runtime-tokio and upstream-runtime-bach crates.

use bolero_generator::{TypeGenerator, ValueGenerator};
use futures_core::Stream;
use futures_util::StreamExt;
use upstream_time::{Instant, SystemTime, Timeout};
use std::{future::Future, time::Duration};
use tokio_util::sync::CancellationToken;

pub mod backoff;
#[cfg(feature = "tokio")]
pub mod byte_pool;
pub mod config;
pub mod deadline_queue;
pub mod error;
#[cfg(feature = "tokio")]
pub mod inflight_byte_budget;
pub mod memory;
pub mod probe;
pub mod rate_limiter;
pub mod subscriber;
pub mod task_monitor;
pub mod trace;

#[cfg(any(test, feature = "bach"))]
pub mod bach;
#[cfg(feature = "tokio")]
pub mod tokio;

pub use ::rayon;

/// The `#[event(task_class)]` attribute macro, re-exported so crates that
/// depend only on the runtime (e.g. core-fs-direct) can declare task classes
/// without taking a dependency on upstream-codegen.
pub use upstream_macros::event;

pub use error::{Error, ErrorKind};
pub use trace::Trace;

pub type Result<T = (), E = Error> = std::result::Result<T, E>;

pub trait WithSubscriber<Namespace> {
    type Subscriber;

    fn subscriber(&self) -> &Self::Subscriber;
}

impl<A, B, NS> WithSubscriber<NS> for (A, B) {
    type Subscriber = Self;

    fn subscriber(&self) -> &Self::Subscriber {
        self
    }
}

impl<T, NS> WithSubscriber<NS> for std::sync::Arc<T>
where
    T: WithSubscriber<NS>,
{
    type Subscriber = T::Subscriber;

    fn subscriber(&self) -> &Self::Subscriber {
        self.as_ref().subscriber()
    }
}

/// Core runtime environment trait that provides access to services like:
/// - Tracing context
/// - Time tracking
/// - Event emission
/// - Task management
/// - Parallel processing
///
/// Each runtime (tokio, bach, test) provides its own implementation.
pub trait Env: Send + Sync + Clone + 'static {
    /// The type of handle returned by spawn
    type JoinHandle<T: Send>: JoinHandle<T>;

    /// The type of handle returned by spawn_compute
    type ComputeJoinHandle<T: Send>: JoinHandle<T>;

    type Timer: upstream_time::Timer + Send + Sync + 'static;

    /// Returns the span ID for the current operation.
    /// This is used to track causality within a single trace.
    ///
    /// Implementations may consult a task-local override (set via
    /// `tokio::scope_request_trace`) so deeply-nested code observes the
    /// originating request's span_id rather than the env's startup default.
    fn span_id(&self) -> trace::Span;

    /// Returns the parent span ID — the sender's span_id from the inbound
    /// wire header. This links this hop back to whoever called it.
    ///
    /// Returns empty when no parent is known (e.g. the root env or when
    /// the request originated locally rather than arriving over the wire).
    fn parent_span(&self) -> trace::ParentSpan {
        trace::ParentSpan(None)
    }

    /// Returns a reference to the env's stored trace context.
    ///
    /// This always returns the env's struct field, *not* any per-request
    /// override. Use [`Env::current_trace`] when you want the trace seen by
    /// the in-flight request (which may differ if HTTP middleware has
    /// installed a task-local override via `tokio::scope_request_trace`).
    fn trace(&self) -> &Trace;

    /// Returns the trace context for the in-flight request, by value.
    ///
    /// Consults any task-local override set by HTTP ingress middleware, and
    /// falls back to the env's stored trace when no override is installed.
    /// Code that emits trace_id on log events / spans (the codegen
    /// `tracing::span!` / `tracing::event!` blocks, manual instrumentation,
    /// outbound RPC frames) should call this rather than [`Env::trace`].
    ///
    /// Default impl just returns `*self.trace()`; runtimes that support
    /// task-local trace propagation override it.
    fn current_trace(&self) -> Trace {
        *self.trace()
    }

    // Returns the trace flags for the current trace.
    fn trace_flags(&self) -> trace::Flags;

    fn trace_header(&self) -> trace::Header {
        trace::Header {
            trace: self.current_trace(),
            flags: self.trace_flags(),
            span_id: self.span_id().into_u64(),
        }
    }

    /// Creates a new environment with the given trace.
    /// This is used to propagate trace context across service boundaries.
    fn accept_trace(&self, header: trace::Header) -> Self;

    /// Returns the start time of the current span.
    /// Used for latency tracking and timeouts.
    fn span_start(&self) -> Instant;

    /// Returns the current monotonic time, similar to `std::time::Instant::now()`.
    ///
    /// This provides a monotonic timestamp that tracks elapsed time since the environment
    /// was created. Use this for:
    /// - Measuring durations between events
    /// - Implementing timeouts and delays
    /// - Performance timing
    ///
    /// The returned time is guaranteed to be monotonic and unaffected by system
    /// clock adjustments. In the tokio runtime, this tracks elapsed time from when
    /// the environment was created. In the bach runtime, this provides simulated
    /// monotonic time for deterministic testing.
    fn now(&self) -> Instant;

    /// Returns the current wall clock time, similar to `std::time::SystemTime::now()`.
    ///
    /// This provides absolute calendar time since the Unix epoch. Use this for:
    /// - Logging timestamps
    /// - Data versioning with real-world time
    /// - Coordination with external systems
    ///
    /// Unlike `now()`, this can go backwards if the system clock is adjusted.
    /// In the tokio runtime, this returns actual system time. In the bach runtime,
    /// this returns simulated wall clock time for deterministic testing.
    ///
    /// For measuring durations or implementing timeouts, use `now()` instead.
    fn system_time(&self) -> SystemTime;

    /// Spawns an async task with per-class instrumentation.
    ///
    /// `_class` is a zero-sized task class struct (declared with
    /// `#[event(task_class)]` or generated per RPC service/method). On every
    /// poll/drop it submits scheduling delay, poll duration, and lifecycle
    /// samples directly through this env's generated publisher — hence the
    /// [`task_monitor::TaskClass<Self>`] bound, which requires `Self` to carry
    /// the class's namespace `Publisher`.
    fn spawn<T: task_monitor::TaskClass<Self>, F>(
        &self,
        _class: T,
        future: F,
    ) -> Self::JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static;

    fn spawn_compute<F, R>(
        &self,
        f: F,
    ) -> impl std::future::Future<Output = Self::ComputeJoinHandle<R>> + Send
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static;

    #[track_caller]
    fn spawn_linked<T: task_monitor::TaskClass<Self>, F>(&self, class: T, future: F) -> LinkHandle
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let token = CancellationToken::new();
        let future = token
            .child_token()
            .run_until_cancelled_owned(Box::pin(future));

        let _ = self.spawn(class, future);

        LinkHandle(token)
    }

    fn spawn_blocking<F, R>(&self, f: F) -> Self::JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static;

    fn timer(&self, initial_duration: Duration) -> Self::Timer;

    fn timeout<F>(
        &self,
        duration: Duration,
        future: F,
    ) -> impl std::future::Future<Output = Result<F::Output, Timeout>> + Send
    where
        F: Future + Send,
        F::Output: Send;

    /// Suspends execution for the specified duration.
    /// This is useful for implementing timeouts and delays.
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send;

    fn sleep_jitter(&self, duration: Duration) -> impl Future<Output = ()> + Send {
        let jitter = 0..duration.as_millis().min(u64::MAX as u128) as u64;
        let jitter = if jitter.end == 0 {
            Duration::ZERO
        } else {
            let jitter = self.random_with(&jitter);
            core::time::Duration::from_millis(jitter)
        };
        self.sleep(duration - jitter)
    }

    /// Suspends execution until the given wall-clock `deadline`.
    ///
    /// The remaining duration is measured against [`system_time`](Self::system_time)
    /// at the moment of the call and handed to [`sleep`](Self::sleep); a `deadline`
    /// at or before the current wall clock resolves immediately (the remaining
    /// duration saturates to zero). Prefer this over a relative [`sleep`](Self::sleep)
    /// when the caller holds an absolute due time (for example a durable deadline read
    /// back from storage), so the sleep targets that instant rather than a duration
    /// recomputed by hand. Under bach, `now()` and `system_time()` advance together,
    /// so the wake lands on the deadline's simulated instant.
    fn sleep_until(&self, deadline: SystemTime) -> impl Future<Output = ()> + Send {
        self.sleep(deadline.duration_since(self.system_time()))
    }

    /// Returns `true` if this runtime is a simulation (e.g., bach).
    /// This can be used to branch behavior that relies on real OS threads
    /// vs. cooperative scheduling.
    fn is_simulation(&self) -> bool {
        false
    }

    /// Yields the task to allow other tasks to be scheduled
    fn yield_now(&self) -> impl Future<Output = ()> + Send;

    /// Consumes a unit of cooperative-scheduling budget, yielding back to the
    /// runtime only if the task's budget has been exhausted.
    ///
    /// Unlike [`Env::yield_now`], this does not unconditionally yield: it is
    /// meant to be called frequently inside long, resource-free computations
    /// (e.g. recursive snapshot traversal) to keep them cooperative without
    /// paying a yield on every call. The bach simulation runtime treats this
    /// as a no-op.
    fn consume_budget(&self) -> impl Future<Output = ()> + Send;

    /// Synchronous, `poll`-context sibling of [`Env::consume_budget`], for
    /// hand-rolled [`Future`]/[`Stream`](futures::Stream) implementations that
    /// pump work in a manual `poll` loop and therefore cannot `.await`.
    ///
    /// Returns [`Poll::Pending`](std::task::Poll::Pending) — with a wake already
    /// arranged — exactly when [`Env::consume_budget`] would have yielded (op
    /// budget exhausted or wall-clock run-slice exceeded). The caller MUST
    /// propagate that `Pending` out of its own `poll` so the task is descheduled
    /// and later re-polled (re-anchoring its time-slice), rather than spinning.
    /// Returns [`Poll::Ready(())`](std::task::Poll::Ready) while there is still
    /// budget to keep making progress within the current poll. The bach
    /// simulation runtime always returns `Ready`.
    fn poll_consume_budget(&self, cx: &mut std::task::Context<'_>) -> std::task::Poll<()>;

    /// Generates a random value of type T using bolero's TypeGenerator.
    /// This is useful for generating random test data and simulating randomized behavior.
    /// The bach runtime provides deterministic values based on the seed,
    /// while the tokio runtime uses thread_rng() for non-deterministic values.
    fn random<T: TypeGenerator>(&self) -> T {
        bolero_generator::any::any()
    }

    fn random_with<V: ValueGenerator + 'static>(&self, g: &V) -> V::Output {
        use bolero_generator::any::Any;
        g.any()
    }

    /// Chooses a random item from a slice.
    /// Returns None if the slice is empty.
    /// The bach runtime provides deterministic selection based on the seed,
    /// while the tokio runtime uses thread_rng() for non-deterministic selection.
    fn pick<'a, T>(&self, slice: &'a [T]) -> Option<&'a T> {
        use bolero_generator::any::AnySliceExt;
        if slice.is_empty() {
            return None;
        }
        Some(slice.pick())
    }

    /// Chooses n random items from an iterator.
    /// Returns a Vec containing the selected items.
    /// If n is greater than the iterator length, returns all items in random order.
    /// The bach runtime provides deterministic selection based on the seed,
    /// while the tokio runtime uses thread_rng() for non-deterministic selection.
    fn pick_multiple<T, I>(&self, iter: I, n: usize) -> Vec<T>
    where
        I: IntoIterator<Item = T>,
    {
        use bolero_generator::any::AnySliceMutExt;

        let mut items: Vec<T> = iter.into_iter().collect();
        items.shuffle();
        items.truncate(n);
        items
    }

    /// Shuffles items in a slice
    fn shuffle<T>(&self, items: &mut [T]) {
        use bolero_generator::any::AnySliceMutExt;

        items.shuffle();
    }

    /// Resolves a DNS hostname to socket addresses.
    /// Returns a Vec of resolved addresses.
    /// The bach runtime provides a virtual network,
    /// while the tokio runtime uses actual DNS resolution.
    fn resolve_dns(
        &self,
        host: &str,
    ) -> impl Future<Output = Result<Vec<std::net::SocketAddr>, Error>> + Send;

    /// Maps a stream of items to futures and processes them in parallel.
    /// - Spawns tasks up to task_limit()
    /// - Provides backpressure when limit is reached
    /// - Returns results in arbitrary order for maximum throughput
    fn par_map<S, F, Fut, T>(
        &self,
        concurrency: usize,
        stream: S,
        f: F,
    ) -> impl Stream<Item = Result<T>>
    where
        S: Stream + Send + 'static,
        S::Item: Send + 'static,
        F: Fn(S::Item) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let this = self.clone();
        stream
            .map(move |item| {
                let map = f(item);
                let this = this.clone();
                async move {
                    let join_handle = this.spawn(task_monitor::Parallel, map);
                    join_handle.join().await
                }
            })
            .buffer_unordered(concurrency)
    }

    // TODO figure this out later
    // /// Maps a stream of items to futures, processes them in parallel,
    // /// and reduces the results to a single value.
    // async fn par_reduce<S, F, R, Fut, T, U>(
    //     &self,
    //     stream: S,
    //     f: F,
    //     reduce: R,
    // ) -> Result<U, Self::JoinError>
    // where
    //     S: Stream + Send + 'static,
    //     S::Item: Send + 'static,
    //     F: Fn(S::Item) -> Fut + Send + Sync + Clone + 'static,
    //     Fut: Future<Output = T> + Send + 'static,
    //     T: Send + 'static,
    //     R: Fn(U, T) -> U + Send + Sync + Clone + 'static,
    //     U: Default + Send + 'static,
    // {
    //     let reduce = Arc::new(reduce);
    //     self.par_map(stream, f)
    //         .try_fold(U::default(), move |acc, item| {
    //             let reduce = reduce.clone();
    //             async move { Ok(reduce(acc, item)) }
    //         })
    //         .await
    // }
}

pub trait JoinHandle<T: Send>: Send {
    fn join(self) -> impl Future<Output = Result<T>> + Send;

    fn abort(&mut self);
}

#[derive(Clone)]
pub struct LinkHandle(#[allow(dead_code)] tokio_util::sync::CancellationToken);

impl LinkHandle {
    pub fn close(&self) {
        self.0.cancel();
    }
}

impl Drop for LinkHandle {
    fn drop(&mut self) {
        self.close();
    }
}

#[derive(Clone, Copy)]
pub struct Caller(&'static std::panic::Location<'static>);

impl Caller {
    #[track_caller]
    pub fn capture() -> Self {
        Self(std::panic::Location::caller())
    }
}

impl core::fmt::Display for Caller {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let file = self.0.file();
        let line = self.0.line();
        if let Some((prefix, path)) = file.rsplit_once("/src/") {
            let prefix = prefix.rsplit_once('/').map_or(prefix, |(_, p)| p);
            write!(f, "{prefix}/src/{path}:{line}")
        } else {
            write!(f, "{file}:{line}")
        }
    }
}

impl core::fmt::Debug for Caller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self)
    }
}

impl core::ops::Deref for Caller {
    type Target = std::panic::Location<'static>;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}
