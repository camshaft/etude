// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Rope-native, zero-copy value model and Visitor/Deserializer seam.
//!
//! **DRAFT / SPIKE.** This crate is the first cut of the `etude-serde` surface described in the
//! cadenza design doc `DESIGN-etude-serde-zerocopy-ecosystem.md` (§5), stood up so that `etude-json`
//! can be built as its first real consumer and feed fit-feedback back into the design before it
//! locks. It is opened for review, not merged: the central design choice (Decision 0 — drop serde's
//! `'de` lifetime and get zero-copy via structural rope-sharing rather than borrowing) is a pending
//! open question on the design PR, and a rejected Decision 0 would reshape these traits.
//!
//! # The one idea (Decision 0)
//! There is no `'de` lifetime anywhere. A "borrowed" value is not a `&'de` reference into an input
//! buffer; it is an *owned-shared* handle onto the source rope's chunks (a [`StrRope`] / [`ByteVec`]
//! slice, refcounted). Zero-copy comes from structural sharing, so a value can outlive the scan with
//! no lifetime threading — the source chunks stay alive as long as any slice holds them. This is what
//! lets [`SeqAccess`]/[`MapAccess`] hand nested values around without the borrow-checker gymnastics a
//! `Deserializer<'de>` forces.
//!
//! # Value shapes
//! - Strings: [`RopeStr`] — `Borrowed` (an O(1) rope slice, the copy-avoidance payoff for unescaped
//!   content) or `Owned` (an unescaped buffer; unescape can never be a pure borrow). The decoder
//!   picks the arm from its cheap has-escapes flag, with no re-scan.
//! - Bytes: [`RopeBytes`] — `Borrowed` sub-rope or `Owned` buffer.
//! - Numbers: [`NumberToken`] — never a borrow, always a lazy decode, but skippable. It carries the
//!   byte-offset spans of the number's components so a value constructor
//!   (`Decimal::from_components`, …) is handed validated digits with no intermediate `String` (§6).

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use etude_bytevec::ByteVec;
use etude_strrope::StrRope;

/// A string value: an O(1) structural-share of the source rope when the content needs no
/// transformation, or an owned buffer when it had to be materialized (an escaped string).
#[derive(Clone, Debug)]
pub enum RopeStr {
    /// A char-safe slice of the source rope — zero copy (an owned-shared handle onto its chunks).
    Borrowed(StrRope),
    /// A materialized buffer — an escaped string unescaped into an owned `String`.
    Owned(String),
}

impl RopeStr {
    /// Whether this is the zero-copy [`RopeStr::Borrowed`] arm.
    pub fn is_borrowed(&self) -> bool {
        matches!(self, RopeStr::Borrowed(_))
    }

    /// The content length in bytes (not chars).
    pub fn len(&self) -> usize {
        match self {
            RopeStr::Borrowed(r) => r.len(),
            RopeStr::Owned(s) => s.len(),
        }
    }

    /// Whether the content is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl fmt::Display for RopeStr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RopeStr::Borrowed(r) => write!(f, "{r}"),
            RopeStr::Owned(s) => f.write_str(s),
        }
    }
}

/// A bytes value: an O(1) sub-rope of the source, or an owned buffer.
#[derive(Clone, Debug)]
pub enum RopeBytes {
    /// A sub-rope of the source — zero copy.
    Borrowed(ByteVec),
    /// A materialized buffer.
    Owned(Vec<u8>),
}

impl RopeBytes {
    /// Whether this is the zero-copy [`RopeBytes::Borrowed`] arm.
    pub fn is_borrowed(&self) -> bool {
        matches!(self, RopeBytes::Borrowed(_))
    }
}

/// A number, as owned-shared [`ByteVec`] sub-ropes of the source — never a borrow of a parsed value,
/// always a lazy, skippable decode. All runs are O(1) structural shares of the same source chunks
/// (numbers are short), so the token is self-contained: the [`Visitor`] can decode without any handle
/// on the source rope.
///
/// It is a *superset* handoff (design §5/§6, settled):
/// - [`lexeme`](NumberToken::lexeme) is the **primary** payload — the whole validated number lexeme as
///   one sub-rope. Its byte iterator (`lexeme.chunks().flat_map(|c| c.iter().copied())`) feeds a
///   from-text value constructor such as `Decimal::parse(impl IntoIterator<Item = u8>)`. One slice, no
///   re-synthesis of the `.`/`e`/sign.
/// - The component runs + flags below are cheap structural metadata for a consumer that wants
///   integer-detection without re-scanning, or a value type that wants pre-split digits
///   (`Decimal::from_components(sign, int_digits, frac_digits, exp)`). The coefficient digits are
///   `integer` then `fraction`; the effective power of ten is `±exponent − fraction.len()`.
///
/// A decoder builds all of these from its recorded spans (`etude_json::Token::span` for the lexeme,
/// `Token::number_parts` for the components) with one `ByteVec::slice` each. (A bare `Span` would not
/// work at the visit boundary — the Visitor has no handle on the source rope to resolve it.)
#[derive(Clone, Debug)]
pub struct NumberToken {
    /// The whole number lexeme (sign, integer, optional fraction, optional exponent) as one sub-rope —
    /// the primary payload for a from-text value constructor.
    pub lexeme: ByteVec,
    /// The lexeme has a leading `-`.
    pub negative: bool,
    /// The integer-part digits (no sign) — a non-empty run for a valid number.
    pub integer: ByteVec,
    /// The fraction digits after `.` (digits only), or `None` if there is no fraction.
    pub fraction: Option<ByteVec>,
    /// The exponent digits after `e`/`E` and its optional sign (digits only), or `None`.
    pub exponent: Option<ByteVec>,
    /// The exponent carries an explicit `-`. `false` when there is no exponent or it is `+`/unsigned.
    pub exponent_negative: bool,
}

impl NumberToken {
    /// Whether the lexeme is an integer — no fraction and no exponent.
    pub fn is_integer(&self) -> bool {
        self.fraction.is_none() && self.exponent.is_none()
    }
}

/// Receives the one value a [`Deserializer`] decodes. Rope-shaped and `'de`-free: string/bytes values
/// arrive as owned-shared [`RopeStr`]/[`RopeBytes`], numbers as a [`NumberToken`], and containers as
/// an accessor the visitor drives.
pub trait Visitor {
    /// The type this visitor builds.
    type Value;

    /// A JSON `null` (or an absent/unit value).
    fn visit_null(self) -> Result<Self::Value, Error>;
    /// A boolean.
    fn visit_bool(self, b: bool) -> Result<Self::Value, Error>;
    /// A string, borrowed (zero-copy) or owned.
    fn visit_str(self, s: RopeStr) -> Result<Self::Value, Error>;
    /// A byte string, borrowed (zero-copy) or owned.
    fn visit_bytes(self, b: RopeBytes) -> Result<Self::Value, Error>;
    /// A number, as its component spans (decode on demand).
    fn visit_number(self, n: NumberToken) -> Result<Self::Value, Error>;
    /// A sequence; drive `seq` to pull each element.
    fn visit_seq<A: SeqAccess>(self, seq: A) -> Result<Self::Value, Error>;
    /// A map; drive `map` to pull each key/value.
    fn visit_map<A: MapAccess>(self, map: A) -> Result<Self::Value, Error>;
}

/// Decodes exactly one value out of a source, driving a [`Visitor`]. No `<'de>` parameter (Decision 0)
/// — a decoded borrowed value is an owned-shared rope handle, not a reference tied to the input.
pub trait Deserializer {
    /// Inspect the next value and dispatch to the matching [`Visitor`] method.
    fn deserialize_any<V: Visitor>(self, visitor: V) -> Result<V::Value, Error>;
    // Typed hints (`deserialize_str`/`deserialize_map`/… as serde has, minus the lifetime) are a later
    // increment; `deserialize_any` is the core the spike exercises.
}

/// Pulls the elements of a sequence, each as a sub-value the caller's [`Visitor`] receives. The
/// accessor is positioned by the underlying scan cursor; a nested element re-enters
/// [`Deserializer::deserialize_any`] internally, so arbitrary nesting threads with no lifetime.
pub trait SeqAccess {
    /// Decode the next element into `visitor`, or `None` at the end of the sequence.
    fn next_element<V: Visitor>(&mut self, visitor: V) -> Result<Option<V::Value>, Error>;

    /// A lower bound on the remaining element count, if cheaply known.
    fn size_hint(&self) -> Option<usize> {
        None
    }
}

/// Pulls the entries of a map. A key is typically a [`RopeStr::Borrowed`]; a value re-enters
/// [`Deserializer::deserialize_any`] and so may itself be a container.
pub trait MapAccess {
    /// Decode the next key into `visitor`, or `None` at the end of the map.
    fn next_key<V: Visitor>(&mut self, visitor: V) -> Result<Option<V::Value>, Error>;
    /// Decode the value paired with the last key returned by [`MapAccess::next_key`].
    fn next_value<V: Visitor>(&mut self, visitor: V) -> Result<V::Value, Error>;

    /// A lower bound on the remaining entry count, if cheaply known.
    fn size_hint(&self) -> Option<usize> {
        None
    }
}

/// A deserialization failure. Draft shape: a message; a decoder maps its own lexical error into this.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    message: String,
}

impl Error {
    /// Build an error from a display message (serde's `de::Error::custom` shape, minus the trait).
    pub fn custom(message: impl fmt::Display) -> Self {
        Error {
            message: alloc::format!("{message}"),
        }
    }

    /// The error message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

#[cfg(test)]
mod tests;
