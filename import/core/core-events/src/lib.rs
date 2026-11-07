// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Cross-cutting events for upstream core crates.
//!
//! This crate defines events that are shared across core crates which sit
//! below `upstream-core` in the dependency graph. Daemons compose these
//! events via `.with_events("core_events")` in their builder.

extern crate self as core_events;

// Re-exports for generated code path resolution.
// The codegen emits `core_events::runtime::Env` etc., so these must be pub.
#[doc(hidden)]
pub use upstream_metrics as metrics;
#[doc(hidden)]
pub use upstream_runtime as runtime;
#[cfg(any(test, feature = "testing"))]
#[doc(hidden)]
pub use upstream_testing as testing;
#[doc(hidden)]
pub use upstream_time as time;

// See `upstream_core`'s equivalent re-exports: generated backbeat event code
// reaches backbeat/zerocopy through its `core_crate` (here `core_events`), so no
// crate consuming this codegen needs a direct dependency on either.
//
// `backbeat` is a module (not a bare `pub use backbeat;`) so we can nest our
// `[backbeat]` daemon-config layer under it as `backbeat::config`. The glob re-export
// keeps every path the event codegen emits — `core_events::backbeat::{Event, global,
// register_event!, ..}` — resolving exactly as before.
pub mod backbeat {
    #[doc(hidden)]
    pub use ::backbeat::*;

    /// `[backbeat]` daemon-config section + wiring for the process-wide backbeat flight
    /// recorder (on-disk event traces + manual signal-triggered dump).
    pub mod config;
}
#[doc(hidden)]
pub use zerocopy::*;

#[doc(hidden)]
pub mod macro_support {
    pub use tracing;
}

mod generated;
pub use generated::*;

// Panic hook callback support
use std::sync::OnceLock;

type PanicCallback = Box<dyn Fn(&str, &str) + Send + Sync>;
static PANIC_CALLBACK: OnceLock<PanicCallback> = OnceLock::new();

/// Register a callback to be invoked from the panic hook.
///
/// Typically called once at daemon startup after the `Env` is created:
/// ```ignore
/// core_events::set_panic_callback({
///     let env = env.clone();
///     move |message, location| {
///         env.publish_core_events_panic(message, location);
///     }
/// });
/// ```
pub fn set_panic_callback(cb: impl Fn(&str, &str) + Send + Sync + 'static) {
    let _ = PANIC_CALLBACK.set(Box::new(cb));
}

/// Invoke the registered panic callback. Called from the panic hook in
/// `core-logging`. No-op if no callback has been registered.
pub fn invoke_panic_callback(message: &str, location: &str) {
    if let Some(cb) = PANIC_CALLBACK.get() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cb(message, location);
        }));
    }
}
