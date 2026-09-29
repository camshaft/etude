// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Zero-copy JSON tokenization over an `etude-bytevec` byte rope.
//!
//! The input is built as a rope of several chunks — as if the bytes had arrived from
//! separate reads. [`etude_json::Tokenizer`] walks the rope and yields tokens that each
//! reference a byte *span* of the input rather than copying its bytes; a consumer only
//! pays to materialize a value (an unescaped string) when it actually wants one.
//!
//! Run with: `cargo run -p etude-json --example tokenize`

use bytes::Bytes;
use etude_bytevec::ByteVec;
use etude_json::{TokenKind, Tokenizer};

fn main() {
    // A two-chunk rope — the tokenizer never copies these bytes. The `"a\tb"` string
    // carries a JSON escape, so it is only unescaped when we ask for its value.
    let mut input = ByteVec::from(Bytes::from_static(br#"{"name":"etude","count":42,"#));
    input.push_back(Bytes::from_static(br#""ratio":3.14,"nested":["a\tb"]}"#));

    println!("input: {} bytes across a rope of chunks\n", input.len());

    for token in Tokenizer::new(&input) {
        let token = token.expect("well-formed JSON");
        let span = token.span();
        match token.kind() {
            TokenKind::String => {
                // Materialize the (unescaped) string only now that we want its value.
                let value = token.decode_string(&input).expect("string token decodes");
                println!(
                    "{:>11} @ {:>2}..{:<2}  {value:?}",
                    "String",
                    span.start(),
                    span.end()
                );
            }
            TokenKind::Number => {
                let shape = if token.number_is_integer().unwrap_or(false) {
                    "integer"
                } else {
                    "decimal"
                };
                println!(
                    "{:>11} @ {:>2}..{:<2}  ({shape})",
                    "Number",
                    span.start(),
                    span.end()
                );
            }
            other => {
                println!(
                    "{:>11} @ {:>2}..{:<2}",
                    format!("{other:?}"),
                    span.start(),
                    span.end()
                );
            }
        }
    }
}
