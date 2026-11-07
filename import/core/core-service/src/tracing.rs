// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use std::time::Duration;

use upstream_runtime::{Env, Result};
use upstream_time::Instant;
use tracing::{Level, Span, event};

#[cfg(any(feature = "http", test))]
pub mod http;

#[cfg(any(feature = "http", test))]
pub use http::HttpSpan;

pub mod consumer;
pub mod provider;

struct SerializeSpan {
    span: Span,
    start: Option<Instant>,
}

impl crate::Span<usize> for SerializeSpan {
    fn exit<E: Env>(self, env: &E, size: &usize) {
        if let Some(start) = self.start {
            let elapsed = env.now().duration_since(start);
            event!(parent: self.span.clone(), Level::TRACE, { ?size, ?elapsed }, "serialized");
        } else {
            event!(parent: self.span.clone(), Level::TRACE, { ?size }, "serialized");
        }
    }
}

struct DeserializeSpan {
    span: Span,
    size: usize,
    start: Option<Instant>,
}

impl<Res> crate::Span<Result<Res>> for DeserializeSpan {
    fn exit<E: Env>(self, env: &E, result: &Result<Res>) {
        let size = self.size;
        let elapsed = self
            .start
            .map_or(Duration::ZERO, |start| env.now().duration_since(start));
        match result {
            Ok(_) => {
                event!(parent: self.span.clone(), Level::TRACE, { ?size, ?elapsed }, "deserialized")
            }
            Err(error) => {
                event!(parent: self.span.clone(), Level::ERROR, { ?size, ?elapsed, %error }, "deserialized")
            }
        }
    }
}

struct ResultSpan {
    span: Span,
    start: Option<Instant>,
    label: &'static str,
}

impl<T> crate::Span<Result<T>> for ResultSpan {
    fn exit<E: Env>(self, env: &E, result: &Result<T>) {
        let elapsed = self
            .start
            .map_or(Duration::ZERO, |start| env.now().duration_since(start));
        match result {
            Ok(_) => {
                event!(parent: self.span.clone(), Level::TRACE, { ?elapsed }, "{}", self.label)
            }
            Err(error) => {
                event!(parent: self.span.clone(), Level::INFO, { ?elapsed, %error }, "{}", self.label)
            }
        }
    }
}
