// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Fixed-width fields, length-prefixed bytes and text, and counts over the [`writer::Buffer`]
//! and [`reader::Buffer`] traits.
//!
//! [`WriteExt`] and [`ReadExt`] are extension traits: a blanket impl adds their methods to every
//! writer and every reader, as [`reader::Infallible`] does for infallible readers.
//!
//! The encoding is little-endian at a fixed width:
//!
//! - A `u8`, `u16`, `u32` or `u64` takes 1, 2, 4 or 8 bytes.
//! - A bool is one byte, `0` or `1`; a read refuses any other byte.
//! - A byte string is a `u64` length followed by that many bytes. Text is a byte string that holds
//!   UTF-8; a read refuses invalid UTF-8.
//! - A count of items is a `u64`; a read refuses a count that the remaining bytes can't hold at
//!   the least size the caller states for an item.
//! - A fixed-size field, such as a 32-byte digest, is its bytes with no prefix.
//!
//! Each write checks its field against the writer's remaining capacity before it writes any of
//! the field's bytes, so a refused write leaves the writer as it was. Each read takes its field
//! across the reader's chunk boundaries, and checks each length and count against the reader's
//! [`buffered_len`](reader::Buffer::buffered_len) before it allocates, so a claimed size can't
//! force an allocation.
//!
//! ```
//! use etude_buffer::codec::{ReadExt as _, WriteExt as _};
//!
//! let mut record: Vec<u8> = Vec::new();
//! record.write_u32(7)?;
//! record.write_text("seven")?;
//!
//! let mut reader = &record[..];
//! assert_eq!(reader.read_u32()?, 7);
//! assert_eq!(reader.read_text()?, "seven");
//! reader.check_end()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use crate::{reader, writer};
use alloc::{string::String, vec::Vec};
use core::fmt;

#[cfg(test)]
mod tests;

/// The width of a length or count prefix, in bytes.
const PREFIX_LEN: usize = 8;

/// Encodes a length or count as its `u64` prefix.
#[inline]
fn prefix(len: usize) -> [u8; PREFIX_LEN] {
    // `usize` is at most 64 bits wide on every target, so the cast keeps the value.
    (len as u64).to_le_bytes()
}

/// Refuses a field of `needed` bytes where the writer can take only `remaining`.
#[inline]
fn check(needed: usize, remaining: usize) -> Result<(), EncodeError> {
    if needed > remaining {
        return Err(EncodeError::Capacity { needed, remaining });
    }
    Ok(())
}

/// Field writes for every [`writer::Buffer`], through a blanket impl.
///
/// Each write checks the whole field against the writer's
/// [`remaining_capacity`](writer::Buffer::remaining_capacity) first, and refuses it with
/// [`EncodeError::Capacity`] without writing any of its bytes.
pub trait WriteExt: writer::Buffer {
    /// Writes a `u8`.
    ///
    /// # Errors
    /// Returns [`EncodeError::Capacity`] where the writer can't take the byte.
    #[inline]
    fn write_u8(&mut self, value: u8) -> Result<(), EncodeError> {
        self.write_raw(&[value])
    }

    /// Writes a `u16` as 2 bytes, little-endian.
    ///
    /// # Errors
    /// Returns [`EncodeError::Capacity`] where the writer can't take 2 bytes.
    #[inline]
    fn write_u16(&mut self, value: u16) -> Result<(), EncodeError> {
        self.write_raw(&value.to_le_bytes())
    }

    /// Writes a `u32` as 4 bytes, little-endian.
    ///
    /// # Errors
    /// Returns [`EncodeError::Capacity`] where the writer can't take 4 bytes.
    #[inline]
    fn write_u32(&mut self, value: u32) -> Result<(), EncodeError> {
        self.write_raw(&value.to_le_bytes())
    }

    /// Writes a `u64` as 8 bytes, little-endian.
    ///
    /// # Errors
    /// Returns [`EncodeError::Capacity`] where the writer can't take 8 bytes.
    #[inline]
    fn write_u64(&mut self, value: u64) -> Result<(), EncodeError> {
        self.write_raw(&value.to_le_bytes())
    }

    /// Writes a bool as one byte, `1` for `true` and `0` for `false`.
    ///
    /// # Errors
    /// Returns [`EncodeError::Capacity`] where the writer can't take the byte.
    #[inline]
    fn write_bool(&mut self, value: bool) -> Result<(), EncodeError> {
        self.write_u8(u8::from(value))
    }

    /// Writes `bytes` as a `u64` length followed by the bytes.
    ///
    /// # Errors
    /// Returns [`EncodeError::Capacity`] where the writer can't take the length and the bytes
    /// together; it then writes neither.
    #[inline]
    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), EncodeError> {
        // A slice holds at most `isize::MAX` bytes, so the sum can't overflow; saturating keeps
        // the check sound regardless.
        let needed = bytes.len().saturating_add(PREFIX_LEN);
        check(needed, self.remaining_capacity())?;
        self.put_slice(&prefix(bytes.len()));
        self.put_slice(bytes);
        Ok(())
    }

    /// Writes `text` as a byte string of its UTF-8 bytes.
    ///
    /// # Errors
    /// As [`write_bytes`](WriteExt::write_bytes).
    #[inline]
    fn write_text(&mut self, text: &str) -> Result<(), EncodeError> {
        self.write_bytes(text.as_bytes())
    }

    /// Writes a count of items as a `u64`; the caller writes the items after it.
    ///
    /// # Errors
    /// Returns [`EncodeError::Capacity`] where the writer can't take 8 bytes.
    #[inline]
    fn write_count(&mut self, count: usize) -> Result<(), EncodeError> {
        self.write_raw(&prefix(count))
    }

    /// Writes `bytes` as they are, with no length: a fixed-size field, such as a digest.
    ///
    /// # Errors
    /// Returns [`EncodeError::Capacity`] where the writer can't take every byte; it then writes
    /// none of them.
    #[inline]
    fn write_raw(&mut self, bytes: &[u8]) -> Result<(), EncodeError> {
        check(bytes.len(), self.remaining_capacity())?;
        self.put_slice(bytes);
        Ok(())
    }
}

impl<T> WriteExt for T where T: writer::Buffer + ?Sized {}

/// Field reads for every [`reader::Buffer`], through a blanket impl.
///
/// The reader's buffered bytes hold the record. Each read takes its field's bytes across as many
/// of the reader's chunks as they span, and [`check_end`](ReadExt::check_end) refuses bytes left
/// after the record's last field.
pub trait ReadExt: reader::Buffer {
    /// Reads a `u8`.
    ///
    /// # Errors
    /// As [`read_raw`](ReadExt::read_raw).
    #[inline]
    fn read_u8(&mut self) -> Result<u8, DecodeError<Self::Error>> {
        Ok(u8::from_le_bytes(self.read_array()?))
    }

    /// Reads a `u16` from 2 bytes, little-endian.
    ///
    /// # Errors
    /// As [`read_raw`](ReadExt::read_raw).
    #[inline]
    fn read_u16(&mut self) -> Result<u16, DecodeError<Self::Error>> {
        Ok(u16::from_le_bytes(self.read_array()?))
    }

    /// Reads a `u32` from 4 bytes, little-endian.
    ///
    /// # Errors
    /// As [`read_raw`](ReadExt::read_raw).
    #[inline]
    fn read_u32(&mut self) -> Result<u32, DecodeError<Self::Error>> {
        Ok(u32::from_le_bytes(self.read_array()?))
    }

    /// Reads a `u64` from 8 bytes, little-endian.
    ///
    /// # Errors
    /// As [`read_raw`](ReadExt::read_raw).
    #[inline]
    fn read_u64(&mut self) -> Result<u64, DecodeError<Self::Error>> {
        Ok(u64::from_le_bytes(self.read_array()?))
    }

    /// Reads a bool from one byte, `1` for `true` and `0` for `false`.
    ///
    /// # Errors
    /// Returns [`DecodeError::InvalidBool`] for any other byte, and otherwise as
    /// [`read_raw`](ReadExt::read_raw).
    #[inline]
    fn read_bool(&mut self) -> Result<bool, DecodeError<Self::Error>> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            byte => Err(DecodeError::InvalidBool(byte)),
        }
    }

    /// Reads a byte string: a `u64` length, then that many bytes.
    ///
    /// The length is checked against the bytes that remain before the bytes are allocated.
    ///
    /// # Errors
    /// Returns [`DecodeError::LengthExceedsRemaining`] where the length is more than the bytes
    /// that remain after it, and otherwise as [`read_raw`](ReadExt::read_raw).
    #[inline]
    fn read_bytes(&mut self) -> Result<Vec<u8>, DecodeError<Self::Error>> {
        let needed = read_len(self)?;
        let mut bytes = Vec::with_capacity(needed);
        while bytes.len() < needed {
            let chunk = self.read_chunk(needed - bytes.len())?;
            if chunk.is_empty() {
                // The reader held fewer bytes than it reported.
                let remaining = bytes.len();
                return Err(DecodeError::Truncated { needed, remaining });
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    /// Reads text: a byte string that holds UTF-8.
    ///
    /// # Errors
    /// Returns [`DecodeError::InvalidText`] where the bytes aren't valid UTF-8, and otherwise as
    /// [`read_bytes`](ReadExt::read_bytes).
    #[inline]
    fn read_text(&mut self) -> Result<String, DecodeError<Self::Error>> {
        let bytes = self.read_bytes()?;
        String::from_utf8(bytes)
            .map_err(|error| DecodeError::InvalidText(error.utf8_error()))
    }

    /// Reads a count of items, each at least `min_item_len` bytes, and checks that the bytes
    /// that remain can hold that many before the caller allocates for them.
    ///
    /// A `min_item_len` of 0 counts as 1, so a count is never more than the bytes that remain.
    ///
    /// # Errors
    /// Returns [`DecodeError::CountExceedsRemaining`] where the bytes that remain after the count
    /// can't hold its items, and otherwise as [`read_raw`](ReadExt::read_raw).
    #[inline]
    fn read_count(&mut self, min_item_len: usize) -> Result<usize, DecodeError<Self::Error>> {
        let count = self.read_u64()?;
        let remaining = self.buffered_len();
        // Compare as `u64`, so a count past `usize::MAX` on a narrower target is refused too.
        let item_len = min_item_len.max(1) as u64;
        let fits = count
            .checked_mul(item_len)
            .is_some_and(|needed| needed <= remaining as u64);
        if !fits {
            return Err(DecodeError::CountExceedsRemaining {
                count,
                min_item_len,
                remaining,
            });
        }
        // `count` is at most `remaining`, a `usize`, so the cast keeps the value.
        Ok(count as usize)
    }

    /// Reads `N` bytes as they are: a fixed-size field, such as a digest.
    ///
    /// # Errors
    /// As [`read_raw`](ReadExt::read_raw).
    #[inline]
    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError<Self::Error>> {
        let mut array = [0; N];
        self.read_raw(&mut array)?;
        Ok(array)
    }

    /// Fills `dest` with the next `dest.len()` bytes as they are.
    ///
    /// # Errors
    /// Returns [`DecodeError::Truncated`] where fewer bytes remain, without reading any of them,
    /// or where the reader holds fewer bytes than its `buffered_len` reports, after it reads the
    /// bytes the reader holds; and [`DecodeError::Reader`] where the reader fails.
    #[inline]
    fn read_raw(&mut self, dest: &mut [u8]) -> Result<(), DecodeError<Self::Error>> {
        let needed = dest.len();
        let remaining = self.buffered_len();
        if needed > remaining {
            return Err(DecodeError::Truncated { needed, remaining });
        }
        let mut filled = 0;
        while filled < needed {
            let chunk = self.read_chunk(needed - filled)?;
            if chunk.is_empty() {
                // The reader held fewer bytes than it reported.
                let remaining = filled;
                return Err(DecodeError::Truncated { needed, remaining });
            }
            dest[filled..filled + chunk.len()].copy_from_slice(&chunk);
            filled += chunk.len();
        }
        Ok(())
    }

    /// Ends the record: refuses any bytes left after its last field.
    ///
    /// # Errors
    /// Returns [`DecodeError::TrailingBytes`] where bytes remain.
    #[inline]
    fn check_end(&self) -> Result<(), DecodeError<Self::Error>> {
        let remaining = self.buffered_len();
        if remaining > 0 {
            return Err(DecodeError::TrailingBytes { remaining });
        }
        Ok(())
    }
}

impl<T> ReadExt for T where T: reader::Buffer + ?Sized {}

/// Reads a byte string's `u64` length, and checks it against the bytes that remain.
#[inline]
fn read_len<R>(reader: &mut R) -> Result<usize, DecodeError<R::Error>>
where
    R: ReadExt + ?Sized,
{
    let len = reader.read_u64()?;
    let remaining = reader.buffered_len();
    // Compare as `u64`, so a length past `usize::MAX` on a narrower target is refused too.
    if len > remaining as u64 {
        return Err(DecodeError::LengthExceedsRemaining { len, remaining });
    }
    // `len` is at most `remaining`, a `usize`, so the cast keeps the value.
    Ok(len as usize)
}

/// A field a [`WriteExt`] write refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    /// The field needs more bytes than the writer can take; none of them were written.
    Capacity {
        /// The bytes the field needs.
        needed: usize,
        /// The bytes the writer could take.
        remaining: usize,
    },
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capacity { needed, remaining } => write!(
                f,
                "the field needs {needed} bytes and the writer can take {remaining}"
            ),
        }
    }
}

impl core::error::Error for EncodeError {}

/// A record a [`ReadExt`] read refused.
///
/// The `E` parameter is the reader's error; it defaults to [`core::convert::Infallible`] for a
/// reader that can't fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError<E = core::convert::Infallible> {
    /// The bytes end inside a field.
    Truncated {
        /// The bytes the field needs.
        needed: usize,
        /// The bytes that remained for it.
        remaining: usize,
    },
    /// A byte string's length is more than the bytes that remain after it.
    LengthExceedsRemaining {
        /// The length the record claims.
        len: u64,
        /// The bytes that remain after the length.
        remaining: usize,
    },
    /// A count is more items than the bytes that remain after it can hold.
    CountExceedsRemaining {
        /// The count the record claims.
        count: u64,
        /// The least size of an item, as the caller stated it.
        min_item_len: usize,
        /// The bytes that remain after the count.
        remaining: usize,
    },
    /// A bool's byte is neither `0` nor `1`.
    InvalidBool(u8),
    /// A text field isn't valid UTF-8.
    InvalidText(core::str::Utf8Error),
    /// Bytes remain after the record's last field.
    TrailingBytes {
        /// The bytes that remain.
        remaining: usize,
    },
    /// The reader failed; carries its error.
    Reader(E),
}

impl<E> From<E> for DecodeError<E> {
    #[inline]
    fn from(error: E) -> Self {
        Self::Reader(error)
    }
}

impl<E: fmt::Display> fmt::Display for DecodeError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { needed, remaining } => write!(
                f,
                "the field needs {needed} bytes and {remaining} remain"
            ),
            Self::LengthExceedsRemaining { len, remaining } => write!(
                f,
                "a length of {len} bytes is more than the {remaining} that remain"
            ),
            Self::CountExceedsRemaining {
                count,
                min_item_len,
                remaining,
            } => write!(
                f,
                "{count} items of at least {min_item_len} bytes don't fit in {remaining} bytes"
            ),
            Self::InvalidBool(byte) => write!(f, "a bool is {byte}, not 0 or 1"),
            Self::InvalidText(error) => write!(f, "a text field isn't valid UTF-8: {error}"),
            Self::TrailingBytes { remaining } => write!(f, "{remaining} bytes follow the record"),
            Self::Reader(error) => write!(f, "the reader failed: {error}"),
        }
    }
}

impl<E: core::error::Error> core::error::Error for DecodeError<E> {}
