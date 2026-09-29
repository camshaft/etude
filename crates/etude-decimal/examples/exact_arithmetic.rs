// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Exact base-10 arithmetic with `etude-decimal` — no floating-point rounding error.
//!
//! Run with: `cargo run -p etude-decimal --example exact_arithmetic`

use etude_decimal::{Decimal, RoundingMode};

fn dec(s: &str) -> Decimal {
    Decimal::parse(s.bytes()).expect("valid decimal literal")
}

fn main() {
    // The classic float trap: 0.1 + 0.2 is not 0.3 in binary floating point.
    let sum = dec("0.1").add(&dec("0.2"));
    println!("0.1 + 0.2 = {sum}   (f64 gives {:.17})", 0.1_f64 + 0.2);
    assert_eq!(sum, dec("0.3"));

    // Scale is preserved exactly: 0.1 * 0.1 = 0.01.
    println!("0.1 * 0.1 = {}", dec("0.1").mul(&dec("0.1")));

    // Exact division reports when a quotient does not terminate; rounding is always explicit.
    let (one, three) = (Decimal::one(), Decimal::from_i64(3));
    println!("1 / 3 exact    = {:?}", one.div(&three)); // None: 0.333... never terminates
    let approx = one
        .div_round(&three, 10, RoundingMode::HalfEven)
        .expect("non-zero divisor, non-zero precision");
    println!("1 / 3 (10 dig) = {approx}  (half-even)");

    // Arbitrary precision: 2^100, squared, is exact (no overflow, no rounding).
    let two_pow_100 = dec("1267650600228229401496703205376");
    println!("2^100 squared  = {}", two_pow_100.mul(&two_pow_100));
}
