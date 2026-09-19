// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

/// An error from a buffer operation.
///
/// The `E` parameter is the wrapped source error; it defaults to [`core::convert::Infallible`]
/// for operations whose only failures are the buffer-level [`OutOfRange`](Error::OutOfRange) /
/// [`InvalidFin`](Error::InvalidFin) variants.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Error<E = core::convert::Infallible> {
    /// A write extended past the buffer's maximum offset.
    OutOfRange,
    /// A final offset was set that conflicts with the buffer's current state.
    InvalidFin,
    /// The wrapped reader failed; carries its error.
    ReaderError(E),
}

impl<E> From<E> for Error<E> {
    #[inline]
    fn from(reader: E) -> Self {
        Self::ReaderError(reader)
    }
}

impl Error {
    /// Maps from an infallible error into a more specific error
    #[inline]
    pub fn mapped<E>(error: Error) -> Error<E> {
        match error {
            Error::OutOfRange => Error::OutOfRange,
            Error::InvalidFin => Error::InvalidFin,
            Error::ReaderError(_) => unreachable!(),
        }
    }
}

#[cfg(feature = "std")]
impl<E: std::error::Error> std::error::Error for Error<E> {}

impl<E: core::fmt::Display> core::fmt::Display for Error<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            Self::OutOfRange => write!(f, "write extends out of the maximum possible offset"),
            Self::InvalidFin => write!(
                f,
                "write modifies the final offset in a non-compliant manner"
            ),
            Self::ReaderError(reader) => write!(f, "the provided reader failed with: {reader}"),
        }
    }
}

#[cfg(feature = "std")]
impl<E: 'static + std::error::Error + Send + Sync> From<Error<E>> for std::io::Error {
    #[inline]
    fn from(error: Error<E>) -> Self {
        let kind = match &error {
            Error::OutOfRange => std::io::ErrorKind::InvalidData,
            Error::InvalidFin => std::io::ErrorKind::InvalidData,
            Error::ReaderError(_) => std::io::ErrorKind::Other,
        };
        Self::new(kind, error)
    }
}
