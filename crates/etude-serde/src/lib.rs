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
//! - Numbers: [`NumberToken`] — never a borrow, always a lazy, skippable decode. One eager sub-rope
//!   (the whole lexeme, for `Decimal::parse`) plus component digit runs kept as offsets and sliced only
//!   on demand (for `Decimal::from_components`), so a skip/scan consumer pays a single slice (§5, §6).

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::ops::Range;
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

/// A number, handed to a [`Visitor`] as one owned-shared sub-rope plus lazy component metadata — never
/// a borrow of a parsed value, always a skippable decode. Self-contained (the Visitor has no handle on
/// the source rope), yet the eager cost is a *single* O(1) sub-rope slice regardless of how much the
/// consumer reads (design §5, settled — materialize-on-demand applied inside the token).
///
/// - [`lexeme`](NumberToken::lexeme) is the **primary** payload and the only eager slice: the whole
///   validated number lexeme. Its byte iterator (`lexeme().chunks().flat_map(|c| c.iter().copied())`)
///   feeds a from-text value constructor such as `Decimal::parse(impl IntoIterator<Item = u8>)`.
/// - The integer / fraction / exponent digit runs are stored as **offset ranges within the lexeme**
///   (private, an invariant the decoder establishes — a consumer can't hand-build mismatched offsets)
///   and materialized only when asked, via [`integer`](Self::integer) / [`fraction`](Self::fraction) /
///   [`exponent`](Self::exponent) (each one O(1) sub-slice of the short single-chunk lexeme). This
///   feeds a pre-split-digit constructor (`Decimal::from_components(sign, int, frac, exp)`), while a
///   skip/scan consumer pays nothing beyond the lexeme and [`is_integer`](Self::is_integer) is free.
#[derive(Clone, Debug)]
pub struct NumberToken {
    lexeme: ByteVec,
    negative: bool,
    integer: Range<usize>,
    fraction: Option<Range<usize>>,
    exponent: Option<Range<usize>>,
    exponent_negative: bool,
}

impl NumberToken {
    /// Build a number token from a validated lexeme and its component offsets. Producer-only (the
    /// decoder / tokenizer): each range MUST index within `lexeme` and cover only that component's
    /// digits (no sign / `.` / `e`), and `integer` must be non-empty for a valid number.
    pub fn new(
        lexeme: ByteVec,
        negative: bool,
        integer: Range<usize>,
        fraction: Option<Range<usize>>,
        exponent: Option<Range<usize>>,
        exponent_negative: bool,
    ) -> Self {
        NumberToken {
            lexeme,
            negative,
            integer,
            fraction,
            exponent,
            exponent_negative,
        }
    }

    /// The whole number lexeme — the primary payload; feed its byte iterator to a from-text ctor.
    pub fn lexeme(&self) -> &ByteVec {
        &self.lexeme
    }

    /// Whether the lexeme has a leading `-`.
    pub fn is_negative(&self) -> bool {
        self.negative
    }

    /// The integer-part digits (no sign) — an O(1) sub-slice of the lexeme, materialized on demand.
    pub fn integer(&self) -> ByteVec {
        self.lexeme.slice(self.integer.clone())
    }

    /// The fraction digits after `.` (digits only), or `None` — materialized on demand.
    pub fn fraction(&self) -> Option<ByteVec> {
        self.fraction.clone().map(|r| self.lexeme.slice(r))
    }

    /// The exponent digits after `e`/`E` and its optional sign (digits only), or `None` — on demand.
    pub fn exponent(&self) -> Option<ByteVec> {
        self.exponent.clone().map(|r| self.lexeme.slice(r))
    }

    /// Whether the exponent carries an explicit `-`. `false` with no exponent or a `+`/unsigned one.
    pub fn exponent_is_negative(&self) -> bool {
        self.exponent_negative
    }

    /// Whether the lexeme is an integer — no fraction and no exponent. Free (no slice).
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
