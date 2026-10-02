# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/camshaft/etude/compare/etude-str-v0.1.0...etude-str-v0.1.1) - 2026-10-02

### Added

- *(str,strrope)* no_std support via a default `std` feature ([#329](https://github.com/camshaft/etude/pull/329))

### Other

- enable deny(missing_docs) on the last three crates ([#333](https://github.com/camshaft/etude/pull/333))
- add crates.io + docs.rs badges to crate READMEs, CI + license to top README ([#326](https://github.com/camshaft/etude/pull/326))
- *(crates)* add per-crate README + readme manifest field for crates.io ([#324](https://github.com/camshaft/etude/pull/324))
- release v0.1.0 ([#322](https://github.com/camshaft/etude/pull/322))

## [0.1.0](https://github.com/camshaft/etude/releases/tag/etude-str-v0.1.0) - 2026-09-28

### Other

- *(crates)* add keywords + categories for first crates.io publish ([#320](https://github.com/camshaft/etude/pull/320))
- *(str)* add a usage doctest to Str ([#307](https://github.com/camshaft/etude/pull/307))
- *(str,strrope)* skip bolero fuzz property tests under miri (unblock the spinning miri run) ([#243](https://github.com/camshaft/etude/pull/243))
- *(str)* cover the from_utf8_unchecked unsafe surface (miri) ([#218](https://github.com/camshaft/etude/pull/218))
- Str Display ignores width/fill/precision — format parity with str broken (third #155-class occurrence) ([#210](https://github.com/camshaft/etude/pull/210))
- drop all-caps emphasis from buffer/ensure/str ([#126](https://github.com/camshaft/etude/pull/126))
- drop the etude-buffer reader dependency and its integration ([#22](https://github.com/camshaft/etude/pull/22))
- impl etude_buffer::reader::Buffer for Str (Str as a readable byte source) ([#15](https://github.com/camshaft/etude/pull/15))
- make Str::from_reader genuinely zero-copy on the single-chunk path ([#11](https://github.com/camshaft/etude/pull/11))
- add a `buffer` feature — read a `Str` out of an etude-buffer reader ([#8](https://github.com/camshaft/etude/pull/8))
- add a `bolero-generator` feature with a UTF-8-safe TypeGenerator ([#5](https://github.com/camshaft/etude/pull/5))
- Add etude-str: a cheaply-clonable, Bytes-backed UTF-8 string ([#3](https://github.com/camshaft/etude/pull/3))
