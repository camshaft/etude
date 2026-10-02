# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/camshaft/etude/compare/etude-strrope-v0.1.0...etude-strrope-v0.1.1) - 2026-10-02

### Added

- *(str,strrope)* no_std support via a default `std` feature ([#329](https://github.com/camshaft/etude/pull/329))

### Other

- enable deny(missing_docs) on the last three crates ([#333](https://github.com/camshaft/etude/pull/333))
- add crates.io + docs.rs badges to crate READMEs, CI + license to top README ([#326](https://github.com/camshaft/etude/pull/326))
- *(crates)* add per-crate README + readme manifest field for crates.io ([#324](https://github.com/camshaft/etude/pull/324))
- release v0.1.0 ([#322](https://github.com/camshaft/etude/pull/322))

## [0.1.0](https://github.com/camshaft/etude/releases/tag/etude-strrope-v0.1.0) - 2026-09-28

### Other

- *(crates)* add keywords + categories for first crates.io publish ([#320](https://github.com/camshaft/etude/pull/320))
- *(strrope)* add a usage doctest to StrRope ([#311](https://github.com/camshaft/etude/pull/311))
- *(fmt)* wrap two multi-line assert_eq! to fix the --all fmt gate red on main ([#305](https://github.com/camshaft/etude/pull/305))
- specialize Chars::count() to a byte scan (chars().count() 202us -> 25us at depth) ([#288](https://github.com/camshaft/etude/pull/288))
- *(bench)* add chars_collect forward-decode bench + record a measured decode dead-end ([#290](https://github.com/camshaft/etude/pull/290))
- *(bench)* align the ops harness with the fleet recipe + add BENCHMARKS.md ([#276](https://github.com/camshaft/etude/pull/276))
- add unsafe from_utf8_unchecked (skip redundant O(n) UTF-8 re-validation) ([#213](https://github.com/camshaft/etude/pull/213))
- optimization pass — chunk-aligned Eq/Ord/Hash, alloc-free Display/Debug, bench (hold: awaiting operator review) ([#119](https://github.com/camshaft/etude/pull/119))
- *(str,strrope)* skip bolero fuzz property tests under miri (unblock the spinning miri run) ([#243](https://github.com/camshaft/etude/pull/243))
- *(strrope)* fence Ord between ropes on content pairs across layouts ([#201](https://github.com/camshaft/etude/pull/201))
- *(bytevec)* let consumer crates opt in to the invariant checker ([#189](https://github.com/camshaft/etude/pull/189))
- *(strrope)* hash fence must catch write-boundary leakage (aHash class) ([#179](https://github.com/camshaft/etude/pull/179))
- *(strrope)* pin Debug parity with str under every formatter flag ([#175](https://github.com/camshaft/etude/pull/175))
- StrRope Display ignores width/fill/precision — format parity with str broken ([#155](https://github.com/camshaft/etude/pull/155))
- *(strrope)* mutation-op differential harness vs a String model ([#128](https://github.com/camshaft/etude/pull/128))
- StrRope::slice bound +1 wraps in release — out-of-bounds range returns the wrong slice silently ([#114](https://github.com/camshaft/etude/pull/114))
- iteration (chars/char_indices/chunks) + structural ops (insert/split_off/slice) ([#111](https://github.com/camshaft/etude/pull/111))
- *(strrope)* differential fence vs the str oracle across chunk layouts ([#109](https://github.com/camshaft/etude/pull/109))
- new crate — StrRope core (UTF-8 string rope over etude-bytevec) ([#104](https://github.com/camshaft/etude/pull/104))
