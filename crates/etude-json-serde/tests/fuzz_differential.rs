// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Property/differential fuzz of the adapter (the parser layer) against `serde_json`, complementing
//! `etude-json`'s tokenizer-level bolero tests. Two targets, both bounded under `cargo test` and
//! fuzzable with `cargo bolero test <name>`:
//!
//! - `adapter_matches_serde_json_on_backslash_free_input` — grammar/accept-reject parity. Arbitrary
//!   bytes mapped onto a JSON-biased alphabet that deliberately EXCLUDES backslash: with no `\` there
//!   are no string escapes, which keeps the oracle exact — it sidesteps the one known, intentional
//!   divergence (lone-surrogate `\u` escapes decode lossily to U+FFFD rather than being rejected; see
//!   `src/tests.rs`). Still exercises structure, numbers, whitespace, the literals, and simple strings.
//! - `escape_roundtrip_matches_serde` — the escape-DECODE path. serde emits an arbitrary string's
//!   escaped JSON literal; the adapter must decode it back identically, including across rope-chunk
//!   boundaries (the cross-chunk escape hazard).

use bytes::Bytes;
use etude_bytevec::ByteVec;
use etude_json_serde::from_rope;
use etude_serde::{Error, MapAccess, NumberToken, RopeBytes, RopeStr, SeqAccess, Visitor};
use serde_json::Value;

/// JSON-biased, backslash-free. Includes the letters of `true`/`false`/`null`, the number characters,
/// structural punctuation, the string quote, and the four whitespace bytes (space/tab/LF/CR — the last
/// three also probe unescaped-control-in-string rejection, on which both parsers agree).
const ALPHABET: &[u8] = b"0123456789.eE+-truefalsn \t\n\r[]{},:\"";

fn map_input(raw: &[u8]) -> Vec<u8> {
    raw.iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()])
        .collect()
}

fn rope(bytes: &[u8]) -> ByteVec {
    let mut r = ByteVec::new();
    if !bytes.is_empty() {
        r.push_back(Bytes::copy_from_slice(bytes));
    }
    r
}

/// Build a rope with a fixed chunk size, so escapes / multi-byte chars can be forced to straddle leaf
/// boundaries (`chunk == 1` splits every byte into its own leaf).
fn rope_chunked(bytes: &[u8], chunk: usize) -> ByteVec {
    let chunk = chunk.max(1);
    let mut r = ByteVec::new();
    let mut i = 0;
    while i < bytes.len() {
        let j = (i + chunk).min(bytes.len());
        r.push_back(Bytes::copy_from_slice(&bytes[i..j]));
        i = j;
    }
    r
}

/// Drives the whole parse but retains nothing — used to assert the adapter never panics on arbitrary
/// input (it must always return `Ok`/`Err`, never unwind).
struct NoopVisitor;

impl Visitor for NoopVisitor {
    type Value = ();

    fn visit_null(self) -> Result<(), Error> {
        Ok(())
    }
    fn visit_bool(self, _: bool) -> Result<(), Error> {
        Ok(())
    }
    fn visit_str(self, _: RopeStr) -> Result<(), Error> {
        Ok(())
    }
    fn visit_bytes(self, _: RopeBytes) -> Result<(), Error> {
        Ok(())
    }
    fn visit_number(self, _: NumberToken) -> Result<(), Error> {
        Ok(())
    }
    fn visit_seq<A: SeqAccess>(self, mut seq: A) -> Result<(), Error> {
        while seq.next_element(NoopVisitor)?.is_some() {}
        Ok(())
    }
    fn visit_map<A: MapAccess>(self, mut map: A) -> Result<(), Error> {
        while map.next_key(NoopVisitor)?.is_some() {
            map.next_value(NoopVisitor)?;
        }
        Ok(())
    }
}

/// Decodes a single JSON string value to its `String`, erroring on any other shape.
struct StringVisitor;

impl Visitor for StringVisitor {
    type Value = String;

    fn visit_str(self, s: RopeStr) -> Result<String, Error> {
        Ok(s.to_string())
    }
    fn visit_null(self) -> Result<String, Error> {
        Err(Error::custom("not a string"))
    }
    fn visit_bool(self, _: bool) -> Result<String, Error> {
        Err(Error::custom("not a string"))
    }
    fn visit_bytes(self, _: RopeBytes) -> Result<String, Error> {
        Err(Error::custom("not a string"))
    }
    fn visit_number(self, _: NumberToken) -> Result<String, Error> {
        Err(Error::custom("not a string"))
    }
    fn visit_seq<A: SeqAccess>(self, _: A) -> Result<String, Error> {
        Err(Error::custom("not a string"))
    }
    fn visit_map<A: MapAccess>(self, _: A) -> Result<String, Error> {
        Err(Error::custom("not a string"))
    }
}

/// Builds a `serde_json::Value`, so the adapter's output can be compared directly to serde's.
struct BuildValue;

impl Visitor for BuildValue {
    type Value = Value;

    fn visit_null(self) -> Result<Value, Error> {
        Ok(Value::Null)
    }
    fn visit_bool(self, b: bool) -> Result<Value, Error> {
        Ok(Value::Bool(b))
    }
    fn visit_str(self, s: RopeStr) -> Result<Value, Error> {
        Ok(Value::String(s.to_string()))
    }
    fn visit_bytes(self, _: RopeBytes) -> Result<Value, Error> {
        Err(Error::custom("json has no bytes"))
    }
    fn visit_number(self, n: NumberToken) -> Result<Value, Error> {
        let bytes = n.lexeme().copy_to_bytes();
        serde_json::from_slice(bytes.as_ref()).map_err(Error::custom)
    }
    fn visit_seq<A: SeqAccess>(self, mut seq: A) -> Result<Value, Error> {
        let mut items = Vec::new();
        while let Some(v) = seq.next_element(BuildValue)? {
            items.push(v);
        }
        Ok(Value::Array(items))
    }
    fn visit_map<A: MapAccess>(self, mut map: A) -> Result<Value, Error> {
        let mut obj = serde_json::Map::new();
        while let Some(key) = map.next_key(BuildValue)? {
            let key = match key {
                Value::String(s) => s,
                _ => return Err(Error::custom("non-string key")),
            };
            obj.insert(key, map.next_value(BuildValue)?);
        }
        Ok(Value::Object(obj))
    }
}

#[test]
fn adapter_matches_serde_json_on_backslash_free_input() {
    use bolero::check;

    check!().with_type::<Vec<u8>>().for_each(|raw| {
        let input = map_input(raw);
        let serde = serde_json::from_slice::<Value>(&input);
        let adapter = from_rope(&rope(&input), BuildValue);
        let shown = || String::from_utf8_lossy(&input).into_owned();
        match (serde, adapter) {
            (Ok(s), Ok(a)) => assert_eq!(a, s, "value mismatch on {:?}", shown()),
            (Err(_), Err(_)) => {}
            (Ok(s), Err(e)) => {
                panic!(
                    "serde accepted but adapter rejected {:?}: serde={s:?} err={e:?}",
                    shown()
                )
            }
            (Err(e), Ok(a)) => {
                panic!(
                    "adapter accepted but serde rejected {:?}: got={a:?} serde_err={e}",
                    shown()
                )
            }
        }
    });
}

#[test]
fn adapter_never_panics_on_arbitrary_bytes() {
    use bolero::check;

    // Robustness: on ARBITRARY bytes (invalid UTF-8, raw control bytes, anything) the adapter must
    // always return Ok/Err and never panic — a Deserializer that unwinds on malformed input is a DoS
    // bug. This is the fuzz that would have caught the from_utf8 `.expect` panic; it also probes across
    // rope-chunk boundaries. No serde comparison — the property is purely "does not panic".
    check!().with_type::<Vec<u8>>().for_each(|raw| {
        for chunk in [1usize, raw.len().max(1)] {
            let _ = from_rope(&rope_chunked(raw, chunk), NoopVisitor);
        }
    });
}

#[test]
fn escape_roundtrip_matches_serde() {
    use bolero::check;

    // Fuzz the escape-DECODE path (the riskiest JSON code) cleanly: let serde emit an arbitrary Rust
    // string's properly-escaped JSON literal (control chars -> `\uXXXX`, quote/backslash/newline/…, raw
    // UTF-8 for the rest — and never a lone surrogate, since a Rust `String` cannot hold one), then
    // require the adapter to decode it back to the same string. `chunk == 1` forces every escape and
    // every multi-byte char to straddle a rope leaf boundary — the rope-specific decode hazard.
    check!().with_type::<String>().cloned().for_each(|s| {
        let json = serde_json::to_string(&s).expect("string serializes");
        // Sanity: serde round-trips its own output.
        let via_serde: String = serde_json::from_str(&json).expect("serde parses its own output");
        assert_eq!(via_serde, s);
        for chunk in [1usize, json.len().max(1)] {
            let got = from_rope(&rope_chunked(json.as_bytes(), chunk), StringVisitor)
                .expect("adapter parses a valid JSON string");
            assert_eq!(
                got, s,
                "adapter decode mismatch (json {json:?}) at chunk={chunk}"
            );
        }
    });
}
