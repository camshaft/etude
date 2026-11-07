// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use core::fmt;
use upstream_runtime::Error;
use upstream_wire::{Type, Wire};

pub use futures::stream;
pub mod consumer;
pub mod provider;
pub mod rpc_response;
pub mod span;
pub mod timer;
pub mod tracing;
pub use span::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Wire)]
#[wire(crate = upstream_wire, borrowed)]
pub struct ServerAddr {
    pub ip: std::net::IpAddr,
    pub acceptor_port: u16,
    pub handshake_port: u16,
}

impl fmt::Display for ServerAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.acceptor_addr().fmt(f)
    }
}

impl ServerAddr {
    pub const fn from_same_handshake_port(addr: std::net::SocketAddr) -> Self {
        Self {
            ip: addr.ip(),
            acceptor_port: addr.port(),
            handshake_port: addr.port(),
        }
    }

    /// Build a `ServerAddr` from the bound listening address and an
    /// optional negotiation handshake port.
    ///
    /// Use from a transport's `Server::local_addr()` impl when the
    /// transport has no separate handshake socket: pass `addr` for the
    /// acceptor (and IP), and `handshake_port` for the negotiation
    /// handshake's advertised port (typically `Handshake::local_handshake_port()`).
    /// `None` falls back to the bound address's port — equivalent to
    /// [`Self::from_same_handshake_port`].
    pub const fn from_local_addr(addr: std::net::SocketAddr, handshake_port: Option<u16>) -> Self {
        Self {
            ip: addr.ip(),
            acceptor_port: addr.port(),
            handshake_port: match handshake_port {
                Some(p) => p,
                None => addr.port(),
            },
        }
    }

    pub const fn ip(&self) -> std::net::IpAddr {
        self.ip
    }

    pub fn set_ip(&mut self, ip: std::net::IpAddr) {
        self.ip = ip;
    }

    #[inline]
    pub const fn acceptor_addr(&self) -> std::net::SocketAddr {
        std::net::SocketAddr::new(self.ip, self.acceptor_port)
    }

    #[inline]
    pub const fn handshake_addr(&self) -> std::net::SocketAddr {
        std::net::SocketAddr::new(self.ip, self.handshake_port)
    }
}

pub trait RequestStream<T>: stream::Stream<Item = Result<T, Error>> + Send + 'static
where
    T: Send + 'static,
{
}

impl<S, T> RequestStream<T> for S
where
    S: stream::Stream<Item = Result<T, Error>> + Send + 'static,
    T: Send + 'static,
{
}

pub trait ResponseStream<T>: stream::Stream<Item = Result<T, Error>> + Send + 'static
where
    T: Send + 'static,
{
}

impl<S, T> ResponseStream<T> for S
where
    S: stream::Stream<Item = Result<T, Error>> + Send + 'static,
    T: Send + 'static,
{
}

#[doc(hidden)]
pub use upstream_sha2_const::Sha256 as Hasher;

pub use s2n_quic_core::event::metrics::aggregate::info::Str;

pub type GlobalId = u64;
pub type LocalId = usize;

/// Per-method support status in a `SupportedSet`.
///
/// Two states. Every *supported* method resolves to a concrete
/// [`MethodVersionEntry`] describing the negotiated wire layout — including
/// the current version, which carries its own entry (`global_id` =
/// the method's primary id; `input_version_idx`/`output_version_idx` = the
/// newest cross-product indices). There is deliberately **no** "supported but
/// version unknown" state, and no separate "current vs older" variant: asking
/// a supported method for its negotiated version always yields a real entry,
/// never absence.
///
///   - [`Unsupported`](Self::Unsupported): the peer does not speak this method.
///   - [`Supported`](Self::Supported): the peer speaks this method at the wire
///     layout described by the entry. For the current version this is the
///     newest layout; `serialize_at(newest)`/`deserialize_at(newest)` are
///     byte-identical to the plain `serialize`/`deserialize` (see
///     `serialize_at_newest_index_is_byte_identical_to_plain_serialize` in
///     core-wire), so the `_at` path is correct and zero-cost there.
///
/// (Earlier designs used `Supported(Option<MethodVersionEntry>)` /
/// `SupportedCurrent`, where the current version was represented by the
/// *absence* of an entry. That overloaded "no entry" to mean "current
/// version," which is exactly the ambiguity this enum removes: a negotiated
/// version is always a concrete version.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodSupport {
    /// The peer does not support this method.
    Unsupported,
    /// The peer supports this method at the layout described by the entry.
    Supported(MethodVersionEntry),
}

impl MethodSupport {
    /// Whether the peer supports this method at any version.
    pub const fn is_supported(&self) -> bool {
        matches!(self, Self::Supported(_))
    }

    /// The negotiated version entry for this method, or `None` if unsupported.
    ///
    /// Every supported method — current or older — carries a concrete entry,
    /// so `Some` here always means "this exact wire layout was negotiated."
    /// The entry flows to `MethodHandle::negotiated_version` (client) and the
    /// server's response path, driving `serialize_at`/`deserialize_at` at the
    /// negotiated indices.
    pub const fn version_entry(&self) -> Option<MethodVersionEntry> {
        match self {
            Self::Supported(entry) => Some(*entry),
            Self::Unsupported => None,
        }
    }
}

/// An entry in the per-method version map. Each entry represents a distinct
/// wire layout (TYPE tree) for the method, identified by a unique `global_id`.
///
/// The `global_id` is hashed over BOTH the input and output cross-product
/// (see `generate_versions` in core-codegen/src/service.rs), so a negotiated
/// `global_id` pins a specific `(input_version, output_version)` pair. Both
/// indices are recorded here so the request and the response can each be
/// (de)serialized at the negotiated layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MethodVersionEntry {
    pub global_id: GlobalId,
    /// 0-based index into the input type's `VERSION_TYPES` array for this combination.
    ///
    /// The server uses this to call `RequestType::__wire_deserialize_at(bytes, input_version_idx)`
    /// after matching `global_id` → method, ensuring the correct monolithic inlined deserializer
    /// is invoked for the negotiated request wire layout.
    ///
    /// For non-versioned methods the versions map is empty, so this field is never read.
    pub input_version_idx: u32,
    /// 0-based index into the output type's `VERSION_TYPES` array for this combination.
    ///
    /// The server uses this to call `ResponseType::serialize_at(bytes, output_version_idx)`
    /// when writing the response, and the client uses it to call
    /// `deserialize_at` when reading the response — so both ends agree on the
    /// negotiated response wire layout. For methods whose output type is not
    /// versioned (`OUT_COUNT == 1`) this is always 0.
    pub output_version_idx: u32,
}

/// A const-constructible hash map for O(1) `global_id` → `V` lookup.
///
/// `HashMap`/`HashSet` cannot be used here because `Method` is a `static`
/// initialized at compile time, and Rust's const evaluation does not support
/// heap allocation. Instead, this uses an open-addressing scheme with a sparse
/// power-of-two array: each entry is placed at `global_id & (table.len() - 1)`.
/// The codegen finds the smallest collision-free power-of-two table size.
/// Empty slots contain `None`. Lookup is a single bitwise AND + compare: O(1).
///
/// A separate dense `keys` array stores all `global_id` values for iteration
/// during version negotiation (the sparse table cannot be iterated for keys
/// because empty slots are `None`).
#[derive(Clone, Copy, Debug)]
pub struct GlobalIdMap<V: Copy + 'static> {
    table: &'static [Option<(GlobalId, V)>],
    keys: &'static [GlobalId],
}

impl<V: Copy + 'static> GlobalIdMap<V> {
    pub const EMPTY: Self = Self {
        table: &[],
        keys: &[],
    };

    pub const fn new(table: &'static [Option<(GlobalId, V)>], keys: &'static [GlobalId]) -> Self {
        assert!(
            table.is_empty() || table.len().is_power_of_two(),
            "GlobalIdMap table size must be a power of two"
        );
        Self { table, keys }
    }

    /// Find the smallest power-of-two table size with no collisions for the given keys.
    ///
    /// Using a power-of-two size enables `key & (size - 1)` (bitwise AND) instead
    /// of `key % size` (modulo), which is faster and avoids clippy::modulo_one.
    #[doc(hidden)]
    pub const fn table_size_for(keys: &[GlobalId]) -> usize {
        let count = keys.len();
        if count == 0 {
            return 0;
        }
        // Start at the smallest power of two >= count
        let mut size = count.next_power_of_two();
        'outer: loop {
            let mask = size - 1;
            let mut slots = [false; 256]; // max reasonable version count
            let mut k = 0;
            while k < count {
                let slot = keys[k] as usize & mask;
                if slots[slot] {
                    size <<= 1; // next power of two
                    continue 'outer;
                }
                slots[slot] = true;
                k += 1;
            }
            return size;
        }
    }

    /// O(1) membership test.
    pub const fn contains(&self, id: GlobalId) -> bool {
        if self.table.is_empty() {
            return false;
        }
        let slot = id as usize & (self.table.len() - 1);
        match &self.table[slot] {
            Some((gid, _)) => *gid == id,
            None => false,
        }
    }

    /// O(1) lookup by global_id. Returns the value if found.
    pub const fn get(&self, id: GlobalId) -> Option<V> {
        if self.table.is_empty() {
            return None;
        }
        let slot = id as usize & (self.table.len() - 1);
        match &self.table[slot] {
            Some((gid, v)) if *gid == id => Some(*v),
            _ => None,
        }
    }

    /// Returns the dense list of all stored global_ids, for iteration during negotiation.
    pub const fn keys(&self) -> &[GlobalId] {
        self.keys
    }

    pub const fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub const fn len(&self) -> usize {
        self.keys.len()
    }
}

/// An enum for selecting one of two alternatives
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Either<A, B> {
    Left(A),
    Right(B),
}

/// Transport-level flags for an RPC method.
#[derive(Clone, Copy, Debug)]
pub struct MethodFlags {
    pub unidirectional: bool,
    pub unreliable: bool,
    pub stream_request: bool,
    pub stream_response: bool,
    pub raw_response: bool,
}

impl MethodFlags {
    /// Pack flags into a single byte for hashing.
    pub const fn to_byte(&self) -> u8 {
        (self.stream_request as u8)
            | (self.stream_response as u8) << 1
            | (self.unidirectional as u8) << 2
            | (self.unreliable as u8) << 3
            | (self.raw_response as u8) << 4
    }
}

/// The transport-level send priority for an RPC method.
///
/// This is a *local* transport concern — it controls how the underlying
/// transport (currently only experimental dcquic) prioritizes the request's
/// credit acquisition. It is deliberately NOT folded into `MethodFlags` or the
/// `global_id` hash: priority does not change the wire layout, so changing it
/// must not perturb version negotiation.
///
/// The 8 levels mirror the `Priority` enum in the vendored
/// `s2n-quic-dc-experimental` crate, monotonic from highest to lowest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Priority {
    Highest = 0,
    VeryHigh = 1,
    High = 2,
    Medium = 3,
    MediumLow = 4,
    Low = 5,
    VeryLow = 6,
    Lowest = 7,
}

impl Default for Priority {
    fn default() -> Self {
        Self::Medium
    }
}

/// Compute a method's `global_id` from pre-computed hash bytes and method metadata.
///
/// This is the shared hash formula used by both the primary `global_id` computation
/// and the per-cross-product version entries. The service codegen calls this function
/// instead of generating its own hash logic.
///
/// The `input_hash` and `output_hash` slices contain `serialize_for_hash` bytes
/// from the type's `__XPRODUCT_HASH_ENTRIES`. The `input_hash_len` / `output_hash_len`
/// parameters specify the valid prefix length within each slice.
pub const fn compute_method_global_id(
    namespace_hasher: Hasher,
    method_name: &str,
    input_hash: &[u8],
    input_hash_len: usize,
    output_hash: &[u8],
    output_hash_len: usize,
    request_header: Option<&upstream_wire::Type>,
    response_header: Option<&upstream_wire::Type>,
    flags: MethodFlags,
) -> GlobalId {
    let mut buffer = [0u8; 4096];
    let mut cursor = 0;
    let mut k = 0;
    while k < input_hash_len {
        buffer[cursor] = input_hash[k];
        cursor += 1;
        k += 1;
    }
    buffer[cursor] = 255;
    cursor += 1;
    k = 0;
    while k < output_hash_len {
        buffer[cursor] = output_hash[k];
        cursor += 1;
        k += 1;
    }
    buffer[cursor] = 255;
    cursor += 1;
    if let Some(ty) = request_header {
        ty.serialize_for_hash(&mut buffer, &mut cursor);
    }
    buffer[cursor] = 255;
    cursor += 1;
    if let Some(ty) = response_header {
        ty.serialize_for_hash(&mut buffer, &mut cursor);
    }
    buffer[cursor] = 255;
    cursor += 1;
    buffer[cursor] = flags.to_byte();
    cursor += 1;
    let (buffer, _) = buffer.split_at(cursor);
    namespace_hasher
        .update(method_name.as_bytes())
        .update(buffer)
        .finalize_u64()
}

#[derive(Clone, Copy, Debug)]
pub struct Method {
    pub name: &'static Str,
    pub inputs: &'static [Input],
    pub output: &'static Output,
    pub local_id: LocalId,
    /// The truncated hash of `SHA256(wire_name, $(inputs.type.id),* output.type.id)`
    ///
    /// Note that the probability of a collisions can be estimated with the following formula:
    ///
    /// ```text
    /// P(collision) ~= 1 - e ^ (-k(k-1)(2*2^64))
    /// ```
    ///
    /// In our case, `k` is the number of methods exposed in a system at any time. Even if a system had
    /// 1 million methods, the probability comes out to:
    ///
    /// ```text
    /// P(collision) ~= 2.71 * 10-8 (1 in 37 million).
    /// ```
    ///
    /// Most deployments will only have a few hundred methods so this is plenty.
    pub global_id: GlobalId,
    /// Transport-level flags for this method.
    pub flags: MethodFlags,
    /// Optional request header type for this method.
    /// If present, the header will be sent before the request payload.
    pub request_header: Option<&'static Type>,
    /// Optional response header type for this method.
    /// If present, the header will be sent before the response payload.
    pub response_header: Option<&'static Type>,
    /// Indicates that this method should receive the peer address.
    /// If true, the peer address will be passed to the provider.
    pub peer_addr: bool,
    /// The transport-level send priority for this method.
    ///
    /// Defaults to [`Priority::Medium`]. This is a local transport concern and
    /// is intentionally excluded from the `global_id` hash (see [`Priority`]).
    pub priority: Priority,
    /// Per-version global_id set for this method.
    /// For non-versioned methods, this is empty.
    /// For versioned methods, contains the global_id of each distinct wire layout.
    pub versions: GlobalIdMap<MethodVersionEntry>,
    /// Introspection-only accessor for this method's per-version wire-layout
    /// TYPE trees (input and output), used by tooling like `upstreamd schemas`
    /// to render the field schema of each version.
    ///
    /// This is a function pointer (not inline data): it returns the input/output
    /// [`MethodVersionTypes`], whose slices reference the request/response
    /// types' existing `Wire::VERSION_TYPES` consts. Storing a fn pointer keeps
    /// the always-linked `Method` static tiny — the `Type` trees are not copied
    /// into it. `None` for hand-rolled methods that don't wire this up.
    pub version_types: Option<fn() -> MethodVersionTypes>,
}

/// Per-version input/output wire-layout TYPE trees for a method, returned by
/// [`Method::version_types`]. Each slice is oldest-first (index 0 = the oldest
/// retained layout), matching `RpcVersionData`'s `version_idx` ordering.
#[derive(Clone, Copy, Debug)]
pub struct MethodVersionTypes {
    pub input: &'static [Type],
    pub output: &'static [Type],
}

impl Method {
    #[doc(hidden)]
    pub const fn sort_by_global_id<const N: usize>(
        mut methods: [&'static Self; N],
    ) -> [&'static Self; N] {
        sort_const::const_quicksort!(&mut methods, |a, b| a.global_id < b.global_id);
        methods
    }

    #[doc(hidden)]
    pub fn for_snapshot(&self) -> impl fmt::Debug + '_ {
        struct ForSnapshot<'a>(&'a Method);

        impl fmt::Debug for ForSnapshot<'_> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                let m = &self.0;

                // omit `local_id` since we're mostly just trying to preserve the global_id across commits

                let mut s = f.debug_struct("Method");

                s.field("global_id", &m.global_id)
                    .field("name", &m.name)
                    .field("inputs", &m.inputs)
                    .field("output", &m.output)
                    .field("unidirectional", &m.flags.unidirectional)
                    .field("unreliable", &m.flags.unreliable)
                    .field("stream_request", &m.flags.stream_request)
                    .field("stream_response", &m.flags.stream_response)
                    .field("peer_addr", &m.peer_addr)
                    .field("raw_response", &m.flags.raw_response);

                // Priority is not part of the global_id, so only surface it
                // when non-default to avoid churning every method snapshot.
                if m.priority != Priority::default() {
                    s.field("priority", &m.priority);
                }

                if let Some(t) = m.request_header {
                    s.field("request_header", &t);
                }

                if let Some(t) = m.response_header {
                    s.field("response_header", &t);
                }

                if !m.versions.is_empty() {
                    s.field("versions", &m.versions);
                }

                s.finish()
            }
        }

        ForSnapshot(self)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Input {
    pub name: &'static Str,
    pub ty: &'static Type,
}

#[derive(Clone, Copy, Debug)]
pub struct Output {
    pub ty: &'static Type,
}

pub trait Methods: 'static + Send + Sync + core::fmt::Debug + Clone + Copy {
    fn info(&self) -> &'static Method;
    fn by_global_id(global_id: GlobalId) -> Option<Self>
    where
        Self: Sized;
}

impl<A: Methods, B: Methods> Methods for Either<A, B> {
    fn info(&self) -> &'static Method {
        match self {
            Either::Left(a) => a.info(),
            Either::Right(b) => b.info(),
        }
    }

    fn by_global_id(global_id: GlobalId) -> Option<Self> {
        A::by_global_id(global_id)
            .map(Either::Left)
            .or_else(|| B::by_global_id(global_id).map(Either::Right))
    }
}

/// A per-peer record of which methods a peer supports and, for each, the
/// **single** negotiated wire version.
///
/// From the client's point of view, "the supported set is the negotiated
/// version": negotiation settles on exactly one version per (peer, method)
/// (the highest layout both ends share), so each method maps to at most one
/// [`MethodVersionEntry`]. There is no multi-version-per-method state.
///
/// What each part is actually for:
/// - [`version_entry`](Self::version_entry) is the **only** method that drives
///   wire behavior. Its result becomes `MethodHandle::negotiated_version` and
///   selects `serialize_at`/`deserialize_at` for both request and response.
/// - [`contains`](Self::contains) (and the generated `supports_<method>()` /
///   `MethodSupport::is_supported`) is a feature-detection hook that is **not
///   currently consulted on the call path**: an `Unsupported` method is not
///   blocked — it simply falls through to current-version dispatch (its
///   `version_entry` is `None`). It exists for forward use and is exercised by
///   tests; do not assume it gates RPCs today.
/// - [`from_negotiated`](Self::from_negotiated) builds the set from a peer's
///   advertised `global_id`s; [`all`](Self::all) is the fallback for the
///   negotiation-off / unknown-peer case (all methods at their current layout).
///
/// The trait and its generated `Set` are retained for backward compatibility
/// with existing generated consumers.
pub trait SupportedSet: 'static + Send + Sync + core::fmt::Debug + Clone {
    type Methods: Methods;

    fn contains(&self, method: &Self::Methods) -> bool;

    /// Returns the negotiated `MethodVersionEntry` for a method, or `None` if
    /// the method is unsupported.
    ///
    /// Every *supported* method yields `Some(entry)` — including one negotiated
    /// to the current version, whose entry carries the method's primary
    /// `global_id` and the newest cross-product indices. `None` means only
    /// "unsupported." (`MethodSupport::version_entry` is the underlying
    /// projection: `Supported(entry) → Some`, `Unsupported → None`.)
    fn version_entry(&self, _method: &Self::Methods) -> Option<MethodVersionEntry>;

    /// Build a `SupportedSet` from negotiated per-peer `global_id`s.
    ///
    /// For each local method, checks if any of the peer's advertised `global_id`s
    /// match (current or version entries). If the current `global_id` matches, the
    /// method is supported at the current version and stores its current-version
    /// entry (primary `global_id` + newest cross-product indices). If a version
    /// `global_id` matches, the method is supported at that older version and the
    /// looked-up `MethodVersionEntry` is stored. Either way a supported method
    /// holds a concrete entry.
    fn from_negotiated(
        _methods: impl Iterator<Item = &'static Method>,
        _peer_global_ids: &[GlobalId],
    ) -> Self;

    /// Returns the set with all methods supported at their current versions.
    fn all() -> Self;
}

impl<A: SupportedSet, B: SupportedSet> SupportedSet for (A, B) {
    type Methods = Either<A::Methods, B::Methods>;

    fn contains(&self, method: &Self::Methods) -> bool {
        match method {
            Either::Left(m) => self.0.contains(m),
            Either::Right(m) => self.1.contains(m),
        }
    }

    fn version_entry(&self, method: &Self::Methods) -> Option<MethodVersionEntry> {
        match method {
            Either::Left(m) => self.0.version_entry(m),
            Either::Right(m) => self.1.version_entry(m),
        }
    }

    fn from_negotiated(
        _methods: impl Iterator<Item = &'static Method>,
        _peer_global_ids: &[GlobalId],
    ) -> Self {
        // Tuple services delegate to each component's from_negotiated.
        // The methods iterator interleaves both services' methods, so we
        // cannot split it here. Each component uses its own Service::methods().
        (A::all(), B::all())
    }

    fn all() -> Self {
        (A::all(), B::all())
    }
}

pub trait Service: 'static + Clone + Copy + Send + Sync {
    type Methods: Methods;
    // needs to be clone for retries
    type Request: 'static + Clone + Send + Sync;
    type RequestHeader: 'static + Clone + Send + Sync;
    type Response: 'static + Send + Sync;
    type ResponseHeader: 'static + Send + Sync;
    type SupportedSet: SupportedSet<Methods = Self::Methods>;

    /// Returns a unique identifier for this service interface.
    ///
    /// The interface ID is computed by hashing all method global_ids together.
    /// This provides a stable identifier for the service that can be used for
    /// versioning and compatibility checking.
    ///
    /// The interface ID is computed as:
    /// ```text
    /// interface_id = SHA256(method1.global_id || method2.global_id || ...)
    /// ```
    /// where methods are sorted by global_id in ascending order.
    const INTERFACE_ID: GlobalId;

    /// Returns an iterator over all the namespaces in the service group
    fn namespaces() -> impl Iterator<Item = &'static Str>;

    /// Returns an iterator over all methods in this service
    ///
    /// This iterator will be ordered by `GlobalId` in ascending order
    fn methods() -> impl Iterator<Item = &'static Method>;

    fn parse_global_id(global_id: GlobalId) -> Option<Self::Methods> {
        Self::Methods::by_global_id(global_id)
    }

    /// Get the method associated with a request
    /// This is used by transports to determine which method to dispatch to
    fn method_for_request(request: &Self::Request) -> Self::Methods;
}

impl<A: Service, B: Service> Service for (A, B) {
    type Methods = Either<A::Methods, B::Methods>;
    type Request = Either<A::Request, B::Request>;
    type RequestHeader = Either<A::RequestHeader, B::RequestHeader>;
    type Response = Either<A::Response, B::Response>;
    type ResponseHeader = Either<A::ResponseHeader, B::ResponseHeader>;
    type SupportedSet = (A::SupportedSet, B::SupportedSet);

    const INTERFACE_ID: GlobalId = {
        upstream_sha2_const::Sha256::new()
            .update(&A::INTERFACE_ID.to_be_bytes())
            .update(&B::INTERFACE_ID.to_be_bytes())
            .finalize_u64()
    };

    fn namespaces() -> impl Iterator<Item = &'static Str> {
        A::namespaces().chain(B::namespaces())
    }

    fn methods() -> impl Iterator<Item = &'static Method> {
        upstream_iter::sorted_union_with(A::methods(), B::methods(), |a, b| {
            a.global_id.cmp(&b.global_id)
        })
    }

    fn parse_global_id(global_id: GlobalId) -> Option<Self::Methods> {
        A::parse_global_id(global_id)
            .map(Either::Left)
            .or_else(|| B::parse_global_id(global_id).map(Either::Right))
    }

    fn method_for_request(request: &Self::Request) -> Self::Methods {
        match request {
            Either::Left(req) => Either::Left(A::method_for_request(req)),
            Either::Right(req) => Either::Right(B::method_for_request(req)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_id_map_empty() {
        let map = GlobalIdMap::<u32>::EMPTY;
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
        assert!(!map.contains(42));
        assert!(map.get(42).is_none());
        assert!(map.keys().is_empty());
    }

    #[test]
    fn global_id_map_contains_and_get() {
        // Build a map with two entries. Power-of-two table size 4.
        // 100 & 3 = 0, 200 & 3 = 0 — collision! Use 101 & 3 = 1, 200 & 3 = 0.
        static TABLE: [Option<(u64, u32)>; 4] = [Some((200, 2)), Some((101, 1)), None, None];
        static KEYS: [u64; 2] = [101, 200];
        let map = GlobalIdMap::new(&TABLE, &KEYS);

        assert!(!map.is_empty());
        assert_eq!(map.len(), 2);

        assert!(map.contains(101));
        assert!(map.contains(200));
        assert!(!map.contains(300));
        assert!(!map.contains(0));

        assert_eq!(map.get(101), Some(1));
        assert_eq!(map.get(200), Some(2));
        assert!(map.get(300).is_none());
    }

    #[test]
    fn global_id_map_keys() {
        static TABLE: [Option<(u64, u32)>; 2] = [Some((10, 0)), Some((21, 1))];
        static KEYS: [u64; 2] = [10, 21];
        let map = GlobalIdMap::new(&TABLE, &KEYS);

        assert_eq!(map.keys(), &[10, 21]);
    }

    #[test]
    fn table_size_for_empty() {
        assert_eq!(GlobalIdMap::<u32>::table_size_for(&[]), 0);
    }

    #[test]
    fn table_size_for_no_collisions() {
        let keys = [100u64, 200, 300, 400, 500];
        let size = GlobalIdMap::<u32>::table_size_for(&keys);

        // Table size must be a power of two
        assert!(
            size.is_power_of_two(),
            "table size {size} is not a power of two"
        );

        // Verify: no two keys map to the same slot using bitwise AND
        let mask = size - 1;
        let mut slots = std::collections::HashSet::new();
        for k in &keys {
            assert!(
                slots.insert(*k as usize & mask),
                "collision at size {size} for key {k}"
            );
        }
    }

    #[test]
    fn method_flags_to_byte_all_false() {
        let flags = MethodFlags {
            stream_request: false,
            stream_response: false,
            unidirectional: false,
            unreliable: false,
            raw_response: false,
        };
        assert_eq!(flags.to_byte(), 0);
    }

    #[test]
    fn method_flags_to_byte_individual_bits() {
        assert_eq!(
            MethodFlags {
                stream_request: true,
                stream_response: false,
                unidirectional: false,
                unreliable: false,
                raw_response: false
            }
            .to_byte(),
            0b00001
        );
        assert_eq!(
            MethodFlags {
                stream_request: false,
                stream_response: true,
                unidirectional: false,
                unreliable: false,
                raw_response: false
            }
            .to_byte(),
            0b00010
        );
        assert_eq!(
            MethodFlags {
                stream_request: false,
                stream_response: false,
                unidirectional: true,
                unreliable: false,
                raw_response: false
            }
            .to_byte(),
            0b00100
        );
        assert_eq!(
            MethodFlags {
                stream_request: false,
                stream_response: false,
                unidirectional: false,
                unreliable: true,
                raw_response: false
            }
            .to_byte(),
            0b01000
        );
        assert_eq!(
            MethodFlags {
                stream_request: false,
                stream_response: false,
                unidirectional: false,
                unreliable: false,
                raw_response: true
            }
            .to_byte(),
            0b10000
        );
    }
}
