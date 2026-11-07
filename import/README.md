# Imported call framework

This directory holds a copy of an upstream project's call framework at its commit
`4e9bc73a7255b0794942236ab79f1a4004e83b39`. Later changes port these files into etude's own crates, and read them
from here.

The crates in this directory aren't members of the workspace, and they don't build as they stand: their manifests
are the upstream project's, which depend on crates that aren't public and on workspace settings that this copy
leaves out.

## Layout

Each file is at `import/` followed by its path in the upstream repository.

- `core/core-wire` (15 source files, 31 snapshots): the wire format, its serializer and deserializer traits, their
  implementations for standard types, varints and tagged encodings, with its tests and a benchmark.
- `core/core-macros` (15 files): the procedural macros for wire types, tagged wire types, services, events and
  variants.
- `core/core-codegen` (10 files): the code generator that the crates' and the examples' builders run, with its
  service, endpoint and event emitters and their tracing and testing sinks.
- `core/core-service` (13 files): the provider and consumer traits, with their spans, timers, call responses and
  tracing.
- `core/core-transport-rpc` (22 source files, 1 snapshot): the call runtime: the stream and transport traits in
  `src/traits.rs`, the in-memory transport, framed async I/O, the client, the server and stall detection in
  `src/service/`, and protocol version negotiation in `src/negotiation/`.
- `core/core-runtime` (9 files): the runtime environment, with its error and trace types, its subscriber, its tokio
  and bach environments, its configuration and its task monitor.
- `core/core-events` (4 files): the runtime's event definitions and the builder that generates their code.
- `core/core-sha2-const` (5 files): a SHA-2 implementation in const functions.
- `examples/example-versioned-rpc` (6 source files, 10 snapshots): an example of a versioned call service, with its
  tests.
- `examples/example-kvstore` (7 source files, 12 snapshots): an example key-value store service and its server, with
  its tests. `builder/counter.rs` is included, though `builder/main.rs` doesn't use it.

## Names

Every occurrence of the upstream project's name, in each case form, is replaced with `upstream`, `Upstream` or
`UPSTREAM`. Crates read `upstream-*`, generated paths read `amzn_upstream_*`, and environment variables read
`UPSTREAM_*`, such as `UPSTREAM_RPC_WORKER_COUNT`. The `amzn` prefix and the copyright lines stay as they are. The
replacement has as many letters as the name it replaces, so each file here has the same size in bytes as its source.

One value changes with the name: `NEGOTIATE_GLOBAL_ID` in `core/core-transport-rpc/src/negotiation/mod.rs` hashes the
string `__upstream_negotiate__`, so it doesn't equal the upstream project's id.

## Files left out

- `core/core-codegen`: `src/http.rs`, `src/http/`, `src/events/metrics` and `src/events/backbeat`, the HTTP, metrics
  and backbeat sinks. `src/lib.rs` still declares `pub mod http`, and `src/builder.rs` refers to it.
- `core/core-service`: `src/tracing/http.rs`.
- `core/core-transport-rpc`: the `config`, `dcquic`, `mux`, `mux_bach`, `quic` and `tcp` modules, the network
  transports and their configuration. `src/lib.rs` still declares them and re-exports `config::Config`. The port
  keeps the stream traits and the in-memory transport.
- `core/core-runtime`: the `backoff`, `byte_pool`, `deadline_queue`, `inflight_byte_budget`, `memory`, `probe` and
  `rate_limiter` modules, which aren't part of the call framework this copy holds. `src/config.rs` and
  `src/task_monitor.rs` are included: `src/tokio.rs` reads `crate::config`, and the client in
  `core-transport-rpc` and the `#[event(task_class)]` attribute use the task monitor. The task monitor's `TaskClass`
  trait is defined in the upstream metrics crate, which this copy leaves out.
- `core/core-events`: `src/backbeat/`. `src/lib.rs` still declares `pub mod backbeat`.
- The upstream workspace's root `Cargo.toml`, which holds settings for the upstream project's own build.

## Upstream crates that aren't public

The manifests depend on these upstream crates, which this copy doesn't include:

- `upstream-buffer` and `upstream-bytevec`, which the port replaces with `etude-buffer` and `etude-bytevec`;
- `upstream-metrics`, `upstream-time`, `upstream-debug`, `upstream-bach-net`, `upstream-config`, `upstream-iter` and
  `upstream-testing`;
- `upstream-aws-http`, behind the transport's KMS authentication feature;
- `upstream-core`, the crate that the examples import the framework through.

## Public dependencies

The manifests depend on these public crates, at the versions the upstream workspace sets:

- `bach` 0.1.1
- `bolero` 0.13 and `bolero-generator` 0.13
- `bytes` 1.5, with the `serde` feature
- `criterion` 0.5
- `dashmap` 5
- `futures` 0.3, `futures-core` 0.3 and `futures-util` 0.3
- `heck` 0.4
- `insta` 1.46, with the `filters` feature
- `linkme` 0.3
- `pin-project-lite` 0.2
- `prettyplease` 0.2
- `proc-macro2` 1.0
- `quote` 1.0
- `rand` 0.9
- `s2n-quic-core` 0.76.0
- `sort-const` 1.0
- `syn` 2.0, with the `full` feature
- `tokio` 1 and `tokio-util` 0.7
- `tracing` 0.1 and `tracing-subscriber` 0.3
