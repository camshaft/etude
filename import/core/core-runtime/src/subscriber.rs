// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! A generic, event-type-erased view of a published event, the single `Testing`
//! subscriber that observes every event, and the pluggable collector ("sink") it
//! funnels events into.
//!
//! Each namespace's generated `Subscriber` trait has one strongly-typed method per
//! event (`on_<ns>_<event>(env, caller, field1, ..)`).
//!
//! - [`Testing<S>`] is one struct, defined here. Each namespace's generated code
//!   `impl`s that namespace's `Subscriber` (and, via the blanket below, its
//!   `WithSubscriber`) for this single struct, so one `Testing` instance is a
//!   valid subscriber for every namespace and its one sink aggregates events
//!   across all namespaces it is attached to.
//! - [`Event`] is an erased view over any event -- its namespace, name, caller,
//!   level, and its fields as `(name, &dyn Debug)` pairs. It can be iterated,
//!   looked up by field name, and (via [`fmt::Display`]) written straight to a
//!   formatter as `namespace:name field={value:?} ...` with no intermediate
//!   allocation.
//! - [`EventSink`] is the inner trait a caller supplies: it receives every event
//!   (including metrics-only ones) as an [`Event`] and decides what to do with it
//!   (collect it, filter it, forward it, count it, ...).
//! - [`SnapshotLog`] is the default sink: it renders each event to a line
//!   (optionally timestamp-prefixed), skips metrics-only (`level == "none"`)
//!   events, and, on drop, hands the joined log to a callback (e.g.
//!   `insta::assert_snapshot!`).

use crate::{Caller, Env, WithSubscriber};
use std::{
    fmt,
    sync::{Arc, Mutex},
};

/// A single event field: its declared name and its value, erased to `&dyn Debug`
/// (field values render with `{:?}`).
pub struct Field<'a> {
    /// The field's declared name (e.g. `"endpoint"`).
    pub name: &'static str,
    /// The field's value, erased so a sink can render it uniformly.
    pub value: &'a dyn fmt::Debug,
}

/// Renders as `name={value:?}`.
impl fmt::Display for Field<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={:?}", self.name, self.value)
    }
}

impl fmt::Debug for Field<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Field")
            .field("name", &self.name)
            .field("value", &format_args!("{:?}", self.value))
            .finish()
    }
}

/// An erased, generic view over a published event.
///
/// This is the "operate on an event generically" surface: a sink can read the
/// event's identity ([`Event::namespace`], [`Event::name`],
/// [`Event::qualified_name`]), its [`Event::caller`] and [`Event::level`],
/// iterate its [`Event::fields`], fetch one by [`Event::field`], or write the
/// whole thing out via [`fmt::Display`] -- all without knowing the event's
/// concrete Rust type.
pub struct Event<'a> {
    namespace: &'static str,
    name: &'static str,
    level: &'static str,
    caller: Caller,
    fields: &'a [Field<'a>],
}

impl<'a> Event<'a> {
    /// Construct an event view. Called from the generated subscriber method,
    /// which owns the borrowed `fields` slice for the duration of the call.
    pub fn new(
        namespace: &'static str,
        name: &'static str,
        level: &'static str,
        caller: Caller,
        fields: &'a [Field<'a>],
    ) -> Self {
        Self {
            namespace,
            name,
            level,
            caller,
            fields,
        }
    }

    /// The event's namespace (e.g. `"rpc"`).
    pub fn namespace(&self) -> &'static str {
        self.namespace
    }

    /// The event's short name (e.g. `"request_completed"`).
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The fully-qualified `namespace:name` (e.g. `"rpc:request_completed"`) -- the
    /// leading token of the rendered log line.
    pub fn qualified_name(&self) -> String {
        format!("{}:{}", self.namespace, self.name)
    }

    /// The event's tracing level (e.g. `"debug"`; `"none"` marks a metrics-only
    /// event that the default [`SnapshotLog`] sink and the tracing subscriber
    /// suppress).
    pub fn level(&self) -> &'static str {
        self.level
    }

    /// Where the event was published from.
    pub fn caller(&self) -> &Caller {
        &self.caller
    }

    /// Iterate over the event's fields in declaration order.
    pub fn fields(&self) -> impl Iterator<Item = &Field<'a>> {
        self.fields.iter()
    }

    /// Look up a single field by name.
    pub fn field(&self, name: &str) -> Option<&Field<'a>> {
        self.fields.iter().find(|f| f.name == name)
    }
}

/// Writes the event as it appears in a snapshot log:
/// `namespace:name field1={value1:?} field2={value2:?} ...`, straight into the
/// formatter (no per-field string allocation). A sink prepends its own prefix
/// (e.g. a timestamp -- see [`SnapshotLog`]).
impl fmt::Display for Event<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.namespace, self.name)?;
        for field in self.fields {
            write!(f, " {field}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Event<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Event")
            .field("namespace", &self.namespace)
            .field("name", &self.name)
            .field("level", &self.level)
            .field("caller", &self.caller)
            .field("fields", &self.fields)
            .finish()
    }
}

/// The pluggable collector every generated `Testing` subscriber method hands its
/// event to. Implement this to subscribe to *everything* and decide, per event,
/// what to do with it.
///
/// The bounds mirror the generated `Subscriber` trait (`Send + Sync + Clone +
/// 'static`) so a [`Testing<S>`] built from a sink is itself a valid subscriber.
pub trait EventSink: Send + Sync + Clone + 'static {
    /// Called once per published event, with the environment it was published in
    /// (for timing/trace context) and the erased [`Event`] view. Metrics-only
    /// events (`level == "none"`) are delivered too; a sink that does not want
    /// them filters on [`Event::level`].
    fn on_event<E: Env>(&self, env: &E, event: &Event<'_>);
}

/// The single testing subscriber. It observes every event of every namespace it
/// is attached to and forwards each one to its [`EventSink`] `S`.
///
/// One `Testing<S>` value is a valid subscriber for *all* namespaces (each
/// namespace's generated code implements its `Subscriber` for this struct), so a
/// single sink can aggregate events across namespaces without any hand-written
/// fan-in. `Testing::new` builds the default [`SnapshotLog`]-backed subscriber;
/// `Testing::with_sink` accepts any sink.
#[derive(Clone)]
pub struct Testing<S = SnapshotLog> {
    sink: S,
}

impl<S: EventSink> Testing<S> {
    /// Build a testing subscriber over an arbitrary sink. The same subscriber
    /// instance can then be reused across every publish site and namespace.
    pub fn with_sink(sink: S) -> Self {
        Self { sink }
    }

    /// The underlying sink. Called by the generated subscriber methods, and
    /// available to callers that want to read back a custom sink's state.
    pub fn sink(&self) -> &S {
        &self.sink
    }
}

impl Testing<SnapshotLog> {
    /// Build a snapshot-logging testing subscriber: each event is rendered to a
    /// `key=value` line, optionally prefixed with a timestamp, and the joined log
    /// is handed to `on_drop` when the subscriber is dropped.
    pub fn new(include_timestamps: bool, on_drop: fn(String)) -> Self {
        Self::with_sink(SnapshotLog::new(include_timestamps, on_drop))
    }
}

/// One `Testing<S>` is its own subscriber for every namespace `NS`. This single
/// blanket (a local trait for a local type -- no orphan concern) is what lets the
/// per-namespace generated `impl Subscriber for Testing<S>` plug in: the crate's
/// generated `WithSubscriber` bridge only requires `Self::Subscriber:
/// <ns>::Subscriber`, which the generated impl provides.
impl<S: EventSink, NS> WithSubscriber<NS> for Testing<S> {
    type Subscriber = Testing<S>;

    fn subscriber(&self) -> &Self::Subscriber {
        self
    }
}

/// The default sink: writes each event to a `key=value` line (optionally prefixed
/// with the environment timestamp) and, when the last clone is dropped, hands the
/// newline-joined log to `on_drop`.
///
/// Metrics-only events (`level == "none"`) are skipped here (not in codegen), so a
/// different sink is free to observe them; their values are timing-dependent and
/// non-deterministic (the tracing subscriber suppresses them too).
#[derive(Clone)]
pub struct SnapshotLog {
    inner: Arc<Mutex<Log>>,
    include_timestamps: bool,
}

struct Log {
    lines: Vec<String>,
    on_drop: fn(String),
}

impl Drop for Log {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            (self.on_drop)(self.lines.join("\n"));
        }
    }
}

impl SnapshotLog {
    /// Build a snapshot sink. `include_timestamps` prepends `[<now>] ` to each
    /// line; `on_drop` receives the joined log when the sink is dropped.
    pub fn new(include_timestamps: bool, on_drop: fn(String)) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Log {
                lines: Vec::new(),
                on_drop,
            })),
            include_timestamps,
        }
    }
}

impl EventSink for SnapshotLog {
    fn on_event<E: Env>(&self, env: &E, event: &Event<'_>) {
        // Metrics-only events carry timing-dependent, non-deterministic values and
        // are suppressed here (this default sink), not in codegen -- a different
        // sink can still observe them via `Event::level`.
        if event.level() == "none" {
            return;
        }
        // `[<now>] ` timestamp prefix when requested, otherwise nothing.
        // `Event`/`Field` write straight to the buffer, so this is the only
        // allocation.
        let line = if self.include_timestamps {
            format!("[{}] {event}", env.now())
        } else {
            event.to_string()
        };
        if let Ok(mut log) = self.inner.lock() {
            log.lines.push(line);
        }
    }
}
