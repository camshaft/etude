use crate::{Endpoint, EventMetadata, emit, events, http};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug)]
pub struct Builder {
    pub namespace: String,
    pub endpoints: Vec<Endpoint>,
    pub event_namespaces: Vec<String>,
    pub http_routers: Vec<http::Router>,
    /// The crate path used in generated code for runtime, metrics, etc.
    /// Defaults to `upstream_core`. Override with `core_crate()` for crates
    /// that cannot depend on `upstream-core` directly.
    pub core_crate: TokenStream,
}

impl Builder {
    pub fn new(namespace: impl ToString) -> Self {
        Self {
            namespace: namespace.to_string(),
            endpoints: vec![],
            event_namespaces: vec![],
            http_routers: vec![],
            core_crate: quote!(upstream_core),
        }
    }

    /// Override the core crate path used in generated code.
    ///
    /// By default, generated code references `upstream_core::runtime`,
    /// `upstream_core::metrics`, etc. For crates that sit below `upstream-core`
    /// in the dependency graph, set this to a crate that re-exports those
    /// sub-crates (e.g. `"core_events"`).
    pub fn core_crate(mut self, path: &str) -> Self {
        self.core_crate = path.parse().expect("valid crate path");
        self
    }

    pub fn with(mut self, endpoint: Endpoint) -> Self {
        self.endpoints.push(endpoint);
        self
    }

    pub fn with_events(mut self, namespace_path: &str) -> Self {
        self.event_namespaces.push(namespace_path.to_string());
        self
    }

    pub fn with_http_router(mut self, router: http::Router) -> Self {
        self.http_routers.push(router);
        self
    }

    pub fn build<P: AsRef<Path>>(mut self, entry: P) {
        // Always include core_events unless this IS the core_events crate
        let core_crate_str = self.core_crate.to_string();
        if core_crate_str != "core_events"
            && !self.event_namespaces.iter().any(|ns| ns == "core_events")
        {
            self.event_namespaces.push("core_events".to_string());
        }

        let entry = entry.as_ref();
        let dir = entry.with_extension("");
        let dir = dir.as_path();

        let endpoints = self.endpoints.iter().map(|endpoint| {
            let name = Ident::new(&endpoint.name, Span::call_site());

            endpoint.build(dir.join("endpoint"));

            quote! {
                pub mod #name;
            }
        });

        emit::rust(dir.join("endpoint.rs"), quote! { #(#endpoints)* }).unwrap();

        let mut service_events = vec![];
        let services = self.generate_services(dir, &mut service_events);
        let mut http_events = vec![];
        let routers = self.generate_http_routers(dir, &mut http_events);

        let composed_namespaces = self
            .event_namespaces
            .iter()
            .map(|namespace| {
                let v: proc_macro2::TokenStream = namespace.parse().unwrap();
                v
            })
            .collect::<Vec<_>>();

        let events = events::generate_events(
            &dir.join("events"),
            &self.namespace,
            &service_events,
            &http_events,
            &composed_namespaces,
            &self.core_crate,
        );
        emit::rust(dir.join("events.rs"), events).unwrap();

        let core_crate = &self.core_crate;

        // Generate trait bounds for the Env trait
        let mut env_trait_bounds = vec![
            quote! { #core_crate::runtime::Env },
            quote! { events::Publisher },
            quote! { events::WithSubscriber },
        ];

        // Add trait bounds for additional namespaces
        for ns in &composed_namespaces {
            env_trait_bounds.push(quote! { #ns::Env });
        }

        // Generate trait bounds as a token stream
        let env_trait_bounds_tokens = quote! {
            #(#env_trait_bounds)+*
        };

        // Generate return type trait bounds for environment factory functions
        let mut factory_trait_bounds = vec![quote! { Env }];
        let mut snapshot_trait_bounds = vec![quote! { Env }];

        // Add WithSubscriber trait bounds for the factory functions
        factory_trait_bounds.push(
            quote! { WithSubscriber<events::Namespace, Subscriber = events::DefaultSubscriber> },
        );
        snapshot_trait_bounds.push(
            quote! { WithSubscriber<events::Namespace, Subscriber = (events::Testing, events::DefaultSubscriber)> },
        );
        for ns in &composed_namespaces {
            let tokens = quote! { WithSubscriber<#ns::events::Namespace, Subscriber = #ns::events::DefaultSubscriber> };
            factory_trait_bounds.push(tokens.clone());
            snapshot_trait_bounds.push(tokens);
        }

        // Generate the factory return type
        let factory_return_type = quote! {
            impl #(#factory_trait_bounds)+*
        };
        let snapshot_return_type = quote! {
            impl #(#snapshot_trait_bounds)+*
        };

        // Generate panic callback registration for tokio_env
        // Only when core_events is a composed namespace (not when building core_events itself)
        let core_events_use = if core_crate_str != "core_events" {
            quote!(use #core_crate::core_events;)
        } else {
            quote!()
        };
        let panic_callback_registration = if core_crate_str != "core_events" {
            quote! {
                #core_crate::core_events::set_panic_callback({
                    let env = env.clone();
                    move |message, location| {
                        use #core_crate::core_events::events::Publisher as _;
                        env.publish_core_events_panic(message, location);
                    }
                });
            }
        } else {
            quote!()
        };

        // Body of the `tokio_env` init closure. When core_events is composed we
        // must bind `env` so the panic callback can capture it, then return it;
        // building core_events itself has no registration, so return the
        // constructed env directly (avoids clippy::let_and_return).
        let tokio_env_init = if core_crate_str != "core_events" {
            quote! {
                let env = runtime::tokio::Env::new(trace, span_id, subscriber);
                #panic_callback_registration
                env
            }
        } else {
            quote! {
                runtime::tokio::Env::new(trace, span_id, subscriber)
            }
        };

        emit::rust(
            dir.join("env.rs"),
            quote! {
                use super::{
                    Env,
                    events,
                };
                #core_events_use
                use #core_crate::runtime::{self, Trace, WithSubscriber};

                #[allow(dead_code)]
                pub fn tokio_env() -> #factory_return_type {
                    static ENV: std::sync::OnceLock<runtime::tokio::Env<events::DefaultComposed>> =
                        std::sync::OnceLock::new();
                    ENV.get_or_init(|| {
                        let trace = Trace::new([0; 16]);
                        let span_id = 0u64;
                        let subscriber = events::DefaultComposed::default();
                        #tokio_env_init
                    }).clone()
                }

                #[allow(dead_code)]
                pub fn tokio_env_with_runtime(
                    handle: #core_crate::runtime::tokio::RuntimeHandle,
                ) -> impl #(#factory_trait_bounds)+* {
                    runtime::tokio::RuntimeEnv::new(tokio_env(), handle)
                }

                #[cfg(any(test, feature = "testing"))]
                #[allow(dead_code)]
                pub fn bach_env() -> #factory_return_type {
                    let trace = Trace::new([0; 16]);
                    let span_id = 0u64;
                    let subscriber = events::DefaultComposed::default();
                    runtime::bach::Env::new(trace, span_id, subscriber)
                }

                #[cfg(any(test, feature = "testing"))]
                #[allow(dead_code)]
                pub fn tokio_snapshot_env(on_drop: fn(String)) -> #snapshot_return_type {
                    #core_crate::testing::init_tracing();

                    let trace = Trace::new([0; 16]);
                    let span_id = 0u64;
                    let subscriber = events::DefaultComposed::default().with_subscriber(
                        (
                            events::Testing::new(false, on_drop),
                            events::DefaultSubscriber::default(),
                        )
                    );
                    runtime::tokio::Env::new(trace, span_id, subscriber)
                }

                #[cfg(any(test, feature = "testing"))]
                #[allow(dead_code)]
                pub fn bach_snapshot_env(on_drop: fn(String)) -> #snapshot_return_type {
                    #core_crate::testing::init_tracing();

                    let trace = Trace::new([0; 16]);
                    let span_id = 0u64;
                    let subscriber = events::DefaultComposed::default().with_subscriber(
                        (
                            events::Testing::new(true, on_drop),
                            events::DefaultSubscriber::default(),
                        )
                    );
                    runtime::bach::Env::new(trace, span_id, subscriber)
                }

                #[macro_export]
                macro_rules! snapshot_env {
                    () => {
                        $crate::env::tokio_snapshot_env(|events| {
                            insta::assert_snapshot!(events);
                        })
                    };
                    (bach) => {
                        $crate::env::bach_snapshot_env(|events| {
                            insta::assert_snapshot!(events);
                        })
                    };
                    (tokio) => {
                        $crate::env::tokio_snapshot_env(|events| {
                            insta::assert_snapshot!(events);
                        })
                    };
                }
            },
        )
        .unwrap();

        emit::rust(
            entry,
            quote!(
                #![allow(dead_code)]

                #core_events_use

                pub mod endpoint;
                pub mod events;
                pub mod env;
                #routers
                #services

                /// Environment trait that combines the base environment capabilities with service-specific publishers
                pub trait Env: #env_trait_bounds_tokens {}

                /// Blanket implementation for any type that implements all required traits
                impl<T> Env for T where T: #env_trait_bounds_tokens {}
            ),
        )
        .unwrap();
    }

    fn generate_services(&self, dir: &Path, events: &mut Vec<EventMetadata>) -> TokenStream {
        if self.endpoints.is_empty() {
            return quote!();
        }

        let mut srvs = BTreeMap::<&str, Srvs>::new();

        for endpoint in &self.endpoints {
            for name in endpoint.consumes.keys() {
                srvs.entry(name).or_default().is_consumed = true;
            }

            for name in endpoint.provides.keys() {
                srvs.entry(name).or_default().is_provided = true;
            }
        }

        let services = srvs.iter().map(|(name, srvs)| {
            let provider = if srvs.is_provided {
                Some(quote! {
                    pub mod provider;
                    pub use provider::Provider;
                })
            } else {
                None
            };
            let consumer = if srvs.is_consumed {
                Some(quote! {
                    pub mod consumer;
                    pub use consumer::Consumer;
                })
            } else {
                None
            };

            emit::rust(
                dir.join(format!("service/{name}.rs")),
                quote! {
                    #consumer
                    #provider
                    pub mod schema;
                    pub use schema::Service;
                },
            )
            .unwrap();

            let name = Ident::new(name, Span::call_site());
            quote! {
                pub mod #name;
            }
        });

        emit::rust(dir.join("service.rs"), quote! { #(#services)* }).unwrap();

        for endpoint in &self.endpoints {
            for (name, service) in endpoint.consumes.iter() {
                let srv = srvs.get_mut(name.as_str()).unwrap();
                if !core::mem::take(&mut srv.is_consumed) {
                    continue;
                }

                service.build_consumer(
                    &self.namespace,
                    &dir.join(format!("service/{name}/consumer.rs")),
                );

                if !core::mem::replace(&mut srv.emitted_schema, true) {
                    service.build_schema(
                        &self.namespace,
                        &dir.join(format!("service/{name}/schema.rs")),
                    );
                }

                if !core::mem::replace(&mut srv.emitted_events, true) {
                    events.extend_from_slice(&service.events);
                    events.extend(service.task_class_events());
                }
            }

            for (name, (service, _impl_path)) in endpoint.provides.iter() {
                let srv = srvs.get_mut(name.as_str()).unwrap();
                if !core::mem::take(&mut srv.is_provided) {
                    continue;
                }

                service.build_provider(
                    &self.namespace,
                    &dir.join(format!("service/{name}/provider.rs")),
                );

                if !core::mem::replace(&mut srv.emitted_schema, true) {
                    service.build_schema(
                        &self.namespace,
                        &dir.join(format!("service/{name}/schema.rs")),
                    );
                }

                if !core::mem::replace(&mut srv.emitted_events, true) {
                    events.extend_from_slice(&service.events);
                    events.extend(service.task_class_events());
                }
            }
        }

        quote!(
            pub mod service;
        )
    }

    fn generate_http_routers(&self, dir: &Path, events: &mut Vec<EventMetadata>) -> TokenStream {
        if self.http_routers.is_empty() {
            return quote!();
        }

        // Generate individual router modules
        let router_modules = self.http_routers.iter().map(|router| {
            let router_name = router.name();
            let router_ident = Ident::new(router_name, Span::call_site());

            // Generate the router implementation
            let tokens = router.build(&self.namespace);

            emit::rust(
                dir.join("http").join(router_name).with_extension("rs"),
                tokens,
            )
            .unwrap();

            // Collect events from this router
            events.extend(router.generate_events(&self.namespace));

            quote! {
                pub mod #router_ident;
            }
        });

        // Generate the main http module
        let http_module = quote! {
            #(#router_modules)*
        };

        emit::rust(dir.join("http.rs"), http_module).unwrap();

        quote!(
            pub mod http;
        )
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Srvs {
    is_consumed: bool,
    is_provided: bool,
    emitted_events: bool,
    emitted_schema: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{Input, Method, Output, Service};
    use std::{fs, path::PathBuf};

    fn probe() -> Service {
        Service::new("probe").with(Method {
            name: "ping".to_string(),
            description: vec!["Answers a ping with its nonce.".to_string()],
            inputs: vec![Input {
                name: "nonce".to_string(),
                description: vec![],
                ty: "u64".to_string(),
            }],
            output: Output {
                description: String::new(),
                ty: "u64".to_string(),
            },
            unidirectional: false,
            unreliable: false,
            stream_request: false,
            stream_response: false,
            request_header: None,
            response_header: None,
            peer_addr: false,
            raw_response: false,
            priority: "Medium".to_string(),
        })
    }

    fn builder() -> Builder {
        Builder::new("probe")
            .with(Endpoint::new("probe_server").provides(probe(), "crate::server::Server"))
            .with(Endpoint::new("probe_client").consumes(probe()))
            .with_events("chitchat")
    }

    /// Every file under `root`, keyed by its path below `root`, with its bytes.
    fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(root, &path, files);
                } else {
                    let bytes = fs::read(&path).unwrap();
                    files.insert(path.strip_prefix(root).unwrap().to_path_buf(), bytes);
                }
            }
        }

        let mut files = BTreeMap::new();
        walk(root, root, &mut files);
        files
    }

    /// A build script's output is an input of the compile cache key of the crate that includes it,
    /// so one builder writes the same bytes wherever it writes them, and no file names the directory
    /// it was written to.
    #[test]
    fn one_builder_writes_the_same_bytes_into_two_directories() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        builder().build(a.path().join("src/generated.rs"));
        builder().build(b.path().join("src/generated.rs"));
        let (a_files, b_files) = (tree(a.path()), tree(b.path()));

        assert!(
            a_files.contains_key(Path::new("src/generated/service/probe/provider.rs")),
            "{:?}",
            a_files.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            a_files.keys().collect::<Vec<_>>(),
            b_files.keys().collect::<Vec<_>>()
        );
        for (path, bytes) in &a_files {
            assert!(
                *bytes == b_files[path],
                "{} differs between the two directories",
                path.display()
            );
            let source = String::from_utf8_lossy(bytes);
            for dir in [a.path(), b.path()] {
                assert!(
                    !source.contains(dir.to_str().unwrap()),
                    "{} names the directory it was written to",
                    path.display()
                );
            }
        }
    }
}
