use super::common::serialize_frame;
use crate::Writer;
use core::{
    future::{Future, poll_fn},
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll, ready},
    time::Duration,
};
use upstream_buffer::{reader, writer};
use upstream_bytevec::Builder;
use upstream_runtime::{Env, Error, Result};
use upstream_time::{Instant, Timer};
use upstream_wire::Serialize;
use std::{io, sync::Arc};

/// Longest a server-streamed response may go without progress. Never zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StallTimeout(Duration);

impl StallTimeout {
    /// `None` for zero: no timeout.
    pub const fn new(timeout: Duration) -> Option<Self> {
        if timeout.is_zero() {
            None
        } else {
            Some(Self(timeout))
        }
    }

    pub const fn get(self) -> Duration {
        self.0
    }
}

/// A count of the request frames a response's reader has seen, compared only for
/// change.
#[derive(Debug, Default)]
pub struct Progress(AtomicUsize);

impl Progress {
    /// Counts one event.
    pub fn bump(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    /// Events counted so far.
    pub fn count(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
}

/// The stall bound one response runs under: the response ends once nothing has
/// progressed for `timeout`. Progress is a request frame arriving, which the
/// request reader counts on `progress`, and the response's own events, which the
/// writer loop counts on a counter of its own: a batch the body made the loop wait
/// for, bytes the transport takes from a chunk write it has not completed, and the
/// completion of a write that took any. The stall timer compares both counts
/// whenever the response waits, so a stall ends the response one timeout after the
/// start of the wait that saw no progress; a frame arriving on another task is seen
/// when the stall timer fires, so a response parked on such frames ends one to two
/// timeouts after the last. A pending write is never abandoned: once it completes,
/// what the loop still holds (under a batch and a frame) goes out with no further
/// bound, then the client reads one `Err(Io(TimedOut))` frame and end-of-stream,
/// never a clean end from a task that runs to completion (a task dropped at
/// shutdown ends its stream by the writer's drop). A write the peer never completes
/// keeps the task, one batch and the stream until the connection dies, and nothing
/// the body held.
#[derive(Debug)]
pub struct StallPolicy {
    pub timeout: StallTimeout,
    /// The method whose response this is; named by the kill WARN and counter.
    pub method: &'static str,
    /// Request frames seen, counted by the request reader; `None` when the request
    /// is not streamed.
    pub progress: Option<Arc<Progress>>,
}

impl StallPolicy {
    fn frames(&self) -> usize {
        self.progress
            .as_ref()
            .map_or(0, |progress| progress.count())
    }
}

/// Where a response was waiting when its stall timeout ended it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum StallPhase {
    /// On its body, with nothing in flight.
    Idle,
    /// On a chunk write the transport had not completed.
    MidWrite,
}

impl StallPhase {
    fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::MidWrite => "mid_write",
        }
    }
}

/// Kill counter, suffixed with the phase and aggregated by method.
const STALLED_RESPONSES: &str = "rpc.server.stalled_responses";

pin_project_lite::pin_project! {
    /// One response body's time since its last progress, against its policy's
    /// timeout. Armed at the response's first wait: the timer is set once and
    /// re-armed only when it fires before the timeout has passed since the last
    /// progress, so a response that never waits pays nothing and progress costs
    /// one clock read at the poll that sees it.
    #[project = StallTimerProj]
    pub(crate) enum StallTimer<'a, T> {
        Off,
        /// Under a policy, before the response's first wait.
        Idle {
            policy: &'a StallPolicy,
        },
        Armed {
            policy: &'a StallPolicy,
            last_progress: Instant,
            // `policy.progress` as of the last poll.
            seen: usize,
            // The response's own progress count as of the last poll.
            seen_own: usize,
            #[pin]
            timer: T,
        },
    }
}

impl<'a, T: Timer + Send> StallTimer<'a, T> {
    pub(crate) fn new(policy: Option<&'a StallPolicy>) -> Self {
        match policy {
            None => Self::Off,
            Some(policy) => Self::Idle { policy },
        }
    }

    pub(crate) fn policy(&self) -> Option<&'a StallPolicy> {
        match self {
            Self::Off => None,
            Self::Idle { policy } | Self::Armed { policy, .. } => Some(*policy),
        }
    }

    /// Resolves with the policy and the idle time once no progress has been seen
    /// for the timeout. `own` is the response's own progress count (see
    /// [`StallPolicy`]). A poll, including the one a timer fire wakes, that finds
    /// the policy's progress or `own` advanced records progress at that instant
    /// and keeps waiting. Never resolves when off. Cancel-safe: dropping it loses
    /// nothing.
    pub(crate) fn stalled<'b, E: Env<Timer = T>>(
        mut self: Pin<&'b mut Self>,
        env: &'b E,
        own: &'b AtomicUsize,
    ) -> impl Future<Output = (&'a StallPolicy, Duration)> + Send + 'b {
        poll_fn(move |cx| self.as_mut().poll_stalled(cx, env, own))
    }

    fn poll_stalled<E: Env<Timer = T>>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        env: &E,
        own: &AtomicUsize,
    ) -> Poll<(&'a StallPolicy, Duration)> {
        if let StallTimerProj::Idle { policy } = self.as_mut().project() {
            let policy = *policy;
            self.set(Self::Armed {
                policy,
                last_progress: env.now(),
                seen: policy.frames(),
                seen_own: own.load(Ordering::Relaxed),
                timer: env.timer(policy.timeout.get()),
            });
        }
        let StallTimerProj::Armed {
            policy,
            last_progress,
            seen,
            seen_own,
            mut timer,
        } = self.project()
        else {
            return Poll::Pending;
        };
        let timeout = policy.timeout.get();
        loop {
            let count = policy.frames();
            let own_now = own.load(Ordering::Relaxed);
            if count != *seen || own_now != *seen_own {
                *seen = count;
                *seen_own = own_now;
                *last_progress = env.now();
            }
            ready!(timer.as_mut().poll(cx));
            let idle = env.now().duration_since(*last_progress);
            if idle >= timeout {
                return Poll::Ready((*policy, idle));
            }
            timer.as_mut().reset(timeout - idle);
        }
    }
}

/// A chunk payload that adds the bytes the transport takes from it, and a mark at
/// the write's end, to `own`, the response's own progress count the stall timer
/// reads while the write is pending.
pub(crate) struct Counted<'a> {
    payload: &'a mut Builder,
    progress: OwnProgress<'a>,
}

impl<'a> Counted<'a> {
    pub(crate) fn new(payload: &'a mut Builder, own: &'a AtomicUsize) -> Self {
        let count = own.load(Ordering::Relaxed);
        Self {
            payload,
            progress: OwnProgress {
                own,
                start: count,
                count,
            },
        }
    }
}

/// The count written through to `own`; borrowed apart from `payload`, which the
/// chunks `Counted` hands out borrow.
struct OwnProgress<'a> {
    own: &'a AtomicUsize,
    start: usize,
    count: usize,
}

impl OwnProgress<'_> {
    #[inline]
    fn add(&mut self, bytes: usize) {
        self.count += bytes;
        self.own.store(self.count, Ordering::Relaxed);
    }
}

/// A write that took bytes marks its end too, so one that took them all up front
/// still counts when it completes; a write that took nothing is not progress.
impl Drop for OwnProgress<'_> {
    fn drop(&mut self) {
        if self.count > self.start {
            self.own.store(self.count + 1, Ordering::Relaxed);
        }
    }
}

impl reader::Storage for Counted<'_> {
    type Error = core::convert::Infallible;

    #[inline]
    fn buffered_len(&self) -> usize {
        self.payload.buffered_len()
    }

    #[inline]
    fn read_chunk(
        &mut self,
        watermark: usize,
    ) -> core::result::Result<reader::storage::Chunk<'_>, Self::Error> {
        let chunk = self.payload.read_chunk(watermark)?;
        self.progress.add(chunk.len());
        Ok(chunk)
    }

    #[inline]
    fn partial_copy_into<Dest>(
        &mut self,
        dest: &mut Dest,
    ) -> core::result::Result<reader::storage::Chunk<'_>, Self::Error>
    where
        Dest: writer::Storage + ?Sized,
    {
        let mut dest = dest.track_write();
        let chunk = self.payload.partial_copy_into(&mut dest)?;
        self.progress.add(dest.written_len() + chunk.len());
        Ok(chunk)
    }

    #[inline]
    fn copy_into<Dest>(&mut self, dest: &mut Dest) -> core::result::Result<(), Self::Error>
    where
        Dest: writer::Storage + ?Sized,
    {
        let mut dest = dest.track_write();
        self.payload.copy_into(&mut dest)?;
        self.progress.add(dest.written_len());
        Ok(())
    }
}

/// How the framing loop stopped. The body is already dropped when the caller acts
/// on it.
pub(crate) enum End {
    /// The body ended, or yielded its terminal `Err` item; `out` holds the tail.
    Drained,
    /// No progress for the timeout. The body was dropped and the kill logged at
    /// the bound; a write pending then has completed, what it left has drained to
    /// under a batch, and `out` holds the rest.
    Stalled,
}

/// Ends a stalled stream so its client never reads a clean end: one
/// `Err(Io(TimedOut))` frame after whatever `out` holds, then end-of-stream, which
/// may wait for the peer's window. A terminal write that fails leaves the stream to
/// the writer's drop. Returns the stall error.
pub(crate) async fn send_stall_error<T: Serialize, W: Writer>(
    stream: W,
    mut out: Builder,
    version_idx: Option<u32>,
) -> Result<()> {
    serialize_frame(Err::<T, Error>(stalled_error()), &mut out, version_idx);
    let _ = stream.write_to_end(out).await;
    Err(stalled_error())
}

/// `Error::Io`, so callers classify it as transport-level.
pub(crate) fn stalled_error() -> Error {
    Error::Io(io::Error::new(
        io::ErrorKind::TimedOut,
        "response body made no progress within the stall timeout",
    ))
}

/// The kill's WARN and counter.
pub(crate) fn report_kill(policy: &StallPolicy, idle: Duration, phase: StallPhase) {
    tracing::warn!(
        method = policy.method,
        idle_secs = idle.as_secs(),
        timeout_secs = policy.timeout.get().as_secs(),
        phase = phase.name(),
        "dropped the body of a streamed response that made no progress within the stall timeout"
    );
    // No registry (tests, ad-hoc bins): the kill is only logged.
    if let Some(registry) = upstream_metrics::querylog::global::get() {
        registry
            .register_counter(
                format!("{STALLED_RESPONSES}.{}", phase.name()),
                Some(format!("Method|{}", policy.method)),
            )
            .increment(1);
    }
}
