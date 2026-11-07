// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! The ids of methods of the call framework this crate is ported for.
//!
//! The framework identifies each RPC method by a 64-bit id: the first 8 bytes,
//! big endian, of the SHA-256 of the service's name, the method's name, the
//! layouts of its types and headers, and its flags. The ids below are the ones
//! that the framework's example key-value service records in its
//! wire-compatibility snapshots, on this repository's `import/call-framework`
//! branch, in
//!
//! `import/examples/example-kvstore/src/snapshots/service/kvstore/`
//!
//! so each test passes only where this crate's digests match the ones those ids
//! were made with.

use etude_sha2_const::Sha256;

/// The `stream_response` bit of a method's flags.
const STREAM_RESPONSE: u8 = 1 << 1;

/// The framework computes each id in a `const` item; this computes `ping`'s id
/// the same way, from its layout written out by hand.
const PING_ID: u64 = {
    const BYTES: [u8; 6] = [b'[', 1, b'1', 1, 0, 0];
    Sha256::new()
        .update(b"kvstore\0ping\0")
        .update(&BYTES)
        .update(&[255])
        .update(&BYTES)
        .update(&[255, 255, 255, 0])
        .finalize_u64()
};

/// The layout the framework hashes for a type: its id, `1`, the layout of each
/// child, then `0`.
fn layout(id: u8, children: &[&[u8]]) -> Vec<u8> {
    let mut out = vec![id, 1];
    for child in children {
        out.extend_from_slice(child);
    }
    out.push(0);
    out
}

/// The layout of `u8`.
fn u8_type() -> Vec<u8> {
    layout(b'1', &[])
}

/// The layout of `usize`.
fn usize_type() -> Vec<u8> {
    layout(b'6', &[])
}

/// The layout of `Bytes`, which the framework hashes as a list of `u8`.
fn bytes_type() -> Vec<u8> {
    layout(b'[', &[u8_type().as_slice()])
}

/// The layout of `Option<T>`: an enum whose variant 0, `None`, has no fields
/// and whose variant 1, `Some`, holds `T`.
fn option_type(inner: &[u8]) -> Vec<u8> {
    let none = layout(0, &[]);
    let some = layout(1, &[inner]);
    layout(b'E', &[none.as_slice(), some.as_slice()])
}

/// A method's id: the first 8 bytes, big endian, of the SHA-256 of the
/// service's name and the method's name, each followed by `0`, then the
/// layouts of the input, the output, the request header and the response
/// header, each followed by `255` (a missing header adds `255` alone), then
/// the flags.
fn method_id(
    service: &str,
    method: &str,
    input: &[u8],
    output: &[u8],
    request_header: Option<&[u8]>,
    response_header: Option<&[u8]>,
    flags: u8,
) -> u64 {
    let parts = [
        input,
        output,
        request_header.unwrap_or_default(),
        response_header.unwrap_or_default(),
    ];
    let mut buffer = Vec::new();
    for part in parts {
        buffer.extend_from_slice(part);
        buffer.push(255);
    }
    buffer.push(flags);
    Sha256::new()
        .update(service.as_bytes())
        .update(&[0])
        .update(method.as_bytes())
        .update(&[0])
        .update(&buffer)
        .finalize_u64()
}

#[test]
fn ping_id() {
    // fn ping(message: Bytes) -> Bytes
    let bytes = bytes_type();
    let id = method_id("kvstore", "ping", &bytes, &bytes, None, None, 0);
    assert_eq!(id, 16376824459338887047);
    assert_eq!(PING_ID, id);
}

#[test]
fn get_id() {
    // fn get(key: Bytes) -> Option<Bytes>
    let bytes = bytes_type();
    let option = option_type(&bytes);
    let id = method_id("kvstore", "get", &bytes, &option, None, None, 0);
    assert_eq!(id, 11822515389179596379);
}

#[test]
fn keys_id() {
    // #[response(stream, header = usize)]
    // fn keys() -> Bytes
    let bytes = bytes_type();
    let header = usize_type();
    let id = method_id(
        "kvstore",
        "keys",
        &[],
        &bytes,
        None,
        Some(header.as_slice()),
        STREAM_RESPONSE,
    );
    assert_eq!(id, 2350028936010563745);
}
