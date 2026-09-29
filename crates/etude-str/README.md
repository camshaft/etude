# etude-str
[![crates.io](https://img.shields.io/crates/v/etude-str.svg)](https://crates.io/crates/etude-str) [![docs.rs](https://docs.rs/etude-str/badge.svg)](https://docs.rs/etude-str)

> A cheaply-clonable, `Bytes`-backed UTF-8 string.

Everywhere a value would otherwise be a `String` (or `Arc<str>`) — an id, a name, a reason, a
target — `Str` is the cheaper choice. Why: such text values are cloned constantly as they thread
through routing, dispatch, and results, and a `String` clone is an allocation + copy; a `Str`
clone is an O(1) `bytes::Bytes` refcount bump. It also gives text and bytes one representation,
so a value crosses the text/binary boundary without re-allocating.

It is a newtype over `bytes::Bytes`. Invariant: the wrapped `Bytes` is always valid UTF-8 —
every constructor establishes it, so `Str::as_str` is a zero-cost view.

## Features

- `serde` (off by default) — `Str` (de)serializes as a plain string.
- `bolero-generator` (off by default) — a derive-free `bolero_generator::TypeGenerator` for
  `Str` (property/fuzz testing), producing valid UTF-8 by construction.

Part of the [etude](https://github.com/camshaft/etude) workspace. See the
[API documentation on docs.rs](https://docs.rs/etude-str).

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](../../LICENSE)).
