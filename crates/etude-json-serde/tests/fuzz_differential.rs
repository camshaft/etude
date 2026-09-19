// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Property/differential fuzz of the adapter (the parser layer) against `serde_json`, complementing
//! `etude-json`'s tokenizer-level bolero tests. For a generated input the adapter and serde_json must
//! agree on accept/reject, and on the built value when both accept.
//!
//! Inputs are arbitrary bytes mapped onto a JSON-biased alphabet that deliberately EXCLUDES backslash:
//! with no `\` there are no string escapes, which keeps the oracle exact — it sidesteps the one known,
//! intentional divergence (lone-surrogate `\u` escapes decode lossily to U+FFFD rather than being
//! rejected; see `src/tests.rs`). The alphabet still exercises structure, numbers, whitespace, the
//! literals, and simple strings exhaustively in random combination. Runs bounded under `cargo test`;
//! fuzz it with `cargo bolero test adapter_matches_serde_json_on_backslash_free_input`.

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
