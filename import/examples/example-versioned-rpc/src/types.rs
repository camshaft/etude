// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

/// A versioned enum with two schema versions:
/// - v1: Linearizable (tag 1.0), Eventual (tag 1.1)
/// - v2: + BoundedStaleness(u64) (tag 2.0, fallback → Eventual)
#[derive(Debug, Clone, PartialEq, upstream_core::wire::Wire)]
pub enum ConsistencyMode {
    #[wire(tag = 1.0)]
    Linearizable,
    #[wire(tag = 1.1)]
    Eventual,
    #[wire(tag = 2.0, fallback = ConsistencyMode::Eventual)]
    BoundedStaleness(u64),
}

/// A versioned struct wrapping ConsistencyMode — creates 2-level nesting.
/// - v1: only `mode` (tag 1.1)
/// - v2: `mode` + `timeout` (tag 2.1, defaults to None)
#[derive(Debug, Clone, PartialEq, upstream_core::wire::Wire)]
pub struct ConsistencyConfig {
    #[wire(tag = 1.1)]
    pub mode: ConsistencyMode,

    #[wire(tag = 2.1, default = None)]
    pub timeout: Option<u64>,
}

/// A versioned request type with 3-level nesting:
///   ReadRequest → ConsistencyConfig → ConsistencyMode
/// - v1: only `key` (tag 1.1)
/// - v2: `key` + `consistency` (tag 2.1, defaults to None)
#[derive(Debug, Clone, PartialEq, upstream_core::wire::Wire)]
pub struct ReadRequest {
    #[wire(tag = 1.1)]
    pub key: u64,

    #[wire(tag = 2.1, default = None)]
    pub consistency: Option<ConsistencyConfig>,
}

/// Response for the client-streaming `sum_reads` method. Non-versioned —
/// the test exercises versioning on the streamed *input* chunks, not the
/// response. `consistency_count` lets the test observe whether the
/// streamed `ReadRequest` chunks carried their v2 `consistency` field: it
/// will be 0 on a v1-negotiated stream (field dropped on the wire) and
/// non-zero on a v2 stream.
#[derive(Debug, Clone, PartialEq, upstream_core::wire::Wire)]
pub struct SumReadsResponse {
    pub key_sum: u64,
    pub chunk_count: u64,
    pub consistency_count: u64,
}

/// A versioned *response* type — the mirror of [`ReadRequest`] on the output
/// side. Methods returning `ReadReply` exercise response-version negotiation:
/// the server serializes the reply via `serialize_at(output_version_idx)` and
/// the client decodes it via `deserialize_at(output_version_idx)`.
///
/// - v1: only `value` (tag 1.1)
/// - v2: `value` + `metadata` (tag 2.1, defaults to None)
///
/// On a v1-negotiated call the `metadata` field is dropped on the wire, so the
/// client observes `metadata == None` even when the server populated it. This
/// is the output-side analogue of `ReadRequest` dropping `consistency` at v1.
#[derive(Debug, Clone, PartialEq, upstream_core::wire::Wire)]
pub struct ReadReply {
    #[wire(tag = 1.1)]
    pub value: u64,

    #[wire(tag = 2.1, default = None)]
    pub metadata: Option<ReplyMetadata>,
}

/// A versioned struct nested inside [`ReadReply`] — gives the output type the
/// same multi-level nesting that `ConsistencyConfig` gives the input type, so
/// the output cross-product has more than two entries.
///
/// - v1: only `created_at` (tag 1.1)
/// - v2: `created_at` + `region` (tag 2.1, defaults to None)
#[derive(Debug, Clone, PartialEq, upstream_core::wire::Wire)]
pub struct ReplyMetadata {
    #[wire(tag = 1.1)]
    pub created_at: u64,

    #[wire(tag = 2.1, default = None)]
    pub region: Option<u64>,
}
