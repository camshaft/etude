// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use crate::type_of;
use bytes::buf::UninitSlice;
use core::fmt;
use etude_buffer::reader::Infallible;
use etude_bytevec::ByteVec;
use etude_wire_derive::Wire;
use std::{
    any::TypeId,
    collections::{BTreeMap, BTreeSet, VecDeque},
};

#[derive(Debug, Clone, Copy)]
pub struct Error {
    kind: ErrorKind,
    ty: &'static crate::Type,
}

impl Error {
    pub fn new(kind: ErrorKind, ty: &'static crate::Type) -> Self {
        Self { kind, ty }
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn ty(&self) -> &'static crate::Type {
        self.ty
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} for type {}", self.kind, self.ty.name)
    }
}

impl crate::Wire for Error {
    const TYPE: crate::Type = crate::Type {
        name: "etude_wire::de::Error",
        id: crate::Type::STRUCT,
        children: &[*type_of::<ErrorKind>()],
    };
}

impl Deserialize for Error {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        let kind = ErrorKind::deserialize(bytes)?;
        let ty = &crate::Type {
            name: "unknown",
            id: 0,
            children: &[],
        };
        Ok(Self { kind, ty })
    }
}

impl std::error::Error for Error {}

impl From<Error> for std::io::Error {
    fn from(value: Error) -> Self {
        Self::new(std::io::ErrorKind::InvalidData, value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Wire)]
#[cfg_attr(
    any(test, feature = "testing"),
    derive(bolero_generator::TypeGenerator)
)]
#[wire(crate = crate)]
pub enum ErrorKind {
    InvalidType,
    InvalidLength,
    InvalidUtf8,
    InvalidBytes,
    InvalidVariant,
    InvalidList,
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorKind::InvalidType => "Invalid type",
            ErrorKind::InvalidLength => "Invalid length",
            ErrorKind::InvalidUtf8 => "Invalid UTF-8",
            ErrorKind::InvalidBytes => "Invalid bytes",
            ErrorKind::InvalidVariant => "Invalid variant",
            ErrorKind::InvalidList => "Invalid list",
        }
        .fmt(f)
    }
}

/// Trait for types that can be deserialized
pub trait Deserialize: Sized + crate::Wire {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error>;

    /// Deserialize using a specific version index into `VERSION_TYPES`.
    ///
    /// Versioned types override this to dispatch to the version-specific
    /// deserializer. Non-versioned types use the default which ignores the
    /// index and calls `deserialize()`.
    ///
    /// Container types (`Option<T>`, `Vec<T>`, `Box<T>`) propagate the index
    /// to their inner type's `deserialize_at`.
    #[inline(always)]
    fn deserialize_at(bytes: &mut ByteVec, _version_idx: u32) -> Result<Self, Error> {
        Self::deserialize(bytes)
    }

    fn deserialize_with_len_prefix(bytes: &mut ByteVec) -> Result<Self, Error> {
        let mut chunk = ByteVec::deserialize(bytes)?;
        let value = Self::deserialize(&mut chunk)?;
        if !chunk.is_empty() {
            return Err(Error::new(ErrorKind::InvalidLength, &Self::TYPE));
        }
        Ok(value)
    }

    #[inline]
    fn deserialize_tagged(bytes: &mut ByteVec) -> Result<Self, Error>
    where
        Self: crate::Wire,
    {
        let id = u64::deserialize(bytes)?;
        if id != const { Self::TYPE.global_id() } {
            return Err(Error::new(ErrorKind::InvalidType, &Self::TYPE));
        }
        Self::deserialize(bytes)
    }
}

impl<T> Deserialize for Box<T>
where
    T: Deserialize,
{
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        Ok(Box::new(T::deserialize(bytes)?))
    }

    #[inline(always)]
    fn deserialize_at(bytes: &mut ByteVec, version_idx: u32) -> Result<Self, Error> {
        Ok(Box::new(T::deserialize_at(bytes, version_idx)?))
    }
}

impl Deserialize for core::convert::Infallible {
    fn deserialize(_bytes: &mut ByteVec) -> Result<Self, Error> {
        Err(Error::new(ErrorKind::InvalidType, type_of::<Self>()))
    }
}

impl<T> Deserialize for Option<T>
where
    T: Deserialize,
{
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        let tag = u8::deserialize(bytes)?;
        match tag {
            0 => Ok(None),
            1 => Ok(Some(T::deserialize(bytes)?)),
            _ => Err(Error::new(ErrorKind::InvalidVariant, type_of::<Self>())),
        }
    }

    #[inline(always)]
    fn deserialize_at(bytes: &mut ByteVec, version_idx: u32) -> Result<Self, Error> {
        let tag = u8::deserialize(bytes)?;
        match tag {
            0 => Ok(None),
            1 => Ok(Some(T::deserialize_at(bytes, version_idx)?)),
            _ => Err(Error::new(ErrorKind::InvalidVariant, type_of::<Self>())),
        }
    }
}

impl Deserialize for bytes::Bytes {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        let bytes: ByteVec = Deserialize::deserialize(bytes)?;
        Ok(bytes.copy_to_bytes())
    }
}

impl Deserialize for ByteVec {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        let len = usize::deserialize(bytes)?;
        let bytes = bytes
            .split_to(len)
            .map_err(|_| Error::new(ErrorKind::InvalidLength, type_of::<Self>()))?;
        Ok(bytes)
    }
}

impl<O> Deserialize for etude_bytevec::Tagged<O>
where
    O: etude_bytevec::tagged::Owner + Default,
{
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        let chunk = ByteVec::deserialize(bytes)?;
        Ok(chunk.into())
    }
}

impl<T: 'static + Deserialize> Deserialize for Vec<T> {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        let len = usize::deserialize(bytes)?;
        if len
            .checked_mul(size_of::<T>())
            .is_none_or(|v| v > u32::MAX as usize)
        {
            return Err(Error::new(ErrorKind::InvalidLength, type_of::<Self>()));
        }

        // The length is the peer's to set, so the bytes left bound what the vector reserves before any element
        // decodes; it grows past that bound only as its elements decode.
        let mut buf = Vec::with_capacity(len.min(bytes.len()));

        if TypeId::of::<T>() == TypeId::of::<u8>() {
            if bytes.len() < len {
                return Err(Error::new(ErrorKind::InvalidLength, type_of::<Self>()));
            }

            let data = buf.as_mut_ptr() as *mut _;
            let slice = unsafe { core::slice::from_raw_parts_mut(data, len) };
            let mut slice = UninitSlice::uninit(slice);
            bytes.infallible_copy_into(&mut slice);
            unsafe { buf.set_len(len) }
        } else {
            for _ in 0..len {
                buf.push(T::deserialize(bytes)?);
            }
        }
        Ok(buf)
    }
}

impl<T: Deserialize> Deserialize for VecDeque<T> {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        let len = usize::deserialize(bytes)?;
        // The length is the peer's to set, so the bytes left bound what the deque reserves before any element
        // decodes; it grows past that bound only as its elements decode.
        let mut deque = VecDeque::with_capacity(len.min(bytes.len()));
        for _ in 0..len {
            deque.push_back(T::deserialize(bytes)?);
        }
        Ok(deque)
    }
}

impl<K: Deserialize + Ord, V: Deserialize> Deserialize for BTreeMap<K, V> {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        let len = usize::deserialize(bytes)?;
        let mut map = BTreeMap::new();
        for _ in 0..len {
            let key = K::deserialize(bytes)?;
            let value = V::deserialize(bytes)?;
            map.insert(key, value);
        }
        Ok(map)
    }
}

impl<T: Deserialize + Ord> Deserialize for BTreeSet<T> {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        let len = usize::deserialize(bytes)?;
        let mut set = BTreeSet::new();
        for _ in 0..len {
            set.insert(T::deserialize(bytes)?);
        }
        Ok(set)
    }
}

impl Deserialize for String {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        let buf = <Vec<u8>>::deserialize(bytes)?;
        String::from_utf8(buf).map_err(|_| Error::new(ErrorKind::InvalidUtf8, type_of::<Self>()))
    }
}

impl Deserialize for std::sync::Arc<str> {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        let v = String::deserialize(bytes)?;
        Ok(v.into())
    }
}

impl<T, E> Deserialize for Result<T, E>
where
    T: Deserialize,
    E: Deserialize,
{
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        let tag = u8::deserialize(bytes)?;

        match tag {
            0 => {
                // Success response
                let value = T::deserialize(bytes)?;
                Ok(Ok(value))
            }
            1 => {
                // Error response
                let err = E::deserialize(bytes)?;
                Ok(Err(err))
            }
            _ => Err(Error::new(ErrorKind::InvalidVariant, type_of::<Self>())),
        }
    }

    /// Propagate `version_idx` to the inner success value's `deserialize_at`.
    ///
    /// The streaming framing layer decodes each frame as `Result<T>` (see
    /// `read_msg_with_len_at` in etude-rpc), so this override is
    /// what makes a negotiated-version streamed request actually decode at
    /// the negotiated layout. Without it the default falls through to
    /// `deserialize` → `T::deserialize` (current version), and a v1-shaped
    /// frame is decoded at the latest layout — over-reading the buffer and
    /// corrupting subsequent frames. Symmetric with `Result<T,E>::serialize_at`.
    fn deserialize_at(bytes: &mut ByteVec, version_idx: u32) -> Result<Self, Error> {
        let tag = u8::deserialize(bytes)?;

        match tag {
            0 => {
                let value = T::deserialize_at(bytes, version_idx)?;
                Ok(Ok(value))
            }
            1 => {
                // Errors are not version-negotiated.
                let err = E::deserialize(bytes)?;
                Ok(Err(err))
            }
            _ => Err(Error::new(ErrorKind::InvalidVariant, type_of::<Self>())),
        }
    }
}

impl Deserialize for bool {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        let v = <u8>::deserialize(bytes)?;
        if v == 0 { Ok(false) } else { Ok(true) }
    }
}

// Macro to implement Deserialize for integer types
macro_rules! impl_deserialize_for_int {
    ($ty:ty, $size:expr) => {
        impl_deserialize_for_int!($ty, $size, |buf| <$ty>::from_be_bytes(buf));
    };
    ($ty:ty, $size:expr, | $buf:ident | $convert:expr) => {
        impl Deserialize for $ty {
            fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
                if bytes.len() < $size {
                    return Err(Error::new(ErrorKind::InvalidLength, type_of::<Self>()));
                }
                let mut $buf = [0u8; $size];
                bytes.infallible_copy_into(&mut &mut $buf[..]);
                Ok($convert)
            }
        }
    };
}

// Implement Deserialize for all other integer types
impl_deserialize_for_int!(u8, 1, |buf| buf[0]);
impl_deserialize_for_int!(u16, 2);
impl_deserialize_for_int!(u32, 4);
impl_deserialize_for_int!(u64, 8);
impl_deserialize_for_int!(u128, 16);

impl Deserialize for usize {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        let value = u64::deserialize(bytes)?;
        let value = value
            .try_into()
            .map_err(|_| Error::new(ErrorKind::InvalidType, type_of::<Self>()))?;
        Ok(value)
    }
}

impl_deserialize_for_int!(i8, 1, |buf| buf[0] as _);
impl_deserialize_for_int!(i16, 2);
impl_deserialize_for_int!(i32, 4);
impl_deserialize_for_int!(i64, 8);
impl_deserialize_for_int!(i128, 16);

impl Deserialize for isize {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        let value = i64::deserialize(bytes)?;
        let value = value
            .try_into()
            .map_err(|_| Error::new(ErrorKind::InvalidType, type_of::<Self>()))?;
        Ok(value)
    }
}

// Floats are read back from their big-endian IEEE-754 bytes.
impl Deserialize for f32 {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        if bytes.len() < 4 {
            return Err(Error::new(ErrorKind::InvalidLength, type_of::<Self>()));
        }
        let mut buf = [0u8; 4];
        bytes.infallible_copy_into(&mut &mut buf[..]);
        Ok(f32::from_be_bytes(buf))
    }
}

impl Deserialize for f64 {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        if bytes.len() < 8 {
            return Err(Error::new(ErrorKind::InvalidLength, type_of::<Self>()));
        }
        let mut buf = [0u8; 8];
        bytes.infallible_copy_into(&mut &mut buf[..]);
        Ok(f64::from_be_bytes(buf))
    }
}

impl<const N: usize> Deserialize for [u8; N] {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
        if bytes.len() < N {
            return Err(Error::new(ErrorKind::InvalidLength, type_of::<Self>()));
        }
        let mut buf = [0u8; N];
        bytes.infallible_copy_into(&mut &mut buf[..]);
        Ok(buf)
    }
}

macro_rules! impl_tuple_deserialize {
    // Base case - empty tuple
    () => {
        impl Deserialize for () {
            fn deserialize(_: &mut ByteVec) -> Result<Self, Error> {
                Ok(())
            }
        }
    };
    // Recursive case - tuple with at least one element
    ($t0:ident $(,$rest:ident)*) => {
        impl_tuple_deserialize!($($rest),*);

        impl<$t0: Deserialize, $($rest: Deserialize),*> Deserialize for ($t0, $($rest),*) {
            #[allow(non_snake_case)]
            fn deserialize(bytes: &mut ByteVec) -> Result<Self, Error> {
                let $t0 = $t0::deserialize(bytes)?;
                $(let $rest = $rest::deserialize(bytes)?;)*
                Ok(($t0, $($rest),*))
            }
        }
    };
}

// Implement for tuples up to 16 elements
impl_tuple_deserialize!(
    T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15
);
