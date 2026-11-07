// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Benchmark: versioned serialize_at/deserialize_at vs non-versioned baseline.
//!
//! ```sh
//! cargo bench -p amzn-upstream-wire --bench serialize_at
//! ```
//!
//! 5 levels × 5 versions of bool-only fields (3,125 cross-product entries).
//! Non-versioned baseline uses identical struct shape without version tags.
//!
//! After the benchmark runs, a summary table is printed to stdout showing
//! latency and overhead comparisons.
//!
//! ```text
//! Versioned:                       Non-versioned:
//! T1: Base (5v) →     5 xp        P1: Base → 1
//! T2:      (5v) →    25 xp        P2:      → 1
//! T3:      (5v) →   125 xp        P3:      → 1
//! T4:      (5v) →   625 xp        P4:      → 1
//! T5:      (5v) → 3,125 xp        P5:      → 1
//! ```

use amzn_upstream_wire::{Deserialize, Serialize, Wire};
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use upstream_bytevec::Builder;

// ── Versioned types: 5 levels × 5 versions, bool-only fields ─────

#[derive(Debug, Clone, PartialEq, amzn_upstream_wire::Wire)]
#[wire(crate = amzn_upstream_wire)]
struct T1 {
    #[wire(tag = 1.1)]
    a: bool,
    #[wire(tag = 2.1, default = false)]
    b: bool,
    #[wire(tag = 3.1, default = false)]
    c: bool,
    #[wire(tag = 4.1, default = false)]
    d: bool,
    #[wire(tag = 5.1, default = false)]
    e: bool,
}

#[derive(Debug, Clone, PartialEq, amzn_upstream_wire::Wire)]
#[wire(crate = amzn_upstream_wire)]
struct T2 {
    #[wire(tag = 1.1)]
    inner: T1,
    #[wire(tag = 2.1, default = false)]
    b: bool,
    #[wire(tag = 3.1, default = false)]
    c: bool,
    #[wire(tag = 4.1, default = false)]
    d: bool,
    #[wire(tag = 5.1, default = false)]
    e: bool,
}

#[derive(Debug, Clone, PartialEq, amzn_upstream_wire::Wire)]
#[wire(crate = amzn_upstream_wire)]
struct T3 {
    #[wire(tag = 1.1)]
    inner: T2,
    #[wire(tag = 2.1, default = false)]
    b: bool,
    #[wire(tag = 3.1, default = false)]
    c: bool,
    #[wire(tag = 4.1, default = false)]
    d: bool,
    #[wire(tag = 5.1, default = false)]
    e: bool,
}

#[derive(Debug, Clone, PartialEq, amzn_upstream_wire::Wire)]
#[wire(crate = amzn_upstream_wire)]
struct T4 {
    #[wire(tag = 1.1)]
    inner: T3,
    #[wire(tag = 2.1, default = false)]
    b: bool,
    #[wire(tag = 3.1, default = false)]
    c: bool,
    #[wire(tag = 4.1, default = false)]
    d: bool,
    #[wire(tag = 5.1, default = false)]
    e: bool,
}

#[derive(Debug, Clone, PartialEq, amzn_upstream_wire::Wire)]
#[wire(crate = amzn_upstream_wire)]
struct T5 {
    #[wire(tag = 1.1)]
    inner: T4,
    #[wire(tag = 2.1, default = false)]
    b: bool,
    #[wire(tag = 3.1, default = false)]
    c: bool,
    #[wire(tag = 4.1, default = false)]
    d: bool,
    #[wire(tag = 5.1, default = false)]
    e: bool,
}

// ── Non-versioned baseline: same shape, no version tags ──────────

#[derive(Debug, Clone, PartialEq, amzn_upstream_wire::Wire)]
#[wire(crate = amzn_upstream_wire)]
struct P1 {
    a: bool,
    b: bool,
    c: bool,
    d: bool,
    e: bool,
}

#[derive(Debug, Clone, PartialEq, amzn_upstream_wire::Wire)]
#[wire(crate = amzn_upstream_wire)]
struct P2 {
    inner: P1,
    b: bool,
    c: bool,
    d: bool,
    e: bool,
}

#[derive(Debug, Clone, PartialEq, amzn_upstream_wire::Wire)]
#[wire(crate = amzn_upstream_wire)]
struct P3 {
    inner: P2,
    b: bool,
    c: bool,
    d: bool,
    e: bool,
}

#[derive(Debug, Clone, PartialEq, amzn_upstream_wire::Wire)]
#[wire(crate = amzn_upstream_wire)]
struct P4 {
    inner: P3,
    b: bool,
    c: bool,
    d: bool,
    e: bool,
}

#[derive(Debug, Clone, PartialEq, amzn_upstream_wire::Wire)]
#[wire(crate = amzn_upstream_wire)]
struct P5 {
    inner: P4,
    b: bool,
    c: bool,
    d: bool,
    e: bool,
}

// ── Value constructors ───────────────────────────────────────────

fn make_versioned() -> T5 {
    T5 {
        inner: T4 {
            inner: T3 {
                inner: T2 {
                    inner: T1 {
                        a: true,
                        b: true,
                        c: true,
                        d: true,
                        e: true,
                    },
                    b: true,
                    c: true,
                    d: true,
                    e: true,
                },
                b: true,
                c: true,
                d: true,
                e: true,
            },
            b: true,
            c: true,
            d: true,
            e: true,
        },
        b: true,
        c: true,
        d: true,
        e: true,
    }
}

fn make_plain() -> P5 {
    P5 {
        inner: P4 {
            inner: P3 {
                inner: P2 {
                    inner: P1 {
                        a: true,
                        b: true,
                        c: true,
                        d: true,
                        e: true,
                    },
                    b: true,
                    c: true,
                    d: true,
                    e: true,
                },
                b: true,
                c: true,
                d: true,
                e: true,
            },
            b: true,
            c: true,
            d: true,
            e: true,
        },
        b: true,
        c: true,
        d: true,
        e: true,
    }
}

// ── Round-trip correctness check ─────────────────────────────────

fn assert_round_trip() {
    let value = make_versioned();
    let xp_count = T5::VERSION_XPRODUCT_COUNT;
    assert_eq!(xp_count, 3125, "expected 5^5 = 3,125 cross-product entries");

    for idx in 0..xp_count as u32 {
        let mut builder = Builder::default();
        Serialize::serialize_at(value.clone(), &mut builder, idx);
        let mut bytes = builder.finish();
        let result = T5::deserialize_at(&mut bytes, idx)
            .unwrap_or_else(|e| panic!("deserialize_at failed at idx {idx}: {e}"));

        if idx == xp_count as u32 - 1 {
            assert_eq!(result, value, "round-trip mismatch at latest idx {idx}");
        }
    }
}

// ── Benchmarks ───────────────────────────────────────────────────

/// Use a true compile-time constant so LLVM can fold the entire dispatch chain.
/// A `let` binding (even from a const expression) enters the inlined function as
/// a register value; `const` gives LLVM a literal it can propagate through all
/// if-else branches and div/mod arithmetic.
const LATEST_IDX: u32 = (T5::VERSION_XPRODUCT_COUNT - 1) as u32;

fn bench_serialize(c: &mut Criterion) {
    assert_round_trip();

    let versioned = make_versioned();
    let plain = make_plain();
    let mut group = c.benchmark_group("serialize");

    group.bench_function("plain", |b| {
        b.iter(|| {
            let mut builder = Builder::default();
            Serialize::serialize(black_box(plain.clone()), &mut builder);
            black_box(builder.finish());
        });
    });

    group.bench_function("versioned", |b| {
        b.iter(|| {
            let mut builder = Builder::default();
            Serialize::serialize_at(black_box(versioned.clone()), &mut builder, LATEST_IDX);
            black_box(builder.finish());
        });
    });

    group.finish();
}

fn bench_deserialize(c: &mut Criterion) {
    let versioned = make_versioned();
    let plain = make_plain();

    let mut b1 = Builder::default();
    Serialize::serialize_at(versioned, &mut b1, LATEST_IDX);
    let versioned_bytes = b1.finish();

    let mut b2 = Builder::default();
    Serialize::serialize(plain, &mut b2);
    let plain_bytes = b2.finish();

    let mut group = c.benchmark_group("deserialize");

    group.bench_function("plain", |b| {
        b.iter(|| {
            let mut bytes = plain_bytes.clone();
            let result = P5::deserialize(&mut bytes).unwrap();
            black_box(result);
        });
    });

    group.bench_function("versioned", |b| {
        b.iter(|| {
            let mut bytes = versioned_bytes.clone();
            let result = T5::deserialize_at(&mut bytes, LATEST_IDX).unwrap();
            black_box(result);
        });
    });

    group.finish();
}

fn bench_round_trip(c: &mut Criterion) {
    let versioned = make_versioned();
    let plain = make_plain();
    let mut group = c.benchmark_group("round_trip");

    group.bench_function("plain", |b| {
        b.iter(|| {
            let mut builder = Builder::default();
            Serialize::serialize(black_box(plain.clone()), &mut builder);
            let mut bytes = builder.finish();
            let result = P5::deserialize(&mut bytes).unwrap();
            black_box(result);
        });
    });

    group.bench_function("versioned", |b| {
        b.iter(|| {
            let mut builder = Builder::default();
            Serialize::serialize_at(black_box(versioned.clone()), &mut builder, LATEST_IDX);
            let mut bytes = builder.finish();
            let result = T5::deserialize_at(&mut bytes, LATEST_IDX).unwrap();
            black_box(result);
        });
    });

    group.finish();
}

fn print_summary(_c: &mut Criterion) {
    // Read criterion's estimates.json files written by previous benchmark groups.
    // CARGO_MANIFEST_DIR is core/core-wire; the target dir is ../../build/private/cargo-target.
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let base = manifest_dir
        .join("../../build/private/cargo-target/criterion")
        .canonicalize()
        .unwrap_or_else(|_| manifest_dir.join("../../build/private/cargo-target/criterion"));

    let benchmarks = [
        ("serialize", "plain", "versioned"),
        ("deserialize", "plain", "versioned"),
        ("round_trip", "plain", "versioned"),
    ];

    eprintln!();
    eprintln!(
        "╔══════════════╦═══════════════════╦═══════════════════╦══════════╦════════════════╗"
    );
    eprintln!(
        "║ Operation    ║ plain (baseline)  ║ versioned         ║ overhead ║ throughput     ║"
    );
    eprintln!(
        "╠══════════════╬═══════════════════╬═══════════════════╬══════════╬════════════════╣"
    );

    for (group, plain_name, versioned_name) in benchmarks {
        let plain_ns = read_criterion_mean_ns(&base, group, plain_name);
        let versioned_ns = read_criterion_mean_ns(&base, group, versioned_name);

        let plain_str = match plain_ns {
            Some(v) => format!("{:>13.2} ns", v),
            None => format!("{:>16}", "?"),
        };
        let versioned_str = match versioned_ns {
            Some(v) => format!("{:>13.2} ns", v),
            None => format!("{:>16}", "?"),
        };
        let overhead_str = match (plain_ns, versioned_ns) {
            (Some(p), Some(v)) => format!("{:>+6.1}%", (v - p) / p * 100.0),
            _ => format!("{:>7}", "?"),
        };
        let throughput_str = match versioned_ns {
            Some(v) => format!("{:>10.1}M/s", 1_000.0 / v),
            None => format!("{:>13}", "?"),
        };

        eprintln!(
            "║ {:<12} ║ {:<17} ║ {:<17} ║ {:<8} ║ {:<14} ║",
            group, plain_str, versioned_str, overhead_str, throughput_str
        );
    }

    eprintln!(
        "╚══════════════╩═══════════════════╩═══════════════════╩══════════╩════════════════╝"
    );
    eprintln!();
    eprintln!("Config: 5 levels × 5 versions, 3,125 cross-product entries, bool-only fields");
    eprintln!("Versioned uses serialize_at/deserialize_at with latest version index (constant).");
    eprintln!();
}

/// Read the mean point estimate (nanoseconds) from criterion's estimates.json.
fn read_criterion_mean_ns(base: &std::path::Path, group: &str, name: &str) -> Option<f64> {
    let path = base.join(group).join(name).join("new/estimates.json");
    let content = std::fs::read_to_string(&path).ok()?;
    // JSON: {"mean":{"confidence_interval":{...},"point_estimate":233.68,...},...}
    let marker = "\"mean\"";
    let mean_pos = content.find(marker)?;
    let after_mean = &content[mean_pos + marker.len()..];
    let pe_marker = "\"point_estimate\":";
    let pe_pos = after_mean.find(pe_marker)?;
    let after_pe = &after_mean[pe_pos + pe_marker.len()..];
    let end = after_pe.find(',')?;
    after_pe[..end].trim().parse::<f64>().ok()
}

criterion_group!(
    benches,
    bench_serialize,
    bench_deserialize,
    bench_round_trip,
    print_summary
);
criterion_main!(benches);
