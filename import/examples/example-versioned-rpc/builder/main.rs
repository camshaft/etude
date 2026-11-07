// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use upstream_codegen::{Builder, Endpoint};

mod versioned_service;

fn main() {
    Builder::new("versioned_rpc")
        .with(
            Endpoint::new("server")
                .provides(versioned_service::versioned(), "crate::server::Server"),
        )
        .with(Endpoint::new("client").consumes(versioned_service::versioned()))
        .build("src/generated.rs");
}
