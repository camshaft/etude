// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! `Sha256` against published test vectors and the `sha2` crate.

use core::fmt::Write as _;
use etude_sha2_const::Sha256;
use sha2::Digest as _;

/// A published SHA-256 test vector.
struct Vector {
    input: &'static str,
    /// The digest of `input`, in hex.
    digest: &'static str,
}

const VECTORS: &[Vector] = &[
    Vector {
        input: "",
        digest: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    },
    Vector {
        input: "abc",
        digest: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    },
    Vector {
        input: "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
        digest: "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
    },
    Vector {
        input: concat!(
            "abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmn",
            "hijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"
        ),
        digest: "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1",
    },
    Vector {
        input: "The quick brown fox jumps over the lazy dog",
        digest: "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592",
    },
];

const ABC: [u8; 32] = Sha256::new().update(b"abc").finalize();
const ABC_ID: u64 = Sha256::new().update(b"abc").finalize_u64();

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(out, "{byte:02x}").unwrap();
    }
    out
}

fn digest(input: &[u8]) -> [u8; 32] {
    Sha256::new().update(input).finalize()
}

/// The `sha2` crate's digest of `input`.
fn reference(input: &[u8]) -> Vec<u8> {
    sha2::Sha256::digest(input).to_vec()
}

/// The digest of `input` passed to `update` in parts: each of `splits` sets
/// the length of the next part, modulo the bytes left plus one.
fn split_digest(input: &[u8], splits: &[usize]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    let mut rest = input;
    for split in splits {
        let (part, tail) = rest.split_at(split % (rest.len() + 1));
        hasher = hasher.update(part);
        rest = tail;
    }
    hasher.update(rest).finalize()
}

#[test]
fn published_vectors() {
    for vector in VECTORS {
        let text = vector.input;
        let input = text.as_bytes();
        let bytes = digest(input);
        assert_eq!(hex(&bytes), vector.digest, "input {text:?}");
        assert_eq!(
            bytes.as_slice(),
            reference(input).as_slice(),
            "input {text:?}"
        );
    }
}

#[test]
fn one_million_a() {
    let chunk = [b'a'; 1000];
    let mut hasher = Sha256::new();
    for _ in 0..1000 {
        hasher = hasher.update(&chunk);
    }
    assert_eq!(
        hex(&hasher.finalize()),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}

#[test]
fn a_const_digest_matches_the_runtime_digest() {
    assert_eq!(ABC, digest(b"abc"));
    assert_eq!(
        hex(&ABC),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn finalize_u64_is_the_first_eight_bytes_big_endian() {
    assert_eq!(ABC_ID, 0xba78_16bf_8f01_cfea);
    assert_eq!(ABC_ID.to_be_bytes(), ABC[..8]);
    for vector in VECTORS {
        let text = vector.input;
        let input = text.as_bytes();
        let id = Sha256::new().update(input).finalize_u64();
        assert_eq!(id.to_be_bytes(), digest(input)[..8], "input {text:?}");
    }
}

#[test]
fn block_and_digest_sizes() {
    assert_eq!(Sha256::BLOCK_SIZE, 64);
    assert_eq!(Sha256::DIGEST_SIZE, 32);
}

#[test]
fn a_clone_goes_on_from_the_state_it_was_cloned_in() {
    let prefix = Sha256::new().update(b"The quick brown fox ");
    let fox = prefix.clone().update(b"jumps over the lazy dog");
    let naps = prefix.update(b"naps");
    let fox_text = b"The quick brown fox jumps over the lazy dog";
    assert_eq!(fox.finalize(), digest(fox_text));
    assert_eq!(naps.finalize(), digest(b"The quick brown fox naps"));
}

#[test]
fn every_split_point_matches_the_one_shot_digest() {
    let input: Vec<u8> = (0..=u8::MAX).cycle().take(200).collect();
    let expected = digest(&input);
    for at in 0..=input.len() {
        let (head, tail) = input.split_at(at);
        let split = Sha256::new().update(head).update(tail).finalize();
        assert_eq!(split, expected, "split at {at}");
    }
}

#[test]
fn every_length_up_to_300_matches_the_reference() {
    let input: Vec<u8> = (0..300u32).map(|i| (i * 7 + 3) as u8).collect();
    for len in 0..=input.len() {
        let bytes = &input[..len];
        assert_eq!(
            digest(bytes).as_slice(),
            reference(bytes).as_slice(),
            "length {len}"
        );
    }
}

#[test]
fn random_inputs_and_splits_match_the_reference() {
    bolero::check!()
        .with_type::<(Vec<u8>, Vec<usize>)>()
        .for_each(|(input, splits)| {
            assert_eq!(
                split_digest(input, splits).as_slice(),
                reference(input).as_slice()
            );
        });
}
