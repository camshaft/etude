use crate::{emit, events::EventMetadata};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{ToTokens, quote};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug)]
pub struct Service {
    pub namespace: String,
    pub methods: BTreeMap<String, Method>,
    pub events: Vec<EventMetadata>,
}

impl Service {
    pub fn new<N: AsRef<str>>(namespace: N) -> Self {
        Self {
            namespace: namespace.as_ref().to_string(),
            methods: BTreeMap::new(),
            events: vec![],
        }
    }

    pub fn with(mut self, method: Method) -> Self {
        self.methods.insert(method.name.clone(), method);
        self
    }

    pub fn with_event(mut self, event: EventMetadata) -> Self {
        self.events.push(event);
        self
    }

    /// PascalCase name of this service's connection-level (outer) task class,
    /// e.g. service `gossip` → `GossipService`. The instrumented connection task
    /// spawned by the RPC server uses this class; its metrics roll up all RPC
    /// handling for the service.
    fn task_class_service_name(&self) -> String {
        use heck::ToPascalCase;
        format!("{}Service", self.namespace.to_pascal_case())
    }

    /// PascalCase name of the per-method (inner, server-side) task class for
    /// `method`, e.g. service `gossip` method `notify` → `GossipNotify`. The
    /// generated provider dispatch wraps each method's handling in this class.
    fn task_class_method_name(&self, method: &Method) -> String {
        use heck::ToPascalCase;
        format!(
            "{}{}",
            self.namespace.to_pascal_case(),
            method.name.to_pascal_case()
        )
    }

    /// PascalCase name of the per-method CLIENT task class for `method`, e.g.
    /// `GossipNotifyClient`. Distinct from the server-side class so client vs
    /// server metrics are unambiguous. Only generated for methods whose consumer
    /// impl spawns a request-write pump (bidi-streaming) — see
    /// [`method_has_client_task_class`](Self::method_has_client_task_class).
    fn task_class_method_client_name(&self, method: &Method) -> String {
        format!("{}Client", self.task_class_method_name(method))
    }

    /// Whether `method`'s consumer impl spawns a detached request-write pump
    /// (and thus needs a per-method client task class). Only bidi-streaming
    /// methods do; every other shape writes its request inline.
    fn method_has_client_task_class(method: &Method) -> bool {
        method.stream_request && method.stream_response
    }

    /// Names of every task class derived from this service: one outer
    /// per-service class, one inner (server) class per method, and one client
    /// class per bidi-streaming method.
    fn task_class_names(&self) -> Vec<String> {
        let mut names = vec![self.task_class_service_name()];
        for method in self.methods.values() {
            names.push(self.task_class_method_name(method));
            if Self::method_has_client_task_class(method) {
                names.push(self.task_class_method_client_name(method));
            }
        }
        names
    }

    /// Companion metrics events for every task class this service derives.
    /// Pushed into the crate's `service_events` so the crate `Publisher` gains
    /// the `publish_<ns>_<class>_{poll,first_poll,instrumented,dropped}` methods
    /// that the generated `TaskClass` impls forward to.
    pub(crate) fn task_class_events(&self) -> Vec<EventMetadata> {
        self.task_class_names()
            .iter()
            .flat_map(|name| crate::events::task_class_companion_event_metadata(name))
            .collect()
    }

    /// Emits the marker struct + `impl TaskClass<E>` for every task class this
    /// service derives (one outer per-service class, one inner per method),
    /// placed in the service's `schema` module. The impls forward to the crate
    /// `Publisher` (in scope from the schema module as `super::super::super`),
    /// so spawning/awaiting them requires an env that publishes `crate_namespace`.
    fn generate_task_classes(&self, crate_namespace: &str) -> TokenStream {
        // From `<crate>/service/<svc>/schema.rs`, the crate root publisher is
        // three modules up (matching how the provider names `super::super::super::Env`).
        let publisher = quote!(super::super::super::events::Publisher);
        let names = self.task_class_names();
        let classes = names.iter().map(|name| {
            crate::events::generate_task_class_with_publisher(name, crate_namespace, &publisher)
        });
        quote! {
            #(#classes)*
        }
    }

    pub(crate) fn build_schema(&self, crate_namespace: &str, out: &Path) {
        let method_init = self.method_info();
        let method_len = self.methods.len();

        let method_list = self.methods.values().map(|method| {
            let method_name = Ident::new(&format!("METHOD_{}", method.name), Span::call_site());
            quote! {
                &#method_name
            }
        });

        let method_variants = self.methods.values().map(|method: &Method| {
            let name = method.struct_name();
            let docs = method.docs();

            quote! {
                #(#docs)*
                #name,
            }
        });

        let request_to_method = self.methods.values().map(|method| {
            let name = method.struct_name();

            quote! {
                Request::#name { .. } => Methods::#name
            }
        });

        let method_lookup: Vec<_> = self
            .methods
            .values()
            .map(|method| {
                let name = method.struct_name();
                let method_name = Ident::new(&format!("METHOD_{}", method.name), Span::call_site());

                quote! {
                    Self::#name => &#method_name,
                }
            })
            .collect();

        let global_id_lookup: Vec<_> = self
            .methods
            .values()
            .map(|method| {
                let name = method.struct_name();
                let method_name = Ident::new(&format!("METHOD_{}", method.name), Span::call_site());

                // Match on the method's global_id directly, or on any global_id in
                // the versions array. Non-versioned methods have versions: &[], so
                // they match only on global_id. Versioned methods also check the
                // versions array to recognize older wire layouts from peers running
                // previous schema versions.
                quote! {
                    id if id == #method_name.global_id || #method_name.versions.contains(id) => Some(Self::#name),
                }
            })
            .collect();

        let request_variants = self
            .methods
            .values()
            .map(|method| method.generate_request_variant());
        let request_header_variants = self
            .methods
            .values()
            .map(|method| method.generate_request_header_variant());
        let response_variants = self
            .methods
            .values()
            .map(|method| method.generate_response_variant());
        let response_header_variants = self
            .methods
            .values()
            .map(|method| method.generate_response_header_variant());

        let supported_set_fields = self.methods.values().map(|method| {
            let name = method.name();
            quote! {
                #name: upstream_core::service::MethodSupport,
            }
        });

        let supported_set_none = self.methods.values().map(|method| {
            let name = method.name();
            quote! {
                #name: upstream_core::service::MethodSupport::Unsupported,
            }
        });

        let supported_set_all = self.methods.values().map(|method| {
            let name = method.name();
            // Supported at the current version → its real current-version entry,
            // built inline (newest cross-product indices). Because this reads a
            // `static` field it lives in the `all()` runtime builder, not a
            // `const` (see the removal of `const Set::ALL`).
            let current_entry = method.current_version_entry_expr();
            quote! {
                #name: upstream_core::service::MethodSupport::Supported(#current_entry),
            }
        });

        let supported_set_iter = self.methods.values().enumerate().map(|(index, method)| {
            let name = method.name();
            let struct_name = method.struct_name();

            quote! {
                #index if self.0.#name.is_supported() => return Some(Methods::#struct_name),
            }
        });

        let supported_set_insert_with_index: Vec<_> = self
            .methods
            .values()
            .enumerate()
            .map(|(index, method)| {
                let name = method.name();

                // A peer that advertises the method's *current* global_id is
                // asking for the current wire layout. Record the real
                // current-version entry (primary global_id + newest
                // cross-product indices) so a negotiated current version is a
                // concrete entry, not absence. The old code fabricated
                // `input_version_idx = versions.len()`, only correct when
                // `OUT_COUNT == 1`; this inline (0,0) entry is always right.
                let current_entry = method.current_version_entry_expr();
                quote! {
                    #index => set.#name = upstream_core::service::MethodSupport::Supported(#current_entry),
                }
            })
            .collect();

        let supported_set_insert_with_enum = self.methods.values().enumerate().map(|(index, method)| {
            let _ = index;
            let name = method.name();
            let struct_name = method.struct_name();

            // `insert` marks a method supported at its current version → its
            // real current-version entry. See `supported_set_insert_with_index`.
            let current_entry = method.current_version_entry_expr();
            quote! {
                Methods::#struct_name => self.#name = upstream_core::service::MethodSupport::Supported(#current_entry),
            }
        });

        let supported_set_contains_enum = self.methods.values().map(|method| {
            let name = method.name();
            let struct_name = method.struct_name();

            quote! {
                Methods::#struct_name => self.#name.is_supported(),
            }
        });

        let supported_set_version_entry_enum = self.methods.values().map(|method| {
            let name = method.name();
            let struct_name = method.struct_name();

            quote! {
                Methods::#struct_name => self.#name.version_entry(),
            }
        });

        let supported_set_insert_version_with_index =
            self.methods.values().enumerate().map(|(index, method)| {
                let name = method.name();

                // Historical (older) layout: carry the looked-up entry verbatim.
                quote! {
                    #index => set.#name = upstream_core::service::MethodSupport::Supported(entry),
                }
            });

        let snapshot_tests = self.methods.values().map(|method| {
            let method_name = &method.name;
            let service_name = &self.namespace;
            let test_name = format!("snapshot_{}", method.name);
            let test_name = Ident::new(&test_name, Span::call_site());
            let struct_name = method.struct_name();

            quote! {
                // The committed snapshot pins the *production* wire format.
                // Under `feature = "negotiation"` (test builds) a service may
                // add test-only versioned fields (e.g. paxos `AcceptRequest`'s
                // `_test_v2_marker`) that intentionally change a method's
                // global_id/shape, so the production snapshot can't match.
                // Skip the snapshot in that config; it still guards the
                // production layout whenever the feature is off. (Crates that
                // don't define a `negotiation` feature always run it.)
                #[cfg(not(feature = "negotiation"))]
                #[test]
                fn #test_name() {
                    let method = Methods::#struct_name;
                    upstream_core::testing::service::assert_snapshot(
                        method.info().for_snapshot(),
                        #service_name,
                        #method_name,
                        env!("CARGO_MANIFEST_DIR"),
                    );
                }
            }
        });

        let requests = self
            .methods
            .values()
            .map(|method| method.generate_request_struct());

        let request_headers = self
            .methods
            .values()
            .map(|method| method.generate_request_header_struct());

        let responses = self
            .methods
            .values()
            .map(|method| method.generate_response_struct());

        let response_headers = self
            .methods
            .values()
            .map(|method| method.generate_response_header_struct());

        let mut namespace = self.namespace.clone();
        namespace.push('\0');

        let task_classes = self.generate_task_classes(crate_namespace);

        let tokens = quote! {
            #![allow(clippy::unit_arg)]

            #task_classes

            #[derive(Copy, Clone, Debug, PartialEq, Eq)]
            pub struct Service;

            impl upstream_core::service::Service for Service {
                type Methods = Methods;
                type Request = Request;
                type RequestHeader = request::Header;
                type Response = Response;
                type ResponseHeader = response::Header;
                type SupportedSet = supported::Set;

                const INTERFACE_ID: upstream_core::service::GlobalId = INTERFACE_ID;

                fn methods() -> impl Iterator<Item = &'static upstream_core::service::Method> {
                    METHODS.iter().copied()
                }

                fn namespaces() -> impl Iterator<Item = &'static upstream_core::metrics::info::Str> {
                    core::iter::once(const {
                        upstream_core::metrics::info::Str::new(#namespace)
                    })
                }

                fn method_for_request(request: &Self::Request) -> Self::Methods {
                    match request {
                        #(#request_to_method,)*
                    }
                }
            }

            #[allow(dead_code)]
            const INIT_HASHER: upstream_core::service::Hasher = upstream_core::service::Hasher::new().update(#namespace.as_bytes());

            #(#method_init)*

            pub static METHODS: [&upstream_core::service::Method; #method_len] = upstream_core::service::Method::sort_by_global_id([#(#method_list),*]);

            const INTERFACE_ID: upstream_core::service::GlobalId = {
                let mut hasher = INIT_HASHER;
                let mut i = 0;
                while i < #method_len {
                    hasher = hasher.update(&METHODS[i].global_id.to_be_bytes());
                    i += 1;
                }

                hasher.finalize_u64()
            };

            #[derive(Clone, Copy, Debug, PartialEq, Eq)]
            pub enum Methods {
                #(#method_variants)*
            }

            impl Methods {
                pub const fn info(&self) -> &'static upstream_core::service::Method {
                    match self {
                        #(#method_lookup)*
                    }
                }
            }

            impl upstream_core::service::Methods for Methods {
                fn info(&self) -> &'static upstream_core::service::Method {
                    match self {
                        #(#method_lookup)*
                    }
                }

                fn by_global_id(global_id: upstream_core::service::GlobalId) -> Option<Self> {
                    match global_id {
                        #(#global_id_lookup)*
                        _ => None,
                    }
                }
            }

            // One variant per method, so variant sizes vary with each method's
            // request payload; boxing the largest to equalize them would change
            // the wire/ownership shape of generated requests, so allow the size
            // difference instead (clippy::large_enum_variant).
            #[derive(Clone, Debug)]
            #[allow(clippy::large_enum_variant)]
            pub enum Request {
                #(#request_variants)*
            }

            pub mod request {
                #(#requests)*

                #[derive(Clone, Debug)]
                pub enum Header {
                    #(#request_header_variants)*
                }

                impl From<core::convert::Infallible> for Header {
                    fn from(_: core::convert::Infallible) -> Self {
                        unreachable!()
                    }
                }

                pub mod header {
                    #(#request_headers)*
                }
            }

            // See `Request` above: per-method variants of differing payload size;
            // allow the size difference rather than reshaping generated responses.
            #[derive(Clone, Debug)]
            #[allow(clippy::large_enum_variant)]
            pub enum Response {
                #(#response_variants)*
            }

            pub mod response {
                #(#responses)*

                #[derive(Clone, Debug)]
                pub enum Header {
                    #(#response_header_variants)*
                }

                impl From<core::convert::Infallible> for Header {
                    fn from(_: core::convert::Infallible) -> Self {
                        unreachable!()
                    }
                }

                pub mod header {
                    #(#response_headers)*
                }
            }

            pub mod supported {
                use upstream_core::service::Method;
                use super::Methods;

                #[derive(Copy, Clone, Debug, PartialEq, Eq)]
                pub struct Set {
                    #(#supported_set_fields)*
                }

                impl Set {
                    pub const NONE: Set = Set { #(#supported_set_none)* };

                    #[deprecated(note = "Use Set::all() instead")]
                    pub const ALL: Set = Set { #(#supported_set_all)* };

                    /// Every **method** supported, each at its **current**
                    /// (newest) version — coverage over methods, not over a
                    /// method's versions (a `SupportedSet` records exactly one
                    /// version per method).
                    ///
                    /// This is the fallback used when negotiation is off
                    /// (`StaticSupportedRegistry`) or the peer is unknown / not
                    /// yet negotiated (an old peer, assumed to speak the current
                    /// layout). Because `MethodSupport` carries a concrete entry
                    /// for every supported method, "current" here is the latest
                    /// cross-product entry, so dispatch runs
                    /// `serialize_at(newest)` — byte-identical to the plain
                    /// `serialize` (see core-wire's
                    /// `serialize_at_newest_index_is_byte_identical_to_plain_serialize`).
                    pub fn all() -> Set {
                        #[allow(deprecated)]
                        Self::ALL
                    }

                    pub fn iter(&self) -> Iter<'_> {
                        Iter(self, super::METHODS.iter())
                    }

                    pub fn insert(&mut self, method: Methods) {
                        match method {
                            #(#supported_set_insert_with_enum)*
                        }
                    }

                    pub fn contains(&self, method: Methods) -> bool {
                        match method {
                            #(#supported_set_contains_enum)*
                        }
                    }
                }

                impl upstream_core::service::SupportedSet for Set {
                    type Methods = Methods;

                    fn contains(&self, method: &Methods) -> bool {
                        (*self).contains(*method)
                    }

                    fn version_entry(&self, method: &Methods) -> Option<upstream_core::service::MethodVersionEntry> {
                        match method {
                            #(#supported_set_version_entry_enum)*
                        }
                    }

                    fn from_negotiated(
                        methods: impl Iterator<Item = &'static upstream_core::service::Method>,
                        peer_global_ids: &[upstream_core::service::GlobalId],
                    ) -> Self {
                        let peer_set: std::collections::HashSet<upstream_core::service::GlobalId> =
                            peer_global_ids.iter().copied().collect();
                        let mut set = Self::NONE;
                        for method in methods {
                            // Check current global_id first
                            if peer_set.contains(&method.global_id) {
                                match method.local_id {
                                    #(#supported_set_insert_with_index)*
                                    _ => {}
                                }
                            } else {
                                // Check version global_ids (newest first in keys)
                                for &version_id in method.versions.keys() {
                                    if peer_set.contains(&version_id) {
                                        if let Some(entry) = method.versions.get(version_id) {
                                            match method.local_id {
                                                #(#supported_set_insert_version_with_index)*
                                                _ => {}
                                            }
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                        set
                    }

                    fn all() -> Self {
                        #[allow(deprecated)]
                        Self::ALL
                    }
                }

                #[derive(Clone)]
                pub struct Iter<'a>(&'a Set, core::slice::Iter<'static, &'static Method>);

                impl Iterator for Iter<'_> {
                    type Item = Methods;

                    fn next(&mut self) -> Option<Self::Item> {
                        loop {
                            let method = self.1.next()?;
                            match method.local_id {
                                #(#supported_set_iter)*
                                _ => continue,
                            }
                        }
                    }
                }

                impl FromIterator<upstream_core::service::GlobalId> for Set {
                    fn from_iter<I: IntoIterator<Item = upstream_core::service::GlobalId>>(iter: I) -> Self {
                        let mut set = Self::NONE;
                        let iter = upstream_core::iter::sorted_intersection_with(super::METHODS.iter().copied(), iter.into_iter(), |method, id| {
                            method.global_id.cmp(id)
                        });

                        for method in iter {
                            match method.local_id {
                                #(#supported_set_insert_with_index)*
                                _ => unsafe {
                                    core::hint::unreachable_unchecked()
                                },
                            }
                        }

                        set
                    }
                }
            }

            #[cfg(test)]
            mod tests {
                use super::*;

                #[test]
                fn unique_global_ids() {
                    let mut ids = std::collections::HashSet::new();
                    for method in METHODS.iter() {
                        assert!(ids.insert(method.global_id), "duplicate global id: {}", method.global_id);
                    }
                }

                /// Verify that `by_global_id` returns the correct variant for every method's
                /// `global_id` and all version `global_id`s.
                #[test]
                fn global_id_lookup_round_trip() {
                    use upstream_core::service::Methods as _;
                    for method in METHODS.iter() {
                        // Current global_id
                        let resolved = Methods::by_global_id(method.global_id);
                        assert!(
                            resolved.is_some(),
                            "by_global_id({}) returned None for method {:?}",
                            method.global_id, method.name,
                        );
                        let resolved = resolved.unwrap();
                        assert_eq!(
                            resolved.info().global_id,
                            method.global_id,
                            "by_global_id({}) resolved to wrong method: expected {:?}, got {:?}",
                            method.global_id, method.name, resolved.info().name,
                        );

                        // Version global_ids (older wire layouts)
                        for &version_id in method.versions.keys() {
                            let resolved = Methods::by_global_id(version_id);
                            assert!(
                                resolved.is_some(),
                                "by_global_id({}) returned None for version of method {:?}",
                                version_id, method.name,
                            );
                            assert_eq!(
                                resolved.unwrap().info().global_id,
                                method.global_id,
                                "version global_id {} resolved to wrong method: expected {:?}",
                                version_id, method.name,
                            );
                        }
                    }
                }

                #(#snapshot_tests)*
            }
        };

        emit::rust(out, &tokens).unwrap();
    }

    pub(crate) fn build_consumer(&self, namespace: &str, out: &Path) {
        let namespace = &format!("{namespace}_{}", self.namespace);

        let method_init = self.methods.values().map(|method| {
            let name = method.struct_name();
            quote! {
                super::schema::Methods::#name
            }
        });

        let trait_methods = self
            .methods
            .values()
            .map(|method| method.generate_consumer_trait_method());

        let methods = self.methods.values().enumerate().map(|(idx, method)| {
            let supports = method.supports();
            // Bidi-streaming methods spawn a request-write pump; class it with
            // this method's client task class (in the sibling `schema` module).
            // Other shapes write inline and never use the token.
            let client_task_class = if Self::method_has_client_task_class(method) {
                let ident = Ident::new(
                    &self.task_class_method_client_name(method),
                    Span::call_site(),
                );
                quote!(super::schema::#ident)
            } else {
                quote!(upstream_core::runtime::task_monitor::Unlabeled)
            };
            let method_body =
                method.generate_consumer_method_body(idx, namespace, &client_task_class);

            quote! {
                #method_body

                #[inline]
                fn #supports(&self) -> bool {
                    self.methods[#idx].is_supported()
                }
            }
        });

        // streamed requests can't be multicast at this time
        let multicast_methods = self
            .methods
            .values()
            .enumerate()
            .filter(|(_idx, method)| {
                // Raw responses don't support multicast right now

                !(method.stream_request || method.raw_response)
            })
            .collect::<Vec<_>>();

        let multicast_trait_methods = multicast_methods
            .iter()
            .map(|(_idx, method)| method.generate_consumer_multicast_trait_method());

        let multicast_method_impls = multicast_methods.iter().map(|(idx, method)| {
            let supports = method.supports();

            // Use the helper function to generate the method body
            let method_body = method.generate_consumer_multicast_method_body(*idx);

            quote! {
                #method_body

                #[inline]
                fn #supports(&self) -> bool {
                    self.sessions.iter().all(|session| session.1.#supports())
                }
            }
        });

        let multicast_modules = multicast_methods
            .iter()
            .map(|(idx, method)| method.generate_multicast_module(*idx, namespace));

        let multicast_module = if multicast_methods.is_empty() {
            quote! {}
        } else {
            quote! {
                pub mod multicast {
                    use super::super::{schema, super::super::Env};
                    #(#multicast_modules)*
                }
            }
        };

        let method_len = self.methods.len();

        let tokens = quote! {
            #![allow(clippy::unit_arg)]

            pub trait Consumer: Clone + Send + Sync + 'static {
                #(#trait_methods)*
            }

            pub trait MulticastConsumer: Clone + Send + Sync + 'static {
                type PeerAddr: upstream_core::service::consumer::PeerAddr;
                type Method: upstream_core::service::consumer::Method<super::Service>;

                #(#multicast_trait_methods)*
            }

            pub trait Registry<PeerAddr: upstream_core::service::consumer::PeerAddr>: upstream_core::service::consumer::Registry<super::Service, PeerAddr>  {
                fn resolve(&self, peer_addr: &PeerAddr) -> impl core::future::Future<
                    Output = upstream_core::runtime::Result<
                        Session<
                            <Self::Peer as upstream_core::service::consumer::Peer<super::Service>>::Method
                        >
                    >
                > + Send;

                fn multicast(&self, addrs: impl Iterator<Item = PeerAddr> + Send) -> impl core::future::Future<
                    Output = upstream_core::runtime::Result<
                        MulticastSession<
                            <Self::Peer as upstream_core::service::consumer::Peer<super::Service>>::Method,
                            PeerAddr,
                        >
                    >
                > + Send;
            }

            impl<R, PeerAddr> Registry<PeerAddr> for R
            where
                R: upstream_core::service::consumer::Registry<super::Service, PeerAddr>,
                PeerAddr: upstream_core::service::consumer::PeerAddr,
            {
                async fn resolve(&self, peer_addr: &PeerAddr) -> upstream_core::runtime::Result<
                    Session<
                        <Self::Peer as upstream_core::service::consumer::Peer<super::Service>>::Method
                    >
                > {
                    let peer = self.resolve_peer(peer_addr).await?;
                    Ok(Session::new(&peer))
                }

                async fn multicast(&self, addrs: impl Iterator<Item = PeerAddr> + Send) -> upstream_core::runtime::Result<
                    MulticastSession<
                        <Self::Peer as upstream_core::service::consumer::Peer<super::Service>>::Method,
                        PeerAddr
                    >
                > {
                    let mut sessions = Vec::new();
                    for addr in addrs {
                        let peer = self.resolve(&addr).await?;
                        sessions.push((addr, peer));
                    }
                    Ok(MulticastSession { sessions: sessions.into() })
                }
            }

            #[derive(Debug)]
            pub struct Session<M>
            where
                M: upstream_core::service::consumer::Method<super::Service>,
             {
                methods: ::std::sync::Arc<[M; #method_len]>,
            }

            impl<M: upstream_core::service::consumer::Method<super::Service>> Clone for Session<M>
            where
                M: upstream_core::service::consumer::Method<super::Service>
            {
                fn clone(&self) -> Self {
                    Self {
                        methods: self.methods.clone(),
                    }
                }
            }

            impl<M> Session<M>
            where
                M: upstream_core::service::consumer::Method<super::Service>
            {
                pub fn new<P: upstream_core::service::consumer::Peer<super::Service, Method = M>>(peer: &P) -> Self {
                    let methods = ::std::sync::Arc::new([#(
                        peer.resolve(#method_init)
                    ),*]);
                    Self { methods }
                }
            }

            impl<M> Consumer for Session<M>
            where
                M: upstream_core::service::consumer::Method<super::Service>
            {
                #(#methods)*
            }

            #[derive(Clone)]
            pub struct MulticastSession<M, PeerAddr>
            where
                M: upstream_core::service::consumer::Method<super::Service>,
                PeerAddr: upstream_core::service::consumer::PeerAddr,
            {
                sessions: ::std::sync::Arc<[(PeerAddr, Session<M>)]>,
            }

            impl<M, PeerAddr> MulticastConsumer for MulticastSession<M, PeerAddr>
            where
                M: upstream_core::service::consumer::Method<super::Service>,
                PeerAddr: upstream_core::service::consumer::PeerAddr,
            {
                type PeerAddr = PeerAddr;
                type Method = M;

                #(#multicast_method_impls)*
            }

            #multicast_module
        };

        emit::rust(out, &tokens).unwrap();
    }

    pub(crate) fn build_provider(&self, namespace: &str, out: &Path) {
        let namespace = &format!("{namespace}_{}", self.namespace);

        // The per-service (outer, connection-level) task class lives in the
        // sibling `schema` module.
        let service_task_class = {
            let ident = Ident::new(&self.task_class_service_name(), Span::call_site());
            quote!(super::schema::#ident)
        };

        let methods: TokenStream = self
            .methods
            .values()
            .map(|method| method.generate_provider_method_signature())
            .collect();

        let dispatch = self.methods.values().map(|method| {
            method.generate_dispatch(namespace, &self.task_class_method_name(method))
        });

        let any_has_peer_addr = self.methods.values().any(|method| method.peer_addr);
        let peer_addr = if any_has_peer_addr {
            quote!(peer_addr)
        } else {
            quote!(_peer_addr)
        };

        let tokens = quote! {
            #![allow(clippy::redundant_closure)]

            pub trait Provider<PeerAddr>: 'static + Send + Sync {
                #methods

                /// Called after every method invocation
                ///
                /// Implementations can return `false` to gracefully shut the server down.
                #[inline]
                fn should_continue(&self) -> bool {
                    true
                }
            }

            #[derive(Clone, Debug)]
            pub struct Instance<P: Provider<PeerAddr>, PeerAddr: 'static + Send + Sync> {
                provider: P,
                // TODO add a `watch` that broadcasts after the provider returns `false` on should_continue
                _peer_addr: core::marker::PhantomData<PeerAddr>,
            }

            impl<P: Provider<PeerAddr>, PeerAddr: 'static + Send + Sync> Instance<P, PeerAddr> {
                pub fn new(provider: P) -> Self {
                    Self { provider, _peer_addr: core::marker::PhantomData }
                }
            }

            #[allow(clippy::let_unit_value)]
            impl<P, E, PeerAddr> upstream_core::service::provider::Acceptor<E, PeerAddr> for Instance<P, PeerAddr>
            where
                P: Provider<PeerAddr>, E: super::super::super::Env, PeerAddr: 'static + Send + Sync
             {
                type Service = super::Service;

                async fn dispatch<Req, Res>(
                    &self,
                    env: &E,
                    #peer_addr: &PeerAddr,
                    method: <Self::Service as upstream_core::service::Service>::Methods,
                    request: Req,
                    response: Res,
                )
                where
                    Req: upstream_core::service::provider::RequestProvider<Self::Service>,
                    Res: upstream_core::service::provider::ResponseProvider<Self::Service>,
                {
                    match method {
                        #(#dispatch)*
                    }
                }

                fn spawn_connection<Fut>(&self, env: &E, future: Fut) -> E::JoinHandle<()>
                where
                    Fut: core::future::Future<Output = ()> + Send + 'static,
                {
                    upstream_core::runtime::Env::spawn(env, #service_task_class, future)
                }

                #[inline]
                async fn while_open(&self) {
                    // TODO
                    core::future::pending().await
                }
            }
        };

        emit::rust(out, &tokens).unwrap();
    }

    fn method_info(&self) -> impl Iterator<Item = TokenStream> {
        self.methods
            .values()
            .enumerate()
            .map(|(index, method)| method.generate_info(index))
    }
}

#[derive(Debug, Clone, Copy)]
enum ImportLocation {
    Consumer,
    Provider,
    MulticastResolve,
    MulticastExecute,
}

#[derive(Clone, Debug)]
pub struct Method {
    pub name: String,
    pub description: Vec<String>,
    pub inputs: Vec<Input>,
    pub output: Output,
    pub unidirectional: bool,
    pub unreliable: bool,
    pub stream_request: bool,
    pub stream_response: bool,
    pub request_header: Option<String>,
    pub response_header: Option<String>,
    pub peer_addr: bool,
    pub raw_response: bool,
    /// The transport-level send priority variant name (e.g. `"High"`),
    /// matching a variant of `upstream_core::service::Priority`. Defaults to
    /// `"Medium"`.
    pub priority: String,
}

impl Method {
    // Helper to generate header parameter if present
    fn generate_request_header_param(&self) -> TokenStream {
        if let Some(ty) = &self.request_header {
            let header = self.unique_name("header");
            let ty: TokenStream = ty.parse().unwrap();
            quote!(#header: #ty,)
        } else {
            quote!()
        }
    }

    fn generate_request_arguments(&self) -> Vec<TokenStream> {
        if self.stream_request {
            let types = self.generate_input_types();
            let ty = if self.inputs.len() == 1 {
                quote!(#(#types)*)
            } else {
                quote!((#(#types),*))
            };
            vec![quote!(request: impl upstream_core::service::RequestStream<#ty>)]
        } else {
            self.inputs
                .iter()
                .map(|input| {
                    let name = Ident::new(&input.name, Span::call_site());
                    let ty: TokenStream = input.ty.parse().unwrap();
                    quote!(#name: #ty)
                })
                .collect()
        }
    }

    fn generate_input_names(&self) -> impl Iterator<Item = TokenStream> + '_ {
        self.inputs.iter().map(|input| {
            let name = Ident::new(&input.name, Span::call_site());
            quote!(#name)
        })
    }

    fn generate_input_types(&self) -> impl Iterator<Item = TokenStream> + '_ {
        self.inputs.iter().map(|input| {
            let ty: TokenStream = input.ty.parse().unwrap();
            quote!(#ty)
        })
    }

    // Helper to generate request wrapper creation
    fn generate_request_mapping(&self) -> TokenStream {
        if self.stream_request {
            quote! {
                let request = request.map(|result| {
                    result.map(|input| input.into())
                });
            }
        } else {
            let input_names = self.generate_input_names();
            quote! {
                let request = Req {
                    #(#input_names),*
                };
            }
        }
    }

    // Helper to generate response extraction
    fn generate_response_extraction(&self) -> TokenStream {
        if self.unidirectional {
            // No response value — the send was already `?`-propagated (and not
            // bound), so just yield unit (avoids a useless `().into()`).
            return quote!(Ok(()));
        }

        if self.raw_response {
            return quote!(Ok(response));
        }

        match (self.response_header.is_some(), self.stream_response) {
            (true, true) => {
                quote! {
                    let header = response.0.into();
                    let response = response.1.map(|result| result.map(|output| output.into()));
                    Ok((header, response))
                }
            }
            (true, false) => {
                quote! {
                    let header = response.0.into();
                    let response = response.1.into();
                    Ok((header, response))
                }
            }
            (false, true) => {
                quote! {
                    Ok(response.map(|result| result.map(|output| output.into())))
                }
            }
            (false, false) => {
                quote! {
                    Ok(response.into())
                }
            }
        }
    }

    fn name(&self) -> Ident {
        Ident::new(&self.name, Span::call_site())
    }

    fn struct_name(&self) -> Ident {
        use heck::ToPascalCase;
        Ident::new(&self.name.to_pascal_case(), Span::call_site())
    }

    fn supports(&self) -> Ident {
        Ident::new(&format!("supports_{}", self.name), Span::call_site())
    }

    fn env_name(&self) -> Ident {
        self.unique_name("env")
    }

    fn peer_addr_name(&self) -> Ident {
        self.unique_name("peer_addr")
    }

    fn unique_name(&self, name: &str) -> Ident {
        let mut name = name.to_string();

        'find: loop {
            for inp in self.inputs.iter() {
                if inp.name == name {
                    name = format!("_{name}");
                    continue 'find;
                }
            }

            return Ident::new(&name, Span::call_site());
        }
    }

    fn docs(&self) -> impl Iterator<Item = TokenStream> {
        self.description.iter().map(|doc| {
            quote! { #[doc = #doc] }
        })
    }

    fn supports_docs(&self) -> impl Iterator<Item = TokenStream> {
        let supports_doc = format!("Returns `true` if `{}` is supported by the peer", self.name);
        core::iter::once(quote!(#[doc = #supports_doc]))
    }

    fn output_ty(&self) -> TokenStream {
        self.output.ty.parse().unwrap()
    }

    /// Expression that builds this method's *current*-version
    /// `MethodVersionEntry` inline: the primary `global_id` (read from the
    /// `METHOD_xxx` static) paired with the newest cross-product indices
    /// (`IN_COUNT - 1`, `OUT_COUNT - 1`). For a multi-input method the request
    /// isn't versioned through one type, so `input_version_idx` is 0; the
    /// output can still be versioned, so `output_version_idx` tracks its newest
    /// layout.
    ///
    /// This is just the (0,0) cross-product cell — the same entry the
    /// `versions` map deliberately omits because its `global_id` duplicates the
    /// primary. It is built on demand rather than stored, so "current" is not a
    /// dedicated construct: the supported-set builders read it the same way
    /// `from_negotiated` reads `super::METHOD_xxx.global_id`. Because it reads a
    /// `static` field it is only valid in a runtime `fn`, not a `const`.
    fn current_version_entry_expr(&self) -> TokenStream {
        let method_static = Ident::new(&format!("METHOD_{}", self.name), Span::call_site());
        let output_ty = self.output_ty();
        let input_version_idx = if self.inputs.len() == 1 {
            let input_ty: TokenStream = self.inputs[0].ty.parse().unwrap();
            quote!((<#input_ty as upstream_core::wire::Wire>::__XPRODUCT_HASH_ENTRIES.len() - 1) as u32)
        } else {
            quote!(0)
        };
        quote! {
            upstream_core::service::MethodVersionEntry {
                global_id: super::#method_static.global_id,
                input_version_idx: #input_version_idx,
                output_version_idx: (<#output_ty as upstream_core::wire::Wire>::__XPRODUCT_HASH_ENTRIES.len() - 1) as u32,
            }
        }
    }

    fn generate_request_variant(&self) -> TokenStream {
        let name = self.struct_name();
        let docs = self.docs();

        let inputs = self.inputs.iter().map(|input| {
            let name = Ident::new(&input.name, Span::call_site());
            let ty: TokenStream = input.ty.parse().unwrap();
            quote! { #name: #ty }
        });

        quote! {
            #(#docs)*
            #name {
                #(#inputs),*
            },
        }
    }

    fn generate_request_header_struct(&self) -> TokenStream {
        let Some(header_ty) = &self.request_header else {
            return quote!();
        };

        let name = self.struct_name();
        let ty: TokenStream = header_ty.parse().unwrap();
        let try_from_error = format!("invalid header type for method {:?}", self.name);

        quote! {
            #[derive(Clone, upstream_core::wire::Wire)]
            pub struct #name(pub #ty);

            impl ::std::convert::TryFrom<super::Header> for #name {
                type Error = upstream_core::runtime::Error;

                #[allow(unreachable_patterns)]
                fn try_from(header: super::Header) -> Result<Self, Self::Error> {
                    match header {
                        super::Header::#name(value) => Ok(Self(value)),
                        _ => Err(upstream_core::runtime::Error::Io(::std::io::Error::new(
                            ::std::io::ErrorKind::InvalidData,
                            #try_from_error,
                        ))),
                    }
                }
            }

            impl From<#name> for super::Header {
                fn from(value: #name) -> Self {
                    super::Header::#name(value.0)
                }
            }

            impl From<#ty> for #name {
                fn from(value: #ty) -> Self {
                    Self(value)
                }
            }

            impl From<#name> for #ty {
                fn from(value: #name) -> Self {
                    value.0
                }
            }
        }
    }

    fn generate_request_header_variant(&self) -> TokenStream {
        if let Some(header_ty) = &self.request_header {
            let name = self.struct_name();
            let ty: TokenStream = header_ty.parse().unwrap();
            quote! {
                #name(#ty),
            }
        } else {
            quote!()
        }
    }

    fn generate_response_variant(&self) -> TokenStream {
        // Skip generating response variants for both unidirectional and raw responses
        if self.unidirectional || self.raw_response {
            return quote!();
        }

        let name = self.struct_name();
        let output: TokenStream = self.output.ty.parse().unwrap();

        quote! {
            #name(#output),
        }
    }

    fn generate_response_header_struct(&self) -> TokenStream {
        let Some(header_ty) = &self.response_header else {
            return quote!();
        };

        let name = self.struct_name();
        let ty: TokenStream = header_ty.parse().unwrap();
        let try_from_error = format!("invalid header type for method {:?}", self.name);

        quote! {
            #[derive(Clone, upstream_core::wire::Wire)]
            pub struct #name(pub #ty);

            impl ::std::convert::TryFrom<super::Header> for #name {
                type Error = upstream_core::runtime::Error;

                #[allow(unreachable_patterns)]
                fn try_from(header: super::Header) -> Result<Self, Self::Error> {
                    match header {
                        super::Header::#name(value) => Ok(Self(value)),
                        _ => Err(upstream_core::runtime::Error::Io(::std::io::Error::new(
                            ::std::io::ErrorKind::InvalidData,
                            #try_from_error,
                        ))),
                    }
                }
            }

            impl From<#name> for super::Header {
                fn from(value: #name) -> Self {
                    super::Header::#name(value.0)
                }
            }

            impl From<#ty> for #name {
                fn from(value: #ty) -> Self {
                    Self(value)
                }
            }

            impl From<#name> for #ty {
                fn from(value: #name) -> Self {
                    value.0
                }
            }
        }
    }

    fn generate_response_header_variant(&self) -> TokenStream {
        if let Some(header_ty) = &self.response_header {
            let name = self.struct_name();
            let ty: TokenStream = header_ty.parse().unwrap();
            quote! {
                #name(#ty),
            }
        } else {
            quote!()
        }
    }

    fn generate_request_struct(&self) -> TokenStream {
        let name = self.struct_name();

        let inputs = self.inputs.iter().map(|input| {
            let name = Ident::new(&input.name, Span::call_site());
            let ty: TokenStream = input.ty.parse().unwrap();
            quote! { #name: #ty }
        });

        let input_names: Vec<_> = self
            .inputs
            .iter()
            .map(|input| {
                let name = Ident::new(&input.name, Span::call_site());
                quote! { #name }
            })
            .collect();

        let input_types: Vec<_> = self
            .inputs
            .iter()
            .map(|input| {
                let ty: TokenStream = input.ty.parse().unwrap();
                quote! { #ty }
            })
            .collect();

        let value = if self.inputs.is_empty() {
            quote! { _value }
        } else {
            quote! { value }
        };

        let conversions = match self.inputs.len() {
            0 => quote! {
                impl From<#name> for () {
                    fn from(_: #name) -> Self {}
                }

                impl From<()> for #name {
                    fn from(_: ()) -> Self {
                        Self {}
                    }
                }
            },
            1 => quote!(
                impl From<#name> for #(#input_types)* {
                    fn from(#value: #name) -> Self {
                        #(#value.#input_names)*
                    }
                }

                impl From<#(#input_types)*> for #name {
                    fn from(#(#input_names)*: #(#input_types)*) -> Self {
                        Self { #(#input_names),* }
                    }
                }
            ),
            _ => quote! {
                impl From<#name> for (#(#input_types),*) {
                    fn from(#value: #name) -> Self {
                        (#(#value.#input_names),*)
                    }
                }

                impl From<(#(#input_types),*)> for #name {
                    fn from((#(#input_names),*): (#(#input_types),*)) -> Self {
                        Self {
                            #(#input_names),*
                        }
                    }
                }
            },
        };

        let try_from_error = format!("invalid request type for method {:?}", self.name);

        quote! {
            // Define a request wrapper that implements Request<S>
            #[derive(Clone, upstream_core::wire::Wire)]
            pub struct #name {
                #(pub #inputs),*
            }

            impl ::std::convert::TryFrom<super::Request> for #name {
                type Error = upstream_core::runtime::Error;

                #[allow(unreachable_patterns)]
                fn try_from(request: super::Request) -> Result<Self, Self::Error> {
                    match request {
                        super::Request::#name { #(#input_names),* } => Ok(Self { #(#input_names),* }),
                        _ => Err(upstream_core::runtime::Error::Io(::std::io::Error::new(
                            ::std::io::ErrorKind::InvalidData,
                            #try_from_error,
                        ))),
                    }
                }
            }

            impl From<#name> for super::Request {
                fn from(#value: #name) -> super::Request {
                    super::Request::#name {
                        #(#input_names: #value.#input_names),*
                    }
                }
            }

            #conversions
        }
    }

    fn generate_response_struct(&self) -> TokenStream {
        if self.unidirectional || self.raw_response {
            return quote! {};
        }

        let name = self.struct_name();
        let try_from_error = format!("invalid request type for method {:?}", self.name);
        let output = self.output_ty();

        quote! {
            // Define a response wrapper that implements Response<S>
            #[derive(upstream_core::wire::Wire)]
            pub struct #name(pub #output);

            impl ::std::convert::TryFrom<super::Response> for #name {
                type Error = upstream_core::runtime::Error;

                #[allow(unreachable_patterns)]
                fn try_from(response: super::Response) -> Result<Self, Self::Error> {
                    match response {
                        super::Response::#name(value) => Ok(Self(value)),
                        _ => Err(upstream_core::runtime::Error::Io(::std::io::Error::new(
                            ::std::io::ErrorKind::InvalidData,
                            #try_from_error
                        ))),
                    }
                }
            }

            impl From<#name> for super::Response {
                fn from(value: #name) -> Self {
                    super::Response::#name(value.0)
                }
            }

            impl From<#output> for #name {
                fn from(value: #output) -> Self {
                    Self(value)
                }
            }

            impl From<#name> for #output {
                fn from(value: #name) -> Self {
                    value.0
                }
            }
        }
    }

    fn generate_return_type(&self) -> TokenStream {
        let output = self.output_ty();

        let return_type = match (&self.response_header, self.stream_response) {
            (Some(header), true) => {
                let header: TokenStream = header.parse().unwrap();
                quote! { (#header, impl upstream_core::service::ResponseStream<#output>) }
            }
            (Some(header), false) => {
                let header: TokenStream = header.parse().unwrap();
                quote! { (#header, #output) }
            }
            (None, true) => {
                quote! { impl upstream_core::service::ResponseStream<#output> }
            }
            (None, false) => output,
        };

        quote!(upstream_core::runtime::Result<#return_type>)
    }

    fn generate_consumer_response_param(&self) -> (TokenStream, TokenStream, TokenStream) {
        if self.raw_response {
            let response = self.unique_name("response");
            let ty = quote!(Res);
            let generic = quote!(#ty: upstream_core::service::rpc_response::Response + Send,);
            let param = quote!(, #response: #ty);
            let return_type = quote!(upstream_core::runtime::Result<#ty::Output>);
            (param, generic, return_type)
        } else {
            let return_type = self.generate_return_type();
            (quote!(), quote!(), return_type)
        }
    }

    fn generate_consumer_method_signature(&self, env_name: &Ident, is_trait: bool) -> TokenStream {
        let name = self.name();
        let inputs = self.generate_request_arguments();
        let header_param = self.generate_request_header_param();
        let (response_param, response_generic, return_type) =
            self.generate_consumer_response_param();

        let where_clause = self
            .raw_response
            .then(|| quote!(where Res::Storage: Send, Res::Output: Send));

        if is_trait {
            quote! {
                fn #name<E: super::super::super::Env, #response_generic>(
                    &self,
                    #env_name: &E,
                    #header_param
                    #(#inputs),*
                    #response_param
                ) -> impl core::future::Future<Output = #return_type> + Send
                #where_clause
            }
        } else {
            quote! {
                async fn #name<E: super::super::super::Env, #response_generic>(
                    &self,
                    #env_name: &E,
                    #header_param
                    #(#inputs),*
                    #response_param
                ) -> #return_type
                #where_clause
            }
        }
    }

    fn generate_header_arg(&self) -> TokenStream {
        if self.request_header.is_some() {
            let header = self.unique_name("header");
            quote!(Some(#header.into()),)
        } else {
            quote!(None,)
        }
    }

    fn generate_raw_request(
        &self,
        method: &TokenStream,
        env_name: &Ident,
        location: ImportLocation,
    ) -> TokenStream {
        let header_arg = self.generate_header_arg();

        let request = quote! {
            let request = #method.create_raw_request::<E, ReqHeader, Req>(
                #env_name,
                #header_arg
                request,
            );
        };

        match location {
            ImportLocation::Consumer => quote! {
                let request = {
                    let serialize_span = span.on_serialize(#env_name, &request);
                    #request
                    serialize_span.exit(#env_name, &request.serialized_len());
                    request
                };
            },
            _ => request,
        }
    }

    fn generate_consumer_request_call(
        &self,
        method: &TokenStream,
        env_name: &Ident,
        raw_request: TokenStream,
        client_task_class: &TokenStream,
    ) -> TokenStream {
        let header_arg = self.generate_header_arg();

        match (
            self.raw_response,
            self.stream_request,
            self.stream_response,
            self.response_header.is_some(),
            self.unidirectional,
        ) {
            // Raw responses need special handling without Res type
            (true, _, _, _, _) => quote! {
                #raw_request
                let response = #method.send_raw::<E, Req, _, _>(#env_name, request, response, span).await?;
            },
            // Streaming request and response with response header — the bidi
            // write pump is classed with this method's client task class.
            (false, true, true, true, _) => quote! {
                let response = #method.clone().send_stream_bidi_with_response_header::<#client_task_class, E, ReqHeader, Req, ResHeader, Res, _>(#env_name, #header_arg request, span).await?;
            },
            // Streaming request and response without response header.
            (false, true, true, false, _) => quote! {
                let response = #method.clone().send_stream_bidi::<#client_task_class, E, ReqHeader, Req, Res, _>(#env_name, #header_arg request, span).await?;
            },
            // Streaming request only
            (false, true, false, _, _) => quote! {
                let response = #method.send_stream_request::<E, ReqHeader, Req, Res, _>(#env_name, #header_arg request, span).await?;
            },
            // Unidirectional — the send resolves to `()`, so don't bind it to a
            // `let` (clippy::let_unit_value); the `?` still propagates the error and
            // `generate_response_extraction` returns `Ok(())`.
            (false, false, false, _, true) => quote! {
                #raw_request
                #method.send_unidirectional::<E, Req, _>(#env_name, request, span).await?;
            },
            // Regular with response header
            (false, false, false, true, false) => quote! {
                #raw_request
                let response = #method.send_with_response_header::<E, Req, ResHeader, Res, _>(#env_name, request, span).await?;
            },
            // Regular without response header
            (false, false, false, false, false) => quote! {
                #raw_request
                let response = #method.send::<E, Req, Res, _>(#env_name, request, span).await?;
            },
            // Streaming response with response header
            (false, false, true, true, _) => quote! {
                #raw_request
                let response = #method.clone().send_stream_response_with_response_header::<E, Req, ResHeader, Res, _>(#env_name, request, span).await?;
            },
            // Streaming response without response header
            (false, false, true, false, _) => quote! {
                #raw_request
                let response = #method.clone().send_stream_response::<E, Req, Res, _>(#env_name, request, span).await?;
            },
        }
    }

    fn generate_impl_imports(&self, location: ImportLocation) -> TokenStream {
        let struct_name = self.struct_name();

        // Only import request header if we're using it and not in MulticastExecute
        let req_header_type = match (location, &self.request_header) {
            (ImportLocation::MulticastExecute, _) => quote!(),
            // A multicast-resolve body writes the request-parts tuple with a
            // full-path type — `::core::convert::Infallible` (headerless) or
            // `header.into()` from the real-typed header param (with header) — and
            // never names the `ReqHeader` alias, so importing it would be unused.
            (ImportLocation::MulticastResolve, _) => quote!(),
            (_, Some(_)) => quote!(use super::schema::request::header::#struct_name as ReqHeader;),
            (ImportLocation::Consumer, None) => {
                quote!(
                    use core::convert::Infallible as ReqHeader;
                )
            }
            _ => quote!(),
        };

        // Only import response header if we're using it and actually need it in this location
        let res_header_type = match (location, &self.response_header, self.unidirectional) {
            (
                ImportLocation::MulticastExecute
                | ImportLocation::Consumer
                | ImportLocation::Provider,
                Some(_),
                false,
            ) => {
                quote!(use super::schema::response::header::#struct_name as ResHeader;)
            }
            _ => quote!(),
        };

        let span_type = self.generate_span_type();
        let span_type = match location {
            // Stream-REQUEST consumer methods (incl. bidi) map the request through
            // `StreamExt` and call `send_stream_*` directly — they never use
            // `create_raw_request` (RawRequest) or `span.on_serialize`/`.exit`
            // (Span / the span trait), so importing those would be unused. Every
            // other shape (incl. stream-RESPONSE with a non-streaming request)
            // still serializes its request via the `#raw_request` path and needs
            // them.
            ImportLocation::Consumer if self.stream_request => quote!(),
            ImportLocation::Consumer => {
                quote!(use upstream_core::service::{Span as _, consumer::{RawRequest as _, span::#span_type as _}};)
            }
            ImportLocation::Provider => {
                quote!(use upstream_core::service::{Span as _, provider::span::#span_type as _};)
            }
            ImportLocation::MulticastExecute => {
                quote!(use upstream_core::service::consumer::{RawRequest as _, span::#span_type as _};)
            }
            _ => quote!(),
        };

        // Import request/response types based on location and method type
        let request_type = match (location, self.unidirectional, self.raw_response) {
            (ImportLocation::MulticastResolve, _, _) => {
                quote!(use super::schema::request::#struct_name as Req;)
            }
            (_, true, _) | (_, _, true) => quote!(use super::schema::request::#struct_name as Req;),
            _ => {
                quote!(use super::schema::{request::#struct_name as Req, response::#struct_name as Res};)
            }
        };

        // Only import StreamExt if we're actually using streaming and not in MulticastResolve
        let stream_ext = if (self.stream_request || self.stream_response)
            && !matches!(location, ImportLocation::MulticastResolve)
        {
            quote!(
                use upstream_core::service::stream::StreamExt as _;
            )
        } else {
            quote!()
        };

        quote! {
            #req_header_type
            #res_header_type
            #request_type
            #span_type
            #stream_ext
        }
    }

    fn generate_span_name(&self, namespace: &str, location: ImportLocation) -> Ident {
        let side = match location {
            ImportLocation::Provider => "provider",
            _ => "consumer",
        };

        let span_name = format!("publish_{namespace}_{side}_{}", self.name);
        Ident::new(&span_name, Span::call_site())
    }

    fn generate_consumer_method_body(
        &self,
        idx: usize,
        namespace: &str,
        client_task_class: &TokenStream,
    ) -> TokenStream {
        let env_name = self.env_name();
        let signature = self.generate_consumer_method_signature(&env_name, false);
        let imports = self.generate_impl_imports(ImportLocation::Consumer);
        let span_name = self.generate_span_name(namespace, ImportLocation::Consumer);

        let request_mapping = self.generate_request_mapping();
        let method = quote!(self.methods[#idx]);
        let raw_request = self.generate_raw_request(&method, &env_name, ImportLocation::Consumer);
        let request_call =
            self.generate_consumer_request_call(&method, &env_name, raw_request, client_task_class);
        let response_mapping = self.generate_response_extraction();

        let span_var = if self.stream_request {
            quote!(let span)
        } else {
            quote!(let mut span)
        };

        quote! {
            #signature {
                #imports
                #request_mapping
                #span_var = #env_name.#span_name();
                #request_call
                #response_mapping
            }
        }
    }

    fn generate_provider_method_signature(&self) -> TokenStream {
        let name = self.name();
        let env_name = self.env_name();
        let peer_addr = if self.peer_addr {
            let name = self.peer_addr_name();
            quote!(#name: &PeerAddr,)
        } else {
            quote!()
        };
        let inputs = self.generate_request_arguments();
        let header_param = self.generate_request_header_param();

        // For raw responses, modify the return type to be a direct buffer
        let return_type = if self.raw_response {
            quote!(
                upstream_core::runtime::Result<
                    impl upstream_core::buffer::reader::storage::Infallible + Send,
                >
            )
        } else {
            self.generate_return_type()
        };

        let method = quote! {
            fn #name<E: super::super::super::Env>(
                &self,
                #env_name: &E,
                #peer_addr
                #header_param
                #(#inputs),*
            ) -> impl core::future::Future<Output = #return_type> + Send;
        };

        let docs = self.docs();
        let supports = self.supports();

        quote! {
            #(#docs)*
            #method

            /// Returns `true` if the method is supported by the provider
            #[inline]
            fn #supports(&self) -> bool {
                true
            }
        }
    }

    fn generate_consumer_trait_method(&self) -> TokenStream {
        let docs = self.docs();
        let env_name = self.env_name();
        let signature = self.generate_consumer_method_signature(&env_name, true);

        let supports = self.supports();
        let supports_docs = self.supports_docs();

        quote! {
            #(#docs)*
            #signature;

            #(#supports_docs)*
            fn #supports(&self) -> bool;
        }
    }

    fn generate_consumer_multicast_trait_method(&self) -> TokenStream {
        debug_assert!(
            !self.stream_request,
            "request streaming is not supported for multicast"
        );

        let docs = self.docs();
        let env_name = self.env_name();
        let name = self.name();
        let inputs = self.generate_request_arguments();
        let header_param = self.generate_request_header_param();

        let supports = self.supports();
        let supports_docs = self.supports_docs();

        quote! {
            #(#docs)*
            fn #name<E: super::super::super::Env>(
                &self,
                #env_name: &E,
                #header_param
                #(#inputs),*
            ) -> multicast::#name::Iter<Self::Method, Self::PeerAddr>;

            #(#supports_docs)*
            fn #supports(&self) -> bool;
        }
    }

    fn generate_consumer_multicast_method_body(&self, idx: usize) -> TokenStream {
        assert!(
            !self.stream_request,
            "request streaming is not supported for multicast"
        );

        let _ = idx; // method index now consulted per-peer in Op::send.
        let env_name = self.env_name();
        let name = self.name();
        let inputs = self.generate_request_arguments();
        let header_param = self.generate_request_header_param();
        let request_mapping = self.generate_request_mapping();
        let imports = self.generate_impl_imports(ImportLocation::MulticastResolve);

        // Build the (Option<ReqHeader>, Req) tuple stored in Iter::request_parts.
        // For headerless methods, `None` would have ambiguous type, so use a
        // turbofish-annotated `Option::<Infallible>::None`.
        let header_tuple_elem = if self.request_header.is_some() {
            let header = self.unique_name("header");
            quote!(::core::option::Option::Some(#header.into()))
        } else {
            quote!(::core::option::Option::<::core::convert::Infallible>::None)
        };

        // Defer `create_raw_request` until `Op::send` runs so each peer's
        // request is built using THAT peer's negotiated MethodVersionEntry
        // (e.g. v1 to a v1-only follower, v2 to a v2-capable one). Building
        // it once here from `sessions[0]` would broadcast the same wire
        // bytes to everyone and silently violate per-peer version
        // negotiation — fine today only because the `Wire` envelope is
        // byte-identical across versions, but that breaks the moment any
        // method gains a real v2 field.
        quote! {
            fn #name<E: super::super::super::Env>(
                &self,
                #env_name: &E,
                #header_param
                #(#inputs),*
            ) -> multicast::#name::Iter<Self::Method, Self::PeerAddr> {
                #imports
                #request_mapping
                let _ = #env_name;

                multicast::#name::Iter {
                    request_parts: core::mem::MaybeUninit::new((#header_tuple_elem, request)),
                    index: 0,
                    sessions: self.sessions.clone(),
                }
            }
        }
    }

    fn generate_multicast_module(&self, idx: usize, namespace: &str) -> TokenStream {
        debug_assert!(
            !self.stream_request,
            "request streaming is not supported for multicast"
        );

        // Raw responses don't currently support multicast
        if self.raw_response {
            return quote!();
        }

        let name = self.name();
        let env_name = self.env_name();
        let return_type = self.generate_return_type();
        let imports = self.generate_impl_imports(ImportLocation::MulticastExecute);
        let span_name = self.generate_span_name(namespace, ImportLocation::MulticastExecute);
        let method = quote!(self.methods[#idx]);
        // Multicast excludes stream_request methods (asserted above), so the
        // bidi arms that consume the client task class are never reached; pass
        // the built-in no-op `Unlabeled` to satisfy the signature.
        let request_call = self.generate_consumer_request_call(
            &method,
            &env_name,
            quote!(),
            &quote!(upstream_core::runtime::task_monitor::Unlabeled),
        );
        let response_mapping = self.generate_response_extraction();

        // Concrete type paths for the request body and (optional) header,
        // used in Op/Iter field declarations where `Req`/`ReqHeader`
        // aliases aren't in scope (those only exist inside `use`-imported
        // function bodies).
        let struct_name = self.struct_name();
        let req_ty = quote!(super::schema::request::#struct_name);
        let req_header_ty = if self.request_header.is_some() {
            quote!(super::schema::request::header::#struct_name)
        } else {
            quote!(::core::convert::Infallible)
        };

        // Construct the raw request inside `Op::send` so each peer encodes
        // using its own negotiated MethodVersionEntry.
        let build_request_per_peer = quote! {
            let (header_opt, body) = self.request_parts;
            let request = self.methods[#idx].create_raw_request::<E, #req_header_ty, #req_ty>(
                #env_name,
                header_opt,
                body,
            );
        };

        let method_constraint =
            quote!(upstream_core::service::consumer::Method<super::super::super::Service>);

        let output_ty: TokenStream = self.output.ty.parse().unwrap();
        let (operation_trait_name, operation_constraint, operation_associated_types) =
            match (&self.response_header, self.stream_response) {
                (Some(header), true) => {
                    let header: TokenStream = header.parse().unwrap();
                    let name = quote!(OperationStreamWithHeader);
                    let constraint = quote! { #name<E, Header = #header, Output = #output_ty> };
                    let types = quote!(type Output = #output_ty; type Header = #header;);
                    (name, constraint, types)
                }
                (Some(header), false) => {
                    let header: TokenStream = header.parse().unwrap();
                    let name = quote!(OperationWithHeader);
                    let constraint = quote! { #name<E, Header = #header, Output = #output_ty> };
                    let types = quote!(type Output = #output_ty; type Header = #header;);
                    (name, constraint, types)
                }
                (None, true) => {
                    let name = quote!(OperationStream);
                    let constraint = quote! { #name<E, Output = #output_ty> };
                    let types = quote!(type Output = #output_ty;);
                    (name, constraint, types)
                }
                (None, false) => {
                    let name = quote!(Operation);
                    let constraint = quote! { #name<E, Output = #output_ty> };
                    let types = quote!(type Output = #output_ty;);
                    (name, constraint, types)
                }
            };

        quote! {
            pub mod #name {
                pub trait Operation<E: super::Env>: upstream_core::service::consumer::#operation_constraint {}

                impl<E: super::Env, T> Operation<E> for T where T: upstream_core::service::consumer::#operation_constraint {}

                #[doc(hidden)]
                pub struct Op<M: #method_constraint, PeerAddr> {
                    pub(crate) methods: ::std::sync::Arc<[M]>,
                    /// `(Option<ReqHeader>, Req)` — the raw request inputs.
                    /// `Op::send` calls `methods[idx].create_raw_request(...)`
                    /// at send time so every peer encodes using its own
                    /// negotiated `MethodVersionEntry`.
                    pub(crate) request_parts: (::core::option::Option<#req_header_ty>, #req_ty),
                    pub(crate) peer_addr: PeerAddr,
                }

                impl<M, PeerAddr> Clone for Op<M, PeerAddr>
                where
                    M: #method_constraint,
                    PeerAddr: Clone,
                {
                    fn clone(&self) -> Self {
                        Self {
                            methods: self.methods.clone(),
                            request_parts: self.request_parts.clone(),
                            peer_addr: self.peer_addr.clone(),
                        }
                    }
                }

                impl<M, PeerAddr, E> upstream_core::service::consumer::#operation_trait_name<E> for Op<M, PeerAddr>
                where
                    M: #method_constraint,
                    PeerAddr: upstream_core::service::consumer::PeerAddr,
                    E: super::Env
                {
                    #operation_associated_types

                    type PeerAddr = PeerAddr;

                    fn peer_addr(&self) -> &Self::PeerAddr {
                        &self.peer_addr
                    }

                    async fn send(self, env: &E) -> #return_type {
                        #imports
                        #build_request_per_peer
                        let mut span = env.#span_name();
                        span.on_serialize_multicast(env, request.serialized_len());
                        #request_call
                        #response_mapping
                    }
                }

                #[doc(hidden)]
                pub struct Iter<M: #method_constraint, PeerAddr> {
                    pub(crate) request_parts: core::mem::MaybeUninit<(
                        ::core::option::Option<#req_header_ty>,
                        #req_ty,
                    )>,
                    pub(crate) index: usize,
                    pub(crate) sessions: ::std::sync::Arc<[(PeerAddr, super::super::Session<M>)]>,
                }

                impl<M, PeerAddr> core::iter::Iterator for Iter<M, PeerAddr>
                where
                    M: #method_constraint,
                    PeerAddr: upstream_core::service::consumer::PeerAddr,
                {
                    type Item = Op<M, PeerAddr>;

                    fn next(&mut self) -> Option<Self::Item> {
                        let (peer_addr, sessions) = self.sessions.get(self.index)?;
                        let peer_addr = peer_addr.clone();
                        let methods = sessions.methods.clone();

                        self.index += 1;

                        let request_parts = if self.index == self.sessions.len() {
                            // avoid cloning the last request
                            unsafe { self.request_parts.assume_init_read() }
                        } else {
                            unsafe { self.request_parts.assume_init_ref() }.clone()
                        };

                        Some(Op {
                            methods,
                            request_parts,
                            peer_addr,
                        })
                    }
                }

                impl<M: #method_constraint, PeerAddr> Drop for Iter<M, PeerAddr> {
                    fn drop(&mut self) {
                        // check if we exhausted the iterator
                        if self.index != self.sessions.len() {
                            unsafe { self.request_parts.assume_init_drop() };
                        }
                    }
                }
            }
        }
    }

    fn generate_span_type(&self) -> TokenStream {
        match (
            self.stream_request,
            self.stream_response,
            self.unidirectional,
        ) {
            (_, _, true) => quote!(Unidirectional),
            (true, true, _) => quote!(bidi::Stream),
            (true, false, _) => quote!(StreamRequest),
            (false, true, _) => quote!(StreamResponse),
            (false, false, _) => quote!(Unary),
        }
    }

    fn generate_write_error(&self, err: impl ToTokens) -> TokenStream {
        let name = if self.raw_response {
            quote!(write_raw_error)
        } else if self.stream_response && self.response_header.is_some() {
            // For streaming responses with headers, use the special error handler
            // that matches the wire format of write_stream_with_header
            quote!(write_stream_header_error)
        } else {
            quote!(write_error)
        };
        quote!(response.#name(env, #err, span).await)
    }

    fn generate_dispatch_read_request(&self) -> TokenStream {
        let request_var = if self.inputs.is_empty() {
            quote!(_request)
        } else {
            quote!(request)
        };

        let request_pat = if self.request_header.is_some() {
            quote!((header, #request_var))
        } else {
            request_var.clone()
        };

        let inputs = self.generate_input_names().collect::<Vec<_>>();
        let name = self.name();

        let read_call = match (self.stream_request, self.request_header.is_some()) {
            (true, true) => quote!(read_stream_with_header::<_, ReqHeader, Req, _>),
            (true, false) => quote!(read_stream::<_, Req, _>),
            (false, true) => quote!(read_with_header::<_, ReqHeader, Req, _>),
            (false, false) => quote!(read::<_, Req, _>),
        };

        // Streamed-request reads take an owned per-frame instrument derived
        // from the span (`request_frames()`), because the returned request
        // stream is consumed lazily inside the provider handler — after
        // `on_process` and while the response is written — so it must outlive
        // borrows of `span`. Non-streamed reads take `&mut span` directly.
        let span_arg = if self.stream_request {
            quote!(, span.request_frames())
        } else {
            quote!(, &mut span)
        };

        let write_error = self.generate_write_error(quote!(err));

        let read_call = quote! {
            let request = request.#read_call(env #span_arg).await;
            let #request_pat = match request {
                Ok(v) => v,
                Err(err) => {
                    return #write_error;
                }
            };
        };

        let request_map = if self.stream_request {
            quote! {
                let #request_var = #request_var.map(|result| {
                    result.map(|req| req.into())
                });
            }
        } else {
            quote!()
        };

        let peer_addr = if self.peer_addr {
            quote!(, peer_addr)
        } else {
            quote!()
        };

        let provider_call = match (self.stream_request, self.request_header.is_some()) {
            (true, true) => quote! {
                // Call the provider method
                self.provider.#name(env #peer_addr, header.into(), #request_var).await
            },
            (true, false) => quote! {
                // Call the provider method
                self.provider.#name(env #peer_addr, #request_var).await
            },
            (false, true) => quote! {
                // Call the provider method
                self.provider.#name(env #peer_addr, header.into() #(, #request_var.#inputs)*).await
            },
            (false, false) => quote! {
                // Call the provider method
                self.provider.#name(env #peer_addr #(, #request_var.#inputs)*).await
            },
        };

        quote! {
            #read_call
            #request_map
            let span_process = span.on_process(env);
            let result = #provider_call;
        }
    }

    fn generate_dispatch_write_response(&self) -> TokenStream {
        let result_pat = if self.response_header.is_some() {
            quote!((header, result))
        } else if self.unidirectional {
            quote!(_result)
        } else {
            quote!(result)
        };

        let stream_mapping = quote!(
            let result = result.map(|result| result.map(Res));
        );

        let write_result = match (
            self.raw_response,
            self.stream_response,
            self.response_header.is_some(),
            self.unidirectional,
        ) {
            // For unidirectional methods, we don't need to send a response
            (_, _, _, true) => quote! {
                span_process.exit(env, &Ok(()));
                response.finish_unidirectional(env, span).await;
            },
            // Raw responses bypass normal serialization and don't use Response variants
            (true, _, _, false) => quote! {
                // For raw responses, we don't wrap in a Response variant
                use upstream_core::buffer::reader::Storage;
                span_process.exit(env, &Ok(result.buffered_len()));
                response.write_raw(env, result, span).await;
            },
            // Regular streaming responses with headers
            (false, true, true, _) => quote! {
                #stream_mapping
                span_process.exit(env, &Ok(()));
                response.write_stream_with_header(env, ResHeader(header), result, span).await;
            },
            // Regular streaming responses without headers
            (false, true, false, _) => quote! {
                #stream_mapping
                span_process.exit(env, &Ok(()));
                response.write_stream(env, result, span).await;
            },
            // Regular responses with headers. The value is wrapped in `Ok` only so
            // the span's `exit` hook can inspect the `&Result`; the `let Ok(..) else`
            // then recovers it without an `unsafe` unchecked-unwrap (the arm is
            // provably unreachable, so the optimizer elides it — clippy would
            // otherwise flag the literal unwrap as unnecessary_literal_unwrap).
            (false, false, true, _) => quote! {
                let result = Ok((ResHeader(header), Res(result)));
                span_process.exit(env, &result);
                let Ok((header, result)) = result else { unreachable!() };
                response.write_with_header(env, header, result, span).await;
            },
            // Regular responses without headers (see the header case above re: the
            // `Ok` wrap + `let Ok(..) else` instead of an unsafe unchecked-unwrap).
            (false, false, false, _) => quote! {
                let result = Ok(Res(result));
                span_process.exit(env, &result);
                let Ok(result) = result else { unreachable!() };
                response.write(env, result, span).await;
            },
        };

        let write_error = self.generate_write_error(quote!(err));

        quote! {
            match result {
                Ok(#result_pat) => {
                    #write_result
                }
                Err(err) => {
                    let result = Err(err);
                    span_process.exit(env, &result);
                    // Recover the error without an unsafe unchecked-unwrap (see the
                    // Ok arm); the Ok arm is provably unreachable here.
                    let Err(err) = result else { unreachable!() };
                    #write_error;
                }
            }
        }
    }

    fn generate_dispatch(&self, namespace: &str, method_task_class: &str) -> TokenStream {
        let struct_name = self.struct_name();
        let imports = self.generate_impl_imports(ImportLocation::Provider);
        let span_name = self.generate_span_name(namespace, ImportLocation::Provider);

        let read_request = self.generate_dispatch_read_request();
        let write_response = self.generate_dispatch_write_response();

        // The per-method (inner) task class lives in the sibling `schema`
        // module. Wrapping the whole arm body — read + provider call + write —
        // and polling it inline (awaited, not spawned) nests per-method
        // scheduling/poll samples inside the connection's per-service class.
        let method_task_class = Ident::new(method_task_class, Span::call_site());

        quote! {
            super::schema::Methods::#struct_name => {
                #imports
                // `env` is `&E`; clone the underlying `E` (not the reference)
                // for the instrumented future's publisher handle.
                let __task_env = ::core::clone::Clone::clone(env);
                upstream_core::runtime::task_monitor::instrument::<
                    super::schema::#method_task_class, _, _,
                >(__task_env, async {
                    let mut span = env.#span_name();
                    #read_request
                    #write_response
                })
                .await
            }
        }
    }

    /// Generate the per-version global_id map for single-input methods.
    ///
    /// Uses `VERSION_XPRODUCT_COUNT` for the input/output type's cross-product version count.
    /// The `input_version_idx` in each entry indexes into the input type's full
    /// cross-product space (for use with `deserialize_at`).
    ///
    /// For each (input_xproduct_idx, output_xproduct_idx) pair, computes a distinct
    /// method global_id using `xproduct_hash_bytes` to produce hash bytes on-the-fly.
    fn generate_versions(
        &self,
        name: &str,
        input_ty: &TokenStream,
        output_ty: &TokenStream,
        method_flags: &TokenStream,
        request_header: &TokenStream,
        response_header: &TokenStream,
    ) -> (TokenStream, TokenStream) {
        let map_name = Ident::new(
            &format!("METHOD_{}_VERSION_IDS", self.name),
            Span::call_site(),
        );

        let versions_static = quote! {
            #[allow(non_upper_case_globals, clippy::modulo_one)]
            static #map_name: upstream_core::service::GlobalIdMap<upstream_core::service::MethodVersionEntry> = {
                const IN_COUNT: usize = <#input_ty as upstream_core::wire::Wire>::__XPRODUCT_HASH_ENTRIES.len();
                const OUT_COUNT: usize = <#output_ty as upstream_core::wire::Wire>::__XPRODUCT_HASH_ENTRIES.len();
                const COUNT: usize = IN_COUNT * OUT_COUNT;

                /// Delegate to the shared `compute_method_global_id` for a given
                /// cross-product (input_idx, output_idx) pair.
                const fn compute_version_global_id(
                    hasher: upstream_core::service::Hasher,
                    name: &str,
                    input_idx: usize,
                    output_idx: usize,
                    request_header: Option<&upstream_core::wire::Type>,
                    response_header: Option<&upstream_core::wire::Type>,
                    flags: upstream_core::service::MethodFlags,
                ) -> u64 {
                    let (ref in_buf, in_len) = <#input_ty as upstream_core::wire::Wire>::__XPRODUCT_HASH_ENTRIES[input_idx];
                    let (ref out_buf, out_len) = <#output_ty as upstream_core::wire::Wire>::__XPRODUCT_HASH_ENTRIES[output_idx];
                    upstream_core::service::compute_method_global_id(
                        hasher, name,
                        in_buf, in_len as usize,
                        out_buf, out_len as usize,
                        request_header, response_header, flags,
                    )
                }

                if COUNT <= 1 {
                    upstream_core::service::GlobalIdMap::EMPTY
                } else {
                    // Collect all cross-product global_ids (dense, for keys).
                    // Skip (i=0, j=0) — the newest input × newest output — because it
                    // produces the same global_id as the primary method global_id.
                    const VERSION_COUNT: usize = COUNT - 1;
                    const KEYS: [u64; VERSION_COUNT] = {
                        let mut keys = [0u64; VERSION_COUNT];
                        let mut ki = 0;
                        let mut i = 0;
                        while i < IN_COUNT {
                            let mut j = 0;
                            while j < OUT_COUNT {
                                // Skip (0,0) — duplicates the primary global_id
                                if i == 0 && j == 0 {
                                    j += 1;
                                    continue;
                                }
                                // Compute global_id using xproduct_hash_bytes on-the-fly.
                                // Iteration is newest-first (i=0 is newest input).
                                // xproduct indices are oldest-first, so invert.
                                keys[ki] = compute_version_global_id(
                                    INIT_HASHER, #name,
                                    IN_COUNT - 1 - i,
                                    OUT_COUNT - 1 - j,
                                    #request_header,
                                    #response_header,
                                    #method_flags,
                                );
                                ki += 1;
                                j += 1;
                            }
                            i += 1;
                        }
                        keys
                    };

                    if VERSION_COUNT == 0 {
                        upstream_core::service::GlobalIdMap::EMPTY
                    } else {

                    const TABLE_SIZE: usize = upstream_core::service::GlobalIdMap::<upstream_core::service::MethodVersionEntry>::table_size_for(&KEYS);

                    // Build sparse table with Option<(GlobalId, MethodVersionEntry)>
                    const TABLE: [Option<(u64, upstream_core::service::MethodVersionEntry)>; TABLE_SIZE] = {
                        let mut table: [Option<(u64, upstream_core::service::MethodVersionEntry)>; TABLE_SIZE] = [None; TABLE_SIZE];
                        // Re-iterate skipping (0,0) to compute input_version_idx correctly
                        let mut ki = 0;
                        let mut i = 0;
                        while i < IN_COUNT {
                            let mut j = 0;
                            while j < OUT_COUNT {
                                if i == 0 && j == 0 { j += 1; continue; }
                                // input_version_idx / output_version_idx index into the
                                // input/output cross-product spaces — the same indices used
                                // by serialize_at/deserialize_at. Iteration is newest-first
                                // (i=0/j=0 is newest), and xproduct indices are ordered
                                // oldest-first, so invert.
                                let input_version_idx = (IN_COUNT - 1 - i) as u32;
                                let output_version_idx = (OUT_COUNT - 1 - j) as u32;
                                table[KEYS[ki] as usize & (TABLE_SIZE - 1)] = Some((
                                    KEYS[ki],
                                    upstream_core::service::MethodVersionEntry {
                                        global_id: KEYS[ki],
                                        input_version_idx,
                                        output_version_idx,
                                    },
                                ));
                                ki += 1;
                                j += 1;
                            }
                            i += 1;
                        }
                        table
                    };

                    upstream_core::service::GlobalIdMap::new(&TABLE, &KEYS)

                    }
                }
            };
        };

        (versions_static, quote!(#map_name))
    }

    fn generate_info(&self, index: usize) -> TokenStream {
        let input = self.inputs.iter().map(|input| {
            let mut name = input.name.clone();
            name.push('\0');
            let ty: TokenStream = input.ty.parse().unwrap();
            quote! (
                upstream_core::service::Input {
                    name: Str::new(#name),
                    ty: upstream_core::wire::type_of::<#ty>(),
                }
            )
        });

        let output_ty: TokenStream = self.output.ty.parse().unwrap();
        let output = quote!(
            upstream_core::service::Output {
                ty: upstream_core::wire::type_of::<#output_ty>(),
            }
        );

        let mut name = self.name.clone();
        name.push('\0');

        let unidirectional = self.unidirectional;
        let unreliable = self.unreliable;
        let stream_request = self.stream_request;
        let stream_response = self.stream_response;
        let peer_addr = self.peer_addr;
        let raw_response = self.raw_response;

        // Priority is local transport metadata — emit it as a `Priority`
        // variant path, kept out of `method_flags`/`compute_method_global_id`.
        let priority_variant = Ident::new(&self.priority, Span::call_site());
        let priority = quote!(upstream_core::service::Priority::#priority_variant);

        let method_flags = quote! {
            upstream_core::service::MethodFlags {
                unidirectional: #unidirectional,
                unreliable: #unreliable,
                stream_request: #stream_request,
                stream_response: #stream_response,
                raw_response: #raw_response,
            }
        };

        let request_header = if let Some(ty) = &self.request_header {
            let ty: TokenStream = ty.parse().unwrap();
            quote!(Some(upstream_core::wire::type_of::<#ty>()))
        } else {
            quote!(None::<&upstream_core::wire::Type>)
        };

        let response_header = if let Some(ty) = &self.response_header {
            let ty: TokenStream = ty.parse().unwrap();
            quote!(Some(upstream_core::wire::type_of::<#ty>()))
        } else {
            quote!(None::<&upstream_core::wire::Type>)
        };

        // Primary global_id uses the latest (last) __XPRODUCT_HASH_ENTRIES for
        // each input/output type. For single-input methods this is the last entry;
        // for multi-input methods we build a combined hash buffer.
        let hash = if self.inputs.len() == 1 {
            let input_ty: TokenStream = self.inputs[0].ty.parse().unwrap();
            quote! {
                const {
                    let __in_entries = <#input_ty as upstream_core::wire::Wire>::__XPRODUCT_HASH_ENTRIES;
                    let (ref __in_buf, __in_len) = __in_entries[__in_entries.len() - 1];
                    let __out_entries = <#output_ty as upstream_core::wire::Wire>::__XPRODUCT_HASH_ENTRIES;
                    let (ref __out_buf, __out_len) = __out_entries[__out_entries.len() - 1];
                    upstream_core::service::compute_method_global_id(
                        INIT_HASHER, #name,
                        __in_buf, __in_len as usize,
                        __out_buf, __out_len as usize,
                        #request_header,
                        #response_header,
                        #method_flags,
                    )
                }
            }
        } else {
            // Multi-input: assemble input hash bytes from each input's TYPE
            let input_types: Vec<_> = self
                .inputs
                .iter()
                .map(|input| {
                    let ty: TokenStream = input.ty.parse().unwrap();
                    quote!(upstream_core::wire::type_of::<#ty>())
                })
                .collect();
            quote! {
                const {
                    // For multi-input methods, build input hash from TYPE trees directly
                    let mut __in_buf = [0u8; 4096];
                    let mut __in_cur = 0usize;
                    #(
                        #input_types.serialize_for_hash(&mut __in_buf, &mut __in_cur);
                    )*
                    let __out_entries = <#output_ty as upstream_core::wire::Wire>::__XPRODUCT_HASH_ENTRIES;
                    let (ref __out_buf, __out_len) = __out_entries[__out_entries.len() - 1];
                    upstream_core::service::compute_method_global_id(
                        INIT_HASHER, #name,
                        &__in_buf, __in_cur,
                        __out_buf, __out_len as usize,
                        #request_header,
                        #response_header,
                        #method_flags,
                    )
                }
            }
        };

        let method_name = Ident::new(&format!("METHOD_{}", self.name), Span::call_site());

        let (versions_static, versions_field) = if self.inputs.len() == 1 {
            let input_ty: TokenStream = self.inputs[0].ty.parse().unwrap();
            self.generate_versions(
                &name,
                &input_ty,
                &output_ty,
                &method_flags,
                &request_header,
                &response_header,
            )
        } else {
            (quote!(), quote!(upstream_core::service::GlobalIdMap::<upstream_core::service::MethodVersionEntry>::EMPTY))
        };

        // Introspection-only accessor for the per-version input/output TYPE
        // trees (used by tooling such as `upstreamd schemas`). A fn pointer
        // returning the request/response types' existing `VERSION_TYPES`
        // consts — no `Type` data is copied into the `Method` static, so this
        // does not reintroduce the inline-version-types compile cost. Only
        // wired for single-input methods (the versionable shape); multi-input
        // methods get `None`, mirroring how `versions` is handled above.
        let version_types_field = if self.inputs.len() == 1 {
            let input_ty: TokenStream = self.inputs[0].ty.parse().unwrap();
            quote!(::core::option::Option::Some(
                || upstream_core::service::MethodVersionTypes {
                    input: upstream_core::wire::version_types_of::<#input_ty>(),
                    output: upstream_core::wire::version_types_of::<#output_ty>(),
                }
            ))
        } else {
            quote!(::core::option::Option::None)
        };

        quote! {
            #versions_static

            #[allow(non_upper_case_globals)]
            pub(super) static #method_name: upstream_core::service::Method = {
                use upstream_core::service::Str;

                upstream_core::service::Method {
                    name: Str::new(#name),
                    inputs: &[#(#input),*],
                    output: &#output,
                    local_id: #index,
                    global_id: #hash,
                    flags: #method_flags,
                    request_header: #request_header,
                    response_header: #response_header,
                    peer_addr: #peer_addr,
                    priority: #priority,
                    versions: #versions_field,
                    version_types: #version_types_field,
                }
            };
        }
    }
}

#[derive(Clone, Debug)]
pub struct Input {
    pub name: String,
    pub description: Vec<String>,
    pub ty: String,
}

#[derive(Clone, Debug)]
pub struct Output {
    pub description: String,
    pub ty: String,
}
