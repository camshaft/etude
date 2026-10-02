# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/camshaft/etude/compare/etude-rational-v0.1.0...etude-rational-v0.1.1) - 2026-10-02

### Fixed

- *(ci)* stop the nightly Miri run hanging past the 6h cap ([#339](https://github.com/camshaft/etude/pull/339))

### Other

- add crates.io + docs.rs badges to crate READMEs, CI + license to top README ([#326](https://github.com/camshaft/etude/pull/326))
- *(crates)* add per-crate README + readme manifest field for crates.io ([#324](https://github.com/camshaft/etude/pull/324))
- release v0.1.0 ([#322](https://github.com/camshaft/etude/pull/322))

## [0.1.0](https://github.com/camshaft/etude/releases/tag/etude-rational-v0.1.0) - 2026-09-28

### Other

- *(crates)* add keywords + categories for first crates.io publish ([#320](https://github.com/camshaft/etude/pull/320))
- *(rational)* add a usage doctest to Rational ([#309](https://github.com/camshaft/etude/pull/309))
- *(perf)* fix into_recip@64b co-regression — cold-extract the negative branch ([#301](https://github.com/camshaft/etude/pull/301))
- *(bench)* add_shared group — shared-factor add/sub coverage (wins every tier, 0.16-0.21x) ([#300](https://github.com/camshaft/etude/pull/300))
- *(perf)* zero-alloc in-place sign flips in into_neg/into_abs/into_recip ([#299](https://github.com/camshaft/etude/pull/299))
- *(perf)* box native-path results via Big::from_i128, not a byte round-trip ([#268](https://github.com/camshaft/etude/pull/268))
- *(perf)* consuming into_recip/into_neg/into_abs reuse owned allocations ([#274](https://github.com/camshaft/etude/pull/274))
- *(bench)* average the core binop board over 8 operand pairs per tier ([#282](https://github.com/camshaft/etude/pull/282))
- *(bench)* add cmp_close — a deterministic continued-fraction worst case ([#279](https://github.com/camshaft/etude/pull/279))
- *(rational)* drop remaining all-caps emphasis + fix a broken intra-doc link ([#232](https://github.com/camshaft/etude/pull/232))
- bank the rest of bigint #207 gcd ripple — full binop board refresh (mul/div 256b 0.39/0.41->0.20/0.21, 1024b ~0.43->0.28) ([#230](https://github.com/camshaft/etude/pull/230))
- fold Display-vs-num-rational parity into the differential oracle (anti-regression for #220) ([#225](https://github.com/camshaft/etude/pull/225))
- *(rational)* fmt escapee in the #220 Display-parity test ([#224](https://github.com/camshaft/etude/pull/224))
- native u64-band normalize — construction 64b 0.27x->0.13x (~1.9x) ([#222](https://github.com/camshaft/etude/pull/222))
- Display honors Formatter padding flags via pad_integral (drop-in parity with num-rational) ([#220](https://github.com/camshaft/etude/pull/220))
- bank bigint #207 in-place Stein gcd — normalize/add_eqden now win every tier ([#216](https://github.com/camshaft/etude/pull/216))
- native u128 mul/div for the 64b band — mul 0.41x->0.054x, div 0.36x->0.045x ([#211](https://github.com/camshaft/etude/pull/211))
- native u128 cmp for the 64b tier — cmp 64b 50.9ns -> 9.7ns (0.93x -> 0.18x) ([#206](https://github.com/camshaft/etude/pull/206))
- bank bigint #197 render ripple — to_string 1024b 1.27x -> 0.58x (last render loss closed) ([#200](https://github.com/camshaft/etude/pull/200))
- coprime fast path in native mul/div — skip redundant x/1 sdivs (mul_i64 -5.5%, div_i64 -7.2%) ([#198](https://github.com/camshaft/etude/pull/198))
- complete mixed-magnitude coverage with div_mixed (0.64x, no hidden loss) ([#193](https://github.com/camshaft/etude/pull/193))
- benchmark mixed-magnitude add/mul (small × large) — coverage, confirms no hidden loss ([#188](https://github.com/camshaft/etude/pull/188))
- native add/sub reduce over gcd(b,d) — add_i64/sub_i64 0.126x -> 0.050x ([#183](https://github.com/camshaft/etude/pull/183))
- native mul/div cross-reduce on i64 originals (u64 gcd) — mul_i64 0.107x -> 0.050x ([#178](https://github.com/camshaft/etude/pull/178))
- bank render crossing from bigint #169 (to_string 4096b 1.02x -> 0.78x) ([#174](https://github.com/camshaft/etude/pull/174))
- lower CMP_SMALL_BYTES 64 -> 16 — cmp 256b 0.65x -> 0.43x ([#172](https://github.com/camshaft/etude/pull/172))
- add/sub reduce over gcd(b,d) not gcd(num, b*d) — 1024b/4096b 0.77x/0.98x -> 0.26x ([#167](https://github.com/camshaft/etude/pull/167))
- continued-fraction cmp q∈{0,1} fast path — cmp 1024b 0.90x->0.44x, 4096b 0.73x->0.26x ([#158](https://github.com/camshaft/etude/pull/158))
- pre-size + direct-write to_decimal_string — render 64b 0.65x -> 0.36x ([#150](https://github.com/camshaft/etude/pull/150))
- cross_reduce_mul borrows via Cow — drop 4 needless Big clones per mul/div ([#148](https://github.com/camshaft/etude/pull/148))
- gcd_u64 fast path in gcd_u128 — from_ratio_i64 0.16x -> 0.103x ([#140](https://github.com/camshaft/etude/pull/140))
- native i128 normalize fast path — small-rational construction 0.73x -> 0.16x ([#138](https://github.com/camshaft/etude/pull/138))
- benchmark neg/abs (every-function coverage) + refresh render board post-bigint #127 ([#135](https://github.com/camshaft/etude/pull/135))
- borrow-first-iteration cmp — closes the last cmp loss (1024b 1.06x -> 0.90x) ([#130](https://github.com/camshaft/etude/pull/130))
- bank the 256b render crossing (to_string 1.36x -> 0.97x via bigint #115) ([#122](https://github.com/camshaft/etude/pull/122))
- *(rational)* drop all-caps words used for emphasis ([#121](https://github.com/camshaft/etude/pull/121))
- native i128 path for small equal-denominator add/sub + div_exact wiring ([#110](https://github.com/camshaft/etude/pull/110))
- native i128 cmp on i64-fitting operands — ~6x faster small (slice 13) ([#100](https://github.com/camshaft/etude/pull/100))
- extend native i128 fast path to add/sub — ~8x faster small (slice 12) ([#97](https://github.com/camshaft/etude/pull/97))
- native i128 fast path for mul/div on i64-fitting operands — ~9x faster small (slice 11) ([#94](https://github.com/camshaft/etude/pull/94))
- track render win (64b 0.66 via #82) + correct large-tier scaling analysis (slice 10) ([#87](https://github.com/camshaft/etude/pull/87))
- add Display impl + alloc-lean to_decimal_string via Big::write_decimal (slice 9) ([#83](https://github.com/camshaft/etude/pull/83))
- large-tier board (2048b/4096b) + hot-path allocation elision (slice 8) ([#74](https://github.com/camshaft/etude/pull/74))
- size-thresholded continued-fraction cmp hybrid — closes cmp@1024b (slice 7) ([#65](https://github.com/camshaft/etude/pull/65))
- cmp fast paths (sign + equal-denominator) + board refresh vs new Stein gcd (slice 6) ([#60](https://github.com/camshaft/etude/pull/60))
- mul/div cross-reduction — cancel gcd(a,d)+gcd(c,b) before multiplying, no final normalize (slice 5) ([#55](https://github.com/camshaft/etude/pull/55))
- *(bytevec)* [**breaking**] replace the flat deque with the tiered RRB rope (atomic rename) ([#54](https://github.com/camshaft/etude/pull/54))
- equal-denominator add/sub fast path + strengthened cmp tests (slice 4) ([#52](https://github.com/camshaft/etude/pull/52))
- gcd-free recip — O(limbs) swap+sign, not a full gcd-normalize (slice 3) ([#45](https://github.com/camshaft/etude/pull/45))
- criterion scoreboard vs num-rational + BENCHMARKS.md baseline (slice 2) ([#43](https://github.com/camshaft/etude/pull/43))
- faithful normalized Rational over etude-bigint, num-rational differential oracle wired first (#slice-1) ([#39](https://github.com/camshaft/etude/pull/39))
