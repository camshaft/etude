// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use super::EventMetadata;
use proc_macro2::TokenStream;
use quote::quote;

/// Generates this namespace's `Subscriber` impl for the single, shared
/// `Testing<S>` subscriber (`core::runtime::subscriber::Testing`).
///
/// Every namespace's generated code implements its `Subscriber` for the one
/// `Testing<S>` type, so a single `Testing` instance (and its one [`EventSink`])
/// observes and aggregates events across all namespaces it is attached to. Its
/// `WithSubscriber` bridge comes from a blanket impl in the runtime, so only the
/// `Subscriber` methods are emitted here.
///
/// Each method erases its typed fields into a generic
/// [`Event`](core::runtime::subscriber::Event) (name + namespace + level + caller +
/// `&dyn Debug` fields) and funnels it into `self.sink().on_event(env, &event)`.
/// Codegen forwards every event, including metrics-only ones, with its level; the
/// sink decides what to do (the default `SnapshotLog` renders via `Display` and
/// skips `level == "none"` events).
pub fn generate_testing(
    namespace: &str,
    events: &[&EventMetadata],
    core_crate: &TokenStream,
) -> TokenStream {
    let subscriber_impls = events.iter().map(|event| {
        let method_name = event.method_name("on", namespace);
        let type_params = event.type_params(true, core_crate);
        let field_params = event.fields.iter().map(|f| f.param());
        let return_type = event.return_type(core_crate);

        let event_name = event.name;
        let level = event.level;
        let field_count = event.fields.len();
        let field_entries = event.fields.iter().map(|field| {
            let field_name = field.name;
            let field_ident = field.name_ident();
            quote! {
                #core_crate::runtime::subscriber::Field {
                    name: #field_name,
                    value: &#field_ident,
                }
            }
        });

        quote! {
            fn #method_name #type_params(&self, env: &E, caller: #core_crate::runtime::Caller, #(#field_params),*) #return_type {
                // Explicit element type so a zero-field event's `[]` still types.
                let __fields: [#core_crate::runtime::subscriber::Field<'_>; #field_count] =
                    [ #(#field_entries),* ];
                self.sink().on_event(
                    env,
                    &#core_crate::runtime::subscriber::Event::new(
                        #namespace,
                        #event_name,
                        #level,
                        caller,
                        &__fields,
                    ),
                );
            }
        }
    });

    quote! {
        use super::*;
        use #core_crate::runtime::subscriber::EventSink;

        // Re-export the single shared testing subscriber so `<crate>::events::Testing`
        // keeps resolving (e.g. the generated builder's `Testing::new(..)`).
        pub use #core_crate::runtime::subscriber::Testing;

        // TODO support spans: span/timer events (a non-`Regular` `event_type`) are
        // observed here like regular events -- their fields are captured and
        // forwarded to the sink -- but without span-lifecycle handling (entry/exit).
        // If such an event ever gains a subscriber method with a span return type,
        // this impl must construct and return that span (as the tracing/metrics
        // subscribers do), not `()`.
        impl<S: EventSink> Subscriber for Testing<S> {
            #(#subscriber_impls)*
        }
    }
}
