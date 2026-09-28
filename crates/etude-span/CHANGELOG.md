# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/camshaft/etude/releases/tag/etude-span-v0.1.0) - 2026-09-28

### Other

- *(crates)* add keywords + categories for first crates.io publish ([#320](https://github.com/camshaft/etude/pull/320))
- *(span)* add usage doctests to Span and Cursor ([#312](https://github.com/camshaft/etude/pull/312))
- *(bench)* refresh tokenize/refill numbers post-#[inline] (#287 landed) — the naive-vs-bulk gap collapses ([#302](https://github.com/camshaft/etude/pull/302))
- *(perf)* #[inline] the Cursor hot path — tokenizer inner loop ~3x (−68%) ([#287](https://github.com/camshaft/etude/pull/287))
- *(bench)* isolate the leaf-boundary refill — ~5-6 ns/refill, flat across leaf size ([#291](https://github.com/camshaft/etude/pull/291))
- *(bench)* add the realistic tokenizer shape — bulk token-run scan vs naive peek/bump ([#289](https://github.com/camshaft/etude/pull/289))
- *(bench)* stand up the harness — Cursor scan vs the per-byte byte_at descent ([#285](https://github.com/camshaft/etude/pull/285))
- *(span)* mixed bump/skip walk differential — fence the bulk cursor path ([#132](https://github.com/camshaft/etude/pull/132))
- extract the chunk Cursor + Span into a reusable crate (operator direction) ([#101](https://github.com/camshaft/etude/pull/101))
