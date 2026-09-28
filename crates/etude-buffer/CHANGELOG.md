# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/camshaft/etude/releases/tag/etude-buffer-v0.1.0) - 2026-09-28

### Other

- *(bench)* close out reader coverage — Chain drain + partial_copy_into trailing-chunk hand-off ([#293](https://github.com/camshaft/etude/pull/293))
- *(bench)* cover the IoSlice vectored drain — converges to the memcpy baseline as segments grow ([#292](https://github.com/camshaft/etude/pull/292))
- *(bench)* measure put_uninit_slice zero-init cost — redundant memset is material ([#267](https://github.com/camshaft/etude/pull/267))
- *(bench)* stand up the bench harness — copy_into copy vs zero-copy path ([#266](https://github.com/camshaft/etude/pull/266))
- put_uninit_slice commits stale heap on a no-op closure — uninit info-leak through a safe API ([#173](https://github.com/camshaft/etude/pull/173))
- document WriteOnce's advisory-by-design gate + pin the boundary ([#166](https://github.com/camshaft/etude/pull/166))
- *(buffer)* Chain order-preservation property under mixed drains ([#163](https://github.com/camshaft/etude/pull/163))
- drop all-caps emphasis from buffer/ensure/str ([#126](https://github.com/camshaft/etude/pull/126))
- real bounds/capacity checks in public writer::Buffer impls (fix release UB) ([#37](https://github.com/camshaft/etude/pull/37))
- *(buffer)* document every public item of etude-buffer ([#20](https://github.com/camshaft/etude/pull/20))
- drop etude-stream; offset tracking stays in s2n-quic-core
- split cursor traits into etude-stream; rename Storage->Buffer
- fix no_std, miri, caching; make kani advisory
- match s2n-quic patterns; restore the vectored_copy Kani proof
- adopt Rust edition 2024 and stop tracking Cargo.lock
- Initial commit: byte-pusher crate workspace
