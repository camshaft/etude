// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use upstream_codegen::service;

/// A service with versioned input types demonstrating multi-level nesting.
///
/// Type hierarchy (3-level nesting):
///   ReadRequest → ConsistencyConfig → ConsistencyMode
///
/// Each versioned type has 2 schema versions, so the codegen iterates
/// `Wire::VERSION_TYPES` to produce a `MethodVersionEntry` per version,
/// each with a distinct `global_id` and `input_version_idx`.
#[service(versioned)]
extern "RPC" {
    /// Read a value by key, with optional nested consistency config.
    /// ReadRequest has 2 schema versions (v1: key only, v2: key + consistency).
    fn read(request: crate::types::ReadRequest) -> Option<u64>;

    /// A non-versioned method for comparison — single version entry.
    fn ping(message: u64) -> u64;

    /// A method with a versioned enum input.
    /// ConsistencyMode has 2 versions — the codegen should produce 2 version entries.
    fn set_consistency(mode: crate::types::ConsistencyMode) -> bool;

    /// A method with a 2-level nested versioned struct input.
    /// ConsistencyConfig has 2 versions — tests the intermediate nesting level.
    fn configure(config: crate::types::ConsistencyConfig) -> bool;

    /// Client-streaming method with a VERSIONED input chunk type.
    ///
    /// `ReadRequest` has 2 schema versions, so this exercises the
    /// streaming framing layer's version dispatch end-to-end:
    /// `send_stream_request` (client) serializes each chunk at the
    /// negotiated `input_version_idx`, and the server's `read_stream`
    /// deserializes each chunk at the same version. Returns the sum of
    /// the `key`s plus a count of chunks that carried a `consistency`
    /// field — so a v1-negotiated stream (consistency dropped) yields a
    /// different result than a v2 stream.
    #[request(stream)]
    fn sum_reads(chunks: crate::types::ReadRequest) -> crate::types::SumReadsResponse;

    /// Unary method with a VERSIONED RESPONSE type and a non-versioned input.
    ///
    /// The `u64` input is non-versioned, so the only thing the negotiated
    /// version changes is how the *response* is encoded. `ReadReply` has 2
    /// schema versions, so this exercises response-version negotiation
    /// end-to-end: the server serializes the reply at the negotiated
    /// `output_version_idx` (`write` → `serialize_at`) and the client decodes
    /// it at the same index (`send` → `deserialize_at`). On a v1-negotiated
    /// call the `metadata` field is dropped on the wire.
    fn read_reply(key: u64) -> crate::types::ReadReply;

    /// Server-streaming method with a VERSIONED RESPONSE chunk type.
    ///
    /// Streams `count` `ReadReply` items back to the client. Exercises the
    /// streaming-response version path: the server's `write_stream` frames
    /// each item via `write_framed_stream_at(output_version_idx)` and the
    /// client's `send_stream_response` decodes via
    /// `read_framed_stream_at(output_version_idx)`. A v1-negotiated stream
    /// must drop `metadata` on every frame without desyncing across frames.
    #[response(stream, header = u64)]
    fn scan_replies(count: u64) -> crate::types::ReadReply;

    /// Unary method with a RESPONSE HEADER and a VERSIONED RESPONSE body.
    ///
    /// The non-versioned `u64` input and non-versioned `u64` header isolate
    /// output-body versioning: this is the only shape that exercises the
    /// client's `deserialize_unary_with_header_at` split (tag → header at the
    /// current layout → body at `output_version_idx`) and the server's
    /// mirrored `write_with_header` split. On a v1-negotiated call the body's
    /// `metadata` field is dropped while the header round-trips unchanged.
    #[response(header = u64)]
    fn read_reply_hdr(key: u64) -> crate::types::ReadReply;

    /// Client-streaming method with a unary VERSIONED RESPONSE.
    ///
    /// Streams `u64` keys to the server, which folds them into a single
    /// `ReadReply`. Exercises `send_stream_request`'s response decode at the
    /// negotiated `output_version_idx` — the request stream is non-versioned
    /// (`u64`), so only the response body honors the version.
    #[request(stream)]
    fn fold_replies(keys: u64) -> crate::types::ReadReply;

    /// Bidirectional method with a VERSIONED RESPONSE stream.
    ///
    /// Streams `u64` keys in and a `ReadReply` per key back out. Exercises
    /// `send_stream_bidi`'s response read via
    /// `read_framed_stream_at(output_version_idx)`. A v1-negotiated stream
    /// drops `metadata` on every response frame without desyncing.
    #[request(stream)]
    #[response(stream)]
    fn echo_replies(keys: u64) -> crate::types::ReadReply;
}
