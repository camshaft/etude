// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use heck::ToSnakeCase;
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, ItemStruct, LitStr, MetaList};

/// Returns true when the `#[event(...)]` argument list is exactly `task_class`.
pub(crate) fn is_task_class_attr(attrs: &TokenStream) -> bool {
    let mut idents = attrs.clone().into_iter();
    match idents.next() {
        Some(proc_macro2::TokenTree::Ident(ident)) => {
            ident == "task_class" && idents.next().is_none()
        }
        _ => false,
    }
}

/// Returns true when the `#[event(...)]` argument list begins with the `tx_class`
/// keyword — e.g. `#[event(tx_class, category = Cleanup)]`. Unlike `task_class`
/// the argument list is NOT a bare ident: it carries a required `category = ...`,
/// parsed by [`TxClassAttr`].
pub(crate) fn is_tx_class_attr(attrs: &TokenStream) -> bool {
    matches!(
        attrs.clone().into_iter().next(),
        Some(proc_macro2::TokenTree::Ident(ident)) if ident == "tx_class"
    )
}

/// Parsed `#[event(tx_class, category = <Data|Metadata|Cleanup>)]` argument list.
struct TxClassAttr {
    category: syn::Ident,
}

impl syn::parse::Parse for TxClassAttr {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let kw: syn::Ident = input.parse()?;
        if kw != "tx_class" {
            return Err(syn::Error::new(kw.span(), "expected `tx_class`"));
        }
        input.parse::<syn::Token![,]>()?;
        let key: syn::Ident = input.parse()?;
        if key != "category" {
            return Err(syn::Error::new(
                key.span(),
                "expected `category = Data|Metadata|Cleanup`",
            ));
        }
        input.parse::<syn::Token![=]>()?;
        let category: syn::Ident = input.parse()?;
        match category.to_string().as_str() {
            "Data" | "Metadata" | "Cleanup" => {}
            other => {
                return Err(syn::Error::new(
                    category.span(),
                    format!("category must be Data, Metadata, or Cleanup (got `{other}`)"),
                ));
            }
        }
        // Tolerate a trailing comma.
        let _ = input.parse::<syn::Token![,]>();
        Ok(TxClassAttr { category })
    }
}

/// Registers a transaction class declared with
/// `#[event(tx_class, category = <Category>)]` in a crate's `builder/events.rs`.
///
/// Unlike [`derive_task_class`] (which requires a *unit* struct), a tx class
/// ACCEPTS fields — they carry the operation's identifying data (`shuffle_id`,
/// `rank`, …) that codegen weaves into the derived `tx_context` tracing/retry
/// label. Like `task_class`, this deliberately does NOT re-emit the struct:
/// `builder/events.rs` compiles only into the build binary, so the real struct
/// (and its `impl TxClass<E>`) is emitted by build-time codegen into
/// `src/generated/events/tx_classes.rs`, where the crate's `src/` can construct
/// it: `client.commit(env, DeleteCheckpoint { rank, step }, f)`.
///
/// This records [`TxClassMetadata`] into the `TX_CLASS_METADATA` linkme slice —
/// struct name, admission category, and each field's `(name, type)` — so codegen
/// ([`generate_tx_classes`]) can emit, per namespace:
/// - the class struct itself (with fields, deriving `Debug, Clone, PartialEq, Eq`),
/// - one `#[derive(AsVariant)]` umbrella enum (one unit variant per class),
/// - one companion completion event keyed by that enum (`publish_<ns>_transaction`),
/// - the `impl TxClass<E>` supplying `category()`, `tx_context()` (name + fields
///   via `Debug`), and `record_completed()` (fires the companion event).
///
/// Because codegen re-emits the field types into a generated module, field types
/// must be nameable there — use fully-qualified paths (e.g. `std::sync::Arc<str>`,
/// `u32`, `bytes::Bytes`), not names that rely on the declaring module's imports.
pub(crate) fn derive_tx_class(attrs: TokenStream, input: ItemStruct) -> TokenStream {
    let category = match syn::parse2::<TxClassAttr>(attrs) {
        Ok(TxClassAttr { category }) => category.to_string(),
        Err(err) => return err.to_compile_error(),
    };

    if !input.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &input.generics,
            "#[event(tx_class)] does not support generic parameters",
        )
        .to_compile_error();
    }

    // Only named-field and unit structs are supported: codegen re-emits the
    // struct with named fields (for `tx_context` derivation) or as a unit.
    if matches!(input.fields, syn::Fields::Unnamed(_)) {
        return syn::Error::new_spanned(
            &input.fields,
            "#[event(tx_class)] does not support tuple structs; use named fields or a unit struct",
        )
        .to_compile_error();
    }

    let struct_name = input.ident.to_string();
    let metadata_ident = syn::Ident::new(
        &format!("__{struct_name}__TX_CLASS_META"),
        input.ident.span(),
    );

    // Reflect each field as a `(name, type-token-fn)` pair so codegen can
    // re-emit the struct definition into `src/generated`.
    let field_entries: Vec<TokenStream> = input
        .fields
        .iter()
        .map(|field| {
            let name = field
                .ident
                .as_ref()
                .map(|i| i.to_string())
                .unwrap_or_default();
            let ty = &field.ty;
            quote! {
                upstream_codegen::TxClassFieldMetadata {
                    name: #name,
                    ty: || upstream_codegen::macro_support::quote!(#ty),
                }
            }
        })
        .collect();

    quote! {
        #[allow(non_upper_case_globals)]
        static #metadata_ident: () = {
            use upstream_codegen::macro_support::linkme;
            #[linkme::distributed_slice(upstream_codegen::TX_CLASS_METADATA)]
            #[linkme(crate = upstream_codegen::macro_support::linkme)]
            static META: &upstream_codegen::TxClassMetadata =
                &upstream_codegen::TxClassMetadata {
                    struct_name: #struct_name,
                    category: #category,
                    fields: &[#(#field_entries),*],
                };
        };
    }
}

/// Registers a task class declared with `#[event(task_class)]` in a crate's
/// `builder/events.rs`. This does two things, both consumed by the build-time
/// codegen (it deliberately does NOT re-emit the struct):
///
/// 1. Records [`TaskClassMetadata`] into the `TASK_CLASS_METADATA` linkme slice,
///    so codegen emits the marker `pub struct` + the `impl TaskClass<E>` whose
///    hooks forward to the companion publishers.
/// 2. Synthesizes the four companion metrics events (`<Name>{Poll,FirstPoll,
///    Instrumented,Dropped}`) at `level = "none"` (metrics-only, no tracing) and
///    runs them through the normal [`derive_event`] pipeline, so the crate's
///    `Publisher` gains `publish_<ns>_<name>_{poll,first_poll,instrumented,
///    dropped}(...)`. The instrumented future submits raw samples through these.
pub(crate) fn derive_task_class(input: ItemStruct) -> TokenStream {
    if !matches!(input.fields, syn::Fields::Unit) {
        return syn::Error::new_spanned(
            &input.fields,
            "#[event(task_class)] is only supported on unit structs (e.g. `struct ProposalTask;`)",
        )
        .to_compile_error();
    }
    if !input.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &input.generics,
            "#[event(task_class)] does not support generic parameters",
        )
        .to_compile_error();
    }

    let struct_name = input.ident.to_string();
    let metadata_ident = syn::Ident::new(
        &format!("__{struct_name}__TASK_CLASS_META"),
        input.ident.span(),
    );

    // Companion metrics events, all level="none" (metrics-only, no tracing).
    // Each instrumented poll/drop submits raw samples through these so they feed
    // the dc-metrics `Summary` histograms directly — rich distributions instead
    // of pre-computed means. The generated `impl TaskClass<E>` forwards each hook
    // to the matching `publish_<ns>_<name>_{poll,first_poll,instrumented,dropped}`.
    let companion_events = task_class_companion_events(&input.ident);

    quote! {
        #companion_events

        #[allow(non_upper_case_globals)]
        static #metadata_ident: () = {
            use upstream_codegen::macro_support::linkme;
            #[linkme::distributed_slice(upstream_codegen::TASK_CLASS_METADATA)]
            #[linkme(crate = upstream_codegen::macro_support::linkme)]
            static META: &upstream_codegen::TaskClassMetadata =
                &upstream_codegen::TaskClassMetadata { struct_name: #struct_name };
        };
    }
}

/// Synthesizes the four companion metrics events for a task class named `ident`
/// and runs them through the normal [`derive_event`] pipeline, so the declaring
/// crate's `Publisher` gains:
///
/// - `publish_<ns>_<name>_poll(scheduled_duration_us, poll_duration_us)`
/// - `publish_<ns>_<name>_first_poll(first_poll_delay_us)`
/// - `publish_<ns>_<name>_instrumented()`
/// - `publish_<ns>_<name>_dropped()`
///
/// Durations are `#[measure]` (distributions); instrumented/dropped are
/// `#[counter]` lifecycle bumps. Shared by `#[event(task_class)]` and the
/// service codegen.
pub(crate) fn task_class_companion_events(ident: &syn::Ident) -> TokenStream {
    // The `TaskClass` prefix marks these events as task-class instrumentation in
    // the flat metric namespace, e.g. `gossip.task_class_gossip_notify_poll.*`.
    // The generated `TaskClass` impl mirrors it in its publisher method names
    // (`publish_<ns>_task_class_<name>_poll`), so the two stay in sync.
    let name = ident.to_string();
    let poll = syn::Ident::new(&format!("TaskClass{name}Poll"), ident.span());
    let first_poll = syn::Ident::new(&format!("TaskClass{name}FirstPoll"), ident.span());
    let instrumented = syn::Ident::new(&format!("TaskClass{name}Instrumented"), ident.span());
    let dropped = syn::Ident::new(&format!("TaskClass{name}Dropped"), ident.span());

    let poll: ItemStruct = syn::parse_quote! {
        #[level = "none"]
        struct #poll {
            #[measure("scheduled_duration", "duration")]
            scheduled_duration: core::time::Duration,
            #[measure("poll_duration", "duration")]
            poll_duration: core::time::Duration,
        }
    };
    let first_poll: ItemStruct = syn::parse_quote! {
        #[level = "none"]
        struct #first_poll {
            #[measure("first_poll_delay", "duration")]
            first_poll_delay: core::time::Duration,
        }
    };
    let instrumented: ItemStruct = syn::parse_quote! {
        #[level = "none"]
        struct #instrumented {
            #[counter("instrumented_count")]
            count: u64,
        }
    };
    let dropped: ItemStruct = syn::parse_quote! {
        #[level = "none"]
        struct #dropped {
            #[counter("dropped_count")]
            count: u64,
        }
    };

    let poll = derive_event(poll);
    let first_poll = derive_event(first_poll);
    let instrumented = derive_event(instrumented);
    let dropped = derive_event(dropped);

    quote! {
        #poll
        #first_poll
        #instrumented
        #dropped
    }
}

pub(crate) fn derive_event(input: ItemStruct) -> TokenStream {
    let struct_name = &input.ident;
    let docs = extract_docs(&input.attrs);
    let level = extract_level(&input.attrs);
    let event_type = extract_event_type(&input.attrs);
    let backbeat_skip = match extract_backbeat_skip(&input.attrs) {
        Ok(skip) => skip,
        Err(err) => return err.to_compile_error(),
    };

    // Generate metadata registration with prefixed static name
    let metadata_ident = format!("__{struct_name}__META");
    let metadata_ident = syn::Ident::new(&metadata_ident, struct_name.span());

    // Extract just the type parameters without angle brackets
    let params = &input.generics.params;
    let type_params = quote!(#params);

    // Parse fields and handle any errors
    let fields = match parse_fields(&input.fields) {
        Ok(fields) => fields,
        Err(err) => return err.to_compile_error(),
    };

    let struct_name = struct_name.to_string();
    let name = struct_name.to_snake_case();

    quote! {
        #[allow(non_upper_case_globals)]
        static #metadata_ident: () = {
            use upstream_codegen::macro_support::linkme;
            #[linkme::distributed_slice(upstream_codegen::EVENT_METADATA)]
            #[linkme(crate = upstream_codegen::macro_support::linkme)]
            static META: &upstream_codegen::EventMetadata = &upstream_codegen::EventMetadata {
                name: #name,
                struct_name: #struct_name,
                docs: &[#(#docs),*],
                fields: &[#(#fields),*],
                generics: || upstream_codegen::macro_support::quote!(#type_params),
                level: #level,
                event_type: #event_type,
                backbeat_skip: #backbeat_skip,
            };
        };
    }
}

/// Returns `true` if `attrs` contains a `#[backbeat(skip)]` attribute.
///
/// Works on both struct-level attrs (exclude the whole event from backbeat) and
/// field-level attrs (exclude a single field). The only supported argument is
/// `skip`; anything else is a clear compile error so typos don't silently no-op —
/// hence the parse error is propagated rather than discarded.
fn extract_backbeat_skip(attrs: &[Attribute]) -> syn::Result<bool> {
    for attr in attrs {
        if !attr.path().is_ident("backbeat") {
            continue;
        }
        let mut skip = false;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                skip = true;
                Ok(())
            } else {
                Err(meta.error("unknown `#[backbeat(...)]` argument (expected `skip`)"))
            }
        })?;
        if skip {
            return Ok(true);
        }
    }
    Ok(false)
}

fn extract_level(attrs: &[Attribute]) -> TokenStream {
    for attr in attrs {
        if attr.meta.path().is_ident("level")
            && let syn::Meta::NameValue(nv) = &attr.meta
            && let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
        {
            let level = s.value();
            match level.as_str() {
                // "none" suppresses the tracing event entirely (metrics-only);
                // see generate_tracing. The other values map to tracing levels.
                "error" | "warn" | "info" | "debug" | "trace" | "none" => {
                    return quote!(#level);
                }
                _ => {
                    let error = format!("invalid tracing level: {level}");
                    return quote!(compile_error!(#error));
                }
            }
        }
    }
    quote!("debug") // Default level
}

fn extract_event_type(attrs: &[Attribute]) -> TokenStream {
    for attr in attrs {
        if attr.meta.path().is_ident("span") {
            if let syn::Meta::NameValue(nv) = &attr.meta
                && let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) = &nv.value
            {
                let result_type = s.value();
                return quote! {
                    upstream_codegen::events::EventType::Span {
                        result_type: || upstream_codegen::macro_support::quote!(#result_type),
                    }
                };
            }
        } else if attr.meta.path().is_ident("timer") {
            return quote! {
                upstream_codegen::events::EventType::Timer
            };
        }
    }
    quote!(upstream_codegen::events::EventType::Regular)
}

fn extract_docs(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter_map(|attr| {
            if attr.meta.path().is_ident("doc")
                && let syn::Meta::NameValue(nv) = &attr.meta
                && let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) = &nv.value
            {
                return Some(s.value());
            }
            None
        })
        .collect()
}

fn parse_fields(fields: &syn::Fields) -> syn::Result<Vec<TokenStream>> {
    fields
        .iter()
        .map(|field| {
            let name = field.ident.as_ref().unwrap();
            let ty = &field.ty;
            let docs = extract_docs(&field.attrs);
            let metrics = parse_field_metrics(&field.attrs)?;
            let backbeat_skip = extract_backbeat_skip(&field.attrs)?;

            Ok(quote! {
                upstream_codegen::FieldMetadata {
                    name: stringify!(#name),
                    docs: &[#(#docs),*],
                    ty: || upstream_codegen::macro_support::quote!(#ty),
                    metrics: &[#(#metrics),*],
                    backbeat_skip: #backbeat_skip,
                }
            })
        })
        .collect()
}

fn parse_field_metrics(attrs: &[Attribute]) -> syn::Result<Vec<TokenStream>> {
    let mut metrics = Vec::new();

    for attr in attrs {
        if let Some(metric) = parse_attr(attr)? {
            metrics.push(metric);
        } else if attr.path().is_ident("doc") || attr.path().is_ident("backbeat") {
            // `backbeat` is handled separately by `extract_backbeat_skip`.
            continue;
        } else {
            return Err(syn::Error::new_spanned(attr, "unexpected attribute"));
        }
    }

    Ok(metrics)
}

pub fn parse_attr(attr: &Attribute) -> syn::Result<Option<TokenStream>> {
    let syn::Meta::List(list) = &attr.meta else {
        return Ok(None);
    };

    parse_meta_list(list)
}

pub fn parse_meta_list(attr: &MetaList) -> syn::Result<Option<TokenStream>> {
    let Some(ident) = attr.path.get_ident() else {
        return Ok(None);
    };

    let metric = match ident.to_string().as_str() {
        "counter" => {
            let CounterArgs {
                name,
                unit,
                expression,
            } = parse_attr_args::<CounterArgs>(attr)?;
            let expression = quote_expr(expression);
            quote! {
                upstream_codegen::MetricMetadata::Counter {
                    name: #name,
                    unit: #unit,
                    expression: #expression,
                }
            }
        }
        "measure" => {
            let MeasureArgs {
                name,
                unit,
                expression,
            } = parse_attr_args::<MeasureArgs>(attr)?;
            let expression = quote_expr(expression);
            quote! {
                upstream_codegen::MetricMetadata::Measure {
                    name: #name,
                    unit: #unit,
                    expression: #expression,
                    optional: false,
                }
            }
        }
        // Like `measure`, but the annotated field is an `Option<_>`: a histogram
        // sample is recorded only when the value is `Some`. Use for observations
        // that are not meaningful for every event (e.g. a compression ratio that
        // only exists for non-empty publishes) so placeholder values never skew
        // the distribution. Same argument shape as `measure`.
        "optional_measure" => {
            let MeasureArgs {
                name,
                unit,
                expression,
            } = parse_attr_args::<MeasureArgs>(attr)?;
            let expression = quote_expr(expression);
            quote! {
                upstream_codegen::MetricMetadata::Measure {
                    name: #name,
                    unit: #unit,
                    expression: #expression,
                    optional: true,
                }
            }
        }
        "bool_counter" => {
            let BoolCounterArgs { name, expression } = parse_attr_args::<BoolCounterArgs>(attr)?;
            let expression = quote_expr(expression);
            quote! {
                upstream_codegen::MetricMetadata::BoolCounter {
                    name: #name,
                    expression: #expression,
                }
            }
        }
        "nominal_counter" => {
            let NominalCounterArgs {
                name,
                unit,
                variant_type,
                expression,
            } = parse_attr_args::<NominalCounterArgs>(attr)?;
            let expression = quote_expr(expression);
            quote! {
                upstream_codegen::MetricMetadata::NominalCounter {
                    name: #name,
                    unit: #unit,
                    variant_type: || upstream_codegen::macro_support::quote!(#variant_type),
                    expression: #expression,
                }
            }
        }
        "nominal_measure" => {
            let NominalMeasureArgs {
                name,
                unit,
                variant_field,
                expression,
            } = parse_attr_args::<NominalMeasureArgs>(attr)?;
            let expression = quote_expr(expression);
            quote! {
                upstream_codegen::MetricMetadata::NominalMeasure {
                    name: #name,
                    unit: #unit,
                    variant_field: || upstream_codegen::macro_support::quote!(#variant_field),
                    expression: #expression,
                }
            }
        }
        "nominal_gauge" => {
            let NominalGaugeArgs {
                name,
                unit,
                variant_field,
                expression,
            } = parse_attr_args::<NominalGaugeArgs>(attr)?;
            let expression = quote_expr(expression);
            quote! {
                upstream_codegen::MetricMetadata::NominalGauge {
                    name: #name,
                    unit: #unit,
                    variant_field: || upstream_codegen::macro_support::quote!(#variant_field),
                    expression: #expression,
                }
            }
        }
        "gauge" => {
            let GaugeArgs {
                name,
                unit,
                expression,
            } = parse_attr_args::<GaugeArgs>(attr)?;
            let expression = quote_expr(expression);
            quote! {
                upstream_codegen::MetricMetadata::Gauge {
                    name: #name,
                    unit: #unit,
                    expression: #expression,
                }
            }
        }
        _ => return Ok(None),
    };

    Ok(Some(metric))
}

fn quote_expr(expr: Option<syn::Expr>) -> TokenStream {
    if let Some(expr) = expr {
        quote!(Some(|| upstream_codegen::macro_support::quote!(#expr)))
    } else {
        quote!(None)
    }
}

// Argument structs for each metric type
#[derive(Debug)]
struct CounterArgs {
    name: LitStr,
    unit: TokenStream,
    expression: Option<syn::Expr>,
}

#[derive(Debug)]
struct MeasureArgs {
    name: LitStr,
    unit: LitStr,
    expression: Option<syn::Expr>,
}

#[derive(Debug)]
struct BoolCounterArgs {
    name: LitStr,
    expression: Option<syn::Expr>,
}

#[derive(Debug)]
struct NominalCounterArgs {
    name: LitStr,
    unit: TokenStream,
    variant_type: syn::Type,
    expression: Option<syn::Expr>,
}
#[derive(Debug)]
struct NominalMeasureArgs {
    name: LitStr,
    unit: LitStr,
    variant_field: syn::Ident,
    expression: Option<syn::Expr>,
}

#[derive(Debug)]
struct NominalGaugeArgs {
    name: LitStr,
    unit: LitStr,
    variant_field: syn::Ident,
    expression: Option<syn::Expr>,
}

#[derive(Debug)]
struct GaugeArgs {
    name: LitStr,
    unit: LitStr,
    expression: Option<syn::Expr>,
}

// Parse implementations
macro_rules! impl_parse_args {
    ($type:ident) => {
        impl syn::parse::Parse for $type {
            fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
                let name = input.parse::<LitStr>()?;
                let expression = if input.peek(syn::Token![,]) {
                    input.parse::<syn::Token![,]>()?;
                    Some(input.parse::<syn::Expr>()?)
                } else {
                    None
                };
                Ok($type { name, expression })
            }
        }
    };
    ($type:ident, with_unit) => {
        impl syn::parse::Parse for $type {
            fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
                let name = input.parse::<LitStr>()?;
                input.parse::<syn::Token![,]>()?;
                let unit = input.parse::<LitStr>()?;
                let expression = if input.peek(syn::Token![,]) {
                    input.parse::<syn::Token![,]>()?;
                    Some(input.parse::<syn::Expr>()?)
                } else {
                    None
                };
                Ok($type { name, unit, expression })
            }
        }
    };
    ($type:ident, with_unit_and_variant_field) => {
        impl syn::parse::Parse for $type {
            fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
                let name = input.parse::<LitStr>()?;
                input.parse::<syn::Token![,]>()?;
                let unit = input.parse::<LitStr>()?;
                input.parse::<syn::Token![,]>()?;
                let variant_field = input.parse::<syn::Ident>()?;
                let expression = if input.peek(syn::Token![,]) {
                    input.parse::<syn::Token![,]>()?;
                    Some(input.parse::<syn::Expr>()?)
                } else {
                    None
                };
                Ok($type { name, unit, variant_field, expression })
            }
        }
    };
    ($type:ident, with_optional_unit) => {
        impl syn::parse::Parse for $type {
            fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
                let name = input.parse::<LitStr>()?;
                let (unit, expression) = if input.peek(syn::Token![,]) {
                    input.parse::<syn::Token![,]>()?;
                    if input.peek(LitStr) {
                        // Next token is a string literal (unit)
                        let unit = input.parse::<LitStr>()?;
                        let expression = if input.peek(syn::Token![,]) {
                            input.parse::<syn::Token![,]>()?;
                            Some(input.parse::<syn::Expr>()?)
                        } else {
                            None
                        };
                        (quote!(Some(#unit)), expression)
                    } else {
                        // Next token is an expression (no unit specified)
                        let expression = Some(input.parse::<syn::Expr>()?);
                        (quote!(None), expression)
                    }
                } else {
                    (quote!(None), None)
                };
                Ok($type { name, unit, expression })
            }
        }
    };
    ($type:ident, with_optional_unit_and_variant_type) => {
        impl syn::parse::Parse for $type {
            fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
                let name = input.parse::<LitStr>()?;
                input.parse::<syn::Token![,]>()?;

                // Check if the next token is a string literal (unit) or a type
                let (unit, variant_type, expression) = if input.peek(LitStr) {
                    // Has unit: nominal_counter("name", "unit", Type, ...)
                    let unit = input.parse::<LitStr>()?;
                    input.parse::<syn::Token![,]>()?;
                    let variant_type = input.parse::<syn::Type>()?;
                    let expression = if input.peek(syn::Token![,]) {
                        input.parse::<syn::Token![,]>()?;
                        Some(input.parse::<syn::Expr>()?)
                    } else {
                        None
                    };
                    (quote!(Some(#unit)), variant_type, expression)
                } else {
                    // No unit: nominal_counter("name", Type, ...)
                    let variant_type = input.parse::<syn::Type>()?;
                    let expression = if input.peek(syn::Token![,]) {
                        input.parse::<syn::Token![,]>()?;
                        Some(input.parse::<syn::Expr>()?)
                    } else {
                        None
                    };
                    (quote!(None), variant_type, expression)
                };

                Ok($type { name, unit, variant_type, expression })
            }
        }
    };
}

impl_parse_args!(BoolCounterArgs);
impl_parse_args!(CounterArgs, with_optional_unit);
impl_parse_args!(NominalCounterArgs, with_optional_unit_and_variant_type);
impl_parse_args!(NominalMeasureArgs, with_unit_and_variant_field);
impl_parse_args!(NominalGaugeArgs, with_unit_and_variant_field);
impl_parse_args!(MeasureArgs, with_unit);
impl_parse_args!(GaugeArgs, with_unit);

fn parse_attr_args<T: syn::parse::Parse>(attr: &MetaList) -> syn::Result<T> {
    attr.parse_args()
}
