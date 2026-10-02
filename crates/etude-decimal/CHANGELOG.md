# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/camshaft/etude/compare/etude-decimal-v0.1.0...etude-decimal-v0.1.1) - 2026-10-02

### Other

- *(decimal)* add an exact-arithmetic example ([#332](https://github.com/camshaft/etude/pull/332))
- add crates.io + docs.rs badges to crate READMEs, CI + license to top README ([#326](https://github.com/camshaft/etude/pull/326))
- *(crates)* add per-crate README + readme manifest field for crates.io ([#324](https://github.com/camshaft/etude/pull/324))
- release v0.1.0 ([#322](https://github.com/camshaft/etude/pull/322))

## [0.1.0](https://github.com/camshaft/etude/releases/tag/etude-decimal-v0.1.0) - 2026-09-28

### Added

- *(decimal)* checked integer extraction to_i64 / to_u64 / to_i128 ([#192](https://github.com/camshaft/etude/pull/192))

### Fixed

- *(decimal)* Display honors padding flags via pad_integral (match etude-rational #220) ([#221](https://github.com/camshaft/etude/pull/221))

### Other

- *(crates)* add keywords + categories for first crates.io publish ([#320](https://github.com/camshaft/etude/pull/320))
- *(decimal)* add a usage doctest to Decimal ([#310](https://github.com/camshaft/etude/pull/310))
- *(test)* close the div differential-oracle gap (independent Some cross-check + None false-reject guard) ([#304](https://github.com/camshaft/etude/pull/304))
- *(test)* add div to the differential oracle vs bigdecimal (closes the div coverage gap) ([#303](https://github.com/camshaft/etude/pull/303))
- *(bench)* bank the div_exact collapse from etude-bigint #295 (single-limb gcd fast path) ([#297](https://github.com/camshaft/etude/pull/297))
- *(bench)* bank the to_string lift from etude-bigint #271 (two decimal digits per to_decimal step) ([#296](https://github.com/camshaft/etude/pull/296))
- *(decimal)* native u128 to_f64 fast path (64b 643ns -> 38ns, 3.39x -> 0.20x) ([#270](https://github.com/camshaft/etude/pull/270))
- *(decimal)* rotate div_round/div_exact over an operand batch per tier (scoreboard honesty) ([#283](https://github.com/camshaft/etude/pull/283))
- *(decimal)* skip the 5 bolero property harnesses under miri (they spin forever) ([#244](https://github.com/camshaft/etude/pull/244))
- *(decimal)* fix broken pad_integral intra-doc link + lowercase one emphasis-cap ([#239](https://github.com/camshaft/etude/pull/239))
- *(decimal)* fuzz parse/parse_prefix no-panic on raw arbitrary bytes ([#238](https://github.com/camshaft/etude/pull/238))
- *(decimal)* refresh add/sub scoreboard — record #226 native-strip flow-through (sub 1024b crosses to a win) ([#235](https://github.com/camshaft/etude/pull/235))
- *(decimal)* native i128 exact-division fast path (div_exact 64b -76%) ([#231](https://github.com/camshaft/etude/pull/231))
- *(decimal)* pin native i64/i128/u128 arithmetic tier boundaries ([#229](https://github.com/camshaft/etude/pull/229))
- *(decimal)* native u128 magnitude tier for mul 64b (products up to 2^128) ([#228](https://github.com/camshaft/etude/pull/228))
- *(decimal)* native i128 trailing-zero strip in normalize for small values ([#226](https://github.com/camshaft/etude/pull/226))
- *(decimal)* document Display ignores format flags (canonical render), pin with a test ([#219](https://github.com/camshaft/etude/pull/219))
- *(decimal)* native i128 scale-and-compare for small unequal-scale cmp ([#209](https://github.com/camshaft/etude/pull/209))
- *(decimal)* normalize divisibility check via last_decimal_digit (bigint #202) ([#205](https://github.com/camshaft/etude/pull/205))
- *(decimal)* refresh to_string scoreboard — 1024b crosses to a win after bigint #197 ([#203](https://github.com/camshaft/etude/pull/203))
- *(decimal)* native i128 tier for mul on i64-coefficient / i128-product ([#199](https://github.com/camshaft/etude/pull/199))
- *(decimal)* normalize strips via one rem_u64(10^9) peek + a single sized divmod ([#196](https://github.com/camshaft/etude/pull/196))
- *(decimal)* native i128 tier for add/sub on full-64-bit coefficients (add 64b 1.11->0.83, sub 1.28->0.99) ([#190](https://github.com/camshaft/etude/pull/190))
- *(decimal)* native i64 fast path for add/sub/mul on small values ([#182](https://github.com/camshaft/etude/pull/182))
- *(decimal)* bound cmp_uneq adjusted exponent from bit_len (flat ~25ns at scale) ([#177](https://github.com/camshaft/etude/pull/177))
- *(decimal)* to_string streams via write_decimal; split wide fractions (wins at scale) ([#176](https://github.com/camshaft/etude/pull/176))
- *(decimal)* cmp magnitude compare without abs clones (flat ~6ns, near parity) ([#170](https://github.com/camshaft/etude/pull/170))
- *(decimal)* cmp unequal-exponent path via decimal_digit_count (no render) ([#168](https://github.com/camshaft/etude/pull/168))
- *(decimal)* to_f64 fast path for small values (single correctly-rounded IEEE op) ([#162](https://github.com/camshaft/etude/pull/162))
- *(decimal)* subtract directly in sub instead of add(neg) ([#154](https://github.com/camshaft/etude/pull/154))
- *(decimal)* assemble the parsed coefficient via Big::from_base_10_pow_k_limbs ([#149](https://github.com/camshaft/etude/pull/149))
- *(decimal)* backfill neg/abs/to_f64/div/new (every-real-work-fn coverage) ([#146](https://github.com/camshaft/etude/pull/146))
- *(decimal)* normalize on etude-bigint scalar primitives (is_odd/rem_u64/divmod_u64) ([#143](https://github.com/camshaft/etude/pull/143))
- *(decimal)* drop all-caps words used for emphasis ([#131](https://github.com/camshaft/etude/pull/131))
- *(decimal)* strip trailing zeros in base-10^9 chunks in normalize ([#129](https://github.com/camshaft/etude/pull/129))
- *(decimal)* equal-exponent cmp fast path (Big::cmp, no decimal strings) ([#120](https://github.com/camshaft/etude/pull/120))
- *(decimal)* criterion scoreboard vs bigdecimal + BENCHMARKS.md baseline ([#116](https://github.com/camshaft/etude/pull/116))
- division — exact `div` (Option) + `div_round` with explicit precision/rounding ([#102](https://github.com/camshaft/etude/pull/102))
- consolidate to ONE chunk-cursor parser; the crate owns number-literal parsing ([#95](https://github.com/camshaft/etude/pull/95))
- convert to f64 directly (correctly rounded), not via a string round-trip ([#89](https://github.com/camshaft/etude/pull/89))
- construct-from-validated-components API + de-reference JSON in docs ([#84](https://github.com/camshaft/etude/pull/84))
- *(decimal)* describe decimal on its own terms, not as a JSON decoder ([#81](https://github.com/camshaft/etude/pull/81))
- render into a fmt::Write sink instead of allocating a String (Display) ([#78](https://github.com/camshaft/etude/pull/78))
- exact arithmetic add/sub/mul + widen exponent to i64 (slice 2) ([#72](https://github.com/camshaft/etude/pull/72))
- new exact base-10 decimal crate — repr, canonicalization, lossless JSON-number parse (slice 1) ([#64](https://github.com/camshaft/etude/pull/64))
