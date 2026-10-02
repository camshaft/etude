# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/camshaft/etude/compare/etude-bytevec-v0.1.0...etude-bytevec-v0.1.1) - 2026-10-02

### Added

- *(bytevec)* add ByteVec::copy_from_slice for borrowed slices ([#337](https://github.com/camshaft/etude/pull/337))
- *(bytevec)* add ByteVec::insert(at, chunk) binary byte-offset insert ([#336](https://github.com/camshaft/etude/pull/336))

### Other

- *(bytevec)* Miri-scale split_matches_oracle + uc5_brute_force (the last two hogs) ([#344](https://github.com/camshaft/etude/pull/344))
- *(bytevec)* Miri-scale the deep_rope oracle/stress tests so nightly Miri completes ([#342](https://github.com/camshaft/etude/pull/342))
- *(bytevec)* make the scale-heavy rope tests Miri-aware so nightly Miri completes ([#341](https://github.com/camshaft/etude/pull/341))
- *(bytevec)* add small_buffer group for the tiny-leaf construct path ([#338](https://github.com/camshaft/etude/pull/338))
- *(bytevec)* add a structural-sharing example ([#335](https://github.com/camshaft/etude/pull/335))
- *(bytevec)* drop "efficient(ly)" filler per the doc quality bar ([#334](https://github.com/camshaft/etude/pull/334))
- add crates.io + docs.rs badges to crate READMEs, CI + license to top README ([#326](https://github.com/camshaft/etude/pull/326))
- *(crates)* add per-crate README + readme manifest field for crates.io ([#324](https://github.com/camshaft/etude/pull/324))
- release v0.1.0 ([#322](https://github.com/camshaft/etude/pull/322))

## [0.1.0](https://github.com/camshaft/etude/releases/tag/etude-bytevec-v0.1.0) - 2026-09-28

### Added

- *(bytevec)* generic freeze hook on Builder for zeroize-on-drop chunks ([#313](https://github.com/camshaft/etude/pull/313))
- *(bytevec)* chunk-aware starts_with / ends_with (literal prefix/suffix), no linearization ([#58](https://github.com/camshaft/etude/pull/58))
- *(bytevec)* runtime inline threshold for Builder (replaces SPECIALIZES)

### Other

- *(crates)* add keywords + categories for first crates.io publish ([#320](https://github.com/camshaft/etude/pull/320))
- Revert "refactor(bytevec): make Behavior a runtime field, not a type parameter ([#317](https://github.com/camshaft/etude/pull/317))" ([#318](https://github.com/camshaft/etude/pull/318))
- *(bytevec)* make Behavior a runtime field, not a type parameter ([#317](https://github.com/camshaft/etude/pull/317))
- *(bytevec)* pin Behavior invariants (post-buffer with_behavior, ZST default) ([#316](https://github.com/camshaft/etude/pull/316))
- *(bytevec)* broaden Freeze into a Builder Behavior trait ([#315](https://github.com/camshaft/etude/pull/315))
- *(bytevec)* parameterize Builder over a Freeze behavior trait ([#314](https://github.com/camshaft/etude/pull/314))
- *(perf)* split maybe_demote so the pop hot path doesn't inline the flatten body ([#250](https://github.com/camshaft/etude/pull/250))
- *(perf)* append a small rope by pushing its chunks, not a tree concat — fold 58->41us ([#263](https://github.com/camshaft/etude/pull/263))
- *(bench)* add split_to cost-by-position workload ([#265](https://github.com/camshaft/etude/pull/265))
- *(bench)* add chunk-size distribution workload (many-tiny vs few-huge) ([#264](https://github.com/camshaft/etude/pull/264))
- *(bench)* add mixed read/write interleave workload ([#262](https://github.com/camshaft/etude/pull/262))
- *(bench)* add sequential-vs-random point-access workload ([#261](https://github.com/camshaft/etude/pull/261))
- *(bytevec)* fence the Extend<Bytes> branch surface (bulk vs loop, and the no-overwrite guard) ([#260](https://github.com/camshaft/etude/pull/260))
- *(bytevec)* fuzz layout-independent rope-vs-rope equality (chunks_content_eq) ([#259](https://github.com/camshaft/etude/pull/259))
- *(bench)* add promote/demote amplitude sweep — pins the hysteresis band edge ([#258](https://github.com/camshaft/etude/pull/258))
- *(bench)* add bulk-vs-incremental build crossover sweep ([#257](https://github.com/camshaft/etude/pull/257))
- *(bench)* add reader fan-out / broadcast workload — cheap fork wins 13-26x ([#256](https://github.com/camshaft/etude/pull/256))
- *(bench)* add interleaved streaming FIFO + boundary-churn workloads ([#254](https://github.com/camshaft/etude/pull/254))
- *(docs)* record the harvested-wins status + the truncate re-sum dead end in BENCHMARKS.md ([#253](https://github.com/camshaft/etude/pull/253))
- *(perf)* read a Small source through a borrowing cursor — reader() fork 495ns → ~4.7ns ([#252](https://github.com/camshaft/etude/pull/252))
- *(perf)* extend into an empty rope uses the from_iter bulk build ([#251](https://github.com/camshaft/etude/pull/251))
- add compact() + CompactionConfig (collapse to contiguous storage) ([#249](https://github.com/camshaft/etude/pull/249))
- add unsafe Rope<Utf8>::from_bytes_unchecked (O(1) unchecked twin of try_from_bytes) ([#208](https://github.com/camshaft/etude/pull/208))
- *(bytevec)* skip the 4 bolero property harnesses under miri (they spin forever) ([#242](https://github.com/camshaft/etude/pull/242))
- document the Reader read-path performance characteristic ([#240](https://github.com/camshaft/etude/pull/240))
- *(bytevec)* fuzz the non-consuming Reader drain across watermarks and tiers ([#234](https://github.com/camshaft/etude/pull/234))
- *(bench)* benchmark the non-destructive Reader read path ([#233](https://github.com/camshaft/etude/pull/233))
- *(bytevec)* negative control — check_invariants must panic on corruption ([#227](https://github.com/camshaft/etude/pull/227))
- *(bytevec)* pin exact tier-transition boundaries and promote/demote hysteresis ([#223](https://github.com/camshaft/etude/pull/223))
- *(bench)* benchmark the Builder write path (put_slice / put_bytes) ([#214](https://github.com/camshaft/etude/pull/214))
- *(bench)* benchmark split_to_copy and clear vs the naive deque ([#195](https://github.com/camshaft/etude/pull/195))
- *(bytevec)* let consumer crates opt in to the invariant checker ([#189](https://github.com/camshaft/etude/pull/189))
- *(bench)* benchmark Rope<Utf8> append_bytes/insert_bytes vs std::String ([#185](https://github.com/camshaft/etude/pull/185))
- *(bytevec)* measure for_socket_read put_uninit_slice zero-init memset (gates deferred reclaim) ([#181](https://github.com/camshaft/etude/pull/181))
- *(bytevec)* make copy_to_bytes_mut single-chunk path actually zero-copy (reclaim, not copy) ([#164](https://github.com/camshaft/etude/pull/164))
- *(buffer)* Chain order-preservation property under mixed drains ([#163](https://github.com/camshaft/etude/pull/163))
- *(bytevec)* fence the std::io adapters and copy_to_bytes_mut in the harness ([#160](https://github.com/camshaft/etude/pull/160))
- document partial_copy_into's maximal-run ordering invariant (+ debug_assert) ([#159](https://github.com/camshaft/etude/pull/159))
- add From<String>/From<&str> for Rope<Utf8> (scanless typed constructors) ([#153](https://github.com/camshaft/etude/pull/153))
- *(bytevec)* fuzz Builder::partial_copy_into ordering with bounded dests ([#151](https://github.com/camshaft/etude/pull/151))
- *(bytevec)* budget-conservation harness for the tagging machinery ([#144](https://github.com/camshaft/etude/pull/144))
- *(bytevec)* backfill starts_with/ends_with/try_from_bytes benches (differential vs contiguous &[u8]) ([#142](https://github.com/camshaft/etude/pull/142))
- *(bytevec)* close three fuzz-blind Builder surfaces in the differential ([#139](https://github.com/camshaft/etude/pull/139))
- *(bytevec)* drop all-caps emphasis from rustdoc/comments per the operator convention ([#134](https://github.com/camshaft/etude/pull/134))
- *(bytevec)* AsContiguousCheck harness op — a Some view is the whole content ([#125](https://github.com/camshaft/etude/pull/125))
- add Rope::as_contiguous() -> Option<&[u8]> (zero-cost borrowed contiguous view) ([#118](https://github.com/camshaft/etude/pull/118))
- stream UTF-8 validation in try_from_bytes (no full-content alloc on the valid path) ([#113](https://github.com/camshaft/etude/pull/113))
- gate arbitrary-byte mutators behind kind::Mutable (Rope<Utf8> can't be corrupted) ([#106](https://github.com/camshaft/etude/pull/106))
- *(bytevec)* pin FromIterator against empty chunks and lying size_hints ([#103](https://github.com/camshaft/etude/pull/103))
- add Rope<Utf8> kind + ByteVec<->Utf8 conversions + trusted mutators ([#98](https://github.com/camshaft/etude/pull/98))
- introduce generic Rope<K> core; ByteVec = Rope<kind::Bytes> ([#93](https://github.com/camshaft/etude/pull/93))
- *(bytevec)* fuzz-chosen front/back interleave op for the double-ended chunk iterator ([#90](https://github.com/camshaft/etude/pull/90))
- make chunks() a DoubleEndedIterator; drop the separate chunks_rev ([#70](https://github.com/camshaft/etude/pull/70)) ([#88](https://github.com/camshaft/etude/pull/88))
- *(bytevec)* RevChunksCheck harness op — chunks_rev is the exact reverse on every shape ([#76](https://github.com/camshaft/etude/pull/76))
- *(bytevec)* add the current live scoreboard (rope vs naive VecDeque) to BENCHMARKS.md ([#73](https://github.com/camshaft/etude/pull/73))
- *(bytevec)* O(suffix) ends_with via a reverse chunk iterator (was O(len)) ([#70](https://github.com/camshaft/etude/pull/70))
- *(bytevec)* rustfmt the restored compare bench (unbreak the fmt cron) ([#71](https://github.com/camshaft/etude/pull/71))
- *(bytevec)* harness op for the chunk-aware starts_with/ends_with ([#58](https://github.com/camshaft/etude/pull/58)) ([#66](https://github.com/camshaft/etude/pull/66))
- *(bytevec)* restore a runnable scoreboard vs a naive chunk deque ([#63](https://github.com/camshaft/etude/pull/63))
- *(bytevec)* re-apply the breaker harness extension + fences onto the renamed crate ([#57](https://github.com/camshaft/etude/pull/57))
- *(bytevec)* [**breaking**] replace the flat deque with the tiered RRB rope (atomic rename) ([#54](https://github.com/camshaft/etude/pull/54))
- *(byterope)* RED — static-tag Handle charges/releases a stale length; owner budget leaks on grow-then-drop (bytevec too) ([#35](https://github.com/camshaft/etude/pull/35))
- *(bytevec)* document every public item of etude-bytevec ([#25](https://github.com/camshaft/etude/pull/25))
- split cursor traits into etude-stream; rename Storage->Buffer
- adopt Rust edition 2024 and stop tracking Cargo.lock
- Initial commit: byte-pusher crate workspace
