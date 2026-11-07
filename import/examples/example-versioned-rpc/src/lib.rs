// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! End-to-end test crate for versioned RPC schema evolution.
//!
//! Defines a versioned Wire struct (`ReadRequest`) and a service that uses it,
//! then verifies that the generated code produces multi-entry `MethodVersionEntry`
//! arrays with distinct `global_id`s.

pub mod types;

#[allow(dead_code)]
mod generated;

#[cfg(test)]
mod tests;
