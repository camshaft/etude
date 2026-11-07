// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use upstream_runtime::Env;

/// A span represents a period of time during which some operation occurs.
///
/// Spans are created at the start of an operation and exited when the operation completes.
/// The span implementation can track timing information and other metadata about the operation.
///
/// The type parameter T represents the result type of the operation being measured.
/// For example, a span measuring a database query might have T = Result<QueryResult>.
pub trait Span<T>: 'static + Send + Sized {
    /// Called when the operation completes with its result.
    ///
    /// The span takes a reference to the result so the caller can still use it after
    /// the span exits.
    fn exit<E: Env>(self, env: &E, result: &T) {
        let _ = env;
        let _ = result;
    }
}

impl<T> Span<T> for () {}

impl<S, T> Span<T> for Option<S>
where
    S: Span<T>,
{
    fn exit<E: Env>(self, env: &E, result: &T) {
        if let Some(span) = self {
            span.exit(env, result);
        }
    }
}

impl<A, B, T> Span<T> for (A, B)
where
    A: Span<T>,
    B: Span<T>,
{
    fn exit<E: Env>(self, env: &E, result: &T) {
        self.0.exit(env, result);
        self.1.exit(env, result);
    }
}
