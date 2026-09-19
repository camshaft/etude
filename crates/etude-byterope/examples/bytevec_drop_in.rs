// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Drop-in validation: this file is written as an `etude_bytevec` **consumer**, but imports
//! everything from `etude_byterope`. It exercises the bytevec public surface — the `ByteVec` /
//! `ByteVecError` type names, the `Builder`, the `Tag`/`Tagged`/`static_bytevec_tag!` machinery,
//! the core ops, the iterator/reader types, and the conversion + equality impls — entirely through
//! the compat aliases. That it compiles is the proof that a bytevec caller can swap only the Cargo
//! dependency (`etude_bytevec` -> `etude_byterope`) with zero source changes (operator ruling
//! 2026-09-19: a true superset drop-in). Running it (`cargo run --example bytevec_drop_in`) also
//! checks the behavior matches.
//!
//! NOTE: the only name written here that is byterope-specific is the crate name in `use` paths — a
//! real consumer would keep writing `etude_bytevec::…`; swapping the dep makes those resolve here.

// The bytevec surface, all under the `etude_byterope` crate via the compat aliases:
use etude_byterope::{Builder, ByteVec, ByteVecError, Bytes, BytesMut};
// The write path is a bytes-buffer trait, exactly as with bytevec's Builder.
use etude_buffer::writer::Buffer as _;

// A static byte-budget tag, declared exactly as a bytevec consumer would.
mod conn {
    etude_byterope::static_bytevec_tag!();
}

fn main() {
    core_ops();
    builder_surface();
    conversions_and_equality();
    iterators_and_reader();
    error_surface();
    tagging();
    println!(
        "bytevec drop-in: OK — the full etude_bytevec surface resolved against etude_byterope"
    );
}

fn core_ops() {
    let mut v = ByteVec::new();
    assert!(v.is_empty());
    v.push_back(Bytes::from_static(b"world"));
    v.push_front(Bytes::from_static(b"hello "));
    assert_eq!(v.len(), 11);
    assert_eq!(v, b"hello world");

    let front = v.split_to(6).expect("split within bounds");
    assert_eq!(front, b"hello ");
    assert_eq!(v, b"world");

    let copied = v.split_to_copy(3).expect("split_to_copy within bounds");
    assert_eq!(&copied[..], b"wor");
    assert_eq!(v, b"ld");

    v.truncate(1);
    assert_eq!(v, b"l");

    let popped = v.pop_front();
    assert_eq!(popped.as_deref(), Some(&b"l"[..]));
    assert!(v.pop_back().is_none());

    let mut a = ByteVec::from(b"ab");
    let mut b = ByteVec::from(b"cd");
    a.append(&mut b);
    assert_eq!(a, b"abcd");
    assert!(b.is_empty());
    assert_eq!(a.get(0).map(|c| &c[..]), Some(&b"ab"[..]));
    assert_eq!(a.copy_to_bytes(), Bytes::from_static(b"abcd"));

    let mut adv = ByteVec::from(b"streaming");
    adv.advance(6).expect("advance within bounds");
    assert_eq!(adv, b"ing");
}

fn builder_surface() {
    let mut builder: Builder = ByteVec::builder(1024).with_inline_threshold(8);
    assert_eq!(builder.inline_threshold(), 8);
    builder.put_slice(b"hello");
    builder.put_bytes(Bytes::from_static(b" "));
    builder.put_bytes_mut(BytesMut::from(&b"world"[..]));
    assert_eq!(builder.len(), 11);
    assert!(!builder.is_empty());

    builder.write_with_len_prefix(|w| w.put_slice(b"!!"));
    let out: ByteVec = builder.finish();
    // "hello world" ++ (u64 be = 2) ++ "!!"
    let mut expected = Vec::from(&b"hello world"[..]);
    expected.extend_from_slice(&2u64.to_be_bytes());
    expected.extend_from_slice(b"!!");
    assert_eq!(&out.copy_to_bytes()[..], &expected[..]);

    // Builder <-> ByteVec conversions.
    let seeded = Builder::from(ByteVec::from(b"seed"));
    let back: ByteVec = seeded.into();
    assert_eq!(back, b"seed");
}

fn conversions_and_equality() {
    assert_eq!(ByteVec::from(b"lit".as_slice()), b"lit");
    assert_eq!(ByteVec::from(vec![1u8, 2, 3]), [1u8, 2, 3]);
    assert_eq!(ByteVec::from(String::from("str")), "str");
    assert_eq!(ByteVec::from("static"), b"static");

    let v = ByteVec::from(b"abc");
    // the ByteVec PartialEq family a bytevec caller relies on
    assert_eq!(v, b"abc"); // &[u8; N]
    assert_eq!(v, b"abc"[..]); // [u8]
    assert_eq!(v, "abc"); // str
    assert_eq!(v, Bytes::from_static(b"abc")); // Bytes
    assert_eq!(v, [Bytes::from_static(b"a"), Bytes::from_static(b"bc")]); // [Bytes; N]
}

fn iterators_and_reader() {
    use etude_byterope::{ChunkIter, DrainIter, Reader};

    let v: ByteVec = [Bytes::from_static(b"ab"), Bytes::from_static(b"cd")]
        .into_iter()
        .collect();

    let it: ChunkIter<'_> = v.chunks();
    assert_eq!(it.len(), 2);
    assert_eq!(v.chunks().flatten().copied().collect::<Vec<u8>>(), b"abcd");

    let r: Reader<'_> = v.reader();
    assert_eq!(r.len(), 4);
    assert!(!r.is_empty());

    let drained: DrainIter = v.into_iter();
    assert_eq!(drained.count(), 2);
}

fn error_surface() {
    let mut v = ByteVec::from(b"xy");
    let err: ByteVecError = v.split_to(99).unwrap_err();
    // Exhaustive match over the bytevec error variants — both must exist for this to compile.
    let msg = match err {
        ByteVecError::OutOfBounds(at) => format!("oob {at}"),
        ByteVecError::OutOfBoundsRange(s, e) => format!("oob-range {s}..{e}"),
    };
    assert_eq!(msg, "oob 99");
    // Converts into std::io::Error, like bytevec's.
    let io_err: std::io::Error = ByteVecError::OutOfBounds(1).into();
    assert_eq!(io_err.kind(), std::io::ErrorKind::UnexpectedEof);
}

fn tagging() {
    // `conn::ByteVec` is the tagged buffer type the macro declares (== Tagged<conn::Tag>).
    let bytes = ByteVec::from(b"payload");
    let tagged: conn::ByteVec = bytes.tag(&conn::Tag);
    assert_eq!(conn::Tag::current(), 7);
    let mut tagged = tagged;
    tagged.push_back(Bytes::from_static(b"!!"));
    assert_eq!(conn::Tag::current(), 9);
    let inner: ByteVec = tagged.untag();
    assert_eq!(inner, b"payload!!");
}
