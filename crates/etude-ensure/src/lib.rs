// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Small, dependency-free control-flow macros.
//!
//! - [`ensure!`] bails out early (`return`/`break`/`continue`) unless a condition holds.
//! - [`assume!`] asserts an invariant: a `debug_assert!` in debug builds, an
//!   optimization hint (`unreachable_unchecked`) in release builds.
//!
//! These are intentionally free of any instrumentation or dependencies so they can be
//! used anywhere bytes are being pushed around.

#![no_std]

/// Returns (or `break`s / `continue`s) early unless a condition holds.
///
/// ```
/// use etude_ensure::ensure;
///
/// fn checked_sub(a: usize, b: usize) -> Option<usize> {
///     ensure!(a >= b, None);
///     Some(a - b)
/// }
///
/// assert_eq!(checked_sub(5, 3), Some(2));
/// assert_eq!(checked_sub(3, 5), None);
/// ```
///
/// The `let` form binds a pattern or takes the given control-flow branch:
///
/// ```
/// use etude_ensure::ensure;
///
/// fn first_even(items: &[u32]) -> Option<u32> {
///     for &item in items {
///         ensure!(item % 2 == 0, continue);
///         return Some(item);
///     }
///     None
/// }
///
/// assert_eq!(first_even(&[1, 3, 4, 5]), Some(4));
/// ```
#[macro_export]
macro_rules! ensure {
    (let $pat:pat = $expr:expr, continue) => {
        let $pat = $expr else {
            continue;
        };
    };
    (let $pat:pat = $expr:expr, break $($ret:expr)?) => {
        let $pat = $expr else {
            break $($ret)?;
        };
    };
    (let $pat:pat = $expr:expr, return $($ret:expr)?) => {
        let $pat = $expr else {
            return $($ret)?;
        };
    };
    (let $pat:pat = $expr:expr $(, $ret:expr)?) => {
        $crate::ensure!(let $pat = $expr, return $($ret)?)
    };
    ($cond:expr, continue) => {
        if !($cond) {
            continue;
        }
    };
    ($cond:expr, break $($expr:expr)?) => {
        if !($cond) {
            break $($expr)?;
        }
    };
    ($cond:expr, return $($expr:expr)?) => {
        if !($cond) {
            return $($expr)?;
        }
    };
    ($cond:expr $(, $ret:expr)?) => {
        $crate::ensure!($cond, return $($ret)?);
    };
}

/// Asserts an invariant.
///
/// In debug builds this is a `debug_assert!`. In release builds a failed assumption is
/// undefined behavior (`unreachable_unchecked`), so only use it where the condition is
/// genuinely guaranteed — it is an optimization hint, not a runtime check.
///
/// # Safety
///
/// The condition MUST hold. Violating it in a release build is undefined behavior.
#[macro_export]
macro_rules! assume {
    (false) => {
        $crate::assume!(false, "assumption failed")
    };
    (false $(, $fmtarg:expr)* $(,)?) => {{
        if cfg!(not(debug_assertions)) {
            core::hint::unreachable_unchecked();
        }

        panic!($($fmtarg),*)
    }};
    ($cond:expr) => {
        $crate::assume!($cond, "assumption failed: {}", stringify!($cond));
    };
    ($cond:expr $(, $fmtarg:expr)* $(,)?) => {
        let v = $cond;

        debug_assert!(v $(, $fmtarg)*);
        if cfg!(not(debug_assertions)) && !v {
            core::hint::unreachable_unchecked();
        }
    };
}
