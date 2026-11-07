use crate::{DEFAULT_BUFFER_CAPACITY, Deserialize, Serialize, de::ErrorKind};
use bolero::check;
use bytes::Bytes;
use etude_bytevec::{Builder, ByteVec};
use insta::assert_debug_snapshot;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt::Debug,
};

mod borrowed;

// Macro for generating deserialize fuzz tests
macro_rules! generate_deserialize_fuzz_test {
    ($test_name:ident, $type:ty) => {
        #[test]
        fn $test_name() {
            bolero::check!()
                .with_type::<ByteVec>()
                .cloned()
                .for_each(|mut bytes| {
                    let _ = <$type>::deserialize(&mut bytes);
                });
        }
    };
}

// Generate deserialize fuzz tests for primitive types
generate_deserialize_fuzz_test!(fuzz_bool_deserialize, bool);
generate_deserialize_fuzz_test!(fuzz_u8_deserialize, u8);
generate_deserialize_fuzz_test!(fuzz_u16_deserialize, u16);
generate_deserialize_fuzz_test!(fuzz_u32_deserialize, u32);
generate_deserialize_fuzz_test!(fuzz_u64_deserialize, u64);
generate_deserialize_fuzz_test!(fuzz_u128_deserialize, u128);
generate_deserialize_fuzz_test!(fuzz_usize_deserialize, usize);
generate_deserialize_fuzz_test!(fuzz_i8_deserialize, i8);
generate_deserialize_fuzz_test!(fuzz_i16_deserialize, i16);
generate_deserialize_fuzz_test!(fuzz_i32_deserialize, i32);
generate_deserialize_fuzz_test!(fuzz_i64_deserialize, i64);
generate_deserialize_fuzz_test!(fuzz_i128_deserialize, i128);
generate_deserialize_fuzz_test!(fuzz_isize_deserialize, isize);

// Generate deserialize fuzz tests for compound types
generate_deserialize_fuzz_test!(fuzz_option_u32_deserialize, Option<u32>);
generate_deserialize_fuzz_test!(fuzz_option_string_deserialize, Option<String>);
generate_deserialize_fuzz_test!(fuzz_result_u32_u8_deserialize, Result<u32, u8>);
generate_deserialize_fuzz_test!(fuzz_result_string_u8_deserialize, Result<String, u8>);
generate_deserialize_fuzz_test!(fuzz_string_deserialize, String);
generate_deserialize_fuzz_test!(fuzz_bytevec_deserialize, ByteVec);
generate_deserialize_fuzz_test!(fuzz_deserialize_error_deserialize, ErrorKind);
generate_deserialize_fuzz_test!(fuzz_unit_deserialize, ());
generate_deserialize_fuzz_test!(fuzz_btreemap_u32_string_deserialize, BTreeMap<u32, String>);
generate_deserialize_fuzz_test!(fuzz_btreemap_string_vec_deserialize, BTreeMap<String, Vec<u8>>);
generate_deserialize_fuzz_test!(fuzz_btreeset_u32_deserialize, BTreeSet<u32>);
generate_deserialize_fuzz_test!(fuzz_vec_u8_deserialize, Vec<u8>);
generate_deserialize_fuzz_test!(fuzz_vec_string_deserialize, Vec<String>);
generate_deserialize_fuzz_test!(fuzz_u8_array_deserialize, [u8; 4]);
generate_deserialize_fuzz_test!(fuzz_tuple_deserialize, (u32, String));
generate_deserialize_fuzz_test!(fuzz_box_u32_deserialize, Box<u32>);
generate_deserialize_fuzz_test!(fuzz_box_string_deserialize, Box<String>);
generate_deserialize_fuzz_test!(fuzz_vec_deque_u32_deserialize, VecDeque<u32>);

// Helper function to test round-trip serialization and deserialization
pub(crate) fn test_round_trip<T>(value: T)
where
    T: Serialize + Deserialize + PartialEq + Debug + Clone,
{
    let mut builder = Builder::default();
    value.clone().serialize(&mut builder);
    let mut bytes = builder.finish();
    let deserialized = T::deserialize(&mut bytes).expect("Failed to deserialize");
    assert_eq!(value, deserialized, "Round trip failed");
}

// Helper function for snapshot testing
pub(crate) fn test_serialization_snapshot<T>(value: T, name: &str)
where
    T: Serialize + Debug,
{
    let mut builder = Builder::default();
    value.serialize(&mut builder);
    let bytes = builder.finish();
    assert_debug_snapshot!(name, bytes);
}

// Macro for generating round-trip tests for types that implement PartialEq
macro_rules! generate_round_trip_test {
    ($test_name:ident, $type:ty) => {
        #[test]
        fn $test_name() {
            bolero::check!()
                .with_type::<$type>()
                .cloned()
                .for_each(|value| {
                    $crate::tests::test_round_trip(value);
                });
        }
    };
}

// Generate round-trip tests for primitive types
generate_round_trip_test!(test_bool_round_trip, bool);
generate_round_trip_test!(test_u8_round_trip, u8);
generate_round_trip_test!(test_u16_round_trip, u16);
generate_round_trip_test!(test_u32_round_trip, u32);
generate_round_trip_test!(test_u64_round_trip, u64);
generate_round_trip_test!(test_u128_round_trip, u128);
generate_round_trip_test!(test_usize_round_trip, usize);
generate_round_trip_test!(test_i8_round_trip, i8);
generate_round_trip_test!(test_i16_round_trip, i16);
generate_round_trip_test!(test_i32_round_trip, i32);
generate_round_trip_test!(test_i64_round_trip, i64);
generate_round_trip_test!(test_i128_round_trip, i128);
generate_round_trip_test!(test_isize_round_trip, isize);

// Floats round-trip bit-for-bit. They are compared by bits rather than by value
// so a generated NaN (never `Eq` to itself) still asserts a correct round trip.
#[test]
fn test_f32_round_trip() {
    bolero::check!()
        .with_type::<f32>()
        .cloned()
        .for_each(|value| {
            let mut builder = Builder::default();
            value.serialize(&mut builder);
            let mut bytes = builder.finish();
            let got = f32::deserialize(&mut bytes).expect("Failed to deserialize");
            assert_eq!(value.to_bits(), got.to_bits(), "f32 round trip failed");
        });
}

#[test]
fn test_f64_round_trip() {
    bolero::check!()
        .with_type::<f64>()
        .cloned()
        .for_each(|value| {
            let mut builder = Builder::default();
            value.serialize(&mut builder);
            let mut bytes = builder.finish();
            let got = f64::deserialize(&mut bytes).expect("Failed to deserialize");
            assert_eq!(value.to_bits(), got.to_bits(), "f64 round trip failed");
        });
}

// Generate round-trip tests for compound types
generate_round_trip_test!(test_option_u32_round_trip, Option<u32>);
generate_round_trip_test!(test_option_string_round_trip, Option<String>);
generate_round_trip_test!(test_result_u32_u8_round_trip, Result<u32, u8>);
generate_round_trip_test!(test_result_string_u8_round_trip, Result<String, u8>);
generate_round_trip_test!(test_string_round_trip, String);
generate_round_trip_test!(test_bytevec_round_trip, ByteVec);
generate_round_trip_test!(test_deserialize_error_round_trip, ErrorKind);
generate_round_trip_test!(test_unit_round_trip, ());
generate_round_trip_test!(test_btreemap_u32_string_round_trip, BTreeMap<u32, String>);
generate_round_trip_test!(test_btreemap_string_vec_round_trip, BTreeMap<String, Vec<u8>>);
generate_round_trip_test!(test_btreeset_u32_round_trip, BTreeSet<u32>);
generate_round_trip_test!(test_vec_u8_round_trip, Vec<u8>);
generate_round_trip_test!(test_vec_string_round_trip, Vec<String>);
generate_round_trip_test!(test_u8_array_round_trip, [u8; 4]);
generate_round_trip_test!(test_tuple_round_trip, (u32, String));
generate_round_trip_test!(test_box_u32_round_trip, Box<u32>);
generate_round_trip_test!(test_box_string_round_trip, Box<String>);

// Macro for generating snapshot tests
macro_rules! generate_snapshot_test {
    ($test_name:ident, $value:expr, $snapshot_name:expr) => {
        #[test]
        fn $test_name() {
            $crate::tests::test_serialization_snapshot($value, $snapshot_name);
        }
    };
}

// Generate snapshot tests
generate_snapshot_test!(test_u8_snapshot, 42u8, "u8_42");
generate_snapshot_test!(test_u16_snapshot, 12345u16, "u16_12345");
generate_snapshot_test!(test_u32_snapshot, 305419896u32, "u32_305419896");
generate_snapshot_test!(
    test_u64_snapshot,
    72623859790382856u64,
    "u64_72623859790382856"
);
generate_snapshot_test!(test_option_some_snapshot, Some(42u8), "option_some_u8_42");
generate_snapshot_test!(test_option_none_snapshot, None::<u8>, "option_none_u8");
generate_snapshot_test!(
    test_result_ok_snapshot,
    Ok::<_, u8>(42u8),
    "result_ok_u8_42"
);
generate_snapshot_test!(
    test_result_err_snapshot,
    Err::<u8, _>(42u8),
    "result_err_u8_42"
);
generate_snapshot_test!(test_string_snapshot, "hello".to_string(), "string_hello");
generate_snapshot_test!(test_unit_snapshot, (), "unit");
generate_snapshot_test!(test_box_u32_snapshot, Box::new(42u32), "box_u32_42");
generate_snapshot_test!(
    test_box_string_snapshot,
    Box::new("hello".to_string()),
    "box_string_hello"
);

// Vec snapshot tests
generate_snapshot_test!(test_vec_u8_snapshot, vec![1u8, 2u8, 3u8], "vec_u8_1_2_3");
generate_snapshot_test!(
    test_vec_string_snapshot,
    vec!["hello".to_string(), "world".to_string()],
    "vec_string_hello_world"
);

// Fixed-size array snapshot test
generate_snapshot_test!(test_u8_array_snapshot, [1u8, 2u8, 3u8, 4u8], "u8_array_4");

// Tuple snapshot test
generate_snapshot_test!(
    test_tuple_snapshot,
    (42u32, "hello".to_string()),
    "tuple_u32_string"
);

// Tests for types that need special handling

// ByteVec snapshot test
#[test]
fn test_bytevec_snapshot() {
    let bytes = ByteVec::from(vec![1, 2, 3]);
    test_serialization_snapshot(bytes, "bytevec_1_2_3");
}

// Bytes snapshot test
#[test]
fn test_bytes_snapshot() {
    let bytes = Bytes::from(vec![1, 2, 3]);
    test_serialization_snapshot(bytes, "bytes_1_2_3");
}

#[test]
fn test_bytes_round_trip() {
    check!().with_type::<Vec<u8>>().cloned().for_each(|value| {
        test_round_trip(Bytes::from(value));
    });
}

// BTreeMap snapshot tests
#[test]
fn test_btreemap_snapshot() {
    let mut map = BTreeMap::new();
    map.insert(1u32, "one".to_string());
    map.insert(2u32, "two".to_string());
    test_serialization_snapshot(map, "btreemap_u32_string");

    let mut nested_map = BTreeMap::new();
    let mut inner1 = BTreeMap::new();
    inner1.insert("a".to_string(), vec![1, 2, 3]);
    let mut inner2 = BTreeMap::new();
    inner2.insert("b".to_string(), vec![4, 5, 6]);
    nested_map.insert(1u32, inner1);
    nested_map.insert(2u32, inner2);
    test_serialization_snapshot(nested_map, "btreemap_nested");
}

// Generate DeserializeError snapshot tests
generate_snapshot_test!(
    test_error_invalid_type_snapshot,
    ErrorKind::InvalidType,
    "error_invalid_type"
);
generate_snapshot_test!(
    test_error_invalid_length_snapshot,
    ErrorKind::InvalidLength,
    "error_invalid_length"
);
generate_snapshot_test!(
    test_error_invalid_utf8_snapshot,
    ErrorKind::InvalidUtf8,
    "error_invalid_utf8"
);
generate_snapshot_test!(
    test_error_invalid_bytes_snapshot,
    ErrorKind::InvalidBytes,
    "error_invalid_bytes"
);
generate_snapshot_test!(
    test_error_invalid_variant_snapshot,
    ErrorKind::InvalidVariant,
    "error_invalid_variant"
);
generate_snapshot_test!(
    test_error_invalid_list_snapshot,
    ErrorKind::InvalidList,
    "error_invalid_list"
);

#[derive(etude_wire_derive::Wire)]
#[wire(crate = crate)]
struct SkipTest {
    a: String,
    #[wire(skip)]
    b: bool,
    c: String,
}

#[test]
fn skip_test() {
    let mut builder = Builder::default();
    SkipTest {
        a: "hello".to_string(),
        b: true,
        c: "world".to_string(),
    }
    .serialize(&mut builder);
    let mut bytes = builder.finish();
    let deserialized = SkipTest::deserialize(&mut bytes).unwrap();
    assert_eq!(deserialized.a, "hello");
    assert!(!deserialized.b);
    assert_eq!(deserialized.c, "world");
}

#[test]
fn test_default_buffer_size_reminder() {
    // dummy test to remind that setting this buffer too large will result in OOM
    assert_eq!(1 << 8, DEFAULT_BUFFER_CAPACITY)
}

/// The test binary's allocator: the system's, which also records the largest allocation each thread asks for, so a
/// test reads what one decode asked for on its own thread whatever other tests run beside it.
mod largest_allocation {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
    };

    thread_local! {
        static LARGEST: Cell<usize> = const { Cell::new(0) };
    }

    fn record(size: usize) {
        // The thread's cell has no destructor and doesn't allocate, so the allocator may reach it; `try_with` fails
        // only while the thread's locals are torn down, and that allocation goes unrecorded.
        let _ = LARGEST.try_with(|largest| largest.set(largest.get().max(size)));
    }

    struct Recording;

    unsafe impl GlobalAlloc for Recording {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            record(layout.size());
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            record(layout.size());
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            record(new_size);
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: Recording = Recording;

    /// Runs `f`, and answers its result with the largest allocation, in bytes, that it asked for on this thread.
    pub(super) fn during<R>(f: impl FnOnce() -> R) -> (R, usize) {
        LARGEST.with(|largest| largest.set(0));
        let result = f();
        (result, LARGEST.with(Cell::get))
    }
}

/// A collection's length is the peer's to set: an input that claims more elements than it holds fails
/// `InvalidLength` once its bytes run out, and reserves under 1 KiB, where it reserved what the length claimed.
#[test]
fn collection_length_past_the_input_fails_without_reserving_it() {
    fn claim(len: u64, held: usize) -> ByteVec {
        let mut input = len.to_be_bytes().to_vec();
        input.resize(input.len() + held, 7);
        ByteVec::from(input)
    }

    fn assert_refused<T: Deserialize + Debug>(len: u64, held: usize) {
        let mut bytes = claim(len, held);
        let (result, largest) = largest_allocation::during(|| T::deserialize(&mut bytes));
        let error = result.expect_err("the input doesn't hold the claimed elements");
        assert_eq!(error.kind(), ErrorKind::InvalidLength, "{}", std::any::type_name::<T>());
        assert!(
            largest < 1024,
            "{} reserved {largest} bytes for an input of {held} bytes past its length",
            std::any::type_name::<T>()
        );
    }

    // The most a length can claim: reserving it panics.
    let most = usize::MAX as u64;
    assert_refused::<VecDeque<u8>>(most, 1);
    assert_refused::<VecDeque<u64>>(most, 1);
    // The most `Vec`'s bound takes: reserving it asks for 4 GiB.
    assert_refused::<Vec<u8>>(u64::from(u32::MAX), 1);
    assert_refused::<String>(u64::from(u32::MAX), 1);
    assert_refused::<std::sync::Arc<str>>(u64::from(u32::MAX), 1);
    assert_refused::<Vec<u64>>(u64::from(u32::MAX) / 8, 1);
    // A length past `Vec`'s bound fails before anything is reserved.
    assert_refused::<Vec<u8>>(most, 1);
    assert_refused::<Vec<u64>>(most, 1);
    // A byte vector refuses a length past its bytes before it reserves them, however many bytes it holds: reserving
    // the bytes left first asks for 1500 bytes here.
    assert_refused::<Vec<u8>>(2048, 1500);
    assert_refused::<String>(2048, 1500);
}

// Versioned Wire struct tests — validates the full end-to-end workflow:
// 1. #[wire(tag = V.S, default = expr)] parsing
// 2. Per-version Serialize, Deserialize, and TYPE tree generation
// 3. VersionedWire trait implementation
// 4. Per-version global_id differences (distinct TYPE trees → distinct hashes)

mod versioned {
    use super::*;
    use crate::{VersionedWire, Wire};

    /// A versioned struct with two schema versions:
    /// - v1: only `mode` (tag 1.1)
    /// - v2: `mode` + `timeout` (tag 2.1, defaults to None)
    #[derive(Debug, Clone, PartialEq, etude_wire_derive::Wire)]
    #[wire(crate = crate)]
    struct ConsistencyLevel {
        #[wire(tag = 1.1)]
        mode: u32,

        #[wire(tag = 2.1, default = None)]
        timeout: Option<u64>,
    }

    #[test]
    fn versioned_wire_trait_metadata() {
        // VersionedWire exposes the correct version bounds
        assert_eq!(ConsistencyLevel::MIN_VERSION, 1);
        assert_eq!(ConsistencyLevel::MAX_VERSION, 2);
        assert_eq!(ConsistencyLevel::VERSION_TYPES.len(), 2);
    }

    #[test]
    fn versioned_type_trees_differ() {
        // Each version has a distinct TYPE tree
        let v1 = &ConsistencyLevel::VERSION_TYPES[0];
        let v2 = &ConsistencyLevel::VERSION_TYPES[1];

        // v1 has 1 child (mode: u32), v2 has 2 children (mode: u32, timeout: Option<u64>)
        assert_eq!(v1.children.len(), 1);
        assert_eq!(v2.children.len(), 2);

        // Wire::TYPE equals the latest version's TYPE tree
        assert_eq!(ConsistencyLevel::TYPE.children.len(), 2);
    }

    #[test]
    fn versioned_global_ids_differ() {
        // Distinct TYPE trees produce distinct global_id hashes
        let v1_id = ConsistencyLevel::VERSION_TYPES[0].global_id();
        let v2_id = ConsistencyLevel::VERSION_TYPES[1].global_id();

        assert_ne!(v1_id, v2_id, "v1 and v2 should have different global_ids");

        // The Wire::TYPE global_id matches the latest version
        assert_eq!(ConsistencyLevel::TYPE.global_id(), v2_id);
    }

    #[test]
    fn versioned_round_trip_latest() {
        // Standard Serialize/Deserialize uses the latest version (v2)
        let value = ConsistencyLevel {
            mode: 42,
            timeout: Some(1000),
        };
        let mut builder = Builder::default();
        value.clone().serialize(&mut builder);
        let mut bytes = builder.finish();
        let deserialized = ConsistencyLevel::deserialize(&mut bytes).unwrap();
        assert_eq!(value, deserialized);
    }

    #[test]
    fn versioned_v1_serialize_drops_v2_fields() {
        // Serializing at v1 drops the timeout field
        let value = ConsistencyLevel {
            mode: 42,
            timeout: Some(1000),
        };

        // Serialize at v1
        let mut builder_v1 = Builder::default();
        value.clone().__wire_serialize_v1(&mut builder_v1);
        let bytes_v1 = builder_v1.finish();

        // Serialize at v2
        let mut builder_v2 = Builder::default();
        value.__wire_serialize_v2(&mut builder_v2);
        let bytes_v2 = builder_v2.finish();

        // v1 should be shorter than v2 (timeout not serialized)
        assert!(bytes_v1.len() < bytes_v2.len());
    }

    #[test]
    fn versioned_v1_deserialize_fills_default() {
        // Serialize at v1 (only mode), deserialize at v1 → timeout gets default None
        let value = ConsistencyLevel {
            mode: 42,
            timeout: Some(999),
        };

        let mut builder = Builder::default();
        value.__wire_serialize_v1(&mut builder);
        let mut bytes = builder.finish();

        let deserialized = ConsistencyLevel::__wire_deserialize_v1(&mut bytes).unwrap();
        assert_eq!(deserialized.mode, 42);
        assert_eq!(deserialized.timeout, None); // default filled
    }

    #[test]
    fn versioned_cross_version_compat() {
        // Simulate a v2 node deserializing a message from a v1 node:
        // The v1 node serializes at v1, the v2 node deserializes at v1.
        let original = ConsistencyLevel {
            mode: 7,
            timeout: Some(500),
        };

        // v1 sender writes only `mode`
        let mut builder = Builder::default();
        original.__wire_serialize_v1(&mut builder);
        let mut bytes = builder.finish();

        // v2 receiver reads with v1 deserializer → fills timeout with None
        let received = ConsistencyLevel::__wire_deserialize_v1(&mut bytes).unwrap();
        assert_eq!(received.mode, 7);
        assert_eq!(received.timeout, None);
    }

    /// A struct with fields declared out of tag order — codegen sorts by tag.
    #[derive(Debug, Clone, PartialEq, etude_wire_derive::Wire)]
    #[wire(crate = crate)]
    struct OutOfOrder {
        // Declared: c (2.1), a (1.1), b (1.2)
        // Wire order: a (1.1), b (1.2), c (2.1)
        #[wire(tag = 2.1, default = false)]
        c: bool,

        #[wire(tag = 1.1)]
        a: u64,

        #[wire(tag = 1.2)]
        b: u32,
    }

    #[test]
    fn versioned_field_ordering() {
        // Serialize at v1: should write a then b (sorted by tag 1.1, 1.2)
        let value = OutOfOrder {
            c: true,
            a: 100,
            b: 200,
        };

        let mut builder_v1 = Builder::default();
        value.clone().__wire_serialize_v1(&mut builder_v1);
        let mut bytes_v1 = builder_v1.finish();

        // Deserialize at v1: should read a, b in tag order, c gets default
        let deserialized = OutOfOrder::__wire_deserialize_v1(&mut bytes_v1).unwrap();
        assert_eq!(deserialized.a, 100);
        assert_eq!(deserialized.b, 200);
        assert_eq!(deserialized.c, false); // default

        // Round-trip at v2 preserves all fields
        test_round_trip(value);
    }

    #[test]
    fn versioned_resolve_serializer_pattern() {
        // Demonstrates how resolve_serializer() dispatch works:
        // Given a negotiated global_id, select the correct (de)serializer.
        let v1_id = ConsistencyLevel::VERSION_TYPES[0].global_id();
        let v2_id = ConsistencyLevel::VERSION_TYPES[1].global_id();

        // Simulate resolve_serializer dispatch
        let negotiated_id = v1_id;
        let value = ConsistencyLevel {
            mode: 42,
            timeout: Some(1000),
        };

        let mut builder = Builder::default();
        match negotiated_id {
            id if id == v1_id => value.__wire_serialize_v1(&mut builder),
            id if id == v2_id => value.__wire_serialize_v2(&mut builder),
            _ => panic!("unknown version"),
        }
        let mut bytes = builder.finish();

        let deserialized = match negotiated_id {
            id if id == v1_id => ConsistencyLevel::__wire_deserialize_v1(&mut bytes).unwrap(),
            id if id == v2_id => ConsistencyLevel::__wire_deserialize_v2(&mut bytes).unwrap(),
            _ => panic!("unknown version"),
        };

        assert_eq!(deserialized.mode, 42);
        assert_eq!(deserialized.timeout, None); // v1 drops timeout
    }

    /// A struct with min_version = 2 — v1 serializers/deserializers are not generated.
    #[derive(Debug, Clone, PartialEq, etude_wire_derive::Wire)]
    #[wire(crate = crate, min_version = 2)]
    struct Pruned {
        #[wire(tag = 1.1)]
        key: u64,

        #[wire(tag = 2.1, default = None)]
        value: Option<u32>,

        #[wire(tag = 3.1, default = None)]
        extra: Option<bool>,
    }

    #[test]
    fn versioned_min_version_pruning() {
        // With min_version = 2, only v2 and v3 TYPE trees are generated
        assert_eq!(Pruned::MIN_VERSION, 2);
        assert_eq!(Pruned::MAX_VERSION, 3);
        assert_eq!(Pruned::VERSION_TYPES.len(), 2); // v2, v3

        // v2 TYPE has 2 children (key, value), v3 has 3 (key, value, extra)
        assert_eq!(Pruned::VERSION_TYPES[0].children.len(), 2);
        assert_eq!(Pruned::VERSION_TYPES[1].children.len(), 3);
    }

    /// Versioned struct with a skip-only field (no tag).
    ///
    /// Skip fields don't participate in versioning — they are not on the wire
    /// and always get Default::default() during deserialization. This test
    /// verifies correct round-trip behavior.
    ///
    /// Previously, `tag + skip` on the same field silently produced broken
    /// code (serializer dropped the field, deserializer tried to read it).
    /// That combination is now rejected at compile time. Skip-only fields
    /// (without a tag) are correctly handled.
    #[derive(Debug, Clone, PartialEq, etude_wire_derive::Wire)]
    #[wire(crate = crate)]
    struct VersionedWithSkip {
        #[wire(tag = 1.1)]
        a: u64,

        #[wire(skip)]
        skipped: u32,

        #[wire(tag = 1.2)]
        b: u64,
    }

    // ── Versioned enum tests ──────────────────────────────────────

    /// A versioned enum with two schema versions:
    /// - v1: Linearizable (tag 1.0), Eventual (tag 1.1)
    /// - v2: + BoundedStaleness(u64) (tag 2.0, fallback → Eventual)
    #[derive(Debug, Clone, PartialEq, etude_wire_derive::Wire)]
    #[wire(crate = crate)]
    enum ConsistencyMode {
        #[wire(tag = 1.0)]
        Linearizable,
        #[wire(tag = 1.1)]
        Eventual,
        #[wire(tag = 2.0, fallback = ConsistencyMode::Eventual)]
        BoundedStaleness(u64),
    }

    #[test]
    fn enum_versioned_wire_trait_metadata() {
        assert_eq!(ConsistencyMode::MIN_VERSION, 1);
        assert_eq!(ConsistencyMode::MAX_VERSION, 2);
        assert_eq!(ConsistencyMode::VERSION_TYPES.len(), 2);
    }

    #[test]
    fn enum_versioned_type_trees_differ() {
        let v1 = &ConsistencyMode::VERSION_TYPES[0];
        let v2 = &ConsistencyMode::VERSION_TYPES[1];

        // v1 has 2 variants, v2 has 3 variants
        assert_eq!(v1.children.len(), 2);
        assert_eq!(v2.children.len(), 3);

        // Wire::TYPE equals the latest version's TYPE tree
        assert_eq!(ConsistencyMode::TYPE.children.len(), 3);
    }

    #[test]
    fn enum_versioned_global_ids_differ() {
        let v1_id = ConsistencyMode::VERSION_TYPES[0].global_id();
        let v2_id = ConsistencyMode::VERSION_TYPES[1].global_id();

        assert_ne!(v1_id, v2_id, "v1 and v2 should have different global_ids");
        assert_eq!(ConsistencyMode::TYPE.global_id(), v2_id);
    }

    #[test]
    fn enum_versioned_round_trip_latest() {
        // All variants round-trip correctly at latest version
        test_round_trip(ConsistencyMode::Linearizable);
        test_round_trip(ConsistencyMode::Eventual);
        test_round_trip(ConsistencyMode::BoundedStaleness(1000));
    }

    #[test]
    fn enum_versioned_v1_serialize_drops_v2_variant() {
        // BoundedStaleness is converted to Eventual when serializing at v1
        let value = ConsistencyMode::BoundedStaleness(1000);

        let mut builder = Builder::default();
        value.__wire_serialize_v1(&mut builder);
        let mut bytes = builder.finish();

        let deserialized = ConsistencyMode::__wire_deserialize_v1(&mut bytes).unwrap();
        assert_eq!(deserialized, ConsistencyMode::Eventual);
    }

    #[test]
    fn enum_versioned_v1_round_trip_existing_variants() {
        // v1 variants round-trip correctly at v1
        let value = ConsistencyMode::Linearizable;
        let mut builder = Builder::default();
        value.clone().__wire_serialize_v1(&mut builder);
        let mut bytes = builder.finish();
        let deserialized = ConsistencyMode::__wire_deserialize_v1(&mut bytes).unwrap();
        assert_eq!(deserialized, ConsistencyMode::Linearizable);

        let value = ConsistencyMode::Eventual;
        let mut builder = Builder::default();
        value.clone().__wire_serialize_v1(&mut builder);
        let mut bytes = builder.finish();
        let deserialized = ConsistencyMode::__wire_deserialize_v1(&mut bytes).unwrap();
        assert_eq!(deserialized, ConsistencyMode::Eventual);
    }

    #[test]
    fn enum_versioned_cross_version_compat() {
        // v1 sender serializes Linearizable, v2 receiver deserializes with v1 deserializer
        let mut builder = Builder::default();
        ConsistencyMode::Linearizable.__wire_serialize_v1(&mut builder);
        let mut bytes = builder.finish();

        // v1 deserializer should work fine
        let received = ConsistencyMode::__wire_deserialize_v1(&mut bytes).unwrap();
        assert_eq!(received, ConsistencyMode::Linearizable);
    }

    // ── Versioned struct with skip ────────────────────────────────

    // ── serialize_at / deserialize_at tests ───────────────────────

    #[test]
    fn versioned_deserialize_at_v1() {
        let value = ConsistencyLevel {
            mode: 42,
            timeout: Some(999),
        };
        let mut builder = Builder::default();
        value.__wire_serialize_v1(&mut builder);
        let mut bytes = builder.finish();

        let result =
            <ConsistencyLevel as crate::Deserialize>::deserialize_at(&mut bytes, 0).unwrap();
        assert_eq!(result.mode, 42);
        assert_eq!(result.timeout, None);
    }

    #[test]
    fn versioned_deserialize_at_v2() {
        let value = ConsistencyLevel {
            mode: 42,
            timeout: Some(1000),
        };
        let mut builder = Builder::default();
        value.__wire_serialize_v2(&mut builder);
        let mut bytes = builder.finish();

        let result =
            <ConsistencyLevel as crate::Deserialize>::deserialize_at(&mut bytes, 1).unwrap();
        assert_eq!(result.mode, 42);
        assert_eq!(result.timeout, Some(1000));
    }

    #[test]
    fn versioned_serialize_at_v1_drops_fields() {
        let value = ConsistencyLevel {
            mode: 42,
            timeout: Some(999),
        };
        let mut builder = Builder::default();
        <ConsistencyLevel as crate::Serialize>::serialize_at(value, &mut builder, 0);
        let mut bytes = builder.finish();

        let result =
            <ConsistencyLevel as crate::Deserialize>::deserialize_at(&mut bytes, 0).unwrap();
        assert_eq!(result.mode, 42);
        assert_eq!(result.timeout, None);
    }

    #[test]
    fn versioned_serialize_at_v2_preserves_all() {
        let value = ConsistencyLevel {
            mode: 42,
            timeout: Some(1000),
        };
        let mut builder = Builder::default();
        <ConsistencyLevel as crate::Serialize>::serialize_at(value.clone(), &mut builder, 1);
        let mut bytes = builder.finish();

        let result =
            <ConsistencyLevel as crate::Deserialize>::deserialize_at(&mut bytes, 1).unwrap();
        assert_eq!(result, value);
    }

    #[test]
    fn non_versioned_deserialize_at_ignores_index() {
        let mut builder = Builder::default();
        42u32.serialize(&mut builder);
        let bytes = builder.finish();

        let mut b1 = bytes.clone();
        let mut b2 = bytes.clone();
        let r1 = u32::deserialize(&mut b1).unwrap();
        let r2 = <u32 as crate::Deserialize>::deserialize_at(&mut b2, 999).unwrap();
        assert_eq!(r1, r2);
    }

    #[test]
    fn non_versioned_serialize_at_ignores_index() {
        let mut b1 = Builder::default();
        let mut b2 = Builder::default();
        42u32.serialize(&mut b1);
        <u32 as crate::Serialize>::serialize_at(42u32, &mut b2, 999);
        assert_eq!(b1.finish(), b2.finish());
    }

    #[test]
    fn option_propagates_version_index_deserialize() {
        // Serialize Some(CL) at v1 using serialize_at
        let value = Some(ConsistencyLevel {
            mode: 7,
            timeout: Some(500),
        });
        let mut builder = Builder::default();
        <Option<ConsistencyLevel> as crate::Serialize>::serialize_at(value, &mut builder, 0);
        let mut bytes = builder.finish();

        let result =
            <Option<ConsistencyLevel> as crate::Deserialize>::deserialize_at(&mut bytes, 0)
                .unwrap();
        assert_eq!(
            result,
            Some(ConsistencyLevel {
                mode: 7,
                timeout: None
            })
        );
    }

    #[test]
    fn enum_deserialize_at_v1_uses_fallback() {
        let value = ConsistencyMode::BoundedStaleness(1000);
        let mut builder = Builder::default();
        <ConsistencyMode as crate::Serialize>::serialize_at(value, &mut builder, 0);
        let mut bytes = builder.finish();

        let result =
            <ConsistencyMode as crate::Deserialize>::deserialize_at(&mut bytes, 0).unwrap();
        assert_eq!(result, ConsistencyMode::Eventual);
    }

    #[test]
    fn enum_deserialize_at_v2_preserves_variant() {
        let value = ConsistencyMode::BoundedStaleness(1000);
        let mut builder = Builder::default();
        <ConsistencyMode as crate::Serialize>::serialize_at(value.clone(), &mut builder, 1);
        let mut bytes = builder.finish();

        let result =
            <ConsistencyMode as crate::Deserialize>::deserialize_at(&mut bytes, 1).unwrap();
        assert_eq!(result, value);
    }

    // ── VERSION_TYPES tests ──────────────────────────────────────

    #[test]
    fn version_types_default_single_entry_for_primitives() {
        assert_eq!(<u32 as Wire>::VERSION_TYPES.len(), 1);
        assert_eq!(
            <u32 as Wire>::VERSION_TYPES[0].global_id(),
            <u32 as Wire>::TYPE.global_id()
        );
        assert_eq!(<String as Wire>::VERSION_TYPES.len(), 1);
        assert_eq!(<Option<u32> as Wire>::VERSION_TYPES.len(), 1);
        assert_eq!(<Vec<u8> as Wire>::VERSION_TYPES.len(), 1);
    }

    // ── 3-level nested versioned type tests ──────────────────────

    /// Level 1 (leaf): Mode with 2 versions
    #[derive(Debug, Clone, PartialEq, etude_wire_derive::Wire)]
    #[wire(crate = crate)]
    struct Mode {
        #[wire(tag = 1.1)]
        basic: bool,
        #[wire(tag = 2.1, default = false)]
        strict: bool,
    }

    /// Level 2: NestedCL contains versioned Mode
    #[derive(Debug, Clone, PartialEq, etude_wire_derive::Wire)]
    #[wire(crate = crate)]
    struct NestedCL {
        #[wire(tag = 1.1)]
        mode: Mode,
        #[wire(tag = 2.1, default = None)]
        timeout: Option<u64>,
    }

    /// Level 3: NestedReadRequest contains versioned NestedCL
    #[derive(Debug, Clone, PartialEq, etude_wire_derive::Wire)]
    #[wire(crate = crate)]
    struct NestedReadRequest {
        #[wire(tag = 1.1)]
        key: u64,
        #[wire(tag = 2.1, default = None)]
        consistency: Option<NestedCL>,
    }

    #[test]
    fn nested_3_level_version_types_count() {
        assert_eq!(Mode::VERSION_TYPES.len(), 2);
        assert_eq!(NestedCL::VERSION_TYPES.len(), 2);
        assert_eq!(NestedReadRequest::VERSION_TYPES.len(), 2);
    }

    #[test]
    fn nested_3_level_global_ids_differ() {
        let mode_v1 = Mode::VERSION_TYPES[0].global_id();
        let mode_v2 = Mode::VERSION_TYPES[1].global_id();
        assert_ne!(mode_v1, mode_v2);

        let cl_v1 = NestedCL::VERSION_TYPES[0].global_id();
        let cl_v2 = NestedCL::VERSION_TYPES[1].global_id();
        assert_ne!(cl_v1, cl_v2);

        let rr_v1 = NestedReadRequest::VERSION_TYPES[0].global_id();
        let rr_v2 = NestedReadRequest::VERSION_TYPES[1].global_id();
        assert_ne!(rr_v1, rr_v2);
    }

    #[test]
    fn nested_3_level_round_trip_latest() {
        let value = NestedReadRequest {
            key: 42,
            consistency: Some(NestedCL {
                mode: Mode {
                    basic: true,
                    strict: true,
                },
                timeout: Some(100),
            }),
        };
        test_round_trip(value);
    }

    #[test]
    fn nested_3_level_serialize_at_v1_drops_inner() {
        let value = NestedReadRequest {
            key: 42,
            consistency: Some(NestedCL {
                mode: Mode {
                    basic: true,
                    strict: true,
                },
                timeout: Some(100),
            }),
        };

        // serialize_at(0) = v1 → only key, drops consistency
        let mut builder = Builder::default();
        <NestedReadRequest as crate::Serialize>::serialize_at(value.clone(), &mut builder, 0);
        let mut bytes = builder.finish();

        let result =
            <NestedReadRequest as crate::Deserialize>::deserialize_at(&mut bytes, 0).unwrap();
        assert_eq!(result.key, 42);
        assert_eq!(result.consistency, None);
    }

    #[test]
    fn nested_3_level_xproduct_types_count() {
        // The cross-product includes ALL combinations of inner field versions, not
        // just chronologically-possible ones. This is intentional: during rolling
        // deployments, inner types can be upgraded independently of outer types.
        //
        // Example: Node A runs Mode@v1 + NestedCL@v1, Node B has upgraded Mode to
        // v2 but still uses NestedCL@v1. Node B serializes NestedCL with Mode@v2
        // layout (basic+strict) but no timeout field. This is NestedCL@v1 × Mode@v2
        // — a combination that must be recognized by all peers.

        // Mode: 2 outer versions, no inner versioned fields → 2 xproduct types
        assert_eq!(Mode::VERSION_XPRODUCT_COUNT, 2);

        // NestedCL: outer_v1 has 1 field (mode: Mode[2 xp]) → segment=2
        //           outer_v2 has 2 fields (mode: Mode[2 xp], timeout: Option<u64>[1 xp]) → segment=2*1=2
        //           total = 2 + 2 = 4
        //
        // All 4 represent physically possible wire layouts:
        //   idx 0: v1 + Mode@v1 (basic only, no timeout)
        //   idx 1: v1 + Mode@v2 (basic+strict, no timeout) ← rolling upgrade case
        //   idx 2: v2 + Mode@v1 (basic only, with timeout)
        //   idx 3: v2 + Mode@v2 (basic+strict, with timeout)
        assert_eq!(NestedCL::VERSION_XPRODUCT_COUNT, 4);

        // NestedReadRequest: outer_v1 has 1 field (key: u64[1 xp]) → segment=1
        //                    outer_v2 has 2 fields (key: u64[1 xp], consistency: Option<NestedCL>[4 xp]) → segment=1*4=4
        //                    total = 1 + 4 = 5
        assert_eq!(NestedReadRequest::VERSION_XPRODUCT_COUNT, 5);
    }

    #[test]
    fn nested_3_level_xproduct_count_correct() {
        // Verify the count is correct and all indices round-trip
        let count = NestedReadRequest::VERSION_XPRODUCT_COUNT;
        assert_eq!(count, 5);
        // All indices should produce valid serialize/deserialize round-trips
        let value = NestedReadRequest {
            key: 42,
            consistency: Some(NestedCL {
                mode: Mode {
                    basic: true,
                    strict: true,
                },
                timeout: Some(100),
            }),
        };
        for idx in 0..count as u32 {
            let mut builder = Builder::default();
            <NestedReadRequest as crate::Serialize>::serialize_at(value.clone(), &mut builder, idx);
            let mut bytes = builder.finish();
            let result = <NestedReadRequest as crate::Deserialize>::deserialize_at(&mut bytes, idx);
            assert!(
                result.is_ok(),
                "serialize_at/deserialize_at failed at idx {idx}"
            );
        }
    }

    #[test]
    fn nested_3_level_serialize_at_v2_preserves_all() {
        let value = NestedReadRequest {
            key: 42,
            consistency: Some(NestedCL {
                mode: Mode {
                    basic: true,
                    strict: true,
                },
                timeout: Some(100),
            }),
        };

        // The last xproduct index = outer_v2 + all inner latest
        let last_idx = (NestedReadRequest::VERSION_XPRODUCT_COUNT - 1) as u32;
        let mut builder = Builder::default();
        <NestedReadRequest as crate::Serialize>::serialize_at(
            value.clone(),
            &mut builder,
            last_idx,
        );
        let mut bytes = builder.finish();

        let result =
            <NestedReadRequest as crate::Deserialize>::deserialize_at(&mut bytes, last_idx)
                .unwrap();
        assert_eq!(result, value);
    }

    /// Load-bearing invariant: `serialize_at(NEWEST_IDX)` must produce
    /// byte-for-byte the same output as the plain `serialize()`, and likewise
    /// `deserialize_at(NEWEST_IDX)` must match `deserialize()`.
    ///
    /// The negotiation layer relies on this: a peer negotiated to the *current*
    /// version carries a real `MethodVersionEntry` whose indices are the newest
    /// cross-product `(IN-1, OUT-1)`. `MethodHandle`/`ResponseContext` then run
    /// the `_at` path unconditionally. If `serialize_at(newest) != serialize()`,
    /// turning on negotiation would silently change the wire format of every
    /// current-version RPC. This test pins that they are identical across all
    /// three nesting levels (leaf `Mode`, mid `NestedCL`, top `NestedReadRequest`).
    #[test]
    fn serialize_at_newest_index_is_byte_identical_to_plain_serialize() {
        fn check_struct<T>(value: T)
        where
            T: Clone + Debug + PartialEq + Serialize + Deserialize + crate::Wire,
        {
            let newest = (T::VERSION_XPRODUCT_COUNT - 1) as u32;

            let mut plain = Builder::default();
            Serialize::serialize(value.clone(), &mut plain);
            let plain = plain.finish();

            let mut at = Builder::default();
            Serialize::serialize_at(value.clone(), &mut at, newest);
            let at = at.finish();

            assert!(
                plain == at,
                "{}: serialize_at({newest}) must equal serialize() — plain {} bytes, at {} bytes",
                core::any::type_name::<T>(),
                plain.len(),
                at.len(),
            );

            // Symmetric on the read side: deserialize_at(newest) == deserialize().
            let mut b1 = plain.clone();
            let mut b2 = plain.clone();
            let via_plain = <T as Deserialize>::deserialize(&mut b1).unwrap();
            let via_at = <T as Deserialize>::deserialize_at(&mut b2, newest).unwrap();
            assert_eq!(
                via_plain,
                via_at,
                "{}: deserialize_at({newest}) must equal deserialize()",
                core::any::type_name::<T>(),
            );
            assert_eq!(
                via_plain,
                value,
                "{}: round-trips",
                core::any::type_name::<T>()
            );
        }

        check_struct(Mode {
            basic: true,
            strict: true,
        });
        check_struct(NestedCL {
            mode: Mode {
                basic: true,
                strict: false,
            },
            timeout: Some(100),
        });
        check_struct(NestedReadRequest {
            key: 42,
            consistency: Some(NestedCL {
                mode: Mode {
                    basic: true,
                    strict: true,
                },
                timeout: Some(100),
            }),
        });
        // Also the field-absent shape (consistency = None) — exercises the
        // newest outer version with an absent Option, the common real case.
        check_struct(NestedReadRequest {
            key: 7,
            consistency: None,
        });
    }

    #[test]
    fn nested_mode_serialize_at_cross_version() {
        // Mode at xproduct idx 0 = outer_v1 → only basic, strict gets default
        let mode = Mode {
            basic: true,
            strict: true,
        };
        let mut builder = Builder::default();
        <Mode as crate::Serialize>::serialize_at(mode, &mut builder, 0);
        let mut bytes = builder.finish();

        let result = <Mode as crate::Deserialize>::deserialize_at(&mut bytes, 0).unwrap();
        assert_eq!(result.basic, true);
        assert_eq!(result.strict, false); // v1 default
    }

    #[test]
    fn nested_cl_serialize_at_cross_version() {
        // NestedCL xproduct layout:
        //   idx 0: outer_v1, mode@0 (v1) → mode basic only, no timeout
        //   idx 1: outer_v1, mode@1 (v2) → mode full, no timeout
        //   idx 2: outer_v2, mode@0, timeout@0 → mode basic only + timeout
        //   idx 3: outer_v2, mode@1, timeout@0 → mode full + timeout
        let cl = NestedCL {
            mode: Mode {
                basic: true,
                strict: true,
            },
            timeout: Some(500),
        };

        // idx 0: outer_v1, Mode@v1 → drops strict, drops timeout
        let mut builder = Builder::default();
        <NestedCL as crate::Serialize>::serialize_at(cl.clone(), &mut builder, 0);
        let mut bytes = builder.finish();
        let result = <NestedCL as crate::Deserialize>::deserialize_at(&mut bytes, 0).unwrap();
        assert_eq!(
            result.mode,
            Mode {
                basic: true,
                strict: false
            }
        );
        assert_eq!(result.timeout, None);

        // idx 1: outer_v1, Mode@v2 → preserves strict, drops timeout
        let mut builder = Builder::default();
        <NestedCL as crate::Serialize>::serialize_at(cl.clone(), &mut builder, 1);
        let mut bytes = builder.finish();
        let result = <NestedCL as crate::Deserialize>::deserialize_at(&mut bytes, 1).unwrap();
        assert_eq!(
            result.mode,
            Mode {
                basic: true,
                strict: true
            }
        );
        assert_eq!(result.timeout, None);

        // idx 3: outer_v2, Mode@v2, timeout → preserves everything
        let mut builder = Builder::default();
        <NestedCL as crate::Serialize>::serialize_at(cl.clone(), &mut builder, 3);
        let mut bytes = builder.finish();
        let result = <NestedCL as crate::Deserialize>::deserialize_at(&mut bytes, 3).unwrap();
        assert_eq!(
            result.mode,
            Mode {
                basic: true,
                strict: true
            }
        );
        assert_eq!(result.timeout, Some(500));

        // idx 2: outer_v2, Mode@v1, timeout → drops strict, preserves timeout
        let mut builder = Builder::default();
        <NestedCL as crate::Serialize>::serialize_at(cl.clone(), &mut builder, 2);
        let mut bytes = builder.finish();
        let result = <NestedCL as crate::Deserialize>::deserialize_at(&mut bytes, 2).unwrap();
        assert_eq!(
            result.mode,
            Mode {
                basic: true,
                strict: false
            }
        );
        assert_eq!(result.timeout, Some(500));
    }

    // ── Exhaustive cross-product round-trip tests ─────────────────

    #[test]
    fn nested_mode_xproduct_count() {
        assert_eq!(Mode::VERSION_XPRODUCT_COUNT, 2);
    }

    #[test]
    fn nested_cl_xproduct_count() {
        assert_eq!(NestedCL::VERSION_XPRODUCT_COUNT, 4);
    }

    #[test]
    fn xproduct_count_matches_version_types_for_simple_struct() {
        // ConsistencyLevel has no nested versioned fields (mode: u32, timeout: Option<u64>).
        // Its xproduct count should equal VERSION_TYPES count (both = 2).
        assert_eq!(
            ConsistencyLevel::VERSION_XPRODUCT_COUNT,
            ConsistencyLevel::VERSION_TYPES.len()
        );
        assert_eq!(ConsistencyLevel::VERSION_XPRODUCT_COUNT, 2);

        // All VERSION_TYPES entries should have distinct global_ids
        assert_ne!(
            ConsistencyLevel::VERSION_TYPES[0].global_id(),
            ConsistencyLevel::VERSION_TYPES[1].global_id(),
            "simple struct version types should have distinct global_ids"
        );
    }

    #[test]
    fn xproduct_types_count_equals_one_for_primitives() {
        // Non-versioned types have exactly 1 xproduct type
        assert_eq!(<u32 as Wire>::VERSION_XPRODUCT_COUNT, 1);
        assert_eq!(<String as Wire>::VERSION_XPRODUCT_COUNT, 1);
        assert_eq!(<bool as Wire>::VERSION_XPRODUCT_COUNT, 1);
        assert_eq!(<Option<u32> as Wire>::VERSION_XPRODUCT_COUNT, 1);
    }

    /// Helper: serialize_at(idx) then deserialize_at(idx) for round-trip.
    fn xproduct_round_trip_at<
        T: crate::Serialize + crate::Deserialize + Clone + PartialEq + Debug,
    >(
        value: &T,
        idx: u32,
    ) -> T {
        let mut builder = Builder::default();
        <T as crate::Serialize>::serialize_at(value.clone(), &mut builder, idx);
        let mut bytes = builder.finish();
        <T as crate::Deserialize>::deserialize_at(&mut bytes, idx).unwrap()
    }

    #[test]
    fn nested_mode_exhaustive_xproduct_round_trip() {
        let mode = Mode {
            basic: true,
            strict: true,
        };
        // idx 0: v1 → basic only
        let r0 = xproduct_round_trip_at(&mode, 0);
        assert_eq!(
            r0,
            Mode {
                basic: true,
                strict: false
            }
        );

        // idx 1: v2 → both fields
        let r1 = xproduct_round_trip_at(&mode, 1);
        assert_eq!(
            r1,
            Mode {
                basic: true,
                strict: true
            }
        );
    }

    #[test]
    fn nested_cl_exhaustive_xproduct_round_trip() {
        // NestedCL layout:
        //   idx 0: outer_v1, Mode@v1 → mode.basic only, no timeout
        //   idx 1: outer_v1, Mode@v2 → mode full, no timeout
        //   idx 2: outer_v2, Mode@v1, timeout@v1 → mode.basic only + timeout
        //   idx 3: outer_v2, Mode@v2, timeout@v1 → mode full + timeout
        let cl = NestedCL {
            mode: Mode {
                basic: true,
                strict: true,
            },
            timeout: Some(500),
        };

        let r0 = xproduct_round_trip_at(&cl, 0);
        assert_eq!(
            r0.mode,
            Mode {
                basic: true,
                strict: false
            }
        );
        assert_eq!(r0.timeout, None);

        let r1 = xproduct_round_trip_at(&cl, 1);
        assert_eq!(
            r1.mode,
            Mode {
                basic: true,
                strict: true
            }
        );
        assert_eq!(r1.timeout, None);

        let r2 = xproduct_round_trip_at(&cl, 2);
        assert_eq!(
            r2.mode,
            Mode {
                basic: true,
                strict: false
            }
        );
        assert_eq!(r2.timeout, Some(500));

        let r3 = xproduct_round_trip_at(&cl, 3);
        assert_eq!(
            r3.mode,
            Mode {
                basic: true,
                strict: true
            }
        );
        assert_eq!(r3.timeout, Some(500));
    }

    #[test]
    fn nested_read_request_all_5_xproduct_indices() {
        // NestedReadRequest xproduct layout (5 entries):
        //   idx 0: outer_v1, key@0 → key=42, consistency=None
        //   idx 1: outer_v2, key@0, consistency@0 → key=42, consistency=Some(CL@idx0)
        //   idx 2: outer_v2, key@0, consistency@1 → key=42, consistency=Some(CL@idx1)
        //   idx 3: outer_v2, key@0, consistency@2 → key=42, consistency=Some(CL@idx2)
        //   idx 4: outer_v2, key@0, consistency@3 → key=42, consistency=Some(CL@idx3)
        let value = NestedReadRequest {
            key: 42,
            consistency: Some(NestedCL {
                mode: Mode {
                    basic: true,
                    strict: true,
                },
                timeout: Some(100),
            }),
        };

        // idx 0: v1 → only key
        let r0 = xproduct_round_trip_at(&value, 0);
        assert_eq!(r0.key, 42);
        assert_eq!(r0.consistency, None);

        // idx 1: v2 + CL@0 (outer_v1, Mode@v1) → key + consistency with mode.basic only, no timeout
        let r1 = xproduct_round_trip_at(&value, 1);
        assert_eq!(r1.key, 42);
        let c1 = r1.consistency.unwrap();
        assert_eq!(
            c1.mode,
            Mode {
                basic: true,
                strict: false
            }
        );
        assert_eq!(c1.timeout, None);

        // idx 2: v2 + CL@1 (outer_v1, Mode@v2) → key + consistency with mode full, no timeout
        let r2 = xproduct_round_trip_at(&value, 2);
        assert_eq!(r2.key, 42);
        let c2 = r2.consistency.unwrap();
        assert_eq!(
            c2.mode,
            Mode {
                basic: true,
                strict: true
            }
        );
        assert_eq!(c2.timeout, None);

        // idx 3: v2 + CL@2 (outer_v2, Mode@v1, timeout) → key + consistency with mode.basic + timeout
        let r3 = xproduct_round_trip_at(&value, 3);
        assert_eq!(r3.key, 42);
        let c3 = r3.consistency.unwrap();
        assert_eq!(
            c3.mode,
            Mode {
                basic: true,
                strict: false
            }
        );
        assert_eq!(c3.timeout, Some(100));

        // idx 4: v2 + CL@3 (outer_v2, Mode@v2, timeout) → ALL LATEST
        let r4 = xproduct_round_trip_at(&value, 4);
        assert_eq!(r4, value);
    }

    #[test]
    fn nested_3_level_none_consistency_round_trips_all_v2_indices() {
        // When consistency is None, all v2 indices should work and produce None
        let value = NestedReadRequest {
            key: 99,
            consistency: None,
        };
        for idx in 1..5u32 {
            let result = xproduct_round_trip_at(&value, idx);
            assert_eq!(result.key, 99);
            assert_eq!(
                result.consistency, None,
                "idx={idx}: None should round-trip as None"
            );
        }
    }

    // ── Versioned struct with skip ────────────────────────────────

    #[test]
    fn versioned_skip_field_round_trip() {
        let value = VersionedWithSkip {
            a: 100,
            skipped: 999, // will be lost — skip fields are not serialized
            b: 200,
        };

        // Serialize: writes a (u64) then b (u64) — skips `skipped`
        let mut builder = Builder::default();
        value.serialize(&mut builder);
        let serialized = builder.finish();

        // Only wire fields are written: 8 bytes (a) + 8 bytes (b) = 16 bytes
        assert_eq!(
            serialized.len(),
            16,
            "serializer should write 16 bytes (a + b, skip omitted)"
        );

        // Deserialize: reads a and b from wire, skipped gets Default::default()
        let mut bytes = serialized;
        let deserialized = VersionedWithSkip::deserialize(&mut bytes).unwrap();

        assert_eq!(deserialized.a, 100);
        assert_eq!(deserialized.skipped, 0); // Default::default() for u32
        assert_eq!(deserialized.b, 200);
    }

    // ── Per-index value assertions for mixed-radix decomposition ──

    #[test]
    fn nested_cl_all_indices_field_values() {
        // NestedCL xproduct layout (4 entries):
        //   idx 0: outer_v1, Mode@v1 → mode.basic=true, mode.strict=false(default), timeout=None
        //   idx 1: outer_v1, Mode@v2 → mode.basic=true, mode.strict=true, timeout=None
        //   idx 2: outer_v2, Mode@v1 → mode.basic=true, mode.strict=false(default), timeout=Some(100)
        //   idx 3: outer_v2, Mode@v2 → mode.basic=true, mode.strict=true, timeout=Some(100)
        let value = NestedCL {
            mode: Mode {
                basic: true,
                strict: true,
            },
            timeout: Some(100),
        };

        // idx 0: v1 + Mode@v1 → strict dropped (default=false), no timeout
        let r0 = ser_deser_at::<NestedCL>(value.clone(), 0);
        assert_eq!(r0.mode.basic, true);
        assert_eq!(r0.mode.strict, false); // dropped by Mode@v1
        assert_eq!(r0.timeout, None); // dropped by outer v1

        // idx 1: v1 + Mode@v2 → strict preserved, no timeout
        let r1 = ser_deser_at::<NestedCL>(value.clone(), 1);
        assert_eq!(r1.mode.basic, true);
        assert_eq!(r1.mode.strict, true);
        assert_eq!(r1.timeout, None);

        // idx 2: v2 + Mode@v1 → strict dropped, timeout preserved
        let r2 = ser_deser_at::<NestedCL>(value.clone(), 2);
        assert_eq!(r2.mode.basic, true);
        assert_eq!(r2.mode.strict, false);
        assert_eq!(r2.timeout, Some(100));

        // idx 3: v2 + Mode@v2 → all preserved (latest)
        let r3 = ser_deser_at::<NestedCL>(value.clone(), 3);
        assert_eq!(r3, value);
    }

    #[test]
    fn nested_read_request_all_indices_field_values() {
        // NestedReadRequest xproduct layout (5 entries):
        //   idx 0: outer_v1 → key only, consistency=None
        //   idx 1: outer_v2, NestedCL@0 → key + consistency(mode@v1, no timeout)
        //   idx 2: outer_v2, NestedCL@1 → key + consistency(mode@v2, no timeout)
        //   idx 3: outer_v2, NestedCL@2 → key + consistency(mode@v1, with timeout)
        //   idx 4: outer_v2, NestedCL@3 → key + consistency(mode@v2, with timeout)
        let value = NestedReadRequest {
            key: 42,
            consistency: Some(NestedCL {
                mode: Mode {
                    basic: true,
                    strict: true,
                },
                timeout: Some(100),
            }),
        };

        // idx 0: v1 → only key
        let r0 = ser_deser_at::<NestedReadRequest>(value.clone(), 0);
        assert_eq!(r0.key, 42);
        assert_eq!(r0.consistency, None);

        // idx 1: v2 + CL@0 → key + consistency(mode@v1, no timeout)
        let r1 = ser_deser_at::<NestedReadRequest>(value.clone(), 1);
        assert_eq!(r1.key, 42);
        let c1 = r1.consistency.unwrap();
        assert_eq!(c1.mode.basic, true);
        assert_eq!(c1.mode.strict, false); // Mode@v1 drops strict
        assert_eq!(c1.timeout, None); // CL outer v1

        // idx 2: v2 + CL@1 → key + consistency(mode@v2, no timeout)
        let r2 = ser_deser_at::<NestedReadRequest>(value.clone(), 2);
        let c2 = r2.consistency.unwrap();
        assert_eq!(c2.mode.strict, true); // Mode@v2 preserves strict
        assert_eq!(c2.timeout, None);

        // idx 3: v2 + CL@2 → key + consistency(mode@v1, with timeout)
        let r3 = ser_deser_at::<NestedReadRequest>(value.clone(), 3);
        let c3 = r3.consistency.unwrap();
        assert_eq!(c3.mode.strict, false); // Mode@v1
        assert_eq!(c3.timeout, Some(100));

        // idx 4: v2 + CL@3 → all latest
        let r4 = ser_deser_at::<NestedReadRequest>(value.clone(), 4);
        assert_eq!(r4, value);
    }

    /// Helper: serialize_at then deserialize_at at the same index.
    fn ser_deser_at<T: crate::Serialize + crate::Deserialize + Clone>(value: T, idx: u32) -> T {
        let mut builder = Builder::default();
        <T as crate::Serialize>::serialize_at(value, &mut builder, idx);
        let mut bytes = builder.finish();
        <T as crate::Deserialize>::deserialize_at(&mut bytes, idx).unwrap()
    }

    // ── Asymmetric field counts (different radixes per level) ────

    /// Leaf: 3 versions (different from the 2-version Mode above)
    #[derive(Debug, Clone, PartialEq, etude_wire_derive::Wire)]
    #[wire(crate = crate)]
    struct Priority {
        #[wire(tag = 1.1)]
        level: u32,
        #[wire(tag = 2.1, default = false)]
        urgent: bool,
        #[wire(tag = 3.1, default = 0u64)]
        deadline: u64,
    }

    /// Mid-level: 2 versions, contains Priority (3 xp) and a u64 (1 xp)
    /// xproduct: v1 segment = Priority[3] = 3, v2 segment = Priority[3] × 1 = 3
    /// total = 6
    #[derive(Debug, Clone, PartialEq, etude_wire_derive::Wire)]
    #[wire(crate = crate)]
    struct Task {
        #[wire(tag = 1.1)]
        priority: Priority,
        #[wire(tag = 2.1, default = None)]
        assignee: Option<u64>,
    }

    /// Top-level: 2 versions, contains Task (6 xp) and a bool (1 xp)
    /// xproduct: v1 segment = Task[6] = 6, v2 segment = Task[6] × 1 = 6
    /// total = 12
    #[derive(Debug, Clone, PartialEq, etude_wire_derive::Wire)]
    #[wire(crate = crate)]
    struct Project {
        #[wire(tag = 1.1)]
        task: Task,
        #[wire(tag = 2.1, default = false)]
        approved: bool,
    }

    #[test]
    fn asymmetric_xproduct_counts() {
        // Priority: 3 outer versions, no inner versioned → 3
        assert_eq!(Priority::VERSION_XPRODUCT_COUNT, 3);
        // Task: v1(Priority[3])=3 + v2(Priority[3]×1)=3 → 6
        assert_eq!(Task::VERSION_XPRODUCT_COUNT, 6);
        // Project: v1(Task[6])=6 + v2(Task[6]×1)=6 → 12
        assert_eq!(Project::VERSION_XPRODUCT_COUNT, 12);
    }

    #[test]
    fn asymmetric_all_indices_round_trip() {
        let value = Project {
            task: Task {
                priority: Priority {
                    level: 5,
                    urgent: true,
                    deadline: 1000,
                },
                assignee: Some(99),
            },
            approved: true,
        };

        for idx in 0..Project::VERSION_XPRODUCT_COUNT as u32 {
            let result = ser_deser_at(value.clone(), idx);
            // Latest index should preserve everything
            if idx == Project::VERSION_XPRODUCT_COUNT as u32 - 1 {
                assert_eq!(result, value, "latest index round-trip mismatch");
            }
        }
    }

    #[test]
    fn asymmetric_per_index_field_values() {
        let value = Project {
            task: Task {
                priority: Priority {
                    level: 5,
                    urgent: true,
                    deadline: 1000,
                },
                assignee: Some(99),
            },
            approved: true,
        };

        // Project xproduct (12 entries):
        //   Outer v1: task only (no approved). Task indices 0..5.
        //     idx 0: Task@0 = Task.v1 + Priority@v1 → level=5, urgent=false(def), deadline=0(def), assignee=None
        //     idx 1: Task@1 = Task.v1 + Priority@v2 → level=5, urgent=true, deadline=0(def), assignee=None
        //     idx 2: Task@2 = Task.v1 + Priority@v3 → level=5, urgent=true, deadline=1000, assignee=None
        //     idx 3: Task@3 = Task.v2 + Priority@v1 → level=5, urgent=false(def), deadline=0(def), assignee=Some(99)
        //     idx 4: Task@4 = Task.v2 + Priority@v2 → level=5, urgent=true, deadline=0(def), assignee=Some(99)
        //     idx 5: Task@5 = Task.v2 + Priority@v3 → level=5, urgent=true, deadline=1000, assignee=Some(99)
        //   Outer v2: task + approved. Task indices 0..5.
        //     idx 6..11: same as 0..5 but with approved=true

        // idx 0: outer_v1, Task@0 = Task.v1 + Priority@v1
        let r0 = ser_deser_at(value.clone(), 0);
        assert_eq!(r0.task.priority.level, 5);
        assert_eq!(r0.task.priority.urgent, false); // Priority@v1 drops urgent
        assert_eq!(r0.task.priority.deadline, 0); // Priority@v1 drops deadline
        assert_eq!(r0.task.assignee, None); // Task.v1 drops assignee
        assert_eq!(r0.approved, false); // outer v1 drops approved

        // idx 2: outer_v1, Task@2 = Task.v1 + Priority@v3 (latest Priority)
        let r2 = ser_deser_at(value.clone(), 2);
        assert_eq!(r2.task.priority.level, 5);
        assert_eq!(r2.task.priority.urgent, true);
        assert_eq!(r2.task.priority.deadline, 1000);
        assert_eq!(r2.task.assignee, None); // Task.v1
        assert_eq!(r2.approved, false); // outer v1

        // idx 5: outer_v1, Task@5 = Task.v2 + Priority@v3 (latest Task inner)
        let r5 = ser_deser_at(value.clone(), 5);
        assert_eq!(r5.task.priority.deadline, 1000);
        assert_eq!(r5.task.assignee, Some(99));
        assert_eq!(r5.approved, false); // still outer v1

        // idx 6: outer_v2, Task@0 = Task.v1 + Priority@v1
        let r6 = ser_deser_at(value.clone(), 6);
        assert_eq!(r6.task.priority.urgent, false);
        assert_eq!(r6.task.assignee, None);
        assert_eq!(r6.approved, true); // outer v2 preserves approved

        // idx 11: outer_v2, Task@5 = all latest
        let r11 = ser_deser_at(value.clone(), 11);
        assert_eq!(r11, value);
    }

    // ── __XPRODUCT_HASH_ENTRIES tests ─────────────────────────────

    #[test]
    fn xproduct_hash_entries_count_matches_xproduct_count() {
        assert_eq!(<u32 as Wire>::__XPRODUCT_HASH_ENTRIES.len(), 1);
        assert_eq!(<Mode as Wire>::__XPRODUCT_HASH_ENTRIES.len(), 2);
        assert_eq!(<NestedCL as Wire>::__XPRODUCT_HASH_ENTRIES.len(), 4);
        assert_eq!(
            <NestedReadRequest as Wire>::__XPRODUCT_HASH_ENTRIES.len(),
            5
        );
        assert_eq!(<ConsistencyMode as Wire>::__XPRODUCT_HASH_ENTRIES.len(), 2);
        assert_eq!(<Priority as Wire>::__XPRODUCT_HASH_ENTRIES.len(), 3);
        assert_eq!(<Task as Wire>::__XPRODUCT_HASH_ENTRIES.len(), 6);
        assert_eq!(<Project as Wire>::__XPRODUCT_HASH_ENTRIES.len(), 12);
    }

    #[test]
    fn xproduct_hash_entries_all_distinct() {
        // All hash entries for NestedCL (4 entries) must be distinct
        let entries = <NestedCL as Wire>::__XPRODUCT_HASH_ENTRIES;
        let mut seen = std::collections::HashSet::new();
        for (i, (buf, len)) in entries.iter().enumerate() {
            let slice = &buf[..*len as usize];
            assert!(
                seen.insert(slice.to_vec()),
                "duplicate hash entry at index {i}"
            );
        }
    }

    #[test]
    fn xproduct_hash_entries_latest_matches_type() {
        // The last hash entry should match TYPE.serialize_for_hash
        let entries = <Mode as Wire>::__XPRODUCT_HASH_ENTRIES;
        let (ref last_buf, last_len) = entries[entries.len() - 1];

        let mut type_buf = [0u8; 4096];
        let mut type_cur = 0usize;
        Mode::TYPE.serialize_for_hash(&mut type_buf, &mut type_cur);

        assert_eq!(last_len, type_cur as u16);
        assert_eq!(&last_buf[..type_cur], &type_buf[..type_cur]);
    }

    #[test]
    fn xproduct_hash_entries_nested_latest_matches_type() {
        // NestedCL's last entry should match NestedCL::TYPE
        let entries = <NestedCL as Wire>::__XPRODUCT_HASH_ENTRIES;
        let (ref last_buf, last_len) = entries[entries.len() - 1];

        let mut type_buf = [0u8; 4096];
        let mut type_cur = 0usize;
        NestedCL::TYPE.serialize_for_hash(&mut type_buf, &mut type_cur);

        assert_eq!(last_len, type_cur as u16);
        assert_eq!(&last_buf[..type_cur], &type_buf[..type_cur]);
    }

    #[test]
    fn xproduct_hash_entries_produce_distinct_global_ids() {
        // NestedReadRequest (5 entries) should produce 5 distinct global_ids
        let entries = <NestedReadRequest as Wire>::__XPRODUCT_HASH_ENTRIES;
        let mut ids = std::collections::HashSet::new();
        for (i, (buf, len)) in entries.iter().enumerate() {
            let mut id_buf = [0u8; 4096];
            id_buf[0] = 0; // sentinel
            let mut j = 0;
            while j < *len as usize {
                id_buf[1 + j] = buf[j];
                j += 1;
            }
            let id = crate::Type::global_id_from_hash_buffer(
                "NestedReadRequest",
                &id_buf,
                1 + *len as usize,
            );
            assert!(ids.insert(id), "duplicate global_id at xproduct index {i}");
        }
        assert_eq!(ids.len(), 5);
    }

    #[test]
    fn xproduct_hash_entries_match_manual_type_trees() {
        // Manually construct the expected Type tree for each NestedCL cross-product
        // index and verify the hash bytes match __XPRODUCT_HASH_ENTRIES exactly.
        //
        // NestedCL layout (4 entries):
        //   idx 0: outer_v1 + Mode@v1 → struct(struct(bool))
        //   idx 1: outer_v1 + Mode@v2 → struct(struct(bool, bool))
        //   idx 2: outer_v2 + Mode@v1 → struct(struct(bool), Option<u64>)
        //   idx 3: outer_v2 + Mode@v2 → struct(struct(bool, bool), Option<u64>)
        use crate::Type;

        // Use statics for 'static lifetime on children slices
        static MODE_V1: Type = Type {
            name: "Mode",
            id: Type::STRUCT,
            children: &[<bool as Wire>::TYPE],
        };
        static MODE_V2: Type = Type {
            name: "Mode",
            id: Type::STRUCT,
            children: &[<bool as Wire>::TYPE, <bool as Wire>::TYPE],
        };
        static OPT_U64: Type = <Option<u64> as Wire>::TYPE;

        static EXPECTED: [Type; 4] = [
            // idx 0: outer_v1, Mode@v1
            Type {
                name: "NestedCL",
                id: Type::STRUCT,
                children: &[MODE_V1],
            },
            // idx 1: outer_v1, Mode@v2
            Type {
                name: "NestedCL",
                id: Type::STRUCT,
                children: &[MODE_V2],
            },
            // idx 2: outer_v2, Mode@v1, timeout
            Type {
                name: "NestedCL",
                id: Type::STRUCT,
                children: &[MODE_V1, OPT_U64],
            },
            // idx 3: outer_v2, Mode@v2, timeout
            Type {
                name: "NestedCL",
                id: Type::STRUCT,
                children: &[MODE_V2, OPT_U64],
            },
        ];

        let entries = <NestedCL as Wire>::__XPRODUCT_HASH_ENTRIES;
        assert_eq!(entries.len(), 4);

        for (i, expected_type) in EXPECTED.iter().enumerate() {
            let mut expected_buf = [0u8; 4096];
            let mut expected_cur = 0usize;
            expected_type.serialize_for_hash(&mut expected_buf, &mut expected_cur);

            let (ref actual_buf, actual_len) = entries[i];
            assert_eq!(
                actual_len, expected_cur as u16,
                "idx {i}: hash length mismatch (expected {expected_cur}, got {actual_len})"
            );
            assert_eq!(
                &actual_buf[..expected_cur],
                &expected_buf[..expected_cur],
                "idx {i}: hash bytes mismatch"
            );
        }
    }

    // ── Result<T,E> container `serialize_at` / `deserialize_at` ──────────
    //
    // `Option<T>` propagation is covered by `option_propagates_version_index_deserialize`
    // above, and struct-level dispatch by `versioned_serialize_at_v1_drops_fields` /
    // `versioned_deserialize_at_v1`. The gap these two tests close is the
    // `Result<T,E>` wrapper: the streaming RPC framing layer
    // (etude-rpc) serializes and deserializes each frame as
    // `Result<T>`, so `Result<T,E>` must forward `version_idx` to the inner
    // `T` on BOTH sides — otherwise a negotiated-version streamed frame
    // (de)serializes at the current layout and desyncs the stream.

    /// `Result<ConsistencyLevel, _>::serialize_at`/`deserialize_at` forwards
    /// the index to the inner struct, round-tripping byte-balanced (no
    /// leftover) at each version — the invariant the framing layer relies
    /// on to read consecutive frames without desyncing.
    #[test]
    fn result_wrapper_propagates_version_idx() {
        for (idx, timeout) in [(0u32, None), (1u32, Some(42u64))] {
            let mut out = etude_bytevec::Builder::default();
            let item: core::result::Result<ConsistencyLevel, crate::de::Error> =
                Ok(ConsistencyLevel { mode: 7, timeout });
            item.serialize_at(&mut out, idx);
            let mut bytes = out.finish();
            let written = bytes.len();

            let decoded =
                <core::result::Result<ConsistencyLevel, crate::de::Error>>::deserialize_at(
                    &mut bytes, idx,
                )
                .unwrap()
                .unwrap();

            assert_eq!(decoded.mode, 7, "idx={idx}");
            assert_eq!(decoded.timeout, timeout, "idx={idx}");
            assert_eq!(
                bytes.len(),
                0,
                "idx={idx}: Result<T> ser/de at the same idx must consume exactly \
                 what was written ({written} bytes)"
            );
        }
    }

    /// Over-read guard: if `Result::deserialize_at` is reverted to the
    /// default passthrough, decoding v1-written bytes at idx 0 invokes the
    /// inner `ConsistencyLevel::deserialize` (current = v2), which reads the
    /// absent `timeout` field and over-reads the buffer. A trailing sentinel
    /// byte proves a correct v1 decode consumes only the v1 frame.
    #[test]
    fn result_wrapper_v1_decode_does_not_overread() {
        let mut out = etude_bytevec::Builder::default();
        let item: core::result::Result<ConsistencyLevel, crate::de::Error> = Ok(ConsistencyLevel {
            mode: 3,
            timeout: None,
        });
        item.serialize_at(&mut out, 0);
        // Append a sentinel byte that a correct v1 decode must NOT touch.
        0xABu8.serialize(&mut out);
        let mut bytes = out.finish();

        let decoded = <core::result::Result<ConsistencyLevel, crate::de::Error>>::deserialize_at(
            &mut bytes, 0,
        )
        .unwrap()
        .unwrap();
        assert_eq!(decoded.mode, 3);
        assert_eq!(decoded.timeout, None);
        assert_eq!(
            bytes.len(),
            1,
            "v1 decode must leave exactly the 1 trailing sentinel byte untouched; \
             if it consumed more, Result::deserialize_at over-read into the next frame"
        );
    }
}

// ── TaggedWire derive macro tests ────────────────────────────────────────────

mod tagged_wire_tests {
    use crate::{TaggedDeserialize, TaggedSerialize, TaggedWire};

    #[derive(TaggedWire, Clone, Debug, PartialEq, Default)]
    #[wire(crate = crate)]
    struct Simple {
        #[wire(tag = 1.1)]
        name: String,
        #[wire(tag = 1.2)]
        value: u64,
    }

    #[test]
    fn tagged_wire_round_trip() {
        let original = Simple {
            name: "hello".to_string(),
            value: 42,
        };

        let mut out = etude_bytevec::Builder::default();
        original.tagged_serialize(&mut out);
        let bytes = out.finish();

        let mut buf = bytes;
        let decoded = Simple::tagged_deserialize(&mut buf).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn tagged_wire_missing_field_gets_default() {
        // Serialize only the "name" field by hand (tag 1.1)
        let mut out = etude_bytevec::Builder::default();
        // tag_v=1, tag_s=1
        crate::Serialize::serialize(1u8, &mut out);
        crate::Serialize::serialize(1u8, &mut out);
        // length prefix for "hello" (5 bytes len prefix + string)
        out.write_with_len_prefix(|out| {
            crate::Serialize::serialize("hello".to_string(), out);
        });
        let bytes = out.finish();

        // Deserialize — "value" field (tag 1.2) is missing, should get default (0)
        let mut buf = bytes;
        let decoded = Simple::tagged_deserialize(&mut buf).unwrap();
        assert_eq!(decoded.name, "hello");
        assert_eq!(decoded.value, 0); // Default::default() for u64
    }

    #[test]
    fn tagged_wire_unknown_field_skipped() {
        // Serialize a Simple, then append an unknown field (tag 9.9)
        let original = Simple {
            name: "test".to_string(),
            value: 99,
        };

        let mut out = etude_bytevec::Builder::default();
        original.tagged_serialize(&mut out);

        // Append unknown field: tag 9.9, length 4, value [0,0,0,1]
        crate::Serialize::serialize(9u8, &mut out);
        crate::Serialize::serialize(9u8, &mut out);
        out.write_with_len_prefix(|out| {
            crate::Serialize::serialize(12345u32, out);
        });
        let bytes = out.finish();

        // Deserialize — unknown field should be skipped
        let mut buf = bytes;
        let decoded = Simple::tagged_deserialize(&mut buf).unwrap();
        assert_eq!(decoded, original);
    }

    #[derive(TaggedWire, Clone, Debug, PartialEq, Default)]
    #[wire(crate = crate)]
    struct MultiVersion {
        #[wire(tag = 1.1)]
        id: u32,
        #[wire(tag = 1.2)]
        data: String,
        #[wire(tag = 2.1)]
        extra: u64,
    }

    #[test]
    fn tagged_wire_multi_field_round_trip() {
        let original = MultiVersion {
            id: 7,
            data: "payload".to_string(),
            extra: 1000,
        };

        let mut out = etude_bytevec::Builder::default();
        original.tagged_serialize(&mut out);
        let bytes = out.finish();

        let mut buf = bytes;
        let decoded = MultiVersion::tagged_deserialize(&mut buf).unwrap();
        assert_eq!(original, decoded);
    }

    #[derive(TaggedWire, Clone, Debug, PartialEq, Default)]
    #[wire(crate = crate)]
    struct Inner {
        #[wire(tag = 1.1)]
        x: u32,
        #[wire(tag = 1.2)]
        y: u32,
    }

    #[derive(TaggedWire, Clone, Debug, PartialEq, Default)]
    #[wire(crate = crate)]
    struct Outer {
        #[wire(tag = 1.1)]
        label: String,
        #[wire(tag = 1.2)]
        inner: Inner,
    }

    #[test]
    fn tagged_wire_nested_struct_round_trip() {
        let original = Outer {
            label: "parent".to_string(),
            inner: Inner { x: 10, y: 20 },
        };

        let mut out = etude_bytevec::Builder::default();
        original.tagged_serialize(&mut out);
        let bytes = out.finish();

        let mut buf = bytes;
        let decoded = Outer::tagged_deserialize(&mut buf).unwrap();
        assert_eq!(original, decoded);
    }

    #[derive(TaggedWire, Clone, Debug, PartialEq, Default)]
    #[wire(crate = crate)]
    struct WithVec {
        #[wire(tag = 1.1)]
        items: Vec<Inner>,
    }

    #[test]
    fn tagged_wire_vec_of_tagged_structs_round_trip() {
        let original = WithVec {
            items: vec![
                Inner { x: 1, y: 2 },
                Inner { x: 3, y: 4 },
                Inner { x: 5, y: 6 },
            ],
        };

        let mut out = etude_bytevec::Builder::default();
        original.tagged_serialize(&mut out);
        let bytes = out.finish();

        let mut buf = bytes;
        let decoded = WithVec::tagged_deserialize(&mut buf).unwrap();
        assert_eq!(original, decoded);
    }

    // ── TaggedWire enum tests ────────────────────────────────────────────────

    #[derive(TaggedWire, Clone, Debug, PartialEq)]
    #[wire(crate = crate)]
    enum Direction {
        #[wire(tag = 1.0)]
        North,
        #[wire(tag = 1.1)]
        South,
        #[wire(tag = 1.2)]
        East,
        #[wire(tag = 1.3)]
        West,
    }

    #[test]
    fn tagged_wire_enum_unit_variant_round_trip() {
        for variant in [
            Direction::North,
            Direction::South,
            Direction::East,
            Direction::West,
        ] {
            let mut out = etude_bytevec::Builder::default();
            variant.tagged_serialize(&mut out);
            let bytes = out.finish();

            let mut buf = bytes;
            let decoded = Direction::tagged_deserialize(&mut buf).unwrap();
            assert_eq!(variant, decoded);
        }
    }

    #[derive(TaggedWire, Clone, Debug, PartialEq)]
    #[wire(crate = crate)]
    enum Address {
        #[wire(tag = 1.1)]
        Socket { ip: u32, port: u16 },
        #[wire(tag = 1.2)]
        Named { name: String },
    }

    #[test]
    fn tagged_wire_enum_struct_variant_round_trip() {
        let socket = Address::Socket {
            ip: 0x0A000001,
            port: 8080,
        };
        let named = Address::Named {
            name: "localhost".to_string(),
        };

        for variant in [socket, named] {
            let mut out = etude_bytevec::Builder::default();
            variant.tagged_serialize(&mut out);
            let bytes = out.finish();

            let mut buf = bytes;
            let decoded = Address::tagged_deserialize(&mut buf).unwrap();
            assert_eq!(variant, decoded);
        }
    }

    #[derive(TaggedWire, Clone, Debug, PartialEq)]
    #[wire(crate = crate)]
    enum Payload {
        #[wire(tag = 1.1)]
        Empty,
        #[wire(tag = 1.2)]
        Single(u64),
        #[wire(tag = 1.3)]
        Pair(u32, String),
    }

    #[test]
    fn tagged_wire_enum_tuple_variant_round_trip() {
        let variants = vec![
            Payload::Empty,
            Payload::Single(999),
            Payload::Pair(42, "hello".to_string()),
        ];

        for variant in variants {
            let mut out = etude_bytevec::Builder::default();
            variant.tagged_serialize(&mut out);
            let bytes = out.finish();

            let mut buf = bytes;
            let decoded = Payload::tagged_deserialize(&mut buf).unwrap();
            assert_eq!(variant, decoded);
        }
    }

    #[derive(TaggedWire, Clone, Debug, PartialEq)]
    #[wire(crate = crate)]
    enum Status {
        #[wire(tag = 1.0, fallback)]
        Unknown,
        #[wire(tag = 1.1)]
        Active,
        #[wire(tag = 1.2)]
        Inactive,
    }

    #[test]
    fn tagged_wire_enum_known_variant_round_trip() {
        for variant in [Status::Unknown, Status::Active, Status::Inactive] {
            let mut out = etude_bytevec::Builder::default();
            variant.tagged_serialize(&mut out);
            let bytes = out.finish();

            let mut buf = bytes;
            let decoded = Status::tagged_deserialize(&mut buf).unwrap();
            assert_eq!(variant, decoded);
        }
    }

    #[test]
    fn tagged_wire_enum_unknown_tag_uses_fallback() {
        // Manually encode an unknown variant (tag 9.9) with empty payload
        let mut out = etude_bytevec::Builder::default();
        crate::Serialize::serialize(9u8, &mut out); // tag_v
        crate::Serialize::serialize(9u8, &mut out); // tag_s
        out.write_with_len_prefix(|_| {}); // empty payload

        let bytes = out.finish();
        let mut buf = bytes;
        let decoded = Status::tagged_deserialize(&mut buf).unwrap();
        assert_eq!(decoded, Status::Unknown);
    }

    #[test]
    fn tagged_wire_enum_unknown_tag_with_data_uses_fallback() {
        // Manually encode an unknown variant (tag 5.3) with some payload data
        let mut out = etude_bytevec::Builder::default();
        crate::Serialize::serialize(5u8, &mut out); // tag_v
        crate::Serialize::serialize(3u8, &mut out); // tag_s
        out.write_with_len_prefix(|out| {
            crate::Serialize::serialize(12345u64, out); // arbitrary payload
        });

        let bytes = out.finish();
        let mut buf = bytes;
        let decoded = Status::tagged_deserialize(&mut buf).unwrap();
        // Unknown tag with data is skipped; fallback variant returned
        assert_eq!(decoded, Status::Unknown);
    }

    // Enum without fallback — unknown tag produces an error
    #[derive(TaggedWire, Clone, Debug, PartialEq)]
    #[wire(crate = crate)]
    enum Strict {
        #[wire(tag = 1.0)]
        A,
        #[wire(tag = 1.1)]
        B,
    }

    #[test]
    fn tagged_wire_enum_no_fallback_unknown_tag_errors() {
        let mut out = etude_bytevec::Builder::default();
        crate::Serialize::serialize(9u8, &mut out);
        crate::Serialize::serialize(9u8, &mut out);
        out.write_with_len_prefix(|_| {});

        let bytes = out.finish();
        let mut buf = bytes;
        let result = Strict::tagged_deserialize(&mut buf);
        assert!(result.is_err());
    }

    // ── Struct field default tests ───────────────────────────────────────────

    #[derive(TaggedWire, Clone, Debug, PartialEq)]
    #[wire(crate = crate)]
    struct WithCustomDefault {
        #[wire(tag = 1.1)]
        name: String,
        #[wire(tag = 2.1, default = 42)]
        priority: u32,
        #[wire(tag = 2.2, default = "fallback".to_string())]
        label: String,
    }

    #[test]
    fn tagged_wire_struct_custom_default_when_field_absent() {
        // Serialize only the "name" field (tag 1.1) — priority and label absent
        let mut out = etude_bytevec::Builder::default();
        crate::Serialize::serialize(1u8, &mut out); // tag_v
        crate::Serialize::serialize(1u8, &mut out); // tag_s
        out.write_with_len_prefix(|out| {
            crate::Serialize::serialize("test".to_string(), out);
        });
        let bytes = out.finish();

        let mut buf = bytes;
        let decoded = WithCustomDefault::tagged_deserialize(&mut buf).unwrap();
        assert_eq!(decoded.name, "test");
        assert_eq!(decoded.priority, 42); // custom default, not 0
        assert_eq!(decoded.label, "fallback"); // custom default, not ""
    }

    #[test]
    fn tagged_wire_struct_custom_default_overridden_when_present() {
        // Serialize all fields — defaults should NOT be used
        let original = WithCustomDefault {
            name: "hello".to_string(),
            priority: 99,
            label: "custom".to_string(),
        };

        let mut out = etude_bytevec::Builder::default();
        original.tagged_serialize(&mut out);
        let bytes = out.finish();

        let mut buf = bytes;
        let decoded = WithCustomDefault::tagged_deserialize(&mut buf).unwrap();
        assert_eq!(decoded, original);
    }

    // ── Enum used as field in a TaggedWire struct ────────────────────────────

    #[derive(TaggedWire, Clone, Debug, PartialEq, Default)]
    #[wire(crate = crate)]
    struct ContainsEnum {
        #[wire(tag = 1.1)]
        addrs: Vec<Address>,
        #[wire(tag = 1.2)]
        status: Status,
    }

    impl Default for Address {
        fn default() -> Self {
            Address::Socket { ip: 0, port: 0 }
        }
    }

    impl Default for Status {
        fn default() -> Self {
            Status::Unknown
        }
    }

    #[test]
    fn tagged_wire_struct_containing_enum_round_trip() {
        let original = ContainsEnum {
            addrs: vec![
                Address::Socket {
                    ip: 0x7F000001,
                    port: 443,
                },
                Address::Named {
                    name: "peer1".to_string(),
                },
            ],
            status: Status::Active,
        };

        let mut out = etude_bytevec::Builder::default();
        original.tagged_serialize(&mut out);
        let bytes = out.finish();

        let mut buf = bytes;
        let decoded = ContainsEnum::tagged_deserialize(&mut buf).unwrap();
        assert_eq!(original, decoded);
    }

    // ── #[wire(unknown)] catch-field tests ───────────────────────────────────

    // `Catch` knows tags 1.1 and 1.3 and carries an unknown-catch field, so a
    // field with any other tag is preserved rather than dropped.
    #[derive(TaggedWire, Clone, Debug, PartialEq, Default)]
    #[wire(crate = crate)]
    struct Catch {
        #[wire(tag = 1.1)]
        a: u32,
        #[wire(tag = 1.3)]
        c: u32,
        #[wire(unknown)]
        rest: crate::Unknown,
    }

    #[test]
    fn tagged_wire_unknown_catch_preserves_and_reemits() {
        // A record with three fields in ascending tag order: a (1.1), an unknown
        // field (1.2), and c (1.3). `Catch` does not know tag 1.2.
        let mut out = etude_bytevec::Builder::default();
        crate::Serialize::serialize(1u8, &mut out);
        crate::Serialize::serialize(1u8, &mut out);
        out.write_with_len_prefix(|out| crate::Serialize::serialize(7u32, out));
        crate::Serialize::serialize(1u8, &mut out);
        crate::Serialize::serialize(2u8, &mut out);
        out.write_with_len_prefix(|out| crate::Serialize::serialize(0xDEAD_BEEFu32, out));
        crate::Serialize::serialize(1u8, &mut out);
        crate::Serialize::serialize(3u8, &mut out);
        out.write_with_len_prefix(|out| crate::Serialize::serialize(9u32, out));
        let original = out.finish();

        // Decode: a and c land in their fields; 1.2 is captured in `rest`.
        let mut buf = original.clone();
        let decoded = Catch::tagged_deserialize(&mut buf).unwrap();
        assert_eq!(decoded.a, 7);
        assert_eq!(decoded.c, 9);
        assert_eq!(decoded.rest.len(), 1);
        assert!(!decoded.rest.is_empty());
        let uf = decoded.rest.iter().next().unwrap();
        assert_eq!((uf.tag_v, uf.tag_s), (1, 2));
        // The raw value bytes are kept exactly: 0xDEADBEEF as a big-endian u32.
        assert_eq!(uf.bytes, [0xDEu8, 0xAD, 0xBE, 0xEF]);

        // The input was already in sorted tag order, so re-encoding is byte-for-byte
        // identical with the unknown field preserved.
        let mut out2 = etude_bytevec::Builder::default();
        decoded.tagged_serialize(&mut out2);
        let reencoded = out2.finish();
        assert_eq!(reencoded, original);
    }

    #[test]
    fn tagged_wire_unknown_catch_emits_in_tag_order() {
        // The same three fields but in scrambled wire order: c (1.3), unknown
        // (1.2), a (1.1). On re-emit they must come out sorted by tag regardless
        // of where the unknown sat in the input.
        let mut scrambled = etude_bytevec::Builder::default();
        crate::Serialize::serialize(1u8, &mut scrambled);
        crate::Serialize::serialize(3u8, &mut scrambled);
        scrambled.write_with_len_prefix(|o| crate::Serialize::serialize(9u32, o));
        crate::Serialize::serialize(1u8, &mut scrambled);
        crate::Serialize::serialize(2u8, &mut scrambled);
        scrambled.write_with_len_prefix(|o| crate::Serialize::serialize(0xDEAD_BEEFu32, o));
        crate::Serialize::serialize(1u8, &mut scrambled);
        crate::Serialize::serialize(1u8, &mut scrambled);
        scrambled.write_with_len_prefix(|o| crate::Serialize::serialize(7u32, o));
        let scrambled = scrambled.finish();

        // Canonical sorted order: a (1.1), unknown (1.2), c (1.3).
        let mut canonical = etude_bytevec::Builder::default();
        crate::Serialize::serialize(1u8, &mut canonical);
        crate::Serialize::serialize(1u8, &mut canonical);
        canonical.write_with_len_prefix(|o| crate::Serialize::serialize(7u32, o));
        crate::Serialize::serialize(1u8, &mut canonical);
        crate::Serialize::serialize(2u8, &mut canonical);
        canonical.write_with_len_prefix(|o| crate::Serialize::serialize(0xDEAD_BEEFu32, o));
        crate::Serialize::serialize(1u8, &mut canonical);
        crate::Serialize::serialize(3u8, &mut canonical);
        canonical.write_with_len_prefix(|o| crate::Serialize::serialize(9u32, o));
        let canonical = canonical.finish();

        let mut buf = scrambled;
        let decoded = Catch::tagged_deserialize(&mut buf).unwrap();
        let mut out = etude_bytevec::Builder::default();
        decoded.tagged_serialize(&mut out);
        let reencoded = out.finish();
        assert_eq!(reencoded, canonical);
    }

    #[test]
    fn tagged_wire_unknown_catch_empty_adds_nothing() {
        let v = Catch {
            a: 1,
            c: 2,
            rest: crate::Unknown::default(),
        };
        assert!(v.rest.is_empty());

        let mut out = etude_bytevec::Builder::default();
        v.clone().tagged_serialize(&mut out);
        let bytes = out.finish();

        // Expected: only the two known fields, in tag order, with no 1.2 entry.
        let mut expected = etude_bytevec::Builder::default();
        crate::Serialize::serialize(1u8, &mut expected);
        crate::Serialize::serialize(1u8, &mut expected);
        expected.write_with_len_prefix(|o| crate::Serialize::serialize(1u32, o));
        crate::Serialize::serialize(1u8, &mut expected);
        crate::Serialize::serialize(3u8, &mut expected);
        expected.write_with_len_prefix(|o| crate::Serialize::serialize(2u32, o));
        let expected = expected.finish();
        assert_eq!(bytes, expected);

        // Round-trips with an empty catch.
        let mut buf = bytes;
        let decoded = Catch::tagged_deserialize(&mut buf).unwrap();
        assert_eq!(decoded, v);
        assert!(decoded.rest.is_empty());
    }

    #[test]
    fn tagged_wire_no_catch_field_skips_and_drops_unknown() {
        // `Simple` knows name (1.1) and value (1.2) but has no catch field, so an
        // unknown tag is skipped on read and gone on re-write.
        let mut out = etude_bytevec::Builder::default();
        crate::Serialize::serialize(1u8, &mut out);
        crate::Serialize::serialize(1u8, &mut out);
        out.write_with_len_prefix(|o| crate::Serialize::serialize("hi".to_string(), o));
        crate::Serialize::serialize(1u8, &mut out);
        crate::Serialize::serialize(2u8, &mut out);
        out.write_with_len_prefix(|o| crate::Serialize::serialize(5u64, o));
        crate::Serialize::serialize(7u8, &mut out);
        crate::Serialize::serialize(7u8, &mut out);
        out.write_with_len_prefix(|o| crate::Serialize::serialize(123u32, o));
        let with_unknown = out.finish();

        let mut buf = with_unknown;
        let decoded = Simple::tagged_deserialize(&mut buf).unwrap();
        assert_eq!(decoded.name, "hi");
        assert_eq!(decoded.value, 5);

        // Re-encode: the unknown tag 7.7 is gone (dropped, not preserved).
        let mut out2 = etude_bytevec::Builder::default();
        decoded.tagged_serialize(&mut out2);
        let reencoded = out2.finish();

        let mut expected = etude_bytevec::Builder::default();
        crate::Serialize::serialize(1u8, &mut expected);
        crate::Serialize::serialize(1u8, &mut expected);
        expected.write_with_len_prefix(|o| crate::Serialize::serialize("hi".to_string(), o));
        crate::Serialize::serialize(1u8, &mut expected);
        crate::Serialize::serialize(2u8, &mut expected);
        expected.write_with_len_prefix(|o| crate::Serialize::serialize(5u64, o));
        let expected = expected.finish();
        assert_eq!(reencoded, expected);
    }

    // v2 knows an extra field `c` (tag 1.3) that v1 does not.
    #[derive(TaggedWire, Clone, Debug, PartialEq, Default)]
    #[wire(crate = crate)]
    struct RecordV2 {
        #[wire(tag = 1.1)]
        a: u32,
        #[wire(tag = 1.2)]
        b: String,
        #[wire(tag = 1.3)]
        c: u64,
    }

    // v1 predates `c` and carries a catch field to preserve it.
    #[derive(TaggedWire, Clone, Debug, PartialEq, Default)]
    #[wire(crate = crate)]
    struct RecordV1 {
        #[wire(tag = 1.1)]
        a: u32,
        #[wire(tag = 1.2)]
        b: String,
        #[wire(unknown)]
        rest: crate::Unknown,
    }

    #[test]
    fn tagged_wire_v2_to_v1_roundtrip_preserves_extra_field() {
        let v2 = RecordV2 {
            a: 42,
            b: "payload".to_string(),
            c: 0xCAFE_F00D,
        };

        // Encode as v2.
        let mut out = etude_bytevec::Builder::default();
        v2.clone().tagged_serialize(&mut out);
        let v2_bytes = out.finish();

        // Decode as v1 — the extra field `c` (1.3) lands in the catch.
        let mut buf = v2_bytes.clone();
        let v1 = RecordV1::tagged_deserialize(&mut buf).unwrap();
        assert_eq!(v1.a, 42);
        assert_eq!(v1.b, "payload");
        assert_eq!(v1.rest.len(), 1);
        let uf = v1.rest.iter().next().unwrap();
        assert_eq!((uf.tag_v, uf.tag_s), (1, 3));

        // Re-encode v1 — the preserved field is re-emitted in tag order, so the
        // bytes match what v2 wrote.
        let mut out2 = etude_bytevec::Builder::default();
        v1.tagged_serialize(&mut out2);
        let v1_bytes = out2.finish();
        assert_eq!(v1_bytes, v2_bytes);

        // Decode again as v2 — the extra field survived the v1 round trip.
        let mut buf2 = v1_bytes;
        let v2_again = RecordV2::tagged_deserialize(&mut buf2).unwrap();
        assert_eq!(v2_again, v2);
        assert_eq!(v2_again.c, 0xCAFE_F00D);
    }

    // ── A value consumes its field's bytes whole ─────────────────────────────

    #[derive(TaggedWire, Clone, Debug, PartialEq, Default)]
    #[wire(crate = crate)]
    struct TimeoutWas {
        #[wire(tag = 1.1)]
        timeout: u64,
    }

    #[derive(TaggedWire, Clone, Debug, PartialEq, Default)]
    #[wire(crate = crate)]
    struct TimeoutIs {
        #[wire(tag = 1.1)]
        timeout: Option<u64>,
    }

    /// A field written as a `u64` and read as an `Option<u64>` takes its first byte as the option's tag and leaves
    /// seven unread: the decode fails `InvalidLength` naming the field's type, where it answered `None` and lost the
    /// stored value.
    #[test]
    fn tagged_wire_field_whose_value_leaves_bytes_unread_fails_to_decode() {
        let mut out = etude_bytevec::Builder::default();
        TimeoutWas { timeout: 30 }.tagged_serialize(&mut out);
        let mut buf = out.finish();
        let error =
            TimeoutIs::tagged_deserialize(&mut buf).expect_err("eight bytes are no Option<u64>");
        assert_eq!(error.kind(), crate::de::ErrorKind::InvalidLength);
        assert_eq!(error.ty().name, <Option<u64> as crate::Wire>::TYPE.name);
    }

    /// The same field written and read as `Option<u64>` is whole, and decodes as before.
    #[test]
    fn tagged_wire_field_whose_value_is_whole_decodes() {
        let original = TimeoutIs { timeout: Some(30) };
        let mut out = etude_bytevec::Builder::default();
        original.tagged_serialize(&mut out);
        let mut buf = out.finish();
        assert_eq!(TimeoutIs::tagged_deserialize(&mut buf).unwrap(), original);
    }

    #[derive(TaggedWire, Clone, Debug, PartialEq)]
    #[wire(crate = crate)]
    enum RetryWas {
        #[wire(tag = 1.1)]
        After(u64),
        #[wire(tag = 1.2)]
        Within { seconds: u64 },
    }

    #[derive(TaggedWire, Clone, Debug, PartialEq)]
    #[wire(crate = crate)]
    enum RetryIs {
        #[wire(tag = 1.1)]
        After(Option<u64>),
        #[wire(tag = 1.2)]
        Within { seconds: Option<u64> },
    }

    /// A variant's payload written as a `u64` and read as an `Option<u64>` fails the same way, a tuple variant's and
    /// a struct variant's alike, named for the payload's field that stopped short.
    #[test]
    fn tagged_wire_variant_payload_that_leaves_bytes_unread_fails_to_decode() {
        for was in [RetryWas::After(30), RetryWas::Within { seconds: 30 }] {
            let mut out = etude_bytevec::Builder::default();
            was.tagged_serialize(&mut out);
            let mut buf = out.finish();
            let error =
                RetryIs::tagged_deserialize(&mut buf).expect_err("eight bytes are no Option<u64>");
            assert_eq!(error.kind(), crate::de::ErrorKind::InvalidLength, "{was:?}");
            assert_eq!(error.ty().name, <Option<u64> as crate::Wire>::TYPE.name);
        }
        for is in [RetryIs::After(Some(30)), RetryIs::Within { seconds: None }] {
            let mut out = etude_bytevec::Builder::default();
            is.tagged_serialize(&mut out);
            let mut buf = out.finish();
            assert_eq!(RetryIs::tagged_deserialize(&mut buf).unwrap(), is);
        }
    }

    // -- TaggedFields --

    use crate::{TaggedField, TaggedFields};

    /// Declared out of tag order, with the catch field between two tagged fields, a field with a custom default and a
    /// raw identifier.
    #[derive(TaggedWire, Clone, Debug, PartialEq, Default)]
    #[wire(crate = crate)]
    struct Listed {
        #[wire(tag = 1.3)]
        later: u64,
        #[wire(unknown)]
        rest: crate::Unknown,
        #[wire(tag = 1.120, default = 7)]
        r#type: u64,
        #[wire(tag = 2.1)]
        first: bool,
    }

    /// Each tag `bytes` holds at its top level, in the order it holds them: a tagged encoding is the fields in turn,
    /// each its two tag bytes, its length and its value.
    fn tags_written(mut bytes: etude_bytevec::ByteVec) -> Vec<(u8, u8)> {
        let mut tags = Vec::new();
        while !bytes.is_empty() {
            let tag_v = <u8 as crate::Deserialize>::deserialize(&mut bytes).unwrap();
            let tag_s = <u8 as crate::Deserialize>::deserialize(&mut bytes).unwrap();
            let len = <u64 as crate::Deserialize>::deserialize(&mut bytes).unwrap() as usize;
            bytes.split_to(len).unwrap();
            tags.push((tag_v, tag_s));
        }
        tags
    }

    /// A struct lists each tagged field's tag and name in the order it declares them, the raw identifier without its
    /// `r#`, and leaves out the catch field, which has no tag.
    #[test]
    fn tagged_fields_lists_each_struct_field_in_declaration_order() {
        assert_eq!(
            Listed::TAGGED_FIELDS,
            &[
                TaggedField {
                    tag_v: 1,
                    tag_s: 3,
                    name: "later",
                },
                TaggedField {
                    tag_v: 1,
                    tag_s: 120,
                    name: "type",
                },
                TaggedField {
                    tag_v: 2,
                    tag_s: 1,
                    name: "first",
                },
            ]
        );
        assert_eq!(Simple::TAGGED_FIELDS.len(), 2);
        assert_eq!(Simple::TAGGED_FIELDS[0].name, "name");
        assert_eq!(Simple::TAGGED_FIELDS[1].name, "value");
    }

    /// The tags a struct lists are the tags its encoding writes, with a catch field (written in tag order) and without
    /// one (written in declaration order).
    #[test]
    fn tagged_fields_of_a_struct_are_the_tags_its_encoding_writes() {
        let mut out = etude_bytevec::Builder::default();
        Listed::default().tagged_serialize(&mut out);
        let mut listed: Vec<(u8, u8)> = Listed::TAGGED_FIELDS
            .iter()
            .map(|f| (f.tag_v, f.tag_s))
            .collect();
        listed.sort();
        assert_eq!(tags_written(out.finish()), listed);

        let mut out = etude_bytevec::Builder::default();
        Simple::default().tagged_serialize(&mut out);
        let listed: Vec<(u8, u8)> = Simple::TAGGED_FIELDS
            .iter()
            .map(|f| (f.tag_v, f.tag_s))
            .collect();
        assert_eq!(tags_written(out.finish()), listed);
    }

    #[derive(TaggedWire, Clone, Debug, PartialEq)]
    #[wire(crate = crate)]
    enum ListedKind {
        #[wire(tag = 1.2)]
        Second(u64),
        #[wire(tag = 1.1, fallback)]
        Other,
        #[wire(tag = 1.7)]
        Named { seconds: u64 },
    }

    /// An enum lists each variant's tag and name in the order it declares them, a unit, a tuple and a struct variant
    /// alike, and the fallback variant among them; each variant's encoding writes the tag listed for it.
    #[test]
    fn tagged_fields_lists_each_enum_variant_in_declaration_order() {
        assert_eq!(
            ListedKind::TAGGED_FIELDS,
            &[
                TaggedField {
                    tag_v: 1,
                    tag_s: 2,
                    name: "Second",
                },
                TaggedField {
                    tag_v: 1,
                    tag_s: 1,
                    name: "Other",
                },
                TaggedField {
                    tag_v: 1,
                    tag_s: 7,
                    name: "Named",
                },
            ]
        );
        let values = [
            ListedKind::Second(3),
            ListedKind::Other,
            ListedKind::Named { seconds: 4 },
        ];
        for (value, field) in values.iter().zip(ListedKind::TAGGED_FIELDS) {
            let mut out = etude_bytevec::Builder::default();
            value.tagged_serialize(&mut out);
            assert_eq!(
                tags_written(out.finish()),
                [(field.tag_v, field.tag_s)],
                "{value:?}"
            );
        }
    }
}
