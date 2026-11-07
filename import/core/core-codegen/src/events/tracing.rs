use super::{EventMetadata, EventType};
use proc_macro2::{Ident, TokenStream};
use quote::quote;

pub fn generate_tracing(
    namespace: &str,
    events: &[&EventMetadata],
    core_crate: &TokenStream,
) -> TokenStream {
    let subscriber_impls = events.iter().map(|event| {
        let method_name = event.method_name("on", namespace);
        let type_params = event.type_params(true, core_crate);
        let field_params = event.fields.iter().map(|f| f.param());
        let map_field_value: Box<dyn Fn(&Ident) -> TokenStream> = if matches!(event.event_type, EventType::HttpSpan { .. }) {
            Box::new(|name: &Ident| quote!(%#core_crate::service::tracing::http::Field(#name)))
        } else {
            Box::new(|name: &Ident| quote!(?#name))
        };

        // Create field assignments for the tracing event
        let field_assignments = event
            .fields
            .iter()
            .map(|field| {
                let name = field.name_ident();
                let value = map_field_value(&name);
                quote! { #name = #value }
            })
            .collect::<Vec<_>>();

        // `level = "none"` suppresses the tracing event entirely (metrics-only
        // events, e.g. per-task-class metrics published from a background loop
        // that must not spam tracing). For such events the subscriber method
        // body is empty — only the metrics subscriber records anything.
        let is_none_level = event.level == "none";
        let level = match event.level {
            "error" => quote!(ERROR),
            "warn" => quote!(WARN),
            "info" => quote!(INFO),
            "debug" => quote!(DEBUG),
            "trace" => quote!(TRACE),
            "none" => quote!(TRACE), // unused; body is suppressed below
            _ => panic!("invalid level: {}", event.level),
        };
        let event_name = event.struct_name;

        let field_idents = event
            .fields
            .iter()
            .map(|field| field.name_ident())
            .collect::<Vec<_>>();
        let base_event = if is_none_level {
            quote! {
                // level = "none": no tracing event emitted.
                let _ = (&env, &caller);
                #(let _ = &#field_idents;)*
            }
        } else {
            quote! {
                tracing::event!(
                    tracing::Level::#level,
                    location = ?caller,
                    #(#field_assignments,)*
                    trace_id = %env.current_trace(),
                    parent_span = %env.parent_span(),
                    span_id = %env.span_id(),
                    #event_name
                );
            }
        };

        let body = match event.event_type {
            EventType::MethodSpan {
                span_type,
                method_name,
                request_type: _,
                response_type: _,
                request_metrics: _,
                response_metrics: _,
            } => {
                let span_type = span_type();

                // TODO implement other span types
                if !span_type.to_string().contains("Unary") {
                    base_event
                } else {
                    let span_type = event.span_type_with_generics();

                    quote! {
                        let span = tracing::span!(
                            tracing::Level::#level,
                            "service",
                            method = #method_name,
                            location = ?caller,
                            #(#field_assignments,)*
                            trace_id = %env.current_trace(),
                            parent_span = %env.parent_span(),
                            span_id = %env.span_id(),
                        );
                        <#core_crate::service::tracing::#span_type>::new(env, span)
                    }
                }
            }
            EventType::HttpSpan { method, path, request_metrics: _, response_metrics: _ } => {
                quote! {
                    let _ = caller;
                    let span = tracing::span!(
                        tracing::Level::#level,
                        "service",
                        method = #method,
                        path = #path,
                        #(#field_assignments,)*
                        trace_id = %env.current_trace(),
                        parent_span = %env.parent_span(),
                        span_id = %env.span_id(),
                    );
                    <#core_crate::service::tracing::HttpSpan>::new(env, span)
                }
            }
            _ => base_event,
        };

        let return_type = event.return_type(core_crate);
        // The `use tracing` import is only needed when we actually emit an event.
        let tracing_use = if is_none_level {
            quote!()
        } else {
            quote!(use ::#core_crate::macro_support::tracing;)
        };
        quote! {
            fn #method_name #type_params(&self, env: &E, caller: #core_crate::runtime::Caller, #(#field_params),*) #return_type {
                #tracing_use

                #body
            }
        }
    });

    quote! {
        use super::*;

        /// Test subscriber that records events using the tracing crate
        #[derive(Clone, Default)]
        pub struct Tracing(());

        impl Tracing {
            pub fn new() -> Self {
                Self::default()
            }
        }

        impl Subscriber for Tracing {
            #(#subscriber_impls)*
        }

        impl #core_crate::runtime::WithSubscriber<Namespace> for Tracing {
            type Subscriber = Tracing;

            fn subscriber(&self) -> &Self::Subscriber {
                self
            }
        }
    }
}
