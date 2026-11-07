// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use upstream_codegen::Builder;

mod events;

fn main() {
    Builder::new("core_events")
        .core_crate("core_events")
        .build("src/generated.rs");
}
