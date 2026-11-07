// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use bytes::Bytes;
use upstream_buffer::writer::Storage as _;
use upstream_bytevec::{self as bytevec, Behavior, ByteVec};
use std::{
    any::TypeId,
    collections::{BTreeMap, BTreeSet, VecDeque},
};

/// Trait for types that can be serialized.
///
/// The output sink is a [`bytevec::Builder<B>`] parameterized over its
/// [`Behavior`] `B` — the zero-sized [`bytevec::DefaultBehavior`] on the common
/// path (freeze in place, no copy), or a caller-selected behavior such as a
/// zeroize-on-drop owner for secret-carrying chunks. The `B` type parameter is
/// monomorphized end-to-end; there is no `Box<dyn>` or dynamic dispatch on any
/// path. Bodies are unchanged from the concrete-`Builder` form — `B` is inferred
/// from the sink at each call.
pub trait Serialize: Sized {
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>);

    /// Serialize using a specific version index into `VERSION_TYPES`.
    ///
    /// Versioned types override this to dispatch to the version-specific
    /// serializer. Non-versioned types use the default which ignores the
    /// index and calls `serialize()`.
    ///
    /// Container types (`Option<T>`, `Vec<T>`, `Box<T>`) propagate the index
    /// to their inner type's `serialize_at`.
    #[inline(always)]
    fn serialize_at<B: Behavior>(self, out: &mut bytevec::Builder<B>, _version_idx: u32) {
        self.serialize(out)
    }

    #[inline]
    fn serialize_with_len_prefix<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        out.write_with_len_prefix(|out| self.serialize(out))
    }

    /// Length-prefixed variant of [`Self::serialize_at`]. Versioned types
    /// dispatch to the version-specific serializer via `serialize_at`;
    /// non-versioned types fall through to plain `serialize` since the
    /// default `serialize_at` ignores its index argument.
    #[inline]
    fn serialize_with_len_prefix_at<B: Behavior>(
        self,
        out: &mut bytevec::Builder<B>,
        version_idx: u32,
    ) {
        out.write_with_len_prefix(|out| self.serialize_at(out, version_idx))
    }

    #[inline]
    fn serialize_tagged<B: Behavior>(self, out: &mut bytevec::Builder<B>)
    where
        Self: crate::Wire,
    {
        const { Self::TYPE.global_id() }.serialize(out);
        self.serialize(out);
    }
}

impl Serialize for core::convert::Infallible {
    fn serialize<B: Behavior>(self, _: &mut bytevec::Builder<B>) {
        unreachable!("cannot construct `Infallible`")
    }
}

impl<T> Serialize for Option<T>
where
    T: Serialize,
{
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        match self {
            None => {
                0u8.serialize(out);
            }
            Some(value) => {
                1u8.serialize(out);
                value.serialize(out);
            }
        }
    }

    #[inline(always)]
    fn serialize_at<B: Behavior>(self, out: &mut bytevec::Builder<B>, version_idx: u32) {
        match self {
            None => {
                0u8.serialize(out);
            }
            Some(value) => {
                1u8.serialize(out);
                value.serialize_at(out, version_idx);
            }
        }
    }
}

impl<T> Serialize for Box<T>
where
    T: Serialize,
{
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        (*self).serialize(out);
    }

    #[inline(always)]
    fn serialize_at<B: Behavior>(self, out: &mut bytevec::Builder<B>, version_idx: u32) {
        (*self).serialize_at(out, version_idx);
    }
}

impl Serialize for Bytes {
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        self.len().serialize(out);
        out.put_bytes(self);
    }
}

impl Serialize for ByteVec {
    fn serialize<B: Behavior>(mut self, out: &mut bytevec::Builder<B>) {
        self.len().serialize(out);
        out.append(&mut self);
    }
}

impl<O: bytevec::tagged::Owner> Serialize for bytevec::Tagged<O> {
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        let chunk: ByteVec = self.into();
        chunk.serialize(out);
    }
}

impl<T: 'static + Serialize> Serialize for Vec<T> {
    fn serialize<B: Behavior>(mut self, out: &mut bytevec::Builder<B>) {
        self.len().serialize(out);
        if TypeId::of::<T>() == TypeId::of::<u8>() {
            let ptr = self.as_mut_ptr() as *mut u8;
            let len = self.len();
            let capacity = self.capacity();
            core::mem::forget(self);

            let v = unsafe { <Vec<u8>>::from_raw_parts(ptr, len, capacity) };
            out.put_bytes(v.into());
        } else {
            for v in self {
                v.serialize(out);
            }
        }
    }
}

impl<T: Serialize> Serialize for VecDeque<T> {
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        self.len().serialize(out);
        for v in self {
            v.serialize(out);
        }
    }
}

impl<K: Serialize, V: Serialize> Serialize for BTreeMap<K, V> {
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        self.len().serialize(out);
        for (k, v) in self {
            k.serialize(out);
            v.serialize(out);
        }
    }
}

impl<T: Serialize> Serialize for BTreeSet<T> {
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        self.len().serialize(out);
        for v in self {
            v.serialize(out);
        }
    }
}

impl Serialize for String {
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        self.into_bytes().serialize(out)
    }
}

impl Serialize for std::sync::Arc<str> {
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        self.len().serialize(out);
        out.put_slice(self.as_bytes());
    }
}

impl<T, E> Serialize for Result<T, E>
where
    T: Serialize,
    E: Serialize,
{
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        match self {
            Ok(value) => {
                // Use 0 to indicate success
                0u8.serialize(out);
                value.serialize(out);
            }
            Err(err) => {
                // Use 1 to indicate error
                1u8.serialize(out);
                err.serialize(out);
            }
        }
    }

    /// Propagate `version_idx` to the inner success value's `serialize_at`
    /// so streaming RPCs serialize each `Ok(T)` chunk at the negotiated
    /// version. Errors stay non-versioned (errors aren't xproduct-typed).
    #[inline(always)]
    fn serialize_at<B: Behavior>(self, out: &mut bytevec::Builder<B>, version_idx: u32) {
        match self {
            Ok(value) => {
                0u8.serialize(out);
                value.serialize_at(out, version_idx);
            }
            Err(err) => {
                1u8.serialize(out);
                err.serialize(out);
            }
        }
    }
}

impl Serialize for bool {
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        out.put_slice(&[self as u8]);
    }
}

// Macro to implement Serialize for integer types
macro_rules! impl_serialize_for_int {
    ($ty:ty, $size:expr) => {
        impl Serialize for $ty {
            fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
                out.put_slice(&self.to_be_bytes());
            }
        }
    };
}

// Implement Serialize for all integer types
// Special case for u8
impl Serialize for u8 {
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        out.put_slice(&[self]);
    }
}
impl_serialize_for_int!(u16, 2);
impl_serialize_for_int!(u32, 4);
impl_serialize_for_int!(u64, 8);
impl_serialize_for_int!(u128, 16);

impl Serialize for usize {
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        out.put_slice(&(self as u64).to_be_bytes());
    }
}

impl Serialize for i8 {
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        out.put_slice(&[self as u8]);
    }
}
impl_serialize_for_int!(i16, 2);
impl_serialize_for_int!(i32, 4);
impl_serialize_for_int!(i64, 8);
impl_serialize_for_int!(i128, 16);

impl Serialize for isize {
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        out.put_slice(&(self as i64).to_be_bytes());
    }
}

// Floats are written as their big-endian IEEE-754 bytes, matching the integer
// encoding above (`f32`/`f64::to_be_bytes`).
impl Serialize for f32 {
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        out.put_slice(&self.to_be_bytes());
    }
}

impl Serialize for f64 {
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        out.put_slice(&self.to_be_bytes());
    }
}

impl<const N: usize> Serialize for [u8; N] {
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        out.put_slice(&self);
    }
}

impl Serialize for crate::de::Error {
    fn serialize<B: Behavior>(self, bytes: &mut bytevec::Builder<B>) {
        self.kind().serialize(bytes)
    }
}

macro_rules! impl_tuple_serialize {
    // Base case - empty tuple
    () => {
        impl Serialize for () {
            fn serialize<B: Behavior>(self, _: &mut bytevec::Builder<B>) {}
        }
    };
    // Recursive case - tuple with at least one element
    ($t0:ident $(,$rest:ident)*) => {
        impl_tuple_serialize!($($rest),*);

        impl<$t0: Serialize, $($rest: Serialize),*> Serialize for ($t0, $($rest),*) {
            #[allow(non_snake_case)]
            fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
                let ($t0, $($rest),*) = self;
                $t0.serialize(out);
                $($rest.serialize(out);)*
            }
        }
    };
}

// Implement for tuples up to 16 elements
impl_tuple_serialize!(
    T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15
);

/// Serialization by reference: encodes a value the caller keeps, without
/// consuming or cloning it.
///
/// The output is byte-identical to [`Serialize::serialize`] (and
/// [`Self::serialize_ref_at`] to [`Serialize::serialize_at`]) for the same
/// value. Chunk-backed fields stay zero-copy: a [`ByteVec`] or [`Bytes`] field
/// is emitted by sharing its chunks (a refcount bump), not by copying.
///
/// `&T` implements [`Serialize`] for every `T: SerializeRef`, so a borrowed
/// value can be passed anywhere a [`Serialize`] value is accepted. Derived
/// types opt in with `#[wire(borrowed)]`; every field type must then implement
/// `SerializeRef`.
pub trait SerializeRef {
    fn serialize_ref<B: Behavior>(&self, out: &mut bytevec::Builder<B>);

    /// By-reference counterpart of [`Serialize::serialize_at`].
    #[inline(always)]
    fn serialize_ref_at<B: Behavior>(&self, out: &mut bytevec::Builder<B>, _version_idx: u32) {
        self.serialize_ref(out)
    }

    /// By-reference counterpart of [`Serialize::serialize_tagged`].
    #[inline]
    fn serialize_ref_tagged<B: Behavior>(&self, out: &mut bytevec::Builder<B>)
    where
        Self: crate::Wire,
    {
        const { Self::TYPE.global_id() }.serialize(out);
        self.serialize_ref(out);
    }
}

impl<T: SerializeRef + ?Sized> Serialize for &T {
    #[inline(always)]
    fn serialize<B: Behavior>(self, out: &mut bytevec::Builder<B>) {
        T::serialize_ref(self, out)
    }

    #[inline(always)]
    fn serialize_at<B: Behavior>(self, out: &mut bytevec::Builder<B>, version_idx: u32) {
        T::serialize_ref_at(self, out, version_idx)
    }
}

impl<T: SerializeRef + ?Sized> SerializeRef for &T {
    #[inline(always)]
    fn serialize_ref<B: Behavior>(&self, out: &mut bytevec::Builder<B>) {
        T::serialize_ref(self, out)
    }

    #[inline(always)]
    fn serialize_ref_at<B: Behavior>(&self, out: &mut bytevec::Builder<B>, version_idx: u32) {
        T::serialize_ref_at(self, out, version_idx)
    }
}

/// `SerializeRef` for `Copy` types whose by-value encoding reads nothing but
/// the value itself: copy it out and reuse [`Serialize::serialize`].
macro_rules! impl_serialize_ref_copy {
    ($($ty:ty),* $(,)?) => {
        $(
            impl $crate::ser::SerializeRef for $ty {
                #[inline(always)]
                fn serialize_ref<B: $crate::bytevec::Behavior>(&self, out: &mut $crate::bytevec::Builder<B>) {
                    $crate::Serialize::serialize(*self, out)
                }
            }
        )*
    };
}
pub(crate) use impl_serialize_ref_copy;

impl_serialize_ref_copy!(
    bool,
    u8,
    u16,
    u32,
    u64,
    u128,
    usize,
    i8,
    i16,
    i32,
    i64,
    i128,
    isize,
    f32,
    f64,
    crate::de::Error,
);

impl<const N: usize> SerializeRef for [u8; N] {
    #[inline(always)]
    fn serialize_ref<B: Behavior>(&self, out: &mut bytevec::Builder<B>) {
        out.put_slice(self);
    }
}

impl SerializeRef for core::convert::Infallible {
    fn serialize_ref<B: Behavior>(&self, _: &mut bytevec::Builder<B>) {
        match *self {}
    }
}

impl<T: SerializeRef> SerializeRef for Option<T> {
    fn serialize_ref<B: Behavior>(&self, out: &mut bytevec::Builder<B>) {
        match self {
            None => {
                0u8.serialize(out);
            }
            Some(value) => {
                1u8.serialize(out);
                value.serialize_ref(out);
            }
        }
    }

    #[inline(always)]
    fn serialize_ref_at<B: Behavior>(&self, out: &mut bytevec::Builder<B>, version_idx: u32) {
        match self {
            None => {
                0u8.serialize(out);
            }
            Some(value) => {
                1u8.serialize(out);
                value.serialize_ref_at(out, version_idx);
            }
        }
    }
}

impl<T: SerializeRef + ?Sized> SerializeRef for Box<T> {
    #[inline(always)]
    fn serialize_ref<B: Behavior>(&self, out: &mut bytevec::Builder<B>) {
        (**self).serialize_ref(out);
    }

    #[inline(always)]
    fn serialize_ref_at<B: Behavior>(&self, out: &mut bytevec::Builder<B>, version_idx: u32) {
        (**self).serialize_ref_at(out, version_idx);
    }
}

impl SerializeRef for Bytes {
    fn serialize_ref<B: Behavior>(&self, out: &mut bytevec::Builder<B>) {
        self.len().serialize(out);
        // `Bytes::clone` is a refcount bump, so the payload is shared, not copied.
        out.put_bytes(self.clone());
    }
}

impl SerializeRef for ByteVec {
    fn serialize_ref<B: Behavior>(&self, out: &mut bytevec::Builder<B>) {
        self.len().serialize(out);
        // `extend` shares the chunks (refcount bumps), matching by-value `append`.
        out.extend(self);
    }
}

impl<O: bytevec::tagged::Owner> SerializeRef for bytevec::Tagged<O> {
    fn serialize_ref<B: Behavior>(&self, out: &mut bytevec::Builder<B>) {
        let chunk: &ByteVec = self;
        chunk.serialize_ref(out);
    }
}

/// Same encoding as `Vec<T>`: a `usize` length, then each element, with
/// `[u8]` written as one slice.
impl<T: 'static + SerializeRef> SerializeRef for [T] {
    fn serialize_ref<B: Behavior>(&self, out: &mut bytevec::Builder<B>) {
        self.len().serialize(out);
        if TypeId::of::<T>() == TypeId::of::<u8>() {
            // SAFETY: `T` is `u8`, so the slice is a `[u8]` of the same length.
            let bytes =
                unsafe { core::slice::from_raw_parts(self.as_ptr() as *const u8, self.len()) };
            out.put_slice(bytes);
        } else {
            for v in self {
                v.serialize_ref(out);
            }
        }
    }
}

impl<T: 'static + SerializeRef> SerializeRef for Vec<T> {
    #[inline(always)]
    fn serialize_ref<B: Behavior>(&self, out: &mut bytevec::Builder<B>) {
        self.as_slice().serialize_ref(out)
    }
}

impl<T: SerializeRef> SerializeRef for VecDeque<T> {
    fn serialize_ref<B: Behavior>(&self, out: &mut bytevec::Builder<B>) {
        self.len().serialize(out);
        for v in self {
            v.serialize_ref(out);
        }
    }
}

impl<K: SerializeRef, V: SerializeRef> SerializeRef for BTreeMap<K, V> {
    fn serialize_ref<B: Behavior>(&self, out: &mut bytevec::Builder<B>) {
        self.len().serialize(out);
        for (k, v) in self {
            k.serialize_ref(out);
            v.serialize_ref(out);
        }
    }
}

/// Same encoding as `String`: a `usize` length, then the UTF-8 bytes.
impl SerializeRef for str {
    fn serialize_ref<B: Behavior>(&self, out: &mut bytevec::Builder<B>) {
        self.len().serialize(out);
        out.put_slice(self.as_bytes());
    }
}

impl SerializeRef for String {
    #[inline(always)]
    fn serialize_ref<B: Behavior>(&self, out: &mut bytevec::Builder<B>) {
        self.as_str().serialize_ref(out)
    }
}

impl SerializeRef for std::sync::Arc<str> {
    #[inline(always)]
    fn serialize_ref<B: Behavior>(&self, out: &mut bytevec::Builder<B>) {
        (**self).serialize_ref(out)
    }
}

impl<T: SerializeRef, E: SerializeRef> SerializeRef for Result<T, E> {
    fn serialize_ref<B: Behavior>(&self, out: &mut bytevec::Builder<B>) {
        match self {
            Ok(value) => {
                0u8.serialize(out);
                value.serialize_ref(out);
            }
            Err(err) => {
                1u8.serialize(out);
                err.serialize_ref(out);
            }
        }
    }

    #[inline(always)]
    fn serialize_ref_at<B: Behavior>(&self, out: &mut bytevec::Builder<B>, version_idx: u32) {
        match self {
            Ok(value) => {
                0u8.serialize(out);
                value.serialize_ref_at(out, version_idx);
            }
            Err(err) => {
                1u8.serialize(out);
                err.serialize_ref(out);
            }
        }
    }
}

macro_rules! impl_tuple_serialize_ref {
    () => {
        impl SerializeRef for () {
            fn serialize_ref<B: Behavior>(&self, _: &mut bytevec::Builder<B>) {}
        }
    };
    ($t0:ident $(,$rest:ident)*) => {
        impl_tuple_serialize_ref!($($rest),*);

        impl<$t0: SerializeRef, $($rest: SerializeRef),*> SerializeRef for ($t0, $($rest),*) {
            #[allow(non_snake_case)]
            fn serialize_ref<B: Behavior>(&self, out: &mut bytevec::Builder<B>) {
                let ($t0, $($rest),*) = self;
                $t0.serialize_ref(out);
                $($rest.serialize_ref(out);)*
            }
        }
    };
}

impl_tuple_serialize_ref!(
    T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15
);
