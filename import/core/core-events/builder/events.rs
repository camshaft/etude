// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use upstream_codegen::event;

#[event]
/// Emitted when a panic occurs in the process
#[level = "error"]
pub struct Panic<'a> {
    /// The panic message
    pub message: &'a str,
    /// The source location of the panic (file:line:col)
    pub location: &'a str,
}

// ── Task Classes ────────────────────────────────────────────────────────
//
// core-transport-rpc has no event/Publisher pipeline of its own, so its
// spawn-site task classes live here in core_events, which is composed into
// every daemon's Env. The instrumented future submits each class's samples
// directly through the generated companion publishers.
//
// Server-side RPC handler tasks are NOT classed here: they use per-service /
// per-method classes generated from the service definition (see the service
// codegen), so metrics can roll up across all RPC or drill into a service or
// method. Client write-pump tasks similarly use generated per-method classes
// where the consumer impl spawns one.
//
// core-fs-direct's I/O-pool workers are not classed here either: they spawn
// under the built-in `Parallel` class, which is what they used before this
// change too (the old `IoPool` / `SharedIoPool` classes were declared but never
// spawned, so removing them loses no metrics).

#[event(task_class)]
/// Client-side RPC connection driver tasks (core-transport-rpc).
pub struct RpcClient;
