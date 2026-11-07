use crate::emit;
use heck::{ToPascalCase, ToShoutySnakeCase, ToSnakeCase};
use linkme::distributed_slice;
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use std::path::Path;

/// Global registry of event metadata
#[distributed_slice]
pub static EVENT_METADATA: [&'static EventMetadata] = [..];

/// Global registry of task-class metadata, populated by `#[event(task_class)]`
/// declarations in `builder/events.rs`. Consumed by [`generate_events`] to
/// emit a `pub struct` + `TaskClass` impl per class, namespaced to the crate.
#[distributed_slice]
pub static TASK_CLASS_METADATA: [&'static TaskClassMetadata] = [..];

/// Metadata for a single task class. The metric namespace is supplied by the
/// `Builder`, not the declaration, so only the struct name is recorded here.
pub struct TaskClassMetadata {
    /// The declared struct identifier, e.g. `"ProposalTask"`.
    pub struct_name: &'static str,
}

/// Global registry of transaction-class metadata, populated by
/// `#[event(tx_class, category = ...)]` declarations in `builder/events.rs`.
/// Consumed by [`generate_tx_classes`] to emit, per crate: the class structs,
/// one `AsVariant` umbrella enum, one companion completion event, and the
/// `impl TxClass<E>` for each class.
#[distributed_slice]
pub static TX_CLASS_METADATA: [&'static TxClassMetadata] = [..];

/// Metadata for a single transaction class declared via `#[event(tx_class, …)]`.
pub struct TxClassMetadata {
    /// The declared struct identifier, e.g. `"DeleteCheckpoint"`.
    pub struct_name: &'static str,
    /// The admission category token: `"Data"`, `"Metadata"`, or `"Cleanup"`.
    pub category: &'static str,
    /// The class's fields — identifying data (e.g. `rank`, `step`) that codegen
    /// re-emits on the generated struct and folds into the derived `tx_context`
    /// tracing/retry label.
    pub fields: &'static [TxClassFieldMetadata],
}

/// One field of a `#[event(tx_class)]` struct — its name and type — recorded so
/// codegen can re-emit the generated struct definition.
pub struct TxClassFieldMetadata {
    /// The field name, e.g. `"rank"`.
    pub name: &'static str,
    /// The field type, e.g. `|| quote!(u32)`.
    pub ty: fn() -> TokenStream,
}

mod backbeat;
mod metrics;
mod testing;
mod tracing;

/// Emits `#[allow(clippy::too_many_arguments)]` when a generated event method
/// exceeds clippy's default 7-argument threshold. Event methods take one arg per
/// declared field (plus `env`/`caller`), so a field-rich event legitimately has
/// 8+ args and there is nothing to refactor — the arity is intrinsic to the event
/// definition. Applied only to the individual methods that need it (not a
/// module-wide allow).
fn too_many_args_allow(arg_count: usize) -> TokenStream {
    if arg_count > 7 {
        quote!(#[allow(clippy::too_many_arguments)])
    } else {
        quote!()
    }
}

/// Generate code for all registered events
pub fn generate_events(
    out: &Path,
    namespace: &str,
    service_events: &[EventMetadata],
    http_events: &[EventMetadata],
    namespaces: &[TokenStream],
    core_crate: &TokenStream,
) -> TokenStream {
    // The tx-class umbrella completion event (`publish_<ns>_transaction`), injected
    // like service/http events so it flows through the normal metrics/publisher
    // pipeline. Empty when the crate declares no `#[event(tx_class)]` classes.
    let tx_class_events = tx_class_companion_event_metadata();

    let all_events: Vec<&EventMetadata> = EVENT_METADATA
        .iter()
        .copied()
        .chain(service_events.iter())
        .chain(http_events.iter())
        .chain(tx_class_events.iter())
        .collect();

    let publisher_subscriber_mapping = all_events
        .iter()
        .map(|event| event.publisher_method_impl(namespace, core_crate))
        .collect::<Vec<_>>();

    let subscriber_methods = all_events
        .iter()
        .map(|event| event.subscriber_method(namespace, core_crate));

    let publisher_methods = all_events
        .iter()
        .map(|event| event.publisher_method(namespace, core_crate));

    let services_import = if service_events.is_empty() {
        quote! {}
    } else {
        quote! {
            use super::service;
        }
    };

    let publisher = quote! {
        use super::*;

        pub trait Publisher {
            #(#publisher_methods)*
        }

        impl<T> Publisher for T
        where
            T: #core_crate::runtime::Env + WithSubscriber,
            <T as WithSubscriber>::Subscriber: Subscriber,
        {
            #(#publisher_subscriber_mapping)*
        }
    };
    emit::rust(out.join("publisher.rs"), publisher).unwrap();

    // Generate option subscriber methods
    {
        let option_subscriber_methods = all_events
            .iter()
            .map(|event| event.option_subscriber_method(namespace, core_crate));
        let option = quote! {
            use super::*;

            impl<A: Subscriber> Subscriber for Option<A> {
                #(#option_subscriber_methods)*
            }
        };
        emit::rust(out.join("option.rs"), option).unwrap();
    }

    // Generate tuple subscriber methods that delegate to both subscribers
    {
        let tuple_subscriber_methods = all_events
            .iter()
            .map(|event| event.tuple_subscriber_method(namespace, core_crate));

        let tuple = quote! {
            use super::*;

            impl Subscriber for () {}

            impl<A, B> Subscriber for (A, B)
            where
                A: Subscriber,
                B: Subscriber,
            {
                // Generate tuple implementations for composing subscribers
                #(#tuple_subscriber_methods)*
            }
        };
        emit::rust(out.join("tuple.rs"), tuple).unwrap();
    }

    let mut builder = metrics::Builder::new();
    all_events
        .iter()
        .for_each(|event| builder.add_event(namespace, event, core_crate));

    let metrics = builder.build(namespace, core_crate);
    emit::rust(out.join("metrics.rs"), metrics).unwrap();

    let tracing = tracing::generate_tracing(namespace, &all_events, core_crate);
    emit::rust(out.join("tracing.rs"), tracing).unwrap();

    let backbeat = backbeat::generate_backbeat(namespace, &all_events, core_crate);
    emit::rust(out.join("backbeat.rs"), backbeat).unwrap();

    let testing = testing::generate_testing(namespace, &all_events, core_crate);
    emit::rust(out.join("testing.rs"), testing).unwrap();

    let composed = generated_composed(namespaces, core_crate);
    emit::rust(out.join("composed.rs"), composed).unwrap();

    let task_classes = generate_task_classes(namespace, core_crate);
    emit::rust(out.join("task_classes.rs"), task_classes).unwrap();

    let tx_classes = generate_tx_classes(namespace, core_crate);
    emit::rust(out.join("tx_classes.rs"), tx_classes).unwrap();

    let namespace_struct = format_ident!("{}", namespace.to_pascal_case());

    quote! {
        #![allow(clippy::needless_lifetimes, clippy::extra_unused_lifetimes)]

        #services_import

        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Ord, PartialOrd, Hash)]
        pub struct #namespace_struct;

        pub type Namespace = #namespace_struct;

        pub trait WithSubscriber {
            type Subscriber: Subscriber;

            fn subscriber(&self) -> &Self::Subscriber;
        }

        impl<T> WithSubscriber for T
        where
            T: #core_crate::runtime::WithSubscriber<Namespace>,
            T::Subscriber: Subscriber
        {
            type Subscriber = T::Subscriber;

            fn subscriber(&self) -> &Self::Subscriber {
                #core_crate::runtime::WithSubscriber::<Namespace>::subscriber(self)
            }
        }

        pub trait Subscriber: Send + Sync + Clone + 'static {
            #(#subscriber_methods)*
        }

        mod composed;
        mod publisher;
        mod metrics;
        mod option;
        mod tracing;
        mod backbeat;
        #[cfg(any(test, feature = "testing"))]
        mod testing;
        mod tuple;
        mod task_classes;
        mod tx_classes;

        pub use composed::*;
        pub use publisher::*;
        pub use metrics::*;
        pub use tracing::*;
        pub use backbeat::*;
        #[cfg(any(test, feature = "testing"))]
        pub use testing::*;
        // A crate may declare no `#[event(task_class)]` classes, leaving this
        // module empty; allow the unused glob rather than conditionalize it.
        #[allow(unused_imports)]
        pub use task_classes::*;
        // Likewise a crate may declare no `#[event(tx_class)]` classes.
        #[allow(unused_imports)]
        pub use tx_classes::*;
    }
}

/// For every task class registered via `#[event(task_class)]` (and every class
/// derived from an RPC service/method), emits the marker `pub struct` and its
/// [`TaskClass<E>`] impl. The impl forwards each per-task hook directly to the
/// class's companion publisher methods, so an instrumented future submits raw
/// samples through the env on every poll/drop — no snapshot, no reporter loop.
///
/// `E` is bounded on this crate's `Publisher`, which is exactly what
/// `Env::spawn`'s `T: TaskClass<Self>` bound requires at each spawn site.
fn generate_task_classes(namespace: &str, core_crate: &TokenStream) -> TokenStream {
    let classes: Vec<_> = TASK_CLASS_METADATA
        .iter()
        .map(|meta| generate_task_class(&meta.struct_name, namespace, core_crate))
        .collect();

    // `use super::*;` brings the crate `Publisher` into scope for the impls; omit
    // it when this crate declares no `#[event(task_class)]` classes so the empty
    // module stays warning-clean.
    let import = if classes.is_empty() {
        quote!()
    } else {
        quote!(
            use super::*;
        )
    };

    quote! {
        #import

        #(#classes)*
    }
}

/// The generated umbrella-enum name that keys a namespace's tx-class completion
/// metric. One unit variant per `#[event(tx_class)]` class, named in
/// SCREAMING_SNAKE via `#[variant(name = ...)]`. Referenced by the injected
/// `Transaction` companion event's nominal dimensions.
fn tx_class_kind_ident() -> Ident {
    format_ident!("TxClassKind")
}

/// Emits `tx_classes.rs` for every `#[event(tx_class, category = ...)]` class in
/// the crate. Per namespace this produces, in one module:
///
/// - **`TxClassKind`** — a `#[derive(AsVariant)]` umbrella enum, one unit variant
///   per class (SCREAMING_SNAKE metric name). This is the single concrete enum the
///   companion metric event keys its nominal dimensions on (a struct field can
///   never be a metric dimension; only one `AsVariant` enum per nominal metric).
/// - **The class struct** for each class (deriving `Debug, Clone, PartialEq, Eq`),
///   with its declared fields — so `src/` can `client.commit(env, Foo { .. }, f)`.
/// - **`impl TxClass<E> for Foo`** supplying `category()` (from the attr),
///   `tx_context()` (label = SCREAMING_SNAKE name + each field colon-joined, so the
///   fields disambiguate distinct invocations), and `record_completed()` (fires
///   the namespace's `publish_<ns>_transaction` companion event, keyed by the
///   matching `TxClassKind` variant).
///
/// The `TxClass` trait + `Category` live in the replica crate, referenced by its
/// real crate name `upstream_replica` (always resolvable in the daemon/proxy
/// crates that declare tx classes; those alias it to `upstream` but the unaliased
/// name resolves too).
fn generate_tx_classes(namespace: &str, _core_crate: &TokenStream) -> TokenStream {
    let metas: Vec<&TxClassMetadata> = TX_CLASS_METADATA.iter().copied().collect();
    if metas.is_empty() {
        // Keep the module warning-clean when a crate declares no tx classes.
        return quote!();
    }

    let kind = tx_class_kind_ident();
    let tx_class_trait = quote!(upstream_replica::op::TxClass);
    let category_ty = quote!(upstream_replica::op::Category);

    // The umbrella enum: one unit variant per class, SCREAMING_SNAKE metric name.
    let kind_variants = metas.iter().map(|m| {
        let ident = format_ident!("{}", m.struct_name);
        let name = m.struct_name.to_shouty_snake_case();
        quote! {
            #[variant(name = #name)]
            #ident
        }
    });
    let kind_enum = quote! {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, upstream_core::metrics::AsVariant)]
        pub enum #kind {
            #(#kind_variants),*
        }
    };

    // The completion metric publisher method, `publish_<ns>_transaction`.
    let publish_transaction = format_ident!("publish_{}_transaction", namespace);

    let per_class = metas.iter().map(|m| {
        let ident = format_ident!("{}", m.struct_name);
        let shouty = m.struct_name.to_shouty_snake_case();
        let category_variant = format_ident!("{}", m.category);

        // Two shapes:
        //  * unit struct (no fields) — a STATIC op. `tx_context()` is the op's
        //    SCREAMING_SNAKE name; the call site constructs it as a unit value
        //    (`Read`), naming the op once.
        //  * a single `tx_context` field — a DYNAMIC op. It carries the exact
        //    runtime tx_context the pre-tx-class code built (`format_key!(…)`,
        //    `tx_id(…)`, …), stored as `TxContext` (`ByteVec`) with NO change of
        //    type or construction. A generated `new(impl Into<ByteVec>)` absorbs
        //    the `Vec<u8>`/`String`/`Bytes` → `ByteVec` coercion that `commit`'s
        //    old `impl Into<TxContext>` parameter used to do, so the call site
        //    passes its original context expression verbatim — no `.into()`.
        let is_dynamic = m.fields.len() == 1 && m.fields[0].name == "tx_context";
        let (struct_def, tx_context_body) = if m.fields.is_empty() {
            (
                quote! {
                    #[derive(Debug, Clone, PartialEq, Eq)]
                    pub struct #ident;
                },
                quote! { upstream_core::bytevec::ByteVec::from(#shouty) },
            )
        } else if is_dynamic {
            let fty = (m.fields[0].ty)();
            (
                quote! {
                    #[derive(Debug, Clone, PartialEq, Eq)]
                    pub struct #ident {
                        tx_context: #fty,
                    }

                    impl #ident {
                        /// Construct from the operation's runtime tx_context —
                        /// the same value/expression the pre-tx-class code passed
                        /// to `commit` (e.g. `format_key!(…)`). The `Into` bound
                        /// absorbs the `Vec<u8>`/`String`/`Bytes` → `ByteVec`
                        /// coercion, so call sites need no `.into()`.
                        pub fn new(tx_context: impl Into<#fty>) -> Self {
                            Self { tx_context: tx_context.into() }
                        }
                    }
                },
                quote! { self.tx_context.clone() },
            )
        } else {
            let msg = format!(
                "#[event(tx_class)] `{}` must be either a unit struct (static op) \
                 or carry a single `tx_context` field (dynamic op)",
                m.struct_name
            );
            (quote! { compile_error!(#msg); }, quote! { unreachable!() })
        };

        quote! {
            #struct_def

            impl<E: Publisher + ?Sized> #tx_class_trait<E> for #ident {
                fn category(&self) -> #category_ty {
                    #category_ty::#category_variant
                }

                fn tx_context(&self) -> upstream_core::bytevec::ByteVec {
                    #tx_context_body
                }

                fn record_completed(
                    &self,
                    env: &E,
                    retries: u32,
                    duration: core::time::Duration,
                    success: bool,
                ) {
                    env.#publish_transaction(retries, duration, success, #kind::#ident);
                }
            }
        }
    });

    quote! {
        use super::*;

        #kind_enum

        #(#per_class)*
    }
}

/// Builds the single companion completion [`EventMetadata`] for a namespace's tx
/// classes — the umbrella `Transaction` event keyed by the generated
/// [`TxClassKind`] enum. Pushed into the crate's event set (like
/// [`task_class_companion_event_metadata`]) so the crate `Publisher` gains
/// `publish_<ns>_transaction(retries, duration, success, class)` — the method the
/// generated `impl TxClass<E>::record_completed` calls.
///
/// Mirrors the previously hand-written per-component `Transaction` event: a
/// `retries` measure, a `duration` nominal-measure + a `class` nominal-counter
/// (both keyed by `TxClassKind`), and a `success` bool-counter. Returns an empty
/// vec when the crate declares no tx classes (so no event, no method).
pub fn tx_class_companion_event_metadata() -> Vec<EventMetadata> {
    if TX_CLASS_METADATA.is_empty() {
        return vec![];
    }

    fn leak_slice<V>(v: impl IntoIterator<Item = V>) -> &'static [V] {
        Box::leak(v.into_iter().collect::<Vec<_>>().into_boxed_slice())
    }

    // Referenced from the generated `events` module, where `pub use tx_classes::*`
    // brings `TxClassKind` into scope by bare name. A plain `fn` (not a closure)
    // so it coerces to the `fn() -> TokenStream` field type.
    fn variant_type() -> TokenStream {
        let kind = tx_class_kind_ident();
        quote!(#kind)
    }

    let fields = vec![
        FieldMetadata {
            name: "retries",
            docs: &[],
            ty: || quote!(u32),
            metrics: &[MetricMetadata::Measure {
                name: "retries",
                unit: "none",
                expression: None,
                optional: false,
            }],
            backbeat_skip: true,
        },
        FieldMetadata {
            name: "duration",
            docs: &[],
            ty: || quote!(core::time::Duration),
            metrics: leak_slice([MetricMetadata::NominalMeasure {
                name: "duration",
                unit: "duration",
                variant_field: || quote!(class),
                expression: None,
            }]),
            backbeat_skip: true,
        },
        FieldMetadata {
            name: "success",
            docs: &[],
            ty: || quote!(bool),
            metrics: &[MetricMetadata::BoolCounter {
                name: "success",
                expression: None,
            }],
            backbeat_skip: true,
        },
        FieldMetadata {
            name: "class",
            docs: &[],
            ty: variant_type,
            metrics: leak_slice([MetricMetadata::NominalCounter {
                name: "class",
                unit: None,
                variant_type,
                expression: None,
            }]),
            backbeat_skip: true,
        },
    ];

    vec![EventMetadata {
        name: "transaction",
        struct_name: "Transaction",
        docs: leak_slice([
            "Per-operation transaction completion: latency + count keyed by this \
             crate's granular tx classes. Generated from `#[event(tx_class)]` \
             declarations; the shared `upstream` `TransactionCompleted` carries \
             only the coarse `Category`.",
        ]),
        fields: leak_slice(fields),
        generics: || quote!(),
        level: "debug",
        event_type: EventType::Regular,
        backbeat_skip: true,
    }]
}

/// Emits the marker struct and `impl<E: Publisher> TaskClass<E>` for a single
/// task class named `struct_name`, forwarding hooks to its companion publishers
/// (`publish_<namespace>_<name>_{poll,first_poll,instrumented,dropped}`). Shared
/// by `#[event(task_class)]` classes and service/method-derived classes.
///
/// `core_crate` is only used to locate the `runtime::task_monitor` path; the
/// `Publisher` trait is referenced unqualified (in scope in the `events`
/// module). For emission into a different module, use
/// [`generate_task_class_with_publisher`].
pub(crate) fn generate_task_class(
    struct_name: &str,
    namespace: &str,
    core_crate: &TokenStream,
) -> TokenStream {
    let task_class = quote!(#core_crate::runtime::task_monitor::TaskClass);
    generate_task_class_inner(struct_name, namespace, &quote!(Publisher), &task_class)
}

/// Like [`generate_task_class`], but with an explicit path to the crate
/// `Publisher` trait (e.g. `super::super::super::events::Publisher`) so the
/// impl can be emitted outside the `events` module — used by the service
/// codegen, which places per-service/method classes in the `schema` module.
/// References `upstream_core` by absolute path (always addressable).
pub(crate) fn generate_task_class_with_publisher(
    struct_name: &str,
    namespace: &str,
    publisher: &TokenStream,
) -> TokenStream {
    let task_class = quote!(upstream_core::runtime::task_monitor::TaskClass);
    generate_task_class_inner(struct_name, namespace, publisher, &task_class)
}

fn generate_task_class_inner(
    struct_name: &str,
    namespace: &str,
    publisher: &TokenStream,
    task_class: &TokenStream,
) -> TokenStream {
    let ident = format_ident!("{}", struct_name);
    let snake = struct_name.to_snake_case();
    // Companion events are named `TaskClass<Name>{Poll,..}`; their publisher
    // methods are therefore `publish_<ns>_task_class_<name>_<kind>`. Keep this
    // `task_class_` segment in sync with `task_class_companion_event_metadata`
    // and the `task_class_companion_events` macro helper.
    let poll = format_ident!("publish_{}_task_class_{}_poll", namespace, snake);
    let first_poll = format_ident!("publish_{}_task_class_{}_first_poll", namespace, snake);
    let instrumented = format_ident!("publish_{}_task_class_{}_instrumented", namespace, snake);
    let dropped = format_ident!("publish_{}_task_class_{}_dropped", namespace, snake);

    quote! {
        #[derive(Default)]
        pub struct #ident;

        impl<E: #publisher> #task_class<E> for #ident {
            fn name() -> &'static str {
                #snake
            }
            fn publish_instrumented(env: &E) {
                env.#instrumented(1);
            }
            fn publish_dropped(env: &E) {
                env.#dropped(1);
            }
            fn publish_first_poll(env: &E, delay: core::time::Duration) {
                env.#first_poll(delay);
            }
            fn publish_poll(
                env: &E,
                scheduled: core::time::Duration,
                poll: core::time::Duration,
            ) {
                env.#poll(scheduled, poll);
            }
        }
    }
}

/// Builds the four companion [`EventMetadata`] values for a task class named
/// `struct_name`, mirroring the events the `#[event(task_class)]` macro
/// synthesizes — but at codegen time, for classes derived programmatically
/// (e.g. one per RPC service/method). Pushing these into the crate's
/// `service_events` makes the crate `Publisher` gain the
/// `publish_<ns>_<name>_{poll,first_poll,instrumented,dropped}` methods that
/// [`generate_task_class`] forwards to.
pub fn task_class_companion_event_metadata(struct_name: &str) -> Vec<EventMetadata> {
    fn leak_string(v: impl ToString) -> &'static str {
        Box::leak(v.to_string().into_boxed_str())
    }
    fn leak_slice<V>(v: impl IntoIterator<Item = V>) -> &'static [V] {
        Box::leak(v.into_iter().collect::<Vec<_>>().into_boxed_slice())
    }

    fn event(struct_name: &str, fields: Vec<FieldMetadata>, doc: &'static str) -> EventMetadata {
        let struct_name = leak_string(struct_name);
        EventMetadata {
            name: leak_string(struct_name.to_snake_case()),
            struct_name,
            docs: leak_slice([doc]),
            fields: leak_slice(fields),
            generics: || quote!(),
            // `none` suppresses the tracing event entirely (metrics-only).
            level: "none",
            event_type: EventType::Regular,
            // Task-class instrumentation is metrics-only; keep it out of backbeat.
            backbeat_skip: true,
        }
    }

    fn measure_field(name: &'static str) -> FieldMetadata {
        FieldMetadata {
            name,
            docs: &[],
            ty: || quote!(core::time::Duration),
            metrics: leak_slice([MetricMetadata::Measure {
                name,
                unit: "duration",
                expression: None,
                // Duration fields always record a sample; only `Option<_>`
                // fields tagged `#[optional_measure]` suppress the degenerate
                // case.
                optional: false,
            }]),
            backbeat_skip: true,
        }
    }

    fn counter_field(name: &'static str) -> FieldMetadata {
        FieldMetadata {
            name: "count",
            docs: &[],
            ty: || quote!(u64),
            metrics: leak_slice([MetricMetadata::Counter {
                name,
                unit: None,
                expression: None,
            }]),
            backbeat_skip: true,
        }
    }

    // `TaskClass` prefix marks these as task-class instrumentation in the flat
    // metric namespace and matches the publisher method names emitted by
    // `generate_task_class_inner` and the `task_class_companion_events` macro.
    vec![
        event(
            &format!("TaskClass{struct_name}Poll"),
            vec![
                measure_field("scheduled_duration"),
                measure_field("poll_duration"),
            ],
            "Per-poll scheduling/poll-duration samples for this task class.",
        ),
        event(
            &format!("TaskClass{struct_name}FirstPoll"),
            vec![measure_field("first_poll_delay")],
            "Spawn→first-poll delay sample for this task class.",
        ),
        event(
            &format!("TaskClass{struct_name}Instrumented"),
            vec![counter_field("instrumented_count")],
            "A task with this class was instrumented.",
        ),
        event(
            &format!("TaskClass{struct_name}Dropped"),
            vec![counter_field("dropped_count")],
            "A task with this class was dropped before completing.",
        ),
    ]
}

fn generated_composed(namespaces: &[TokenStream], core_crate: &TokenStream) -> TokenStream {
    // Generate a use statement for core_events if it's referenced as a namespace
    // and the core crate is not core_events itself
    let core_events_use = if core_crate.to_string() != "core_events"
        && namespaces.iter().any(|ns| ns.to_string() == "core_events")
    {
        quote!(use #core_crate::core_events;)
    } else {
        quote!()
    };

    let composed_generics: Vec<_> = (0..namespaces.len())
        .map(|idx| format_ident!("Sub{idx}"))
        .collect();
    let fields: Vec<_> = namespaces
        .iter()
        .map(|ns| {
            let v = ns.to_string().replace("::", "_").replace(" ", "");
            format_ident!("{v}")
        })
        .collect();

    let field_tys = composed_generics
        .iter()
        .zip(&fields)
        .map(|(ty, name)| quote!(pub #name: #ty,));

    let namespace_impls = namespaces.iter().zip(&composed_generics).zip(&fields).map(|((ns, ty), field)| {
        quote! {
            impl<CrateSub, #(#composed_generics),*> #core_crate::runtime::WithSubscriber<#ns::events::Namespace> for Composed<CrateSub, #(#composed_generics),*>
            where
                #ty: #ns::events::Subscriber,
             {
                type Subscriber = #ty;

                fn subscriber(&self) -> &Self::Subscriber {
                    &self.#field
                }
            }
        }
    });

    let defaults = namespaces
        .iter()
        .map(|ns| quote!(#ns::events::DefaultSubscriber));

    quote! {
        use super::*;
        #core_events_use

        // Composes via the existing 2-tuple `Subscriber` impl, nested. Backbeat's
        // own `capture` feature is the real on/off switch: with it off, every
        // `record` call dead-code-eliminates, so there's no upstream-side gate.
        pub type DefaultSubscriber = (Tracing, (Metrics, Backbeat));
        pub type DefaultComposed = Composed<DefaultSubscriber, #(#defaults),*>;

        #[derive(Clone, Debug, Default)]
        pub struct Composed<CrateSub, #(#composed_generics),*> {
            pub crate_sub: CrateSub,
            #(#field_tys)*
        }

        impl<CrateSub, #(#composed_generics),*> Composed<CrateSub, #(#composed_generics),*> {
            pub fn with_subscriber<NewCrateSub>(self, crate_sub: NewCrateSub) -> Composed<NewCrateSub, #(#composed_generics),*> {
                Composed {
                    crate_sub,
                    #(#fields: self.#fields),*
                }
            }
        }

        impl<CrateSub, #(#composed_generics),*> #core_crate::runtime::WithSubscriber<Namespace> for Composed<CrateSub, #(#composed_generics),*>
        where
            CrateSub: Subscriber,
         {
            type Subscriber = CrateSub;

            fn subscriber(&self) -> &Self::Subscriber {
                &self.crate_sub
            }
        }

        #(#namespace_impls)*
    }
}

/// Type of event
#[derive(Clone, Debug)]
pub enum EventType {
    /// A regular event that is emitted once
    Regular,
    /// A span that has entry fields and a result type
    Span {
        /// The type that will be passed to exit()
        result_type: fn() -> TokenStream,
    },
    HttpSpan {
        method: &'static str,
        path: &'static str,
        /// Custom metrics for request processing
        request_metrics: &'static [MetricMetadata],
        /// Custom metrics for response processing
        response_metrics: &'static [MetricMetadata],
    },
    /// A span wrapping the lifecycle of a service method
    MethodSpan {
        /// The name of the method that this span wraps
        method_name: &'static str,
        /// The path to the span trait that will be used for this method
        span_type: fn() -> TokenStream,
        /// The request type for this method
        request_type: fn() -> TokenStream,
        /// The response type for this method (None for unidirectional methods)
        response_type: Option<fn() -> TokenStream>,
        /// Custom metrics for request processing
        request_metrics: &'static [MetricMetadata],
        /// Custom metrics for response processing
        response_metrics: &'static [MetricMetadata],
    },
    /// A timer event that records elapsed time when drop
    Timer,
}

/// Metadata for an event type
#[derive(Clone)]
pub struct EventMetadata {
    /// The event name (e.g. "rpc.request_completed")
    pub name: &'static str,
    /// The struct name (e.g., "RpcRequestCompleted")
    pub struct_name: &'static str,
    /// Documentation for the event
    pub docs: &'static [&'static str],
    /// The event's fields and their metadata
    pub fields: &'static [FieldMetadata],
    /// Function to generate the struct's generics
    pub generics: fn() -> TokenStream,
    /// The tracing level for this event (defaults to debug)
    pub level: &'static str,
    /// The type of event (regular or span)
    pub event_type: EventType,
    /// When `true`, this event is excluded from the generated backbeat
    /// subscriber entirely (no event struct, empty subscriber body). Set by a
    /// struct-level `#[backbeat(skip)]`. Note that `level == "none"` events are
    /// also excluded from backbeat regardless of this flag.
    pub backbeat_skip: bool,
}

impl std::fmt::Debug for EventMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let generics = (self.generics)();
        f.debug_struct("EventMetadata")
            .field("name", &self.name)
            .field("struct_name", &self.struct_name)
            .field("docs", &self.docs)
            .field("fields", &self.fields)
            .field("generics", &generics.to_string())
            .field("level", &self.level)
            .field("backbeat_skip", &self.backbeat_skip)
            .finish()
    }
}

impl EventMetadata {
    fn method_name(&self, prefix: &str, namespace: &str) -> Ident {
        Ident::new(
            &format!("{prefix}_{namespace}_{}", self.struct_name.to_snake_case()),
            Span::call_site(),
        )
    }

    fn doc_attrs(&self) -> TokenStream {
        let header = format!("# Event `{}`", self.struct_name);
        let docs = self.docs.iter().map(|doc| {
            quote! { #[doc = #doc] }
        });
        quote!(
            #[doc = #header]
            #[doc = ""]
            #(#docs)*
        )
    }

    fn type_params(&self, with_env: bool, core_crate: &TokenStream) -> TokenStream {
        let params = (self.generics)();
        match (!with_env, params.is_empty()) {
            (true, true) => quote!(),
            (true, false) => quote!(<#params>),
            (false, true) => quote!(<E: #core_crate::runtime::Env>),
            (false, false) => quote!(<#params, E: #core_crate::runtime::Env>),
        }
    }

    fn return_type(&self, core_crate: &TokenStream) -> TokenStream {
        let span = self.span_type_with_generics();
        if span.is_empty() {
            quote!()
        } else {
            quote!(-> impl #core_crate::service::#span)
        }
    }

    fn span_type_with_generics(&self) -> TokenStream {
        match &self.event_type {
            EventType::Regular => quote!(),
            EventType::Span { result_type } => {
                let result_ty = result_type();
                quote!(Span<#result_ty>)
            }
            EventType::HttpSpan { .. } => {
                // HTTP spans return a response that implements IntoResponse
                quote!(Span<axum::response::Response>)
            }
            EventType::MethodSpan {
                span_type,
                request_type,
                response_type,
                ..
            } => {
                let span_ty = span_type();
                let req_ty = request_type();
                let args = if let Some(res_ty) = response_type {
                    let res_ty = res_ty();
                    quote!(<#req_ty, #res_ty>)
                } else {
                    quote!(<#req_ty>)
                };

                quote!(#span_ty #args)
            }
            EventType::Timer => {
                quote!(timer::Timer)
            }
        }
    }

    fn subscriber_method(&self, namespace: &str, core_crate: &TokenStream) -> TokenStream {
        let method_name = self.method_name("on", namespace);
        let event_docs = self.doc_attrs();
        let field_docs = self.fields.iter().flat_map(|f| f.doc_attrs());
        let type_params = self.type_params(true, core_crate);
        let field_names = self.fields.iter().map(|f| f.name_ident());
        let field_params = self.fields.iter().map(|f| f.param());
        let return_type = self.return_type(core_crate);
        // self + env + caller + one arg per event field; a field-rich event
        // legitimately exceeds clippy's 7-arg threshold and there is nothing to
        // refactor (operator-approved targeted allow).
        let too_many_args = too_many_args_allow(3 + self.fields.len());

        quote! {
            #event_docs
            #(#field_docs)*
            #too_many_args
            fn #method_name #type_params(&self, env: &E, caller: #core_crate::runtime::Caller, #(#field_params),*) #return_type {
                let _ = env;
                let _ = caller;
                #(let _ = #field_names;)*
            }
        }
    }

    fn option_subscriber_method(&self, namespace: &str, core_crate: &TokenStream) -> TokenStream {
        let method_name = self.method_name("on", namespace);
        let type_params = self.type_params(true, core_crate);
        let field_names = self
            .fields
            .iter()
            .map(|f| f.name_ident())
            .collect::<Vec<_>>();
        let field_params = self.fields.iter().map(|f| f.param());
        let return_type = self.return_type(core_crate);

        // A unit-returning method must not use `Option::map` purely for its side
        // effect (clippy::option_map_unit_fn); emit an `if let Some(..)` instead.
        // A value-returning method genuinely maps the inner subscriber's result
        // into an `Option<..>`, so `map` is correct there.
        let body = if return_type.is_empty() {
            quote! {
                if let Some(__subscriber__) = self.as_ref() {
                    __subscriber__.#method_name(env, caller, #(#field_names),*);
                }
            }
        } else {
            quote! {
                self.as_ref().map(|__subscriber__| __subscriber__.#method_name(env, caller, #(#field_names),*))
            }
        };

        quote! {
            fn #method_name #type_params(&self, env: &E, caller: #core_crate::runtime::Caller, #(#field_params),*) #return_type {
                #body
            }
        }
    }

    fn tuple_subscriber_method(&self, namespace: &str, core_crate: &TokenStream) -> TokenStream {
        let method_name = self.method_name("on", namespace);
        let type_params = self.type_params(true, core_crate);
        let field_names = self
            .fields
            .iter()
            .map(|f| f.name_ident())
            .collect::<Vec<_>>();
        let field_params = self.fields.iter().map(|f| f.param());
        let return_type = self.return_type(core_crate);

        match &self.event_type {
            EventType::Regular => {
                // For regular events, call both subscribers and return ()
                quote! {
                    fn #method_name #type_params(&self, env: &E, caller: #core_crate::runtime::Caller, #(#field_params),*) #return_type {
                        self.0.#method_name(env, caller, #(#field_names),*);
                        self.1.#method_name(env, caller, #(#field_names),*);
                    }
                }
            }
            EventType::Timer
            | EventType::Span { .. }
            | EventType::HttpSpan { .. }
            | EventType::MethodSpan { .. } => {
                quote! {
                    fn #method_name #type_params(&self, env: &E, caller: #core_crate::runtime::Caller, #(#field_params),*) #return_type {
                        let span_a = self.0.#method_name(env, caller, #(#field_names),*);
                        let span_b = self.1.#method_name(env, caller, #(#field_names),*);
                        (span_a, span_b)
                    }
                }
            }
        }
    }

    fn publisher_method(&self, namespace: &str, core_crate: &TokenStream) -> TokenStream {
        let method_name = self.method_name("publish", namespace);
        let with_caller_name = self.method_name("publish_with_caller", namespace);
        let event_docs = self.doc_attrs();
        let field_docs = self
            .fields
            .iter()
            .flat_map(|f| f.doc_attrs())
            .collect::<Vec<_>>();
        let type_params = self.type_params(false, core_crate);
        let field_params = self.fields.iter().map(|f| f.param()).collect::<Vec<_>>();
        let field_names = self.fields.iter().map(|f| f.name_ident());
        let return_type = self.return_type(core_crate);
        // `publish` = self + one arg per field; `publish_with_caller` adds caller.
        // A field-rich event legitimately exceeds clippy's 7-arg threshold
        // (operator-approved targeted allow).
        let publish_allow = too_many_args_allow(1 + self.fields.len());
        let with_caller_allow = too_many_args_allow(2 + self.fields.len());

        quote! {
            #event_docs
            #(#field_docs)*
            #[track_caller]
            #publish_allow
            fn #method_name #type_params(&self, #(#field_params),*) #return_type {
                let caller = #core_crate::runtime::Caller::capture();
                self.#with_caller_name(caller, #(#field_names),*)
            }

            #event_docs
            #(#field_docs)*
            #with_caller_allow
            fn #with_caller_name #type_params(&self, caller: #core_crate::runtime::Caller, #(#field_params), *) #return_type;
        }
    }

    fn publisher_method_impl(&self, namespace: &str, core_crate: &TokenStream) -> TokenStream {
        let publisher_name = self.method_name("publish_with_caller", namespace);
        let subscriber_name = self.method_name("on", namespace);
        let type_params = self.type_params(false, core_crate);
        let field_params = self.fields.iter().map(|f| f.param()).collect::<Vec<_>>();
        let field_names = self
            .fields
            .iter()
            .map(|f| f.name_ident())
            .collect::<Vec<_>>();
        let return_type = self.return_type(core_crate);

        quote! {
            #[track_caller]
            fn #publisher_name #type_params(&self, caller: #core_crate::runtime::Caller, #(#field_params),*) #return_type {
                // TODO use the cache once it's ready
                // let env = Cache::new(self);
                self.subscriber().#subscriber_name(self, caller, #(#field_names),*)
            }
        }
    }

    fn register_call(&self, metric_index: usize, info_index: usize) -> TokenStream {
        quote! {
            counters[#metric_index].write(registry.register_counter(&METRIC_INFO[#info_index]));
        }
    }

    fn record_call(&self, metric_index: usize, info_index: usize) -> TokenStream {
        quote!(self.inner.counters[#metric_index].record(&METRIC_INFO[#info_index], 1u64))
    }

    fn info(&self, namespace: &str, id: usize, core_crate: &TokenStream) -> TokenStream {
        let name = format!("{namespace}.{}\0", self.name);
        quote! {
            #core_crate::metrics::info::Builder {
                id: #id,
                name: #core_crate::metrics::info::Str::new(#name),
                units: #core_crate::metrics::Units::None,
            }.build()
        }
    }
}

/// Metadata for a field in an event
pub struct FieldMetadata {
    /// The field name (e.g., "endpoint")
    pub name: &'static str,
    /// Documentation for the field
    pub docs: &'static [&'static str],
    /// The field type (e.g., "&str")
    pub ty: fn() -> TokenStream,
    /// The metrics associated with this field
    pub metrics: &'static [MetricMetadata],
    /// When `true`, this field is excluded from the generated backbeat event
    /// struct even if its type is backbeat-POD-compatible. Set by a field-level
    /// `#[backbeat(skip)]` (e.g. to drop a high-cardinality primitive).
    pub backbeat_skip: bool,
}

impl std::fmt::Debug for FieldMetadata {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ty = (self.ty)();
        f.debug_struct("FieldMetadata")
            .field("name", &self.name)
            .field("docs", &self.docs)
            .field("ty", &ty.to_string())
            .field("metrics", &self.metrics)
            .field("backbeat_skip", &self.backbeat_skip)
            .finish()
    }
}

impl FieldMetadata {
    fn name_ident(&self) -> Ident {
        Ident::new(self.name, Span::call_site())
    }

    fn doc_attrs(&self) -> TokenStream {
        let header = format!("## `{}`", self.name);
        let docs = self.docs.iter().map(|doc| {
            quote! { #[doc = #doc] }
        });

        // Only include metrics section if there are metrics
        let metrics_section = if !self.metrics.is_empty() {
            let metrics = self.metrics.iter().map(|metric| {
                let desc = format!("- {}", metric.describe());
                quote! { #[doc = #desc] }
            });
            quote! {
                #[doc = ""]
                #[doc = "### Metrics"]
                #[doc = ""]
                #(#metrics)*
            }
        } else {
            quote!()
        };

        quote!(
            #[doc = #header]
            #[doc = ""]
            #(#docs)*
            #metrics_section
        )
    }

    fn param(&self) -> TokenStream {
        let name = self.name_ident();
        let ty = (self.ty)();
        quote! { #name: #ty }
    }

    fn param_unused(&self) -> TokenStream {
        let name = Ident::new(&format!("_{}", self.name), Span::call_site());
        let ty = (self.ty)();
        quote! { #name: #ty }
    }
}

/// Metadata for a metric type
#[derive(Clone, Debug)]
pub enum MetricMetadata {
    /// A simple counter.
    /// See https://docs.rs/metrics/latest/metrics/#counters:
    /// "A monotonically increasing value which can only be increased or be reset
    /// to zero on restart."
    /// For example: total requests.
    Counter {
        name: &'static str,
        unit: Option<&'static str>,
        expression: Option<fn() -> TokenStream>,
    },
    /// A sampled value.
    /// See https://docs.rs/metrics/latest/metrics/#histograms
    /// "stores an arbitrary number of observations of a specific measurement and
    /// provides statistical analysis over the observed values."
    /// For example: latency request.
    Measure {
        name: &'static str,
        unit: &'static str,
        expression: Option<fn() -> TokenStream>,
        /// When `true`, the field type is `Option<_>` and the histogram sample
        /// is recorded only when the value is `Some`. This lets an event omit a
        /// degenerate sample (e.g. a compression ratio for an empty publish)
        /// without dropping the whole event or feeding a placeholder value that
        /// would skew the distribution. The metric is still registered, so the
        /// name and any sibling counters on the event are unaffected.
        optional: bool,
    },
    /// A counter with buckets.
    /// "Nominal" here is the statistics definition, basically meaning
    /// "categorized into groups". This is a counter with a separate count
    /// for each variant.
    /// For example: total requests, but split by return code.
    NominalCounter {
        name: &'static str,
        unit: Option<&'static str>,
        variant_type: fn() -> TokenStream,
        expression: Option<fn() -> TokenStream>,
    },
    /// A measure with buckets.
    /// "Nominal" here is the statistics definition, basically meaning
    /// "categorized into groups". This is a measure with a separate histogram
    /// for each variant.
    /// For example: request latency, but split by return code.
    NominalMeasure {
        name: &'static str,
        unit: &'static str,
        variant_field: fn() -> TokenStream,
        expression: Option<fn() -> TokenStream>,
    },
    /// A boolean NominalCounter.
    /// A NominalCounter, but with only two fixed variants: "true" or "false".
    /// For example: total requests, but split by success=true/false.
    BoolCounter {
        name: &'static str,
        expression: Option<fn() -> TokenStream>,
    },
    /// A gauge with buckets.
    /// "Nominal" here is the statistics definition, basically meaning
    /// "categorized into groups". This is a gauge with a separate value
    /// for each variant.
    /// For example: active connections, but split by connection type.
    NominalGauge {
        name: &'static str,
        unit: &'static str,
        variant_field: fn() -> TokenStream,
        expression: Option<fn() -> TokenStream>,
    },
    /// A gauge.
    /// See https://docs.rs/metrics/latest/metrics/#gauges
    /// "A gauge is a metric that can go up and down, arbitrarily, over time."
    /// Basically, a counter where the value isn't strictly increasing.
    /// Each measurement can overwrite the previous measurement.
    /// For example: active nodes in a cluster
    Gauge {
        name: &'static str,
        unit: &'static str,
        expression: Option<fn() -> TokenStream>,
    },
}

impl MetricMetadata {
    fn unit_tokens(unit: &str, core_crate: &TokenStream) -> TokenStream {
        match unit {
            "none" => quote!(#core_crate::metrics::Units::None),
            "bytes" => quote!(#core_crate::metrics::Units::Bytes),
            "duration" => quote!(#core_crate::metrics::Units::Duration),
            "percent" => quote!(#core_crate::metrics::Units::Percent),
            _ => {
                let message = format!(
                    "invalid unit: {unit:?}. Valid units are: none, bytes, duration, percent"
                );
                quote!(compile_error!(#message))
            }
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Counter { name, unit, .. } => {
                if let Some(unit) = unit {
                    format!("Counts occurrences (`{name}`, unit: {unit})")
                } else {
                    format!("Counts occurrences (`{name}`)")
                }
            }
            Self::BoolCounter { name, .. } => {
                format!("Counts true values (`{name}`)")
            }
            Self::NominalCounter { name, unit, .. } => {
                if let Some(unit) = unit {
                    format!("Counts occurrences by value (`{name}`, unit: {unit})")
                } else {
                    format!("Counts occurrences by value (`{name}`)")
                }
            }
            Self::Measure { name, unit, .. } => {
                format!("Records {unit} measurements (`{name}`)")
            }
            Self::NominalMeasure { name, unit, .. } => {
                format!("Records {unit} measurements by value (`{name}`)")
            }
            Self::NominalGauge { name, unit, .. } => {
                format!("Tracks current {unit} value by variant (`{name}`)")
            }
            Self::Gauge { name, unit, .. } => {
                format!("Tracks current {unit} value (`{name}`)")
            }
        }
    }

    fn register_call(
        &self,
        metric_index: usize,
        info_index: usize,
        variant: &TokenStream,
    ) -> TokenStream {
        // TODO use the metric_index as the pointer offset
        match self {
            Self::Counter { .. } => quote! {
                counters[#metric_index].write(registry.register_counter(&METRIC_INFO[#info_index]));
            },
            Self::BoolCounter { .. } => quote! {
                bool_counters[#metric_index].write(registry.register_bool_counter(&METRIC_INFO[#info_index]));
            },
            Self::NominalCounter { .. } => quote! {{
                let offset = nominal_counters.len();
                let mut count = 0;
                for v in <#variant as AsVariant>::VARIANTS.iter() {
                    nominal_counters.push(registry.register_nominal_counter(&METRIC_INFO[#info_index], v));
                    count += 1;
                }
                debug_assert_ne!(count, 0, "field type needs at least one variant");
                offset
            }},
            Self::Measure { .. } => quote! {
                measures[#metric_index].write(registry.register_measure(&METRIC_INFO[#info_index]));
            },
            Self::NominalMeasure { .. } => quote! {{
                let offset = nominal_measures.len();
                let mut count = 0;
                for v in <#variant as AsVariant>::VARIANTS.iter() {
                    nominal_measures.push(registry.register_nominal_measure(&METRIC_INFO[#info_index], v));
                    count += 1;
                }
                debug_assert_ne!(count, 0, "field type needs at least one variant");
                offset
            }},
            Self::NominalGauge { .. } => quote! {{
                let offset = nominal_gauges.len();
                let mut count = 0;
                for v in <#variant as AsVariant>::VARIANTS.iter() {
                    nominal_gauges.push(registry.register_nominal_gauge(&METRIC_INFO[#info_index], v));
                    count += 1;
                }
                debug_assert_ne!(count, 0, "field type needs at least one variant");
                offset
            }},
            Self::Gauge { .. } => quote! {
                gauges[#metric_index].write(registry.register_gauge(&METRIC_INFO[#info_index]));
            },
        }
    }

    fn info(
        &self,
        namespace: &str,
        event: &EventMetadata,
        id: usize,
        core_crate: &TokenStream,
    ) -> TokenStream {
        let name = match self {
            Self::Counter { name, .. } => name,
            Self::BoolCounter { name, .. } => name,
            Self::NominalCounter { name, .. } => name,
            Self::Measure { name, .. } => name,
            Self::NominalMeasure { name, .. } => name,
            Self::NominalGauge { name, .. } => name,
            Self::Gauge { name, .. } => name,
        };
        let name = format!("{namespace}.{}.{name}\0", event.name);

        // TODO match the unit strings to correct variants from s2n-quic-core

        let units = match self {
            Self::Counter { unit, .. } => {
                if let Some(unit) = unit {
                    Self::unit_tokens(unit, core_crate)
                } else {
                    quote!(#core_crate::metrics::Units::None)
                }
            }
            Self::BoolCounter { .. } => quote!(#core_crate::metrics::Units::None),
            Self::NominalCounter { unit, .. } => {
                if let Some(unit) = unit {
                    Self::unit_tokens(unit, core_crate)
                } else {
                    quote!(#core_crate::metrics::Units::None)
                }
            }
            Self::Measure { unit, .. } => Self::unit_tokens(unit, core_crate),
            Self::NominalMeasure { unit, .. } => Self::unit_tokens(unit, core_crate),
            Self::NominalGauge { unit, .. } => Self::unit_tokens(unit, core_crate),
            Self::Gauge { unit, .. } => Self::unit_tokens(unit, core_crate),
        };

        quote! {
            #core_crate::metrics::info::Builder {
                id: #id,
                name: #core_crate::metrics::info::Str::new(#name),
                units: #units,
            }.build()
        }
    }

    fn record_call<V: quote::ToTokens>(
        &self,
        metric_index: usize,
        info_index: usize,
        value: &V,
        core_crate: &TokenStream,
    ) -> TokenStream {
        self.record_call_with_inner(
            metric_index,
            info_index,
            value,
            &quote!(self.inner),
            core_crate,
        )
    }

    fn record_call_with_inner<V: quote::ToTokens, I: quote::ToTokens>(
        &self,
        metric_index: usize,
        info_index: usize,
        value: &V,
        inner: &I,
        core_crate: &TokenStream,
    ) -> TokenStream {
        // TODO the checkpoint metric needs to query the env with the current time and measure the elapsed time within the span
        let value = value.to_token_stream();

        macro_rules! expr {
            ($expression:ident) => {
                expr!($expression, value)
            };
            ($expression:ident, $default_expr:expr) => {{
                let expr = if let Some(expr) = $expression {
                    let expr = expr();
                    if expr.is_empty() {
                        quote!(compile_error!("metric expression is empty"))
                    } else if value.is_empty() {
                        expr
                    } else {
                        quote!((#value).#expr)
                    }
                } else {
                    $default_expr
                };
                if expr.is_empty() {
                    quote!(compile_error!("metric expression is empty"))
                } else {
                    expr
                }
            }};
        }

        match self {
            Self::Counter { expression, .. } => {
                let expr = expr!(expression, quote!(#value));
                quote!(#inner.counters[#metric_index].record(&METRIC_INFO[#info_index], #expr))
            }
            Self::Measure {
                expression,
                optional,
                ..
            } => {
                if *optional {
                    // The field is an `Option<_>`; only record a sample when it
                    // is `Some`. Any expression is applied to the unwrapped inner
                    // value so `#[optional_measure("x", "unit", .field)]` still
                    // works. `None` records nothing, keeping placeholder /
                    // not-applicable observations out of the histogram.
                    let inner_expr = if let Some(expression) = expression {
                        let expression = expression();
                        if expression.is_empty() {
                            quote!(compile_error!("metric expression is empty"))
                        } else {
                            quote!((__optional_measure_value).#expression)
                        }
                    } else {
                        quote!(__optional_measure_value)
                    };
                    quote!(if let Some(__optional_measure_value) = #value {
                        #inner.measures[#metric_index].record(&METRIC_INFO[#info_index], #inner_expr)
                    })
                } else {
                    let expr = expr!(expression);
                    quote!(#inner.measures[#metric_index].record(&METRIC_INFO[#info_index], #expr))
                }
            }
            Self::Gauge { expression, .. } => {
                let expr = expr!(expression);
                quote!(#inner.gauges[#metric_index].record(&METRIC_INFO[#info_index], #expr))
            }
            Self::BoolCounter { expression, .. } => {
                let expr = expr!(expression);
                quote!(#inner.bool_counters[#metric_index].record(&METRIC_INFO[#info_index], #expr))
            }
            Self::NominalCounter { expression, .. } => {
                let expr = expr!(expression);
                quote!({
                    use #core_crate::metrics::NominalRecorder;
                    let v = (#expr).as_variant();
                    #inner.nominal_counters[#inner.nominal_counters_offsets[#metric_index] + v.id].record(&METRIC_INFO[#info_index], v, 1u64)
                })
            }
            Self::NominalMeasure {
                expression,
                variant_field,
                ..
            } => {
                let expr = expr!(expression);
                let variant = variant_field();

                quote!({
                    use #core_crate::metrics::NominalRecorder;
                    let v = (#variant).as_variant();
                    #inner.nominal_measures[#inner.nominal_measures_offsets[#metric_index] + v.id].record(&METRIC_INFO[#info_index], v, #expr)
                })
            }
            Self::NominalGauge {
                expression,
                variant_field,
                ..
            } => {
                let expr = expr!(expression);
                let variant = variant_field();

                quote!({
                    use #core_crate::metrics::NominalRecorder;
                    let v = (#variant).as_variant();
                    #inner.nominal_gauges[#inner.nominal_gauges_offsets[#metric_index] + v.id].record(&METRIC_INFO[#info_index], v, #expr)
                })
            }
        }
    }
}
