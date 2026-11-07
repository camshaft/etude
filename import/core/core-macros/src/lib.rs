// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Procedural macros for upstream
//!
//! This crate provides procedural macros for generating reflection on code in upstream.
//! upstream-codegen then uses the reflected metadata to generate code at build time. This
//! allows more powerful compositions across the compilation unit rather than just a single
//! macro call-site.

mod event;
mod service;
mod tagged_wire;
mod util;
mod variant;
mod wire;

use proc_macro::TokenStream;
use syn::{Ident, parse_macro_input};

/// Derives the Event trait for a struct and generates the necessary metadata.
///
/// # Example
/// ```ignore
/// use upstream_macros::event;
/// use std::time::Duration;
///
/// #[event]
/// struct RpcRequestCompleted<'a> {
///     endpoint: &'a str,
///     #[measure("latency", "milliseconds")]
///     latency: Duration,
/// }
/// ```
///
/// # Task classes
///
/// Annotating a unit struct with `#[event(task_class)]` instead generates a
/// spawn-site task class: the struct is re-emitted and given a [`TaskClass`]
/// impl backed by a static `TaskMonitor` registered in the global registry.
/// Unlike a regular event it produces no metric metadata of its own — it is a
/// marker passed to `Env::spawn`, and the runtime reports its metrics
/// generically. Task classes can be declared anywhere, next to their spawn
/// sites; they need not live in an event file.
///
/// ```ignore
/// #[event(task_class)]
/// /// Drives a single Paxos proposal to consensus.
/// struct ProposalTask;
/// ```
#[proc_macro_attribute]
pub fn event(attrs: TokenStream, input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input);
    let attrs = proc_macro2::TokenStream::from(attrs);
    if event::is_tx_class_attr(&attrs) {
        TokenStream::from(event::derive_tx_class(attrs, input))
    } else if event::is_task_class_attr(&attrs) {
        TokenStream::from(event::derive_task_class(input))
    } else {
        TokenStream::from(event::derive_event(input))
    }
}

/// Derives Serialize and Deserialize traits for a struct.
///
/// # Example
/// ```ignore
/// use upstream_wire::Wire;
///
/// #[derive(Wire)]
/// struct Message {
///     id: u64,
///     payload: Vec<u8>,
/// }
/// ```
#[proc_macro_derive(Wire, attributes(wire))]
pub fn derive_wire(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input);
    TokenStream::from(wire::derive_wire(input))
}

/// Derives AsVariant trait for an enum.
///
/// # Example
/// ```ignore
/// use upstream_core::metrics::AsVariant;
///
/// #[derive(AsVariant)]
/// enum Status {
///     Ok,
///     Retried,
///     Pending,
///     Error
/// }
/// ```
#[proc_macro_derive(AsVariant, attributes(variant))]
pub fn derive_as_variant(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input);
    TokenStream::from(variant::derive_as_variant(input))
}

#[proc_macro_derive(TaggedWire, attributes(wire))]
pub fn derive_tagged_wire(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input);
    TokenStream::from(tagged_wire::derive(input))
}

/// Generates a service definition from an extern block.
///
/// # Example
/// ```ignore
/// use upstream_macros::service;
///
/// #[service(my_service_name)]
/// extern "RPC" {
///     /// Get a value by key
///     fn get(key: Bytes) -> Option<Bytes>;
/// }
/// ```
#[proc_macro_attribute]
pub fn service(attr: TokenStream, item: TokenStream) -> TokenStream {
    if attr.is_empty() {
        return quote::quote!(compile_error!("service must define a name")).into();
    }
    let name = parse_macro_input!(attr as Ident);
    let input = parse_macro_input!(item as syn::ItemForeignMod);
    match service::service(name, input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}
