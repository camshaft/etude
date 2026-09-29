# etude-json

> Copy-avoiding JSON over the etude byte-rope: a tokenizer whose tokens reference spans of the
> input rope rather than copying bytes.

The point of this crate is to read JSON without copying its bytes. `Tokenizer` walks a `ByteVec`
and yields `Token`s that each reference a byte range of that input rope (a `Span`) rather than
owning a copy of the bytes. A caller that wants the bytes takes an O(1) structural-sharing
`ByteVec::slice` of the span; only a caller that needs a transformed value — an unescaped string,
a parsed number — pays for materialization, and only then.

## What the tokenizer does and does not do

It is a lexer, not a parser. It validates each token in isolation (a string's escapes, a number's
grammar, a keyword's spelling) and reports the byte offset of the first malformed byte, but it
does not enforce JSON's grammar between tokens: the sequence `] ,` tokenizes into two structural
tokens without complaint. Grammar and nesting are a parser's job (a later layer built on this
iterator). Whitespace (space, tab, CR, LF) between tokens is skipped.

## Copy-avoiding strings

A `String` token carries the span of its raw content (between the quotes) and a flag for whether
that content contains escape sequences. A caller can take the raw content span as a zero-copy
rope slice when `Token::string_has_escapes` is `false` (the common case), or call
`Token::decode_string` to materialize the unescaped `String` when escapes are present.

## Strictness

By default (`Strictness::Strict`, via `Tokenizer::new`) the tokenizer enforces full JSON string
correctness — content must be valid UTF-8 and every `\u` surrogate must be paired — so its
accept/reject matches `serde_json`. `Tokenizer::with_strictness` can instead select
`Strictness::Lenient`, which accepts a documented superset (non-UTF-8 content and lone surrogates
pass, to be resolved lossily on decode).

Part of the [etude](https://github.com/camshaft/etude) workspace. See the
[API documentation on docs.rs](https://docs.rs/etude-json).

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](../../LICENSE)).
