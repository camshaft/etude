//! The stall timer, on the frames a client decodes from the wire.

use super::{
    common::{
        STREAM_BATCH_SIZE, read_framed_stream, serialize_frame, throwaway_env,
        write_framed_stream_at,
    },
    stall::{Progress, StallPolicy, StallTimeout, stalled_error},
};
use crate::{Reader, Writer};
use bolero::{Any as _, AnySliceExt as _};
use futures::{Stream, StreamExt, stream};
use upstream_buffer::{reader, writer};
use upstream_bytevec::{Builder, ByteVec};
use upstream_metrics::querylog::Reporter;
use upstream_runtime::{Error, Result};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    collections::VecDeque,
    future::{Future, poll_fn},
    io,
    pin::Pin,
    sync::{Arc, Mutex, OnceLock},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{sync::Notify, time::Instant};

const T: Duration = Duration::from_secs(3600);
/// Two of these exceed `STREAM_BATCH_SIZE`, so a batch holds two items.
const BIG_ITEM: usize = 100 * 1024;

fn stall(timeout: Duration) -> Option<StallPolicy> {
    StallTimeout::new(timeout).map(|timeout| StallPolicy {
        timeout,
        method: "test",
        progress: Some(Arc::default()),
    })
}

/// The request counter of a policy `stall` built.
fn frames(policy: &StallPolicy) -> Arc<Progress> {
    policy
        .progress
        .as_ref()
        .expect("a streamed request")
        .clone()
}

/// One scripted outcome per `write_chunk` call; `Accept` once the script runs out.
enum Step {
    Accept,
    ParkUntil(Arc<Notify>),
    /// Takes the whole chunk, then parks: a writer that buffers before it sends.
    TakeThenParkUntil(Arc<Notify>),
    /// Parks, then takes one contiguous chunk and completes, leaving the rest.
    ParkUntilThenTakeOne(Arc<Notify>),
    /// Parks, then takes up to this many bytes and completes, leaving the rest.
    ParkUntilThenTakeSome(Arc<Notify>, usize),
    /// Parks, then takes nothing and reports this many bytes.
    ParkUntilThenReportOnly(Arc<Notify>, usize),
    /// Takes up to this many bytes and completes.
    TakeSome(usize),
    /// Takes nothing and reports this many bytes, like a writer flushing what an
    /// earlier call took.
    ReportOnly(usize),
    /// Takes nothing and reports nothing, on this and every later call, completing
    /// without ever returning `Pending`.
    Stuck,
    /// Never wakes its task, so only the stall timer polls it.
    Drain {
        bytes_per_poll: usize,
        draining_polls: usize,
    },
}

/// What reached the wire.
#[derive(Default)]
struct WireLog {
    bytes: ByteVec,
    finished: bool,
    write_calls: usize,
    write_completions: usize,
    /// Bytes each completed write left in its chunk.
    tails: Vec<usize>,
    /// The largest chunk a write was handed.
    max_len: usize,
    /// When writes took bytes: each `Drain` poll that took some, and each
    /// completion of a write that took some.
    took: Vec<Instant>,
    /// A `Drain` write has started and not completed.
    draining: bool,
}

impl WireLog {
    /// Counts a completed write; `wired` is the wire's length when it started.
    fn complete(&mut self, wired: usize) {
        self.write_completions += 1;
        self.draining = false;
        if self.bytes.len() > wired {
            self.took.push(Instant::now());
        }
    }
}

fn take_rest<Payload: reader::storage::Infallible>(payload: &mut Payload, log: &mut WireLog) {
    while !payload.buffer_is_empty() {
        let chunk = payload.infallible_read_chunk(usize::MAX);
        writer::Storage::put_slice(&mut log.bytes, &chunk);
    }
}

/// A `Writer` whose `write_chunk` follows a script.
struct ScriptedWriter {
    log: Arc<Mutex<WireLog>>,
    script: VecDeque<Step>,
}

impl ScriptedWriter {
    fn new(script: Vec<Step>) -> (Self, Arc<Mutex<WireLog>>) {
        let log = Arc::new(Mutex::new(WireLog::default()));
        (
            Self {
                log: log.clone(),
                script: script.into(),
            },
            log,
        )
    }
}

impl Writer for ScriptedWriter {
    fn write_chunk<Payload>(
        &mut self,
        payload: &mut Payload,
    ) -> impl Future<Output = io::Result<usize>> + Send
    where
        Payload: reader::storage::Infallible + Send,
    {
        let step = self.script.pop_front().unwrap_or(Step::Accept);
        if matches!(step, Step::Stuck) {
            self.script.push_front(Step::Stuck);
        }
        let log = self.log.clone();
        async move {
            let len = payload.buffered_len();
            let wired = {
                let mut log = log.lock().unwrap();
                log.write_calls += 1;
                log.max_len = log.max_len.max(len);
                log.bytes.len()
            };
            match step {
                Step::Accept => {}
                Step::ParkUntil(release) => release.notified().await,
                Step::TakeThenParkUntil(release) => {
                    take_rest(payload, &mut log.lock().unwrap());
                    release.notified().await
                }
                Step::ParkUntilThenTakeOne(release) => {
                    release.notified().await;
                    let chunk = payload.infallible_read_chunk(usize::MAX);
                    let mut log = log.lock().unwrap();
                    writer::Storage::put_slice(&mut log.bytes, &chunk);
                    log.tails.push(len - chunk.len());
                    log.complete(wired);
                    return Ok(chunk.len());
                }
                Step::TakeSome(bytes) => {
                    let mut log = log.lock().unwrap();
                    payload.infallible_copy_into(&mut writer::Storage::with_write_limit(
                        &mut log.bytes,
                        bytes,
                    ));
                    log.complete(wired);
                    return Ok(bytes);
                }
                Step::ParkUntilThenTakeSome(release, bytes) => {
                    release.notified().await;
                    let mut log = log.lock().unwrap();
                    payload.infallible_copy_into(&mut writer::Storage::with_write_limit(
                        &mut log.bytes,
                        bytes,
                    ));
                    log.complete(wired);
                    return Ok(bytes);
                }
                Step::ParkUntilThenReportOnly(release, bytes) => {
                    release.notified().await;
                    log.lock().unwrap().complete(wired);
                    return Ok(bytes);
                }
                Step::ReportOnly(bytes) => {
                    log.lock().unwrap().complete(wired);
                    return Ok(bytes);
                }
                Step::Stuck => {
                    log.lock().unwrap().complete(wired);
                    return Ok(0);
                }
                Step::Drain {
                    bytes_per_poll,
                    mut draining_polls,
                } => {
                    log.lock().unwrap().draining = true;
                    poll_fn(|_| {
                        if draining_polls > 0 {
                            draining_polls -= 1;
                            let mut log = log.lock().unwrap();
                            let before = log.bytes.len();
                            payload.infallible_copy_into(&mut writer::Storage::with_write_limit(
                                &mut log.bytes,
                                bytes_per_poll,
                            ));
                            if log.bytes.len() > before {
                                log.took.push(Instant::now());
                            }
                        }
                        if payload.buffer_is_empty() {
                            Poll::Ready(())
                        } else {
                            Poll::Pending
                        }
                    })
                    .await
                }
            }
            let mut log = log.lock().unwrap();
            take_rest(payload, &mut log);
            log.complete(wired);
            Ok(len)
        }
    }

    fn keep_alive(&mut self, _enabled: bool) {}

    fn finish(self) -> impl Future<Output = io::Result<()>> + Send {
        self.log.lock().unwrap().finished = true;
        async { Ok(()) }
    }
}

/// A body that records when it is dropped.
struct Probed<S> {
    inner: Pin<Box<S>>,
    dropped: Arc<OnceLock<Instant>>,
}

impl<S> Drop for Probed<S> {
    fn drop(&mut self) {
        let _ = self.dropped.set(Instant::now());
    }
}

impl<S: Stream> Stream for Probed<S> {
    type Item = S::Item;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<S::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

fn probed<S: Stream>(inner: S) -> (Probed<S>, Arc<OnceLock<Instant>>) {
    let dropped = Arc::new(OnceLock::new());
    (
        Probed {
            inner: Box::pin(inner),
            dropped: dropped.clone(),
        },
        dropped,
    )
}

/// Replays recorded wire bytes to `read_framed_stream`.
struct Replay(ByteVec);

impl Reader for Replay {
    fn read_chunk<Payload>(
        &mut self,
        payload: &mut Payload,
    ) -> impl Future<Output = io::Result<usize>> + Send
    where
        Payload: writer::Storage + Send,
    {
        use reader::storage::Infallible as _;
        let before = self.0.len();
        self.0.infallible_copy_into(payload);
        let copied = before - self.0.len();
        async move { Ok(copied) }
    }
}

type Frame = std::result::Result<Vec<u8>, io::ErrorKind>;

/// The frames a client decodes from the wire, `Io` errors as their kind.
async fn decoded(log: &Arc<Mutex<WireLog>>) -> Vec<Frame> {
    let bytes = log.lock().unwrap().bytes.clone();
    read_framed_stream::<Vec<u8>, _>(Replay(bytes), ByteVec::new())
        .await
        .unwrap()
        .map(|frame| {
            frame.map_err(|e| match e {
                Error::Io(e) => e.kind(),
                other => panic!("not an Io error frame: {other:?}"),
            })
        })
        .collect()
        .await
}

/// The response's result; fails the test if the response outlives every bound
/// under test.
async fn ended(response: impl Future<Output = Result<()>>) -> Result<()> {
    tokio::time::timeout(100 * T, response)
        .await
        .expect("the response never ended")
}

fn io_kind(result: Result<()>) -> io::ErrorKind {
    io::Error::from(result.expect_err("the stream must end in an error")).kind()
}

fn item(s: &str) -> Result<Vec<u8>> {
    Ok(s.as_bytes().to_vec())
}

fn big(fill: u8) -> Result<Vec<u8>> {
    Ok(vec![fill; BIG_ITEM])
}

fn ok(s: &str) -> Frame {
    Ok(s.as_bytes().to_vec())
}

fn header() -> Builder {
    let mut out = Builder::default();
    serialize_frame(item("header"), &mut out, None);
    out
}

fn after(delay: Duration, value: &'static str) -> impl Stream<Item = Result<Vec<u8>>> + Send {
    stream::once(async move {
        tokio::time::sleep(delay).await;
        item(value)
    })
}

/// A body that reads `frames` request frames, one every half timeout, then waits
/// for good; it never yields an item.
fn reads_request_frames(
    progress: Arc<Progress>,
    frames: u32,
) -> impl Stream<Item = Result<Vec<u8>>> + Send {
    stream::once(async move {
        for _ in 0..frames {
            tokio::time::sleep(T / 2).await;
            progress.bump();
        }
        std::future::pending().await
    })
}

#[tokio::test(start_paused = true)]
async fn gate_parked_body_ends_with_one_timed_out_frame() {
    let (writer, log) = ScriptedWriter::new(vec![]);
    let (body, dropped) = probed(stream::iter([item("prefix")]).chain(stream::pending()));
    let start = Instant::now();
    let result = ended(write_framed_stream_at(
        writer,
        body,
        header(),
        None,
        &throwaway_env(),
        (),
        stall(T),
    ))
    .await;
    assert_eq!(io_kind(result), io::ErrorKind::TimedOut);
    assert_eq!(start.elapsed(), T);
    assert_eq!(dropped.get(), Some(&(start + T)));
    assert_eq!(
        decoded(&log).await,
        vec![ok("header"), ok("prefix"), Err(io::ErrorKind::TimedOut)]
    );
    assert!(log.lock().unwrap().finished);
}

#[tokio::test(start_paused = true)]
async fn body_parked_mid_write_is_dropped_at_the_bound_and_ends_after_the_write() {
    let release = Arc::new(Notify::new());
    let (writer, log) = ScriptedWriter::new(vec![Step::Accept, Step::ParkUntil(release.clone())]);
    let (body, dropped) =
        probed(stream::iter([big(0), big(1), big(2), big(3)]).chain(stream::pending()));
    let env = throwaway_env();
    let start = Instant::now();
    let task = tokio::spawn(async move {
        write_framed_stream_at(writer, body, header(), None, &env, (), stall(T)).await
    });

    tokio::time::sleep(3 * T).await;
    assert_eq!(dropped.get(), Some(&(start + T)));
    {
        let log = log.lock().unwrap();
        assert_eq!((log.write_calls, log.write_completions), (2, 1));
    }
    assert!(!task.is_finished());

    release.notify_one();
    let result = task.await.unwrap();
    assert_eq!(io_kind(result), io::ErrorKind::TimedOut);
    assert_eq!(start.elapsed(), 3 * T);
    assert_eq!(
        decoded(&log).await,
        vec![
            ok("header"),
            Ok(vec![0; BIG_ITEM]),
            Ok(vec![1; BIG_ITEM]),
            Ok(vec![2; BIG_ITEM]),
            Ok(vec![3; BIG_ITEM]),
            Err(io::ErrorKind::TimedOut),
        ]
    );

    let mut wire = header();
    for fill in 0..4 {
        serialize_frame(big(fill), &mut wire, None);
    }
    serialize_frame(Err::<Vec<u8>, Error>(stalled_error()), &mut wire, None);
    let log = log.lock().unwrap();
    assert_eq!(log.bytes, ByteVec::from(wire));
    assert!(log.finished);
}

#[tokio::test(start_paused = true)]
async fn stall_timeout_off_never_ends_a_parked_body() {
    let (writer, _log) = ScriptedWriter::new(vec![]);
    let (body, dropped) = probed(stream::pending::<Result<Vec<u8>>>());
    let env = throwaway_env();
    let task = tokio::spawn(async move {
        write_framed_stream_at(
            writer,
            body,
            header(),
            None,
            &env,
            (),
            stall(Duration::ZERO),
        )
        .await
    });

    tokio::time::sleep(100 * T).await;
    assert!(!task.is_finished());
    assert!(dropped.get().is_none());
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn slow_but_steady_progress_is_never_ended() {
    let (writer, log) = ScriptedWriter::new(vec![]);
    let body = stream::iter(0..10).then(|i| async move {
        tokio::time::sleep(T / 2).await;
        item(&format!("slow-{i}"))
    });
    let start = Instant::now();
    ended(write_framed_stream_at(
        writer,
        body,
        header(),
        None,
        &throwaway_env(),
        (),
        stall(T),
    ))
    .await
    .unwrap();
    assert_eq!(start.elapsed(), 5 * T);
    let expected: Vec<Frame> = std::iter::once(ok("header"))
        .chain((0..10).map(|i| ok(&format!("slow-{i}"))))
        .collect();
    assert_eq!(decoded(&log).await, expected);
    assert!(log.lock().unwrap().finished);
}

#[tokio::test(start_paused = true)]
async fn stall_timer_counts_from_the_last_progress() {
    let (writer, log) = ScriptedWriter::new(vec![]);
    let start = Instant::now();
    let result = ended(write_framed_stream_at(
        writer,
        after(T / 2, "late").chain(stream::pending()),
        header(),
        None,
        &throwaway_env(),
        (),
        stall(T),
    ))
    .await;
    assert_eq!(io_kind(result), io::ErrorKind::TimedOut);
    assert_eq!(start.elapsed(), T + T / 2);
    assert_eq!(
        decoded(&log).await,
        vec![ok("header"), ok("late"), Err(io::ErrorKind::TimedOut)]
    );
}

#[tokio::test(start_paused = true)]
async fn stall_timer_counts_from_an_item_the_body_waited_for() {
    let release = Arc::new(Notify::new());
    let (writer, log) = ScriptedWriter::new(vec![Step::Accept, Step::ParkUntil(release.clone())]);
    let (body, dropped) = probed(after(T / 2, "late").chain(stream::pending()));
    let env = throwaway_env();
    let start = Instant::now();
    let task = tokio::spawn(async move {
        write_framed_stream_at(writer, body, header(), None, &env, (), stall(T)).await
    });

    tokio::time::sleep(3 * T).await;
    assert_eq!(dropped.get(), Some(&(start + T / 2 + T)));
    release.notify_one();
    assert_eq!(io_kind(task.await.unwrap()), io::ErrorKind::TimedOut);
    assert_eq!(
        decoded(&log).await,
        vec![ok("header"), ok("late"), Err(io::ErrorKind::TimedOut)]
    );
}

#[tokio::test(start_paused = true)]
async fn stall_timer_counts_from_the_last_write_when_the_body_never_waits() {
    let first = Arc::new(Notify::new());
    let second = Arc::new(Notify::new());
    let (writer, log) = ScriptedWriter::new(vec![
        Step::ParkUntil(first.clone()),
        Step::ParkUntil(second.clone()),
    ]);
    let (body, dropped) =
        probed(stream::iter([big(0), big(1), big(2), big(3)]).chain(stream::pending()));
    let env = throwaway_env();
    let start = Instant::now();
    let task = tokio::spawn(async move {
        write_framed_stream_at(writer, body, header(), None, &env, (), stall(T)).await
    });

    tokio::time::sleep(T / 2).await;
    first.notify_one();
    tokio::time::sleep(3 * T).await;
    assert_eq!(dropped.get(), Some(&(start + T / 2 + T)));
    second.notify_one();
    assert_eq!(io_kind(task.await.unwrap()), io::ErrorKind::TimedOut);
    assert_eq!(
        decoded(&log).await,
        vec![
            ok("header"),
            Ok(vec![0; BIG_ITEM]),
            Ok(vec![1; BIG_ITEM]),
            Ok(vec![2; BIG_ITEM]),
            Ok(vec![3; BIG_ITEM]),
            Err(io::ErrorKind::TimedOut),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn finished_body_is_dropped_before_its_final_write() {
    let release = Arc::new(Notify::new());
    let (writer, log) = ScriptedWriter::new(vec![Step::ParkUntil(release.clone())]);
    let (body, dropped) = probed(stream::iter([item("only")]));
    let env = throwaway_env();
    let task = tokio::spawn(async move {
        write_framed_stream_at(writer, body, header(), None, &env, (), stall(T)).await
    });

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(dropped.get().is_some());
    assert!(!task.is_finished());

    release.notify_one();
    task.await.unwrap().unwrap();
    assert_eq!(decoded(&log).await, vec![ok("header"), ok("only")]);
    assert!(log.lock().unwrap().finished);
}

#[tokio::test(start_paused = true)]
async fn an_early_timer_fire_does_not_cancel_a_progressing_write() {
    let release = Arc::new(Notify::new());
    let (writer, log) = ScriptedWriter::new(vec![
        Step::Accept,
        Step::ParkUntil(release.clone()),
        Step::Accept,
    ]);
    let body = stream::iter([item("one")])
        .chain(after(T / 2, "two"))
        .chain(after(T * 8 / 10, "three"));
    let releaser = tokio::spawn(async move {
        tokio::time::sleep(T * 12 / 10).await;
        release.notify_one();
    });
    ended(write_framed_stream_at(
        writer,
        body,
        header(),
        None,
        &throwaway_env(),
        (),
        stall(T),
    ))
    .await
    .unwrap();
    releaser.await.unwrap();
    assert_eq!(
        decoded(&log).await,
        vec![ok("header"), ok("one"), ok("two"), ok("three")]
    );
    let log = log.lock().unwrap();
    assert_eq!(log.write_calls, log.write_completions);
    assert!(log.finished);
}

#[tokio::test(start_paused = true)]
async fn body_ending_in_its_own_error_is_dropped_before_the_final_write() {
    let release = Arc::new(Notify::new());
    let (writer, log) = ScriptedWriter::new(vec![Step::ParkUntil(release.clone())]);
    let (body, dropped) = probed(stream::iter([
        item("last"),
        Err(Error::Io(io::Error::other("disk"))),
    ]));
    let env = throwaway_env();
    let task = tokio::spawn(async move {
        write_framed_stream_at(writer, body, header(), None, &env, (), stall(T)).await
    });

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(dropped.get().is_some());
    assert!(!task.is_finished());

    release.notify_one();
    task.await.unwrap().unwrap();
    assert_eq!(
        decoded(&log).await,
        vec![ok("header"), ok("last"), Err(io::ErrorKind::Other)]
    );
    assert!(log.lock().unwrap().finished);
}

#[tokio::test(start_paused = true)]
async fn a_slowly_draining_write_outlives_many_timeouts() {
    const STEP: usize = 64 * 1024;
    let item = || Ok(vec![0u8; 12 * STEP]);
    let mut batch = header();
    serialize_frame(item(), &mut batch, None);
    let (writer, log) = ScriptedWriter::new(vec![Step::Drain {
        bytes_per_poll: batch.len().div_ceil(12),
        draining_polls: usize::MAX,
    }]);
    let start = Instant::now();
    ended(write_framed_stream_at(
        writer,
        stream::iter([item()]),
        header(),
        None,
        &throwaway_env(),
        (),
        stall(T),
    ))
    .await
    .unwrap();
    assert_eq!(start.elapsed(), 11 * T);
    assert_eq!(
        decoded(&log).await,
        vec![ok("header"), Ok(vec![0; 12 * STEP])]
    );
    assert!(log.lock().unwrap().finished);
}

#[tokio::test(start_paused = true)]
async fn a_trickling_write_is_never_ended_while_bytes_still_move() {
    const PER_POLL: usize = 4 * 1024;
    const POLLS: usize = 40;
    let item = || Ok(vec![0u8; POLLS * PER_POLL]);
    let mut batch = header();
    serialize_frame(item(), &mut batch, None);
    let polls = batch.len().div_ceil(PER_POLL);
    let (writer, log) = ScriptedWriter::new(vec![Step::Drain {
        bytes_per_poll: PER_POLL,
        draining_polls: usize::MAX,
    }]);
    let start = Instant::now();
    ended(write_framed_stream_at(
        writer,
        stream::iter([item()]),
        header(),
        None,
        &throwaway_env(),
        (),
        stall(T),
    ))
    .await
    .unwrap();
    assert_eq!(start.elapsed(), T * (polls as u32 - 1));
    assert_eq!(
        decoded(&log).await,
        vec![ok("header"), Ok(vec![0; POLLS * PER_POLL])]
    );
    assert!(log.lock().unwrap().finished);
}

#[tokio::test(start_paused = true)]
async fn a_write_that_stops_draining_has_its_body_dropped_one_timeout_after_its_last_byte() {
    const PER_POLL: usize = 64 * 1024;
    let (writer, log) = ScriptedWriter::new(vec![Step::Drain {
        bytes_per_poll: PER_POLL,
        draining_polls: 3,
    }]);
    let (body, dropped) = probed(stream::iter([big(0), big(1)]).chain(stream::pending()));
    let env = throwaway_env();
    let start = Instant::now();
    let task = tokio::spawn(async move {
        write_framed_stream_at(writer, body, header(), None, &env, (), stall(T)).await
    });

    tokio::time::sleep(5 * T).await;
    assert_eq!(dropped.get(), Some(&(start + 3 * T)));
    assert!(!task.is_finished());
    task.abort();
    let log = log.lock().unwrap();
    assert_eq!(log.bytes.len(), 3 * PER_POLL);
    assert_eq!((log.write_calls, log.write_completions), (1, 0));
    assert!(!log.finished);
}

#[tokio::test(start_paused = true)]
async fn request_frames_keep_a_parked_body_alive_until_one_timeout_after_the_last() {
    const FRAMES: u32 = 21;
    let policy = stall(T).unwrap();
    let (writer, log) = ScriptedWriter::new(vec![]);
    let (body, dropped) = probed(reads_request_frames(frames(&policy), FRAMES));
    let start = Instant::now();
    let result = ended(write_framed_stream_at(
        writer,
        body,
        header(),
        None,
        &throwaway_env(),
        (),
        Some(policy),
    ))
    .await;
    assert_eq!(io_kind(result), io::ErrorKind::TimedOut);
    let last_frame = start + T / 2 * FRAMES;
    assert_eq!(dropped.get(), Some(&(last_frame + T)));
    assert_eq!(
        decoded(&log).await,
        vec![ok("header"), Err(io::ErrorKind::TimedOut)]
    );
    assert!(log.lock().unwrap().finished);
}

#[tokio::test(start_paused = true)]
async fn progress_counted_off_the_response_task_is_seen_when_its_timer_fires() {
    let policy = stall(T).unwrap();
    let progress = frames(&policy);
    let (writer, _log) = ScriptedWriter::new(vec![]);
    let (body, dropped) = probed(stream::pending::<Result<Vec<u8>>>());
    let env = throwaway_env();
    let start = Instant::now();
    let task = tokio::spawn(async move {
        write_framed_stream_at(writer, body, header(), None, &env, (), Some(policy)).await
    });

    tokio::time::sleep(T / 2).await;
    for _ in 0..10 {
        progress.bump();
        tokio::time::sleep(T).await;
    }
    assert_eq!(io_kind(task.await.unwrap()), io::ErrorKind::TimedOut);
    assert_eq!(dropped.get(), Some(&(start + 11 * T)));
}

#[tokio::test(start_paused = true)]
async fn a_write_completing_after_taking_its_chunk_up_front_counts_as_progress() {
    let release = Arc::new(Notify::new());
    let (writer, log) =
        ScriptedWriter::new(vec![Step::Accept, Step::TakeThenParkUntil(release.clone())]);
    let (body, dropped) =
        probed(stream::iter([big(0), big(1), big(2), big(3)]).chain(stream::pending()));
    let env = throwaway_env();
    let start = Instant::now();
    let task = tokio::spawn(async move {
        write_framed_stream_at(writer, body, header(), None, &env, (), stall(T)).await
    });

    tokio::time::sleep(T * 9 / 10).await;
    release.notify_one();
    tokio::time::sleep(3 * T).await;
    assert_eq!(dropped.get(), Some(&(start + T * 9 / 10 + T)));
    assert_eq!(io_kind(task.await.unwrap()), io::ErrorKind::TimedOut);
    assert_eq!(
        decoded(&log).await,
        vec![
            ok("header"),
            Ok(vec![0; BIG_ITEM]),
            Ok(vec![1; BIG_ITEM]),
            Ok(vec![2; BIG_ITEM]),
            Ok(vec![3; BIG_ITEM]),
            Err(io::ErrorKind::TimedOut),
        ]
    );
}

/// Stable dcQUIC takes part of a chunk, then on the next call reports those bytes
/// without taking more, and can complete with nothing taken and nothing reported
/// while its queue drains; the loop retries all of these.
#[tokio::test(start_paused = true)]
async fn completions_that_take_nothing_are_retried() {
    let (writer, log) = ScriptedWriter::new(vec![
        Step::TakeSome(60 * 1024),
        Step::ReportOnly(20 * 1024),
        Step::ReportOnly(0),
    ]);
    ended(write_framed_stream_at(
        writer,
        stream::iter([big(0), big(1)]),
        header(),
        None,
        &throwaway_env(),
        (),
        stall(T),
    ))
    .await
    .unwrap();
    assert_eq!(
        decoded(&log).await,
        vec![ok("header"), Ok(vec![0; BIG_ITEM]), Ok(vec![1; BIG_ITEM])]
    );
    let log = log.lock().unwrap();
    assert_eq!(
        log.write_calls, 5,
        "three scripted completions, the write that drained the batch, the final write"
    );
    assert!(log.finished);
}

/// A writer that completes every call synchronously without taking anything never
/// lets the loop idle. The retry polls the stall timer and yields, so the body is
/// still dropped one bound after the first such completion and the task that
/// advances paused time gets to run.
#[tokio::test(start_paused = true)]
async fn a_writer_that_never_takes_has_its_body_dropped_at_the_bound() {
    let (writer, log) = ScriptedWriter::new(vec![Step::Stuck]);
    let (body, dropped) = probed(stream::iter([big(0), big(1)]).chain(stream::pending()));
    let env = throwaway_env();
    let start = Instant::now();
    let task = tokio::spawn(async move {
        write_framed_stream_at(writer, body, header(), None, &env, (), stall(T)).await
    });
    let clock = tokio::spawn(async {
        loop {
            tokio::time::advance(T / 10).await;
        }
    });

    tokio::time::sleep(3 * T).await;
    assert_eq!(dropped.get(), Some(&(start + T)));
    assert!(!task.is_finished());
    task.abort();
    clock.abort();
    let log = log.lock().unwrap();
    assert!(
        log.write_calls > 1,
        "the completion that took nothing was not retried"
    );
    assert!(log.bytes.is_empty());
    assert!(!log.finished);
}

#[tokio::test(start_paused = true)]
async fn a_tail_the_pending_write_left_precedes_the_timed_out_frame() {
    let release = Arc::new(Notify::new());
    let (writer, log) = ScriptedWriter::new(vec![
        Step::Accept,
        Step::ParkUntilThenTakeOne(release.clone()),
    ]);
    // The parked batch is one item, so what the write leaves is under a batch and
    // can only go out with the error frame.
    let (body, dropped) = probed(stream::iter([big(0), big(1), big(2)]).chain(stream::pending()));
    let env = throwaway_env();
    let start = Instant::now();
    let task = tokio::spawn(async move {
        write_framed_stream_at(writer, body, header(), None, &env, (), stall(T)).await
    });

    tokio::time::sleep(3 * T).await;
    assert_eq!(dropped.get(), Some(&(start + T)));
    release.notify_one();
    assert_eq!(io_kind(task.await.unwrap()), io::ErrorKind::TimedOut);
    assert_eq!(
        decoded(&log).await,
        vec![
            ok("header"),
            Ok(vec![0; BIG_ITEM]),
            Ok(vec![1; BIG_ITEM]),
            Ok(vec![2; BIG_ITEM]),
            Err(io::ErrorKind::TimedOut),
        ]
    );
    let log = log.lock().unwrap();
    assert_eq!(
        log.write_calls, 3,
        "the tail and the error frame did not go out in the one terminal write"
    );
    assert!(
        matches!(log.tails.as_slice(), [left] if *left > 0),
        "the pending write completed with its chunk only partly taken: {:?}",
        log.tails
    );
    assert!(log.finished);
}

/// A timed-out write that leaves a batch or more behind is followed by further
/// writes of what is left, under no bound, before the error frame; the body is
/// still dropped at the bound.
#[tokio::test(start_paused = true)]
async fn a_batch_the_timed_out_write_left_drains_before_the_error_frame() {
    let release_first = Arc::new(Notify::new());
    let release_second = Arc::new(Notify::new());
    let (writer, log) = ScriptedWriter::new(vec![
        Step::Accept,
        Step::ParkUntilThenTakeSome(release_first.clone(), 1024),
        Step::ParkUntil(release_second.clone()),
    ]);
    let (body, dropped) =
        probed(stream::iter([big(0), big(1), big(2), big(3)]).chain(stream::pending()));
    let env = throwaway_env();
    let start = Instant::now();
    let task = tokio::spawn(async move {
        write_framed_stream_at(writer, body, header(), None, &env, (), stall(T)).await
    });

    tokio::time::sleep(3 * T).await;
    assert_eq!(dropped.get(), Some(&(start + T)));
    assert!(!task.is_finished());
    release_first.notify_one();
    tokio::time::sleep(3 * T).await;
    assert!(
        !task.is_finished(),
        "the write of what the timed-out write left was ended by a bound"
    );
    release_second.notify_one();
    assert_eq!(io_kind(task.await.unwrap()), io::ErrorKind::TimedOut);
    assert_eq!(
        decoded(&log).await,
        vec![
            ok("header"),
            Ok(vec![0; BIG_ITEM]),
            Ok(vec![1; BIG_ITEM]),
            Ok(vec![2; BIG_ITEM]),
            Ok(vec![3; BIG_ITEM]),
            Err(io::ErrorKind::TimedOut),
        ]
    );
    let log = log.lock().unwrap();
    assert_eq!(log.write_calls, 4);
    assert!(log.finished);
}

/// The write step flushes until less than a batch is left before the body is asked
/// for more, so on a writer that takes one segment per call no write is handed more
/// than a batch and the item that crossed it.
#[tokio::test(start_paused = true)]
async fn a_segment_writer_is_never_handed_more_than_a_batch_and_an_item() {
    const ITEMS: u8 = 16;
    const SEGMENT: usize = 64 * 1024;
    let frame = {
        let mut out = Builder::default();
        serialize_frame(big(0), &mut out, None);
        out.len()
    };
    let (writer, log) = ScriptedWriter::new((0..64).map(|_| Step::TakeSome(SEGMENT)).collect());
    let body = stream::iter((0..ITEMS).map(big));
    ended(write_framed_stream_at(
        writer,
        body,
        header(),
        None,
        &throwaway_env(),
        (),
        stall(T),
    ))
    .await
    .unwrap();
    let decoded = decoded(&log).await;
    assert_eq!(decoded.len(), 1 + usize::from(ITEMS));
    assert!(decoded.iter().all(Result::is_ok));
    let log = log.lock().unwrap();
    assert!(log.finished);
    assert!(
        log.max_len < STREAM_BATCH_SIZE + frame,
        "a write was handed {} bytes, more than a batch and a frame ({})",
        log.max_len,
        STREAM_BATCH_SIZE + frame
    );
}

/// Everything the test thread's subscriber formats.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn captured_log() -> (tracing::subscriber::DefaultGuard, Captured) {
    let log = Captured::default();
    let sink = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || sink.clone())
        .finish();
    (tracing::subscriber::set_default(subscriber), log)
}

/// Serializes the tests that install the process-wide metrics registry, which admits
/// one reporter at a time.
static REPORTING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test(start_paused = true)]
async fn a_kill_is_reported_once_with_its_method_and_phase() {
    let _reporting = REPORTING.lock().await;
    let (reporter, metrics) = {
        let (builder, handle) = Reporter::builder().prometheus();
        (builder.build().expect("a backend"), handle)
    };
    let (_subscriber, log) = captured_log();
    let (writer, _wire) = ScriptedWriter::new(vec![]);
    let (body, _dropped) = probed(stream::iter([item("prefix")]).chain(stream::pending()));
    let result = ended(write_framed_stream_at(
        writer,
        body,
        header(),
        None,
        &throwaway_env(),
        (),
        stall(T),
    ))
    .await;
    assert_eq!(io_kind(result), io::ErrorKind::TimedOut);

    // A kill during a pending write that then completes having taken nothing:
    // neither the write's select nor the retry's may report the kill again.
    let release = Arc::new(Notify::new());
    let (writer, _wire) = ScriptedWriter::new(vec![
        Step::Accept,
        Step::ParkUntilThenReportOnly(release.clone(), 1024),
    ]);
    let (body, _dropped) =
        probed(stream::iter([big(0), big(1), big(2), big(3)]).chain(stream::pending()));
    let env = throwaway_env();
    let task = tokio::spawn(async move {
        write_framed_stream_at(writer, body, header(), None, &env, (), stall(T)).await
    });
    tokio::time::sleep(3 * T).await;
    release.notify_one();
    assert_eq!(io_kind(task.await.unwrap()), io::ErrorKind::TimedOut);

    reporter.flush();
    let rendered = metrics.render();
    for line in [
        "rpc_server_stalled_responses_idle{method=\"test\"} 1",
        "rpc_server_stalled_responses_mid_write{method=\"test\"} 1",
    ] {
        assert!(
            rendered.lines().any(|l| l == line),
            "{line} missing from:\n{rendered}"
        );
    }

    let log = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    let warns: Vec<&str> = log
        .lines()
        .filter(|line| line.contains("dropped the body of a streamed response"))
        .collect();
    assert_eq!(warns.len(), 2, "{log}");
    for (warn, phase) in warns.iter().zip(["phase=\"idle\"", "phase=\"mid_write\""]) {
        for field in [
            "WARN",
            "method=\"test\"",
            "idle_secs=3600",
            "timeout_secs=3600",
            phase,
        ] {
            assert!(warn.contains(field), "{field} missing from: {warn}");
        }
    }
}

/// How a `ScriptedBody` ends once its items are out.
#[derive(Clone, Copy, Debug)]
enum Ending {
    /// End of stream, after this delay.
    Clean(Duration),
    /// Its own `Err` item, after this delay.
    Error(Duration),
    /// Pending forever.
    Never,
}

fn body_error() -> Error {
    Error::Io(io::Error::other("body"))
}

/// What a `ScriptedBody` did.
#[derive(Clone, Default)]
struct BodyTrace {
    /// When each item was yielded, and whether the loop waited for it: the body's
    /// previous poll returned `Pending` and no write started since.
    yielded: Vec<(Instant, bool)>,
    /// Returned its ending.
    ended: bool,
    /// `WireLog::took` entries when the body was dropped.
    took_before_drop: Option<usize>,
}

/// A body that yields each item after its delay, then its ending.
struct ScriptedBody {
    items: VecDeque<(Duration, Vec<u8>)>,
    ending: Option<Ending>,
    sleep: Option<Pin<Box<tokio::time::Sleep>>>,
    /// `WireLog::write_calls` when the body last returned `Pending`.
    pending_at: Option<usize>,
    wire: Arc<Mutex<WireLog>>,
    trace: Arc<Mutex<BodyTrace>>,
}

impl ScriptedBody {
    /// Whether `delay` has passed since the poll that first waited for it.
    fn waited_out(&mut self, delay: Duration, cx: &mut Context<'_>) -> bool {
        if delay.is_zero() {
            return true;
        }
        let sleep = self
            .sleep
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(delay)));
        if sleep.as_mut().poll(cx).is_pending() {
            return false;
        }
        self.sleep = None;
        true
    }
}

impl Stream for ScriptedBody {
    type Item = Result<Vec<u8>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        let Some(ending) = this.ending else {
            return Poll::Ready(None);
        };
        let delay = match (this.items.front(), ending) {
            (Some(&(delay, _)), _) | (None, Ending::Clean(delay) | Ending::Error(delay)) => {
                Some(delay)
            }
            (None, Ending::Never) => None,
        };
        if !delay.is_some_and(|delay| this.waited_out(delay, cx)) {
            this.pending_at = Some(this.wire.lock().unwrap().write_calls);
            return Poll::Pending;
        }
        let waited = this.pending_at.take() == Some(this.wire.lock().unwrap().write_calls);
        let mut trace = this.trace.lock().unwrap();
        if let Some((_, item)) = this.items.pop_front() {
            trace.yielded.push((Instant::now(), waited));
            return Poll::Ready(Some(Ok(item)));
        }
        trace.ended = true;
        this.ending = None;
        Poll::Ready(matches!(ending, Ending::Error(_)).then(|| Err(body_error())))
    }
}

impl Drop for ScriptedBody {
    fn drop(&mut self) {
        let took = self.wire.lock().unwrap().took.len();
        self.trace.lock().unwrap().took_before_drop = Some(took);
    }
}

/// A writer step as generated. Each park is released `release_after` after the
/// park before it is released, the first after the response starts.
#[derive(Clone, Copy, Debug)]
enum Move {
    Accept,
    TakeSome(usize),
    ReportOnly(usize),
    ParkThenTakeSome {
        release_after: Duration,
        bytes: usize,
    },
    ParkThenReportOnly {
        release_after: Duration,
        bytes: usize,
    },
    Drain {
        bytes_per_poll: usize,
        draining_polls: usize,
    },
}

/// A random body and writer script.
#[derive(Debug)]
struct Scenario {
    /// Each item's delay and length; item `i` is that many bytes of `i`.
    items: Vec<(Duration, usize)>,
    ending: Ending,
    script: Vec<Move>,
}

impl Scenario {
    fn generate() -> Self {
        let delay = || {
            if (0..8u8).any() == 0 {
                5 * T / 4
            } else {
                T / 4 * (0..4u32).any()
            }
        };
        let release_after = || *[T / 2, 3 * T / 2].pick();
        let items = (0..(1..=12usize).any())
            .map(|_| {
                let len = if bolero::any() {
                    BIG_ITEM
                } else {
                    (10..100usize).any()
                };
                (delay(), len)
            })
            .collect();
        let ending = match (0..3u8).any() {
            0 => Ending::Clean(delay()),
            1 => Ending::Error(delay()),
            _ => Ending::Never,
        };
        let script = (0..(1..=8usize).any())
            .map(|_| match (0..6u8).any() {
                0 => Move::Accept,
                1 => Move::TakeSome((1..=BIG_ITEM).any()),
                2 => Move::ReportOnly((0..=BIG_ITEM).any()),
                3 => Move::ParkThenTakeSome {
                    release_after: release_after(),
                    bytes: (1..=BIG_ITEM).any(),
                },
                4 => Move::ParkThenReportOnly {
                    release_after: release_after(),
                    bytes: (0..=BIG_ITEM).any(),
                },
                _ => Move::Drain {
                    bytes_per_poll: (1..=STREAM_BATCH_SIZE).any(),
                    draining_polls: (1..=8usize).any(),
                },
            })
            .collect();
        Self {
            items,
            ending,
            script,
        }
    }

    /// By when the response must have ended or dropped its body: 3T past the
    /// body's delays, the parks' releases, and a timeout per `Drain` poll.
    fn liveness_bound(&self) -> Duration {
        let ending = match self.ending {
            Ending::Clean(delay) | Ending::Error(delay) => delay,
            Ending::Never => Duration::ZERO,
        };
        let body = self.items.iter().map(|&(delay, _)| delay).sum::<Duration>() + ending;
        let script = self
            .script
            .iter()
            .map(|step| match *step {
                Move::ParkThenTakeSome { release_after, .. }
                | Move::ParkThenReportOnly { release_after, .. } => release_after,
                Move::Drain { draining_polls, .. } => T * draining_polls as u32,
                Move::Accept | Move::TakeSome(_) | Move::ReportOnly(_) => Duration::ZERO,
            })
            .sum::<Duration>();
        3 * T + body + script
    }

    /// How the response ends where the scenario alone decides it. On a script of only
    /// `Accept`, `TakeSome` and `ReportOnly`, every write completes at once, so an
    /// item's delay runs while the loop waits for it: a body whose delays are all
    /// under a timeout is delivered whole, and one that waits a timeout or more for
    /// an item, or its ending, is dropped one timeout after the last item before it,
    /// which the client reads. A slow write lets a delay run unseen, so on other
    /// scripts only a body that never ends is foreseen dropped.
    fn foreseen(&self) -> Option<Foreseen> {
        let writes_at_once = self
            .script
            .iter()
            .all(|step| matches!(step, Move::Accept | Move::TakeSome(_) | Move::ReportOnly(_)));
        let ending_stalls = match self.ending {
            Ending::Clean(delay) | Ending::Error(delay) => delay >= T,
            Ending::Never => true,
        };
        let stalls_after = self
            .items
            .iter()
            .position(|&(delay, _)| delay >= T)
            .or(ending_stalls.then_some(self.items.len()));
        match stalls_after {
            None => writes_at_once.then_some(Foreseen::Delivered),
            Some(items) if writes_at_once => Some(Foreseen::DroppedAfter {
                items,
                at: self.items[..items]
                    .iter()
                    .map(|&(delay, _)| delay)
                    .sum::<Duration>()
                    + T,
            }),
            Some(_) => matches!(self.ending, Ending::Never).then_some(Foreseen::Dropped),
        }
    }
}

/// How a scenario's response ends, foreseen from the scenario alone.
#[derive(Clone, Copy, Debug)]
enum Foreseen {
    /// `Ok`, after every item and the body's own ending.
    Delivered,
    /// `TimedOut` after the first `items` items; the body is dropped `at` past the
    /// start.
    DroppedAfter { items: usize, at: Duration },
    /// The body is dropped as stalled.
    Dropped,
}

/// Frames as `header`, `<fill> x <len>`, and error kinds.
fn summary(frames: &[Frame]) -> Vec<String> {
    frames
        .iter()
        .map(|frame| match frame {
            Ok(bytes) if bytes.as_slice() == b"header" => "header".to_string(),
            Ok(bytes) => format!("{} x {}", bytes[0], bytes.len()),
            Err(kind) => format!("Err({kind:?})"),
        })
        .collect()
}

fn flat(bytes: &ByteVec) -> Vec<u8> {
    bytes
        .chunks()
        .flat_map(|chunk| chunk.iter().copied())
        .collect()
}

async fn run_scenario(scenario: &Scenario) {
    let foreseen = scenario.foreseen();
    let env = throwaway_env();
    let mut releases = Vec::new();
    let mut park = |release_after| {
        let release = Arc::new(Notify::new());
        releases.push((release_after, release.clone()));
        release
    };
    let script = scenario
        .script
        .iter()
        .map(|step| match *step {
            Move::Accept => Step::Accept,
            Move::TakeSome(bytes) => Step::TakeSome(bytes),
            Move::ReportOnly(bytes) => Step::ReportOnly(bytes),
            Move::ParkThenTakeSome {
                release_after,
                bytes,
            } => Step::ParkUntilThenTakeSome(park(release_after), bytes),
            Move::ParkThenReportOnly {
                release_after,
                bytes,
            } => Step::ParkUntilThenReportOnly(park(release_after), bytes),
            Move::Drain {
                bytes_per_poll,
                draining_polls,
            } => Step::Drain {
                bytes_per_poll,
                draining_polls,
            },
        })
        .collect();
    let (writer, log) = ScriptedWriter::new(script);
    let items: Vec<Vec<u8>> = scenario
        .items
        .iter()
        .enumerate()
        .map(|(i, &(_, len))| vec![i as u8; len])
        .collect();
    let trace = Arc::new(Mutex::new(BodyTrace::default()));
    let (body, dropped) = probed(ScriptedBody {
        items: scenario
            .items
            .iter()
            .map(|&(delay, _)| delay)
            .zip(items.iter().cloned())
            .collect(),
        ending: Some(scenario.ending),
        sleep: None,
        pending_at: None,
        wire: log.clone(),
        trace: trace.clone(),
    });
    let start = Instant::now();
    let mut response = tokio::spawn(async move {
        write_framed_stream_at(writer, body, header(), None, &env, (), stall(T)).await
    });
    tokio::spawn(async move {
        for (after, release) in releases {
            tokio::time::sleep(after).await;
            release.notify_one();
        }
    });
    let ended = tokio::time::timeout_at(start + scenario.liveness_bound(), &mut response)
        .await
        .ok()
        .map(|joined| joined.unwrap().map_err(|e| io::Error::from(e).kind()));

    let dropped_at = dropped.get().copied();
    let trace = trace.lock().unwrap().clone();
    let (wire, took, finished, draining, max_len) = {
        let log = log.lock().unwrap();
        (
            log.bytes.clone(),
            log.took.clone(),
            log.finished,
            log.draining,
            log.max_len,
        )
    };
    let frames = match ended {
        Some(_) => Some(decoded(&log).await),
        None => None,
    };
    let since = |at: &Instant| at.duration_since(start);
    let report = format!(
        "{scenario:#?}\nended: {ended:?}; dropped at: {:?}; finished: {finished}; draining: \
         {draining}; {} bytes on the wire; largest write {max_len}\nyielded at (waited for): \
         {:?}\ntook at: {:?}; recorded before the drop: {:?}\nframes: {:?}",
        dropped_at.as_ref().map(since),
        wire.len(),
        trace
            .yielded
            .iter()
            .map(|(at, waited)| (since(at), *waited))
            .collect::<Vec<_>>(),
        took.iter().map(since).collect::<Vec<_>>(),
        trace.took_before_drop,
        frames.as_deref().map(summary),
    );

    let killed = match ended {
        Some(Ok(())) => false,
        Some(Err(kind)) => {
            assert_eq!(
                kind,
                io::ErrorKind::TimedOut,
                "the response failed\n{report}"
            );
            true
        }
        None => !trace.ended,
    };
    match foreseen {
        Some(Foreseen::Delivered) => assert!(
            ended == Some(Ok(())) && trace.yielded.len() == items.len(),
            "the response did not end Ok after every item\n{report}"
        ),
        Some(Foreseen::DroppedAfter { items: yielded, at }) => assert!(
            ended == Some(Err(io::ErrorKind::TimedOut))
                && trace.yielded.len() == yielded
                && dropped_at == Some(start + at),
            "the response did not time out after {yielded} items with its body dropped at \
             {at:?}\n{report}"
        ),
        Some(Foreseen::Dropped) => assert!(killed, "the body was not dropped as stalled\n{report}"),
        None => {}
    }
    let mut expected: Vec<Frame> = std::iter::once(ok("header"))
        .chain(items[..trace.yielded.len()].iter().cloned().map(Ok))
        .collect();
    if killed {
        assert!(
            !trace.ended,
            "a body that ended was dropped as stalled\n{report}"
        );
        expected.push(Err(io::ErrorKind::TimedOut));
    } else {
        assert!(
            trace.ended,
            "the response ended Ok though its body never ended\n{report}"
        );
        if let Ending::Error(_) = scenario.ending {
            expected.push(Err(io::ErrorKind::Other));
        }
    }
    match frames {
        Some(frames) => {
            assert!(
                frames == expected,
                "the client read {:?}, not {:?}\n{report}",
                summary(&frames),
                summary(&expected)
            );
            assert!(finished, "the stream was never finished\n{report}");
        }
        None => {
            assert!(
                dropped_at.is_some(),
                "the response neither ended nor dropped its body by the bound\n{report}"
            );
            assert!(
                draining,
                "the response is held by no pending `Drain` write\n{report}"
            );
            let mut full = Builder::default();
            for frame in &expected {
                serialize_expected(frame, &mut full);
            }
            assert!(
                flat(&ByteVec::from(full)).starts_with(&flat(&wire)),
                "the wire is not a prefix of {:?}\n{report}",
                summary(&expected)
            );
            response.abort();
        }
    }

    if killed {
        let dropped_at = dropped_at.expect("a stalled body was dropped");
        let took_before_drop = trace.took_before_drop.expect("a dropped body counted");
        assert!(
            dropped_at >= start + T,
            "the body was dropped within a timeout of the start\n{report}"
        );
        if let Some(&last) = took[..took_before_drop].last() {
            assert!(
                dropped_at >= last + T,
                "the body was dropped within a timeout of a write's progress\n{report}"
            );
        }
        if let Some(&(last, _)) = trace.yielded.iter().rev().find(|(_, waited)| *waited) {
            assert!(
                dropped_at >= last + T,
                "the body was dropped within a timeout of an item the loop waited for\n{report}"
            );
        }
    }

    let largest_frame = expected
        .iter()
        .map(|frame| {
            let mut out = Builder::default();
            serialize_expected(frame, &mut out);
            out.len()
        })
        .max()
        .unwrap_or(0);
    assert!(
        max_len < STREAM_BATCH_SIZE + largest_frame,
        "a write was handed more than a batch and one frame\n{report}"
    );
}

/// `frame` as the loop serializes it, appended to `out`.
fn serialize_expected(frame: &Frame, out: &mut Builder) {
    let frame = match frame {
        Ok(bytes) => Ok(bytes.clone()),
        Err(io::ErrorKind::TimedOut) => Err(stalled_error()),
        Err(_) => Err(body_error()),
    };
    serialize_frame(frame, out, None);
}

/// A random body on a random writer script, on paused time. The client reads the
/// header, the items the body yielded in order, and the body's own error or one
/// `TimedOut` frame if the stall bound dropped the body. That drop is never within
/// a timeout of the start, of a write taking bytes or completing after taking some,
/// or of an item the loop waited for. No write is handed more than a batch and one
/// frame. By 3T past all the scripted waiting, the
/// response has ended or dropped its body, and only a pending `Drain` write, which
/// nothing polls once the body is gone, keeps it from ending. Where the scenario
/// alone decides how the response ends, it ends that way.
#[test]
fn random_bodies_on_random_writers_end_framed_and_are_never_dropped_early() {
    bolero::check!().with_iterations(200).run(|| {
        let scenario = Scenario::generate();
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap()
            .block_on(run_scenario(&scenario));
    });
}

thread_local! {
    static THREAD_HEAP: Cell<isize> = const { Cell::new(0) };
}

/// The system allocator, counting the bytes each thread allocates less those it frees.
struct ThreadCounted;

impl ThreadCounted {
    fn add(bytes: isize) {
        let _ = THREAD_HEAP.try_with(|live| live.set(live.get() + bytes));
    }

    /// The calling thread's count.
    fn live() -> isize {
        THREAD_HEAP.with(Cell::get)
    }
}

// SAFETY: every call goes to `System` unchanged, and the count is a thread-local `Cell`,
// which never allocates.
unsafe impl GlobalAlloc for ThreadCounted {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            Self::add(layout.size() as isize);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            Self::add(layout.size() as isize);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        Self::add(-(layout.size() as isize));
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(ptr, layout, new_size) };
        if !moved.is_null() {
            Self::add(new_size as isize - layout.size() as isize);
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: ThreadCounted = ThreadCounted;

/// Pins the calling thread to the CPU it is on.
#[cfg(target_os = "linux")]
fn pin_to_this_cpu() {
    // SAFETY: `set` is a zeroed `cpu_set_t`, the only memory these calls touch.
    unsafe {
        let cpu = libc::sched_getcpu();
        assert!(cpu >= 0, "{}", io::Error::last_os_error());
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_SET(cpu as usize, &mut set);
        assert_eq!(
            libc::sched_setaffinity(0, size_of::<libc::cpu_set_t>(), &set),
            0,
            "{}",
            io::Error::last_os_error()
        );
    }
}

fn alive_tasks() -> usize {
    tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks()
}

/// How a soak response ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fate {
    /// Its body ends.
    Clean,
    /// Its body parks with nothing in flight and is dropped at the bound.
    IdleKill,
    /// A write parks across the bound, which drops the body; the write is released at
    /// twice the bound.
    MidWriteKill,
}

/// A soak response, spawned when its wave starts.
struct Soaked {
    fate: Fate,
    items: Vec<Vec<u8>>,
    release: Arc<Notify>,
    wire: Arc<Mutex<WireLog>>,
    dropped: Arc<OnceLock<Instant>>,
    task: tokio::task::JoinHandle<Result<()>>,
}

impl Soaked {
    /// Response `i` of `wave`: `i % 6` picks its shape and `i / 6 % 3` how many small
    /// items past the first it yields.
    fn spawn(wave: usize, i: usize) -> Self {
        let items: Vec<Vec<u8>> = match i % 6 {
            3 => (0..2).map(|fill| vec![fill; BIG_ITEM]).collect(),
            5 => (0..4).map(|fill| vec![fill; BIG_ITEM]).collect(),
            _ => (0..=i / 6 % 3)
                .map(|j| format!("{wave}.{i}.{j}").into_bytes())
                .collect(),
        };
        let release = Arc::new(Notify::new());
        let yields = stream::iter(items.clone().into_iter().map(Ok::<_, Error>));
        let (fate, script, body) = match i % 6 {
            0 => (Fate::Clean, vec![], yields.boxed()),
            1 => (
                Fate::Clean,
                vec![],
                yields
                    .then(|item| async move {
                        tokio::time::sleep(T / 2).await;
                        item
                    })
                    .boxed(),
            ),
            2 | 3 => (
                Fate::IdleKill,
                vec![],
                yields.chain(stream::pending()).boxed(),
            ),
            4 => (
                Fate::MidWriteKill,
                vec![Step::ParkUntil(release.clone())],
                yields.chain(stream::pending()).boxed(),
            ),
            _ => (
                Fate::MidWriteKill,
                vec![Step::Accept, Step::ParkUntil(release.clone())],
                yields.chain(stream::pending()).boxed(),
            ),
        };
        let (writer, wire) = ScriptedWriter::new(script);
        let (body, dropped) = probed(body);
        let env = throwaway_env();
        let task = tokio::spawn(async move {
            write_framed_stream_at(writer, body, header(), None, &env, (), stall(T)).await
        });
        Self {
            fate,
            items,
            release,
            wire,
            dropped,
            task,
        }
    }
}

/// Spawns `n` soak responses at once, releases the parked writes at twice the bound,
/// and checks every response at three times the bound. Returns the idle and mid-write
/// kills.
async fn soak_wave(wave: usize, n: usize) -> (usize, usize) {
    let start = Instant::now();
    let responses: Vec<Soaked> = (0..n).map(|i| Soaked::spawn(wave, i)).collect();
    let count = |fate| responses.iter().filter(|r| r.fate == fate).count();
    let kills = (count(Fate::IdleKill), count(Fate::MidWriteKill));

    tokio::time::sleep(2 * T).await;
    for (i, r) in responses.iter().enumerate() {
        assert_eq!(
            r.task.is_finished(),
            r.fate != Fate::MidWriteKill,
            "wave {wave} response {i} ({:?}) at twice the bound",
            r.fate
        );
        assert!(
            r.dropped.get().is_some(),
            "wave {wave} response {i} ({:?}) still holds its body",
            r.fate
        );
    }
    assert_eq!(
        alive_tasks(),
        kills.1,
        "wave {wave}: tasks besides the parked writes"
    );
    for r in responses.iter().filter(|r| r.fate == Fate::MidWriteKill) {
        r.release.notify_one();
    }
    tokio::time::sleep(T).await;
    assert_eq!(alive_tasks(), 0, "wave {wave} left tasks behind");

    for (i, r) in responses.into_iter().enumerate() {
        let at = format!("wave {wave} response {i} ({:?})", r.fate);
        let result = r.task.await.unwrap();
        let mut expected: Vec<Frame> = std::iter::once(ok("header"))
            .chain(r.items.into_iter().map(Ok))
            .collect();
        if r.fate == Fate::Clean {
            result.unwrap_or_else(|e| panic!("{at} failed: {e:?}"));
        } else {
            assert_eq!(io_kind(result), io::ErrorKind::TimedOut, "{at}");
            assert_eq!(r.dropped.get(), Some(&(start + T)), "{at}");
            expected.push(Err(io::ErrorKind::TimedOut));
        }
        let frames = decoded(&r.wire).await;
        assert!(
            frames == expected,
            "{at}: the client read {:?}, not {:?}",
            summary(&frames),
            summary(&expected)
        );
        assert!(r.wire.lock().unwrap().finished, "{at} was never finished");
        assert_eq!(
            Arc::strong_count(&r.wire),
            1,
            "{at}: its writer outlived it"
        );
        assert_eq!(
            Arc::strong_count(&r.dropped),
            1,
            "{at}: its body outlived it"
        );
    }
    kills
}

/// Twenty waves of a hundred concurrent responses on one paused runtime, mixing bodies
/// that end, bodies parked with nothing in flight, and bodies behind a write parked
/// across the bound. In every wave each kill lands at the bound and its client reads
/// the items then one `TimedOut` frame, every other response delivers everything, and
/// once the wave is over no task is left and every body and writer has been dropped.
/// From the second wave to the last the test thread's heap grows by under a byte per
/// response, and the kill counters count every kill by phase.
#[test]
fn waves_of_killed_and_clean_responses_leave_no_tasks_or_heap_behind() {
    const WAVES: usize = 20;
    const PER_WAVE: usize = 100;
    let _reporting = REPORTING.blocking_lock();
    let (reporter, metrics) = {
        let (builder, handle) = Reporter::builder().prometheus();
        // Only `flush`, on this thread, drains the registry.
        let day = Duration::from_secs(24 * 3600);
        let reporter = builder.period(day).absorb_period(day).build();
        (reporter.expect("a backend"), handle)
    };
    // Every kill records into the same per-CPU metrics page.
    #[cfg(target_os = "linux")]
    pin_to_this_cpu();
    let mut live = Vec::with_capacity(WAVES);
    let (mut idle, mut mid_write) = (0, 0);
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(async {
            assert_eq!(alive_tasks(), 0);
            for wave in 0..WAVES {
                let kills = soak_wave(wave, PER_WAVE).await;
                idle += kills.0;
                mid_write += kills.1;
                reporter.flush();
                live.push(ThreadCounted::live());
            }
        });
    let growth = live[WAVES - 1] - live[1];
    let responses = (WAVES - 2) * PER_WAVE;
    // Under a byte per response.
    assert!(
        growth < responses as isize,
        "the heap grew {growth} bytes over {responses} responses after the second wave: {live:?}"
    );

    reporter.flush();
    let rendered = metrics.render();
    for line in [
        format!("rpc_server_stalled_responses_idle{{method=\"test\"}} {idle}"),
        format!("rpc_server_stalled_responses_mid_write{{method=\"test\"}} {mid_write}"),
    ] {
        assert!(
            rendered.lines().any(|l| l == line),
            "{line} missing from:\n{rendered}"
        );
    }
}
