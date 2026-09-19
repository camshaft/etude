// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Trait-level validation of the seam: a trivial in-memory [`Deserializer`] over a small value tree,
//! driven by a rendering [`Visitor`]. This proves the `'de`-free traits are implementable and — the
//! key point for the design (fit-feedback (a)) — that nested [`SeqAccess`]/[`MapAccess`] values
//! thread with no lifetime plumbing. The real validation is the `etude-json` adapter against the live
//! tokenizer (next increment); this stands in until then and pins the trait ergonomics.

use super::*;
use alloc::vec;
use alloc::vec::IntoIter;
use etude_strrope::StrRope;

/// A tiny in-memory value, standing in for a decoded document.
enum Val {
    Null,
    Bool(bool),
    Str(RopeStr),
    Int,
    Arr(vec::Vec<Val>),
    Obj(vec::Vec<(RopeStr, Val)>),
}

/// An in-memory [`Deserializer`]: dispatches one `Val` to the visitor.
struct ValDe(Val);

impl Deserializer for ValDe {
    fn deserialize_any<V: Visitor>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0 {
            Val::Null => visitor.visit_null(),
            Val::Bool(b) => visitor.visit_bool(b),
            Val::Str(s) => visitor.visit_str(s),
            Val::Int => visitor.visit_number(NumberToken {
                lexeme: ByteVec::from(&b"-1"[..]),
                negative: true,
                integer: ByteVec::from(&b"1"[..]),
                fraction: None,
                exponent: None,
                exponent_negative: false,
            }),
            Val::Arr(items) => visitor.visit_seq(SeqDe {
                iter: items.into_iter(),
            }),
            Val::Obj(entries) => visitor.visit_map(MapDe {
                iter: entries.into_iter(),
                pending: None,
            }),
        }
    }
}

struct SeqDe {
    iter: IntoIter<Val>,
}

impl SeqAccess for SeqDe {
    fn next_element<V: Visitor>(&mut self, visitor: V) -> Result<Option<V::Value>, Error> {
        match self.iter.next() {
            Some(val) => ValDe(val).deserialize_any(visitor).map(Some),
            None => Ok(None),
        }
    }
}

struct MapDe {
    iter: IntoIter<(RopeStr, Val)>,
    pending: Option<Val>,
}

impl MapAccess for MapDe {
    fn next_key<V: Visitor>(&mut self, visitor: V) -> Result<Option<V::Value>, Error> {
        match self.iter.next() {
            Some((key, val)) => {
                self.pending = Some(val);
                ValDe(Val::Str(key)).deserialize_any(visitor).map(Some)
            }
            None => Ok(None),
        }
    }

    fn next_value<V: Visitor>(&mut self, visitor: V) -> Result<V::Value, Error> {
        let val = self
            .pending
            .take()
            .ok_or_else(|| Error::custom("next_value called before next_key"))?;
        ValDe(val).deserialize_any(visitor)
    }
}

/// A visitor that renders any value to a compact canonical string — recursing through containers, so
/// a passing render proves seq/map threading (and that an owned-shared value flows through the visit
/// boundary).
struct Render;

impl Visitor for Render {
    type Value = String;

    fn visit_null(self) -> Result<String, Error> {
        Ok(String::from("null"))
    }
    fn visit_bool(self, b: bool) -> Result<String, Error> {
        Ok(String::from(if b { "true" } else { "false" }))
    }
    fn visit_str(self, s: RopeStr) -> Result<String, Error> {
        // Tag which arm arrived so the test can assert the borrowed/owned split flows through.
        let tag = if s.is_borrowed() { 'b' } else { 'o' };
        Ok(alloc::format!("{tag}\"{s}\""))
    }
    fn visit_bytes(self, _b: RopeBytes) -> Result<String, Error> {
        Ok(String::from("<bytes>"))
    }
    fn visit_number(self, n: NumberToken) -> Result<String, Error> {
        Ok(String::from(if n.is_integer() { "int" } else { "num" }))
    }
    fn visit_seq<A: SeqAccess>(self, mut seq: A) -> Result<String, Error> {
        let mut out = String::from("[");
        let mut first = true;
        while let Some(elem) = seq.next_element(Render)? {
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&elem);
        }
        out.push(']');
        Ok(out)
    }
    fn visit_map<A: MapAccess>(self, mut map: A) -> Result<String, Error> {
        let mut out = String::from("{");
        let mut first = true;
        while let Some(key) = map.next_key(Render)? {
            if !first {
                out.push(',');
            }
            first = false;
            let value = map.next_value(Render)?;
            out.push_str(&key);
            out.push(':');
            out.push_str(&value);
        }
        out.push('}');
        Ok(out)
    }
}

#[test]
fn nested_seq_map_threads_without_lifetimes() {
    // {"a": [null, true, -1, "borrowed"], "owned": {"k": false}}
    let doc = Val::Obj(vec![
        (
            RopeStr::Borrowed(StrRope::from("a")),
            Val::Arr(vec![
                Val::Null,
                Val::Bool(true),
                Val::Int,
                Val::Str(RopeStr::Borrowed(StrRope::from("borrowed"))),
            ]),
        ),
        (
            RopeStr::Owned(String::from("owned")),
            Val::Obj(vec![(
                RopeStr::Borrowed(StrRope::from("k")),
                Val::Bool(false),
            )]),
        ),
    ]);

    let rendered = ValDe(doc).deserialize_any(Render).unwrap();
    // Borrowed keys render with the `b` tag, the owned key with `o`; the nesting threads through.
    assert_eq!(
        rendered,
        r#"{b"a":[null,true,int,b"borrowed"],o"owned":{b"k":false}}"#
    );
}

#[test]
fn rope_str_arms_and_helpers() {
    let borrowed = RopeStr::Borrowed(StrRope::from("héllo"));
    assert!(borrowed.is_borrowed());
    assert_eq!(borrowed.len(), "héllo".len());
    assert!(!borrowed.is_empty());

    let owned = RopeStr::Owned(String::from(""));
    assert!(!owned.is_borrowed());
    assert!(owned.is_empty());

    assert!(RopeBytes::Borrowed(ByteVec::new()).is_borrowed());
}

#[test]
fn number_token_integer_flag() {
    let int = NumberToken {
        lexeme: ByteVec::from(&b"123"[..]),
        negative: false,
        integer: ByteVec::from(&b"123"[..]),
        fraction: None,
        exponent: None,
        exponent_negative: false,
    };
    assert!(int.is_integer());

    let float = NumberToken {
        lexeme: ByteVec::from(&b"1.5"[..]),
        negative: false,
        integer: ByteVec::from(&b"1"[..]),
        fraction: Some(ByteVec::from(&b"5"[..])),
        exponent: None,
        exponent_negative: false,
    };
    assert!(!float.is_integer());
}

#[test]
fn error_carries_a_message() {
    let e = Error::custom("bad token at 7");
    assert_eq!(e.message(), "bad token at 7");
    assert_eq!(alloc::format!("{e}"), "bad token at 7");
}
