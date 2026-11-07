// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Per-task-class instrumentation for spawn-site classification.
//!
//! The [`TaskClass`] trait lives in `upstream-metrics` and is re-exported here.
//! This module adds the [`Instrumented`] future that, on every poll/drop,
//! submits scheduling/poll/lifecycle samples through the class's generated
//! publisher (carried by a cloned `env`). Wrap a future with
//! [`instrument`] — used by `Env::spawn` for spawned tasks and inline (awaited
//! directly) by the RPC dispatch codegen for per-method instrumentation.

use std::{
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::OnceLock,
    task::{Context, Poll},
};

pub use upstream_metrics::task_monitor::{Parallel, TaskClass, Test, Unlabeled};

/// Wraps `task` so its scheduling/poll/lifecycle metrics are submitted, via the
/// class `T`, through the publisher carried by `env`.
///
/// `env` is cloned into the future so each poll can call `T::publish_*`. Works
/// both for spawned tasks (`Env::spawn`) and for futures awaited inline.
pub fn instrument<T, E, F>(env: E, task: F) -> Instrumented<T, E, F>
where
    T: TaskClass<E>,
    F: Future,
{
    T::publish_instrumented(&env);
    Instrumented {
        task,
        state: InstrumentedState::Created {
            created_at: instant_now(),
        },
        env,
        _class: PhantomData,
    }
}

// ── Instrumented Future ──────────────────────────────────────────────────────

enum InstrumentedState {
    Created { created_at: u64 },
    Polled { last_poll_end: u64 },
    Completed,
}

pin_project_lite::pin_project! {
    /// A future instrumented with a [`TaskClass`]. Submits samples through the
    /// cloned `env` on every poll, and a drop sample if cancelled.
    pub struct Instrumented<T, E, F>
    where
        T: TaskClass<E>,
    {
        #[pin]
        task: F,
        state: InstrumentedState,
        env: E,
        _class: PhantomData<fn() -> T>,
    }

    impl<T, E, F> PinnedDrop for Instrumented<T, E, F>
    where
        T: TaskClass<E>,
    {
        fn drop(this: Pin<&mut Self>) {
            // Only count drops of tasks that never ran to completion — i.e.
            // cancelled tasks. A completed future is also dropped, but that is
            // not a cancellation.
            let projected = this.project();
            if !matches!(projected.state, InstrumentedState::Completed) {
                T::publish_dropped(projected.env);
            }
        }
    }
}

impl<T, E, F> Future for Instrumented<T, E, F>
where
    T: TaskClass<E>,
    F: Future,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let poll_start = instant_now();
        // Anchor the cooperative time-slice to the start of this poll. Every
        // poll re-stamps it, so the slice resets whenever the task yielded for
        // ANY reason (tokio coop, a socket/lock/timer await, or an explicit
        // `yield_now`) and was later re-polled — not just on our own yields.
        // `consume_budget` reads this to decide when a CPU-bound loop has held
        // the worker long enough. See [`slice_elapsed`].
        // Install this poll's anchor for the duration of the poll. The returned
        // guard restores the previous anchor on return (or panic) so it never
        // outlives the poll that set it: otherwise a non-instrumented task later
        // scheduled on this worker would observe a stale `Some(timestamp)` via
        // `slice_elapsed` and yield spuriously (those tasks must fall back to
        // tokio's op budget alone), and a nested instrumented poll would wipe the
        // outer poll's anchor instead of restoring it.
        let _slice_guard = enter_slice(poll_start);
        let this = self.project();
        let env = this.env;

        // On the first poll, report the spawn→first-poll delay; on subsequent
        // polls, the scheduler wait since the previous poll ended (reported
        // below alongside the poll duration). Polling after completion is a
        // contract violation, but be defensive (treat as zero wait).
        let scheduled_ns = match this.state {
            InstrumentedState::Created { created_at } => {
                T::publish_first_poll(env, dur(poll_start.saturating_sub(*created_at)));
                0
            }
            InstrumentedState::Polled { last_poll_end } => {
                poll_start.saturating_sub(*last_poll_end)
            }
            InstrumentedState::Completed => 0,
        };

        let result = this.task.poll(cx);

        let poll_end = instant_now();
        T::publish_poll(
            env,
            dur(scheduled_ns),
            dur(poll_end.saturating_sub(poll_start)),
        );

        *this.state = if result.is_ready() {
            InstrumentedState::Completed
        } else {
            InstrumentedState::Polled {
                last_poll_end: poll_end,
            }
        };

        result
    }
}

/// Converts a nanosecond count to a [`Duration`](core::time::Duration) for the
/// task-class publish hooks.
#[inline]
fn dur(ns: u64) -> core::time::Duration {
    core::time::Duration::from_nanos(ns)
}

/// Returns current time in nanoseconds since an arbitrary epoch (monotonic).
#[inline]
fn instant_now() -> u64 {
    static EPOCH: OnceLock<std::time::Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(std::time::Instant::now);
    epoch.elapsed().as_nanos() as u64
}

thread_local! {
    /// `instant_now()` value (nanoseconds) at the start of the current poll on
    /// this worker thread, set by [`Instrumented::poll`]. The cooperative
    /// time-slice (`Env::consume_budget`) measures against this so a CPU-bound
    /// loop yields once it has held the worker for the configured slice —
    /// re-anchored every poll, so any await that deschedules the task resets it.
    ///
    /// `None` means "no instrumented poll is in progress on this thread" (e.g. a
    /// task spawned directly on tokio without going through `env.spawn`); in
    /// that case the time-slice is inactive and only tokio's op budget applies.
    static SLICE_START_NS: core::cell::Cell<Option<u64>> = const { core::cell::Cell::new(None) };
}

/// Installs `now_ns` as the cooperative time-slice anchor and returns a guard
/// that restores the previous anchor when dropped (on poll return or panic).
///
/// Save-and-restore — rather than set-then-clear-to-`None` — keeps the anchor
/// from outliving the poll that set it (a later non-instrumented task on this
/// worker must see `None` and fall back to tokio's budget) while also letting a
/// nested instrumented poll restore the outer poll's anchor instead of wiping it.
#[inline]
fn enter_slice(now_ns: u64) -> SliceGuard {
    let prev = SLICE_START_NS.with(|c| c.replace(Some(now_ns)));
    SliceGuard(prev)
}

struct SliceGuard(Option<u64>);

impl Drop for SliceGuard {
    fn drop(&mut self) {
        SLICE_START_NS.with(|c| c.set(self.0));
    }
}

/// How long the current poll's task has been running since its slice began, or
/// `None` if no instrumented poll is active on this thread (no anchor set).
///
/// Used by `Env::consume_budget` to decide whether to yield on top of tokio's
/// op-count budget. The anchor is re-stamped at the start of every poll by
/// [`Instrumented::poll`], so callers never need to reset it manually — a yield
/// (cooperative or otherwise) re-polls the task and the slice starts fresh.
#[inline]
pub fn slice_elapsed() -> Option<core::time::Duration> {
    let start = SLICE_START_NS.with(|c| c.get())?;
    Some(dur(instant_now().saturating_sub(start)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::Arc,
        task::{Wake, Waker},
    };

    fn noop_waker() -> Waker {
        struct Noop;
        impl Wake for Noop {
            fn wake(self: Arc<Self>) {}
        }
        Waker::from(Arc::new(Noop))
    }

    /// A future that returns `Pending` `remaining` times, then `Ready`.
    struct Pending {
        remaining: u32,
    }
    impl Future for Pending {
        type Output = u32;
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u32> {
            if self.remaining == 0 {
                Poll::Ready(42)
            } else {
                self.remaining -= 1;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    /// Drives an instrumented future to completion: exercises the Created →
    /// Polled → Completed state transitions and the submit sites. The built-in
    /// `Test` class submits nothing, so this asserts the state machine runs and
    /// completes rather than observing metric values.
    #[test]
    fn instrumented_future_runs_to_completion() {
        let fut = instrument::<Test, _, _>((), Pending { remaining: 3 });
        let mut fut = Box::pin(fut);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        loop {
            if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
                assert_eq!(out, 42);
                break;
            }
        }
    }

    /// Polling once then dropping exercises the cancellation (non-Completed
    /// drop) branch of `PinnedDrop` without panicking.
    #[test]
    fn instrumented_future_cancelled_drop_does_not_panic() {
        let fut = instrument::<Parallel, _, _>((), Pending { remaining: 5 });
        let mut fut = Box::pin(fut);
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        assert!(fut.as_mut().poll(&mut cx).is_pending());
        drop(fut);
    }

    /// Without an active poll anchor, the time-slice is inactive
    /// (`slice_elapsed` is `None`) so callers fall back to tokio's budget.
    #[test]
    fn slice_elapsed_is_none_without_anchor() {
        // Fresh thread → no anchor set.
        std::thread::spawn(|| {
            assert_eq!(slice_elapsed(), None);
        })
        .join()
        .unwrap();
    }

    /// The slice anchor is active *inside* an instrumented poll (so
    /// `consume_budget` can measure run time) but is cleared once the poll
    /// returns, so a later non-instrumented task on the same worker sees `None`.
    #[test]
    fn slice_anchor_is_scoped_to_the_poll() {
        /// Samples `slice_elapsed()` from inside its own poll, then returns.
        struct SampleInsidePoll;
        impl Future for SampleInsidePoll {
            type Output = Option<core::time::Duration>;
            fn poll(
                self: Pin<&mut Self>,
                _cx: &mut Context<'_>,
            ) -> Poll<Option<core::time::Duration>> {
                Poll::Ready(slice_elapsed())
            }
        }

        std::thread::spawn(|| {
            assert_eq!(slice_elapsed(), None, "no anchor before any poll");

            let mut fut = Box::pin(instrument::<Test, _, _>((), SampleInsidePoll));
            let waker = noop_waker();
            let mut cx = Context::from_waker(&waker);
            let Poll::Ready(inside) = fut.as_mut().poll(&mut cx) else {
                panic!("future should be ready");
            };

            // Anchor was active during the instrumented poll...
            assert!(inside.is_some(), "anchor must be set inside the poll");
            // ...and is restored to `None` once the poll returns, so a
            // non-instrumented task on this worker falls back to tokio's budget.
            assert_eq!(
                slice_elapsed(),
                None,
                "anchor must be cleared after the poll returns"
            );
        })
        .join()
        .unwrap();
    }
}
