# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/camshaft/etude/releases/tag/etude-json-v0.1.0) - 2026-09-28

### Other

- *(test)* faithful-decomposition oracle for number_parts() — beyond-f64 precision ([#306](https://github.com/camshaft/etude/pull/306))
- bulk-skip digit runs in scan_number (−62% on big numbers, −9% on short) ([#269](https://github.com/camshaft/etude/pull/269))
- *(json)* fuzz the Strict-mode span soundness contract — accepted String token spans are valid UTF-8 ([#255](https://github.com/camshaft/etude/pull/255))
- *(json)* lowercase an emphasis-cap in Strictness::Lenient doc ([#248](https://github.com/camshaft/etude/pull/248))
- shrink Token 120→96B — fold mutually-exclusive string/number data into one Payload enum ([#247](https://github.com/camshaft/etude/pull/247))
- add configurable Strictness (Strict validates UTF-8 + surrogate pairs at lex; Lenient = superset) ([#246](https://github.com/camshaft/etude/pull/246))
- *(json)* lowercase one emphasis-cap in decode_str's doc ([#241](https://github.com/camshaft/etude/pull/241))
- *(json)* fuzz tokenizer no-panic + accept-superset on arbitrary bytes ([#237](https://github.com/camshaft/etude/pull/237))
- *(json)* track the invalid-UTF-8 string-content divergence from serde_json ([#236](https://github.com/camshaft/etude/pull/236))
- *(json)* enforce the tokenize allocation budget (zero shallow, stack-only deep) ([#194](https://github.com/camshaft/etude/pull/194))
- *(bytevec)* let consumer crates opt in to the invariant checker ([#189](https://github.com/camshaft/etude/pull/189))
- record number sub-spans during scan + expose Token::number_parts (decode_number groundwork) ([#171](https://github.com/camshaft/etude/pull/171))
- add JSON conformance edge-case tests (deep nesting, UTF-8/surrogate/BOM, stream semantics) ([#161](https://github.com/camshaft/etude/pull/161))
- add public-API integration tests (curated RFC 8259 edge cases, differential vs serde_json) ([#156](https://github.com/camshaft/etude/pull/156))
- drop all-caps emphasis from docs/comments (fleet doc convention) ([#152](https://github.com/camshaft/etude/pull/152))
- *(bench)* backfill Token::decode_string differential bench + bulk-copy decode runs ([#137](https://github.com/camshaft/etude/pull/137))
- *(bench)* measure the O(log n)-per-token span-resolution cost ([#105](https://github.com/camshaft/etude/pull/105))
- extract the chunk Cursor + Span into a reusable crate (operator direction) ([#101](https://github.com/camshaft/etude/pull/101))
- bulk-scan string runs within a rope leaf (close the big-string gap) ([#91](https://github.com/camshaft/etude/pull/91))
- benchmark scoreboard vs serde_json — allocations first-class (slice 3) ([#86](https://github.com/camshaft/etude/pull/86))
- decode escaped strings over a contiguous buffer (O(len), not O(len·log n)) ([#80](https://github.com/camshaft/etude/pull/80))
- chunk-streaming tokenizer cursor (O(n), not O(n log n)) ([#77](https://github.com/camshaft/etude/pull/77))
- migrate dep etude-byterope -> etude-bytevec (flag-day #54) ([#67](https://github.com/camshaft/etude/pull/67))
- copy-avoiding rope->JSON-token iterator (slice 1) ([#62](https://github.com/camshaft/etude/pull/62))
