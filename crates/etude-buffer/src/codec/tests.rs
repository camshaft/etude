// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::reader::{Chain, Chunk, Infallible as _, IoSlice};

/// One field of a record, as a test writes and reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Field {
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    Bool(bool),
    Bytes(Vec<u8>),
    Text(String),
    Digest([u8; 32]),
    Raw(Vec<u8>),
    /// A count, then each item as a `u16`.
    List(Vec<u16>),
}

impl Field {
    /// Writes the field with the writes under test.
    fn encode<W>(&self, out: &mut W) -> Result<(), EncodeError>
    where
        W: writer::Buffer + ?Sized,
    {
        match self {
            Self::U8(value) => out.write_u8(*value),
            Self::U16(value) => out.write_u16(*value),
            Self::U32(value) => out.write_u32(*value),
            Self::U64(value) => out.write_u64(*value),
            Self::Bool(value) => out.write_bool(*value),
            Self::Bytes(bytes) => out.write_bytes(bytes),
            Self::Text(text) => out.write_text(text),
            Self::Digest(digest) => out.write_raw(digest),
            Self::Raw(bytes) => out.write_raw(bytes),
            Self::List(items) => {
                out.write_count(items.len())?;
                for item in items {
                    out.write_u16(*item)?;
                }
                Ok(())
            }
        }
    }

    /// Writes the field with the `bytes` crate's `BufMut`, the reference encoding.
    fn reference(&self, out: &mut Vec<u8>) {
        use bytes::BufMut;

        match self {
            Self::U8(value) => out.put_u8(*value),
            Self::U16(value) => out.put_u16_le(*value),
            Self::U32(value) => out.put_u32_le(*value),
            Self::U64(value) => out.put_u64_le(*value),
            Self::Bool(value) => out.put_u8(u8::from(*value)),
            Self::Bytes(bytes) => {
                out.put_u64_le(bytes.len() as u64);
                out.put_slice(bytes);
            }
            Self::Text(text) => {
                out.put_u64_le(text.len() as u64);
                out.put_slice(text.as_bytes());
            }
            Self::Digest(digest) => out.put_slice(digest),
            Self::Raw(bytes) => out.put_slice(bytes),
            Self::List(items) => {
                out.put_u64_le(items.len() as u64);
                for item in items {
                    out.put_u16_le(*item);
                }
            }
        }
    }

    /// Reads a field of this one's kind with the reads under test.
    fn decode<R>(&self, input: &mut R) -> Result<Self, DecodeError<R::Error>>
    where
        R: reader::Buffer + ?Sized,
    {
        Ok(match self {
            Self::U8(_) => Self::U8(input.read_u8()?),
            Self::U16(_) => Self::U16(input.read_u16()?),
            Self::U32(_) => Self::U32(input.read_u32()?),
            Self::U64(_) => Self::U64(input.read_u64()?),
            Self::Bool(_) => Self::Bool(input.read_bool()?),
            Self::Bytes(_) => Self::Bytes(input.read_bytes()?),
            Self::Text(_) => Self::Text(input.read_text()?),
            Self::Digest(_) => Self::Digest(input.read_array()?),
            Self::Raw(bytes) => {
                let mut raw = vec![0; bytes.len()];
                input.read_raw(&mut raw)?;
                Self::Raw(raw)
            }
            Self::List(_) => {
                let count = input.read_count(2)?;
                let mut items = Vec::new();
                for _ in 0..count {
                    items.push(input.read_u16()?);
                }
                Self::List(items)
            }
        })
    }
}

/// Writes `fields` with the writes under test.
fn encode<W>(fields: &[Field], out: &mut W) -> Result<(), EncodeError>
where
    W: writer::Buffer + ?Sized,
{
    for field in fields {
        field.encode(out)?;
    }
    Ok(())
}

/// The reference encoding of `fields`.
fn reference(fields: &[Field]) -> Vec<u8> {
    let mut out = Vec::new();
    for field in fields {
        field.reference(&mut out);
    }
    out
}

/// Reads a record of `fields`' kinds with the reads under test, and checks its end.
fn decode<R>(fields: &[Field], input: &mut R) -> Result<Vec<Field>, DecodeError<R::Error>>
where
    R: reader::Buffer + ?Sized,
{
    let mut decoded = Vec::new();
    for field in fields {
        decoded.push(field.decode(input)?);
    }
    input.check_end()?;
    Ok(decoded)
}

/// One field of each kind, with an empty one of each kind whose length varies.
fn sample() -> Vec<Field> {
    vec![
        Field::U8(0xA5),
        Field::U16(0xBEEF),
        Field::U32(0xDEAD_BEEF),
        Field::U64(0x0123_4567_89AB_CDEF),
        Field::Bool(true),
        Field::Bool(false),
        Field::Bytes(b"payload".to_vec()),
        Field::Bytes(vec![]),
        Field::Text("h\u{e9}llo".into()),
        Field::Text(String::new()),
        Field::Digest([7; 32]),
        Field::Raw(vec![1, 2, 3]),
        Field::List(vec![1, 0x0203, 0xFFFF]),
        Field::List(vec![]),
    ]
}

/// A field of the kind `kind` picks, made from `value` and `bytes`.
fn field(kind: u8, value: u64, bytes: &[u8]) -> Field {
    match kind % 10 {
        0 => Field::U8(value as u8),
        1 => Field::U16(value as u16),
        2 => Field::U32(value as u32),
        3 => Field::U64(value),
        4 => Field::Bool(value % 2 == 1),
        5 => Field::Bytes(bytes.to_vec()),
        6 => Field::Text(String::from_utf8_lossy(bytes).into_owned()),
        7 => {
            let mut digest = [0; 32];
            let len = bytes.len().min(32);
            digest[..len].copy_from_slice(&bytes[..len]);
            Field::Digest(digest)
        }
        8 => Field::Raw(bytes.to_vec()),
        _ => {
            let low = value as u8;
            let items = bytes.iter().map(|&high| u16::from_le_bytes([low, high]));
            Field::List(items.collect())
        }
    }
}

/// Cuts `bytes` into pieces of the lengths `cuts` lists, and the rest as the last piece.
fn split<'a>(bytes: &'a [u8], cuts: &[u8]) -> Vec<&'a [u8]> {
    let mut parts = Vec::new();
    let mut rest = bytes;
    for cut in cuts {
        let (part, tail) = rest.split_at(usize::from(*cut).min(rest.len()));
        parts.push(part);
        rest = tail;
    }
    parts.push(rest);
    parts
}

/// Reads a count of items of `min_item_len` bytes from `record`.
fn count_of(record: &[u8], min_item_len: usize) -> Result<usize, DecodeError> {
    let mut reader = record;
    reader.read_count(min_item_len)
}

/// A count of `count`, then `rest` zero bytes.
fn count_then(count: u64, rest: usize) -> Vec<u8> {
    let mut record = count.to_le_bytes().to_vec();
    record.resize(record.len() + rest, 0);
    record
}

/// A reader over `bytes` that serves `reads` reads, and fails each read after them.
struct FailAfter<'a> {
    bytes: &'a [u8],
    reads: usize,
}

impl reader::Buffer for FailAfter<'_> {
    type Error = &'static str;

    fn buffered_len(&self) -> usize {
        self.bytes.len()
    }

    fn read_chunk(&mut self, watermark: usize) -> Result<Chunk<'_>, Self::Error> {
        if self.reads == 0 {
            return Err("boom");
        }
        self.reads -= 1;
        Ok(self.bytes.infallible_read_chunk(watermark))
    }

    fn partial_copy_into<Dest>(&mut self, dest: &mut Dest) -> Result<Chunk<'_>, Self::Error>
    where
        Dest: writer::Buffer + ?Sized,
    {
        if self.reads == 0 {
            return Err("boom");
        }
        self.reads -= 1;
        Ok(self.bytes.infallible_partial_copy_into(dest))
    }
}

/// A reader that reports `extra` more buffered bytes than `bytes` holds.
struct Overstated<'a> {
    bytes: &'a [u8],
    extra: usize,
}

impl reader::Buffer for Overstated<'_> {
    type Error = core::convert::Infallible;

    fn buffered_len(&self) -> usize {
        self.bytes.len() + self.extra
    }

    fn read_chunk(&mut self, watermark: usize) -> Result<Chunk<'_>, Self::Error> {
        self.bytes.read_chunk(watermark)
    }

    fn partial_copy_into<Dest>(&mut self, dest: &mut Dest) -> Result<Chunk<'_>, Self::Error>
    where
        Dest: writer::Buffer + ?Sized,
    {
        self.bytes.partial_copy_into(dest)
    }
}

#[test]
fn writes_match_the_reference() {
    let fields = sample();
    let mut encoded: Vec<u8> = Vec::new();
    encode(&fields, &mut encoded).unwrap();
    assert_eq!(encoded, reference(&fields));
}

#[test]
fn each_field_round_trips_through_a_slice_and_a_chain_split_anywhere() {
    for field in sample() {
        let fields = [field];
        let bytes = reference(&fields);

        let mut slice = &bytes[..];
        assert_eq!(decode(&fields, &mut slice), Ok(fields.to_vec()));

        for at in 0..=bytes.len() {
            let (a, b) = bytes.split_at(at);
            let mut chain = Chain::new(a, b);
            assert_eq!(
                decode(&fields, &mut chain),
                Ok(fields.to_vec()),
                "split at {at}"
            );
        }
    }
}

#[test]
fn a_record_round_trips_split_anywhere_and_in_one_byte_chunks() {
    let fields = sample();
    let bytes = reference(&fields);

    for at in 0..=bytes.len() {
        let (a, b) = bytes.split_at(at);
        let mut chain = Chain::new(a, b);
        assert_eq!(
            decode(&fields, &mut chain),
            Ok(fields.clone()),
            "split at {at}"
        );
    }

    let parts: Vec<&[u8]> = bytes.chunks(1).collect();
    let mut reader = IoSlice::new(&parts);
    assert_eq!(decode(&fields, &mut reader), Ok(fields));
}

#[test]
fn every_truncation_of_a_record_is_refused() {
    let fields = sample();
    let bytes = reference(&fields);
    for len in 0..bytes.len() {
        let mut reader = &bytes[..len];
        let error = decode(&fields, &mut reader).unwrap_err();
        let short = matches!(
            error,
            DecodeError::Truncated { .. }
                | DecodeError::LengthExceedsRemaining { .. }
                | DecodeError::CountExceedsRemaining { .. }
        );
        assert!(short, "truncated to {len} bytes: {error:?}");
    }
}

#[test]
fn a_field_short_of_bytes_is_refused_without_reading_them() {
    let mut reader: &[u8] = &[1, 2, 3];
    let short = DecodeError::Truncated {
        needed: 4,
        remaining: 3,
    };
    assert_eq!(reader.read_u32(), Err(short));
    assert_eq!(reader.len(), 3);
    assert_eq!(reader.read_u16(), Ok(0x0201));
}

#[test]
fn bytes_left_after_a_record_are_refused() {
    let fields = sample();
    let mut bytes = reference(&fields);
    bytes.push(0);
    let mut reader = &bytes[..];
    let trailing = DecodeError::TrailingBytes { remaining: 1 };
    assert_eq!(decode(&fields, &mut reader), Err(trailing));
}

#[test]
fn a_bool_other_than_0_or_1_is_refused() {
    for byte in 0..=u8::MAX {
        let bytes = [byte];
        let mut reader = &bytes[..];
        let expected = match byte {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError::InvalidBool(byte)),
        };
        assert_eq!(reader.read_bool(), expected);
    }
}

#[test]
fn invalid_text_is_refused() {
    // 0xC3 starts a 2-byte sequence, and 0x28 doesn't continue one.
    let mut record: Vec<u8> = Vec::new();
    record.write_bytes(&[0xC3, 0x28]).unwrap();
    let mut reader = &record[..];
    let error = reader.read_text().unwrap_err();
    let DecodeError::InvalidText(utf8) = error else {
        panic!("expected invalid text, read {error:?}");
    };
    assert_eq!(utf8.valid_up_to(), 0);
    assert_eq!(utf8.error_len(), Some(1));
}

#[test]
fn a_length_past_the_remaining_bytes_is_refused() {
    let mut record: Vec<u8> = Vec::new();
    record.write_u64(4).unwrap();
    record.write_raw(b"abc").unwrap();

    let mut reader = &record[..];
    let too_long = DecodeError::LengthExceedsRemaining {
        len: 4,
        remaining: 3,
    };
    assert_eq!(reader.read_bytes(), Err(too_long));
    assert_eq!(reader.len(), 3);

    let mut exact: Vec<u8> = Vec::new();
    exact.write_bytes(b"abc").unwrap();
    let mut reader = &exact[..];
    assert_eq!(reader.read_bytes(), Ok(b"abc".to_vec()));
}

#[test]
fn a_claimed_length_or_count_of_u64_max_is_refused() {
    let claim = u64::MAX.to_le_bytes();

    let too_long = DecodeError::LengthExceedsRemaining {
        len: u64::MAX,
        remaining: 0,
    };
    let mut reader = &claim[..];
    assert_eq!(reader.read_bytes(), Err(too_long));
    let mut reader = &claim[..];
    assert_eq!(reader.read_text(), Err(too_long));

    let too_many = DecodeError::CountExceedsRemaining {
        count: u64::MAX,
        min_item_len: 1,
        remaining: 0,
    };
    assert_eq!(count_of(&claim, 1), Err(too_many));
}

#[test]
fn a_count_the_remaining_bytes_cannot_hold_is_refused() {
    assert_eq!(count_of(&count_then(3, 6), 2), Ok(3));
    let too_many = DecodeError::CountExceedsRemaining {
        count: 4,
        min_item_len: 2,
        remaining: 6,
    };
    assert_eq!(count_of(&count_then(4, 6), 2), Err(too_many));

    // The count times the item size overflows a `u64`.
    let huge = u64::MAX / 2 + 1;
    let overflow = DecodeError::CountExceedsRemaining {
        count: huge,
        min_item_len: 2,
        remaining: 6,
    };
    assert_eq!(count_of(&count_then(huge, 6), 2), Err(overflow));

    // An item of 0 bytes counts as 1, so a count can't pass the bytes that remain.
    assert_eq!(count_of(&count_then(1, 1), 0), Ok(1));
    let zero_sized = DecodeError::CountExceedsRemaining {
        count: 2,
        min_item_len: 0,
        remaining: 1,
    };
    assert_eq!(count_of(&count_then(2, 1), 0), Err(zero_sized));
}

#[test]
fn a_reader_error_is_returned() {
    let mut record: Vec<u8> = Vec::new();
    record.write_bytes(b"payload").unwrap();
    let failed = DecodeError::Reader("boom");

    // The first read fails.
    let mut reader = FailAfter {
        bytes: &record,
        reads: 0,
    };
    assert_eq!(reader.read_u8(), Err(failed));

    // The length's read is served, and the first read of the payload fails.
    let mut reader = FailAfter {
        bytes: &record,
        reads: 1,
    };
    assert_eq!(reader.read_bytes(), Err(failed));
}

#[test]
fn a_reader_short_of_its_buffered_len_is_refused() {
    let short = DecodeError::Truncated {
        needed: 4,
        remaining: 2,
    };

    let mut reader = Overstated {
        bytes: &[1, 2],
        extra: 2,
    };
    assert_eq!(reader.read_u32(), Err(short));

    let mut record: Vec<u8> = Vec::new();
    record.write_u64(4).unwrap();
    record.write_raw(&[1, 2]).unwrap();
    let mut reader = Overstated {
        bytes: &record,
        extra: 2,
    };
    assert_eq!(reader.read_bytes(), Err(short));
}

#[test]
fn an_encoding_into_a_bounded_slice_equals_the_vec_encoding() {
    let fields = sample();
    let expected = reference(&fields);
    let mut bounded = vec![0u8; expected.len()];
    let mut out = &mut bounded[..];
    encode(&fields, &mut out).unwrap();
    assert!(out.is_empty());
    assert_eq!(bounded, expected);
}

#[test]
fn a_write_past_the_capacity_is_refused_without_a_partial_field() {
    let mut fields = sample();
    // A list is a count and then fields of its own; an empty one is its count alone.
    fields.retain(|field| !matches!(field, Field::List(_)));
    fields.push(Field::List(vec![]));

    for field in &fields {
        let one = core::slice::from_ref(field);
        let needed = reference(one).len();
        for remaining in 0..needed {
            let mut bounded = vec![0xEE_u8; remaining];
            let mut out = &mut bounded[..];
            let refused = EncodeError::Capacity { needed, remaining };
            assert_eq!(encode(one, &mut out), Err(refused));
            assert_eq!(out.len(), remaining, "{field:?}");
            assert!(bounded.iter().all(|byte| *byte == 0xEE), "{field:?}");
        }

        let mut bounded = vec![0u8; needed];
        let mut out = &mut bounded[..];
        assert_eq!(encode(one, &mut out), Ok(()));
        assert_eq!(bounded, reference(one));
    }
}

#[test]
fn a_write_past_a_write_limit_is_refused_without_panicking() {
    let mut out: Vec<u8> = Vec::new();
    let mut limited = writer::Limit::new(&mut out, 10);
    limited.write_u64(1).unwrap();

    let refused = EncodeError::Capacity {
        needed: 9,
        remaining: 2,
    };
    assert_eq!(limited.write_bytes(b"x"), Err(refused));
    assert_eq!(writer::Buffer::remaining_capacity(&limited), 2);
    limited.write_u16(0xFFFF).unwrap();
    assert_eq!(writer::Buffer::remaining_capacity(&limited), 0);
    assert_eq!(out, [1, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF]);
}

#[test]
fn errors_say_what_was_refused() {
    let capacity = EncodeError::Capacity {
        needed: 9,
        remaining: 2,
    };
    assert_eq!(
        capacity.to_string(),
        "the field needs 9 bytes and the writer can take 2"
    );

    let mut reader: &[u8] = &[2, 0, 0, 0, 0, 0, 0, 0, 0xC3, 0x28];
    let invalid_text = reader.read_text().unwrap_err();
    let cases: [(DecodeError, &str); 6] = [
        (
            DecodeError::Truncated {
                needed: 4,
                remaining: 3,
            },
            "the field needs 4 bytes and 3 remain",
        ),
        (
            DecodeError::LengthExceedsRemaining {
                len: 4,
                remaining: 3,
            },
            "a length of 4 bytes is more than the 3 that remain",
        ),
        (
            DecodeError::CountExceedsRemaining {
                count: 4,
                min_item_len: 2,
                remaining: 6,
            },
            "4 items of at least 2 bytes don't fit in 6 bytes",
        ),
        (DecodeError::InvalidBool(2), "a bool is 2, not 0 or 1"),
        (
            DecodeError::TrailingBytes { remaining: 1 },
            "1 bytes follow the record",
        ),
        (
            invalid_text,
            "a text field isn't valid UTF-8: invalid utf-8 sequence of 1 bytes from index 0",
        ),
    ];
    for (error, expected) in cases {
        assert_eq!(error.to_string(), expected);
    }

    let reader_error: DecodeError<&str> = DecodeError::Reader("boom");
    assert_eq!(reader_error.to_string(), "the reader failed: boom");
}

#[test]
#[cfg_attr(miri, ignore)] // the property search is too expensive under miri
fn writes_and_reads_match_the_reference() {
    bolero::check!()
        .with_type::<(Vec<(u8, u64, Vec<u8>)>, Vec<u8>)>()
        .for_each(|(specs, cuts)| {
            let fields: Vec<Field> = specs
                .iter()
                .map(|(kind, value, bytes)| field(*kind, *value, bytes))
                .collect();
            let expected = reference(&fields);

            // The writes give the reference encoding, into a vec and into a bounded slice.
            let mut encoded: Vec<u8> = Vec::new();
            encode(&fields, &mut encoded).unwrap();
            assert_eq!(encoded, expected);
            let mut bounded = vec![0u8; expected.len()];
            let mut out = &mut bounded[..];
            encode(&fields, &mut out).unwrap();
            assert!(out.is_empty());
            assert_eq!(bounded, expected);

            // The reads take the fields back across the chunk boundaries `cuts` sets.
            let parts = split(&expected, cuts);
            let mut reader = IoSlice::new(&parts);
            assert_eq!(decode(&fields, &mut reader), Ok(fields.clone()));

            // A byte more is refused as trailing, and a byte less as short.
            let mut longer = expected.clone();
            longer.push(0);
            let trailing = DecodeError::TrailingBytes { remaining: 1 };
            assert_eq!(decode(&fields, &mut &longer[..]), Err(trailing));
            if !expected.is_empty() {
                let mut shorter = &expected[..expected.len() - 1];
                assert!(decode(&fields, &mut shorter).is_err());
            }
        });
}
