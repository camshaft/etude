# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/camshaft/etude/compare/etude-bigint-v0.1.0...etude-bigint-v0.1.1) - 2026-10-02

### Added

- *(bigint)* impl Ord/PartialOrd for Big ([#328](https://github.com/camshaft/etude/pull/328))

### Other

- add crates.io + docs.rs badges to crate READMEs, CI + license to top README ([#326](https://github.com/camshaft/etude/pull/326))
- *(crates)* add per-crate README + readme manifest field for crates.io ([#324](https://github.com/camshaft/etude/pull/324))
- release v0.1.0 ([#322](https://github.com/camshaft/etude/pull/322))

## [0.1.0](https://github.com/camshaft/etude/releases/tag/etude-bigint-v0.1.0) - 2026-09-28

### Added

- *(bigint)* native-scalar accessors is_even/is_odd + rem_u64/divmod_u64 for downstream decimal ([#133](https://github.com/camshaft/etude/pull/133))
- *(bigint)* O(1) bit_len / byte_len size accessors ([#56](https://github.com/camshaft/etude/pull/56))
- *(bigint)* encapsulate the repr behind a stable API + add a num-bigint benchmark scoreboard ([#34](https://github.com/camshaft/etude/pull/34))
- *(bigint)* port the arbitrary-precision Big integer into a new etude-bigint crate ([#32](https://github.com/camshaft/etude/pull/32))

### Other

- *(crates)* add keywords + categories for first crates.io publish ([#320](https://github.com/camshaft/etude/pull/320))
- *(bigint)* add a usage doctest to Big ([#308](https://github.com/camshaft/etude/pull/308))
- *(bigint)* average the gcd bench over an 8-pair batch — kill the operand-value false-alarm cell ([#298](https://github.com/camshaft/etude/pull/298))
- *(bigint)* single-limb gcd fast path — gcd(wide, small) O(n) instead of O(bit_len·n) ([#295](https://github.com/camshaft/etude/pull/295))
- *(bigint)* format two decimal digits per step in to_decimal — every tier faster ([#271](https://github.com/camshaft/etude/pull/271))
- &mut-accumulator surface — add_assign/sub_assign (0.34–0.63× vs num-bigint in-place +=) ([#281](https://github.com/camshaft/etude/pull/281))
- *(bigint)* correct bench comments that claimed a nonexistent inline magnitude repr ([#278](https://github.com/camshaft/etude/pull/278))
- *(bigint)* skip the differential bolero harness under miri (it spins forever) ([#245](https://github.com/camshaft/etude/pull/245))
- *(bigint)* large-tier mul/divmod probe — measures the subquadratic crossover ([#217](https://github.com/camshaft/etude/pull/217))
- *(bigint)* add a direct abs vs num-bigint assertion — closes the last oracle gap ([#215](https://github.com/camshaft/etude/pull/215))
- *(bigint)* refresh BENCHMARKS scoreboard summary + clear emphasis-caps ([#212](https://github.com/camshaft/etude/pull/212))
- *(bigint)* subtract in place in the Stein gcd loop — all four gcd tiers now win ([#207](https://github.com/camshaft/etude/pull/207))
- *(bench)* extend gcd bench to 4096b, exposing the one remaining gcd gap ([#204](https://github.com/camshaft/etude/pull/204))
- last_decimal_digit via limb sum (3-7x faster than rem_u64(10) for decimal's canonicalize check) ([#202](https://github.com/camshaft/etude/pull/202))
- raise to_decimal recursive threshold 10 -> 64 (1024b/4096b cross to wins) ([#197](https://github.com/camshaft/etude/pull/197))
- *(docs)* lowercase emphasis-caps in published rustdoc (operator directive) ([#191](https://github.com/camshaft/etude/pull/191))
- limb-level from_i128 / to_i128_checked (unlocks decimal's i128 fast path) ([#187](https://github.com/camshaft/etude/pull/187))
- pre-reserve capacity in the byte serializers (halves the map-key encode) ([#186](https://github.com/camshaft/etude/pull/186))
- single-buffer back-to-front write_decimal_chunk ([#180](https://github.com/camshaft/etude/pull/180))
- skip the wasted top squaring in to_decimal_string (4096b now beats num-bigint) ([#169](https://github.com/camshaft/etude/pull/169))
- decimal_digit_count — exact base-10 digit count without rendering ([#165](https://github.com/camshaft/etude/pull/165))
- *(docs)* correct to_decimal gap-1 analysis — squarings are not the residual (measured) ([#157](https://github.com/camshaft/etude/pull/157))
- from_base_10_pow_k_limbs — build a Big from base-10^k decimal chunks ([#145](https://github.com/camshaft/etude/pull/145))
- *(bench)* backfill benches for every unbenched public function ([#141](https://github.com/camshaft/etude/pull/141))
- *(bigint)* reciprocal qhat in Knuth divmod — faster multi-limb divmod + recursive to_decimal ([#127](https://github.com/camshaft/etude/pull/127))
- *(bigint)* reciprocal single-limb div_rem_limb_inplace — n/small beats num-bigint every tier ([#123](https://github.com/camshaft/etude/pull/123))
- *(bigint)* reciprocal ÷10^19 in to_decimal peel — no u128 divide libcall (256b 1.95x->1.38x) ([#115](https://github.com/camshaft/etude/pull/115))
- *(bigint)* branchless u64 sub_mag — closes sub@4096b (1.20x -> 1.02x) ([#112](https://github.com/camshaft/etude/pull/112))
- *(bigint)* wide-operand gcd differential coverage (planted factors, unbalanced widths) ([#107](https://github.com/camshaft/etude/pull/107))
- *(bigint)* quotient-only div_exact — skip the discarded-remainder alloc (64b 0.67x num-bigint) ([#99](https://github.com/camshaft/etude/pull/99))
- *(bigint)* add clone + from_i64 scoreboard benches — document the 64b small-value gap ([#96](https://github.com/camshaft/etude/pull/96))
- *(bigint)* fmt escapee in write_decimal's chunk-pad selection ([#92](https://github.com/camshaft/etude/pull/92))
- *(bigint)* stack-scratch linear to_decimal — alloc-free 2..=10-limb render (256b 2.19x->1.95x) ([#85](https://github.com/camshaft/etude/pull/85))
- *(bigint)* single-limb to_decimal fast path — 64b crosses num-bigint (2.06x -> 0.86x) ([#82](https://github.com/camshaft/etude/pull/82))
- *(bigint)* sink-writing write_decimal — alloc-free decimal render + faster to_decimal_string ([#79](https://github.com/camshaft/etude/pull/79))
- *(bigint)* recursive divide-and-conquer to_decimal_string above a 10-limb crossover ([#75](https://github.com/camshaft/etude/pull/75))
- *(bigint)* in-place divide-by-limb in to_decimal_string — no per-chunk quotient alloc ([#69](https://github.com/camshaft/etude/pull/69))
- *(bigint)* direct signed subtract — no negated-magnitude clone (sub beats num-bigint at 256b/1024b) ([#61](https://github.com/camshaft/etude/pull/61))
- *(bytevec)* [**breaking**] replace the flat deque with the tiered RRB rope (atomic rename) ([#54](https://github.com/camshaft/etude/pull/54))
- *(bigint)* Karatsuba multiply above a 40-limb crossover — mul/4096b 1.24x -> 1.03x ([#53](https://github.com/camshaft/etude/pull/53))
- *(bigint)* binary (Stein) GCD — now beats/matches num-bigint (was 2.6x slower) ([#51](https://github.com/camshaft/etude/pull/51))
- *(bigint)* wasm multiply measurement — confirms the per-target wide_mul gate ([#50](https://github.com/camshaft/etude/pull/50))
- *(bigint)* chunked to_decimal_string (÷10^19, 19 digits/step) — 6-11x faster ([#48](https://github.com/camshaft/etude/pull/48))
- *(bigint)* u128-free widening multiply on wasm (synthesized from u32 halves), keep u64 limbs ([#44](https://github.com/camshaft/etude/pull/44))
- *(bigint)* Knuth Algorithm D divmod — now ~2x FASTER than num-bigint (was 7-20x slower) ([#41](https://github.com/camshaft/etude/pull/41))
- *(bigint)* adopt base-2^64 u64 limbs (u128 intermediates) — beats num-bigint on add/mul/cmp ([#36](https://github.com/camshaft/etude/pull/36))
