use upstream_bytevec::ByteVec;
use upstream_macros::AsVariant;
use upstream_wire::{Wire, de};
use std::{fmt, io};

// TODO remove circular dependency on `service`
type GlobalId = u64;

#[derive(Clone, Debug, Wire)]
#[wire(crate = upstream_wire)]
pub struct Application {
    pub error_id: GlobalId,
    pub data: ByteVec,
}

impl fmt::Display for Application {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ERR-{:x}", self.error_id)
    }
}

impl std::error::Error for Application {}

#[derive(Clone, Copy, Debug, AsVariant)]
#[variant(crate = s2n_quic_core::event::metrics::aggregate)]
pub enum ErrorKind {
    Message,
    Wire,
    MethodNotSupported,
    Application,
    IoAddrInUse,
    IoAddrNotAvailable,
    IoAlreadyExists,
    IoArgumentListTooLong,
    IoBrokenPipe,
    IoConnectionAborted,
    IoConnectionRefused,
    IoConnectionReset,
    IoCrossesDevices,
    IoDeadlock,
    IoDirectoryNotEmpty,
    IoExecutableFileBusy,
    IoFilesystemLoop,
    IoFileTooLarge,
    IoHostUnreachable,
    IoInProgress,
    IoInterrupted,
    IoInvalidData,
    IoInvalidFilename,
    IoInvalidInput,
    IoIsADirectory,
    IoNetworkDown,
    IoNetworkUnreachable,
    IoNotADirectory,
    IoNotConnected,
    IoNotFound,
    IoNotSeekable,
    IoOther,
    IoOutOfMemory,
    IoPermissionDenied,
    IoQuotaExceeded,
    IoReadOnlyFilesystem,
    IoResourceBusy,
    IoStaleNetworkFileHandle,
    IoStorageFull,
    IoTimedOut,
    IoTooManyLinks,
    IoUncategorized,
    IoUnexpectedEof,
    IoUnsupported,
    IoWouldBlock,
    IoWriteZero,
}

impl From<io::ErrorKind> for ErrorKind {
    fn from(value: io::ErrorKind) -> Self {
        match value {
            io::ErrorKind::NotFound => Self::IoNotFound,
            io::ErrorKind::PermissionDenied => Self::IoPermissionDenied,
            io::ErrorKind::ConnectionRefused => Self::IoConnectionRefused,
            io::ErrorKind::ConnectionReset => Self::IoConnectionReset,
            io::ErrorKind::HostUnreachable => Self::IoHostUnreachable,
            io::ErrorKind::NetworkUnreachable => Self::IoNetworkUnreachable,
            io::ErrorKind::ConnectionAborted => Self::IoConnectionAborted,
            io::ErrorKind::NotConnected => Self::IoNotConnected,
            io::ErrorKind::AddrInUse => Self::IoAddrInUse,
            io::ErrorKind::AddrNotAvailable => Self::IoAddrNotAvailable,
            io::ErrorKind::NetworkDown => Self::IoNetworkDown,
            io::ErrorKind::BrokenPipe => Self::IoBrokenPipe,
            io::ErrorKind::AlreadyExists => Self::IoAlreadyExists,
            io::ErrorKind::WouldBlock => Self::IoWouldBlock,
            io::ErrorKind::NotADirectory => Self::IoNotADirectory,
            io::ErrorKind::IsADirectory => Self::IoIsADirectory,
            io::ErrorKind::DirectoryNotEmpty => Self::IoDirectoryNotEmpty,
            io::ErrorKind::ReadOnlyFilesystem => Self::IoReadOnlyFilesystem,
            io::ErrorKind::StaleNetworkFileHandle => Self::IoStaleNetworkFileHandle,
            io::ErrorKind::InvalidInput => Self::IoInvalidInput,
            io::ErrorKind::InvalidData => Self::IoInvalidData,
            io::ErrorKind::TimedOut => Self::IoTimedOut,
            io::ErrorKind::WriteZero => Self::IoWriteZero,
            io::ErrorKind::StorageFull => Self::IoStorageFull,
            io::ErrorKind::NotSeekable => Self::IoNotSeekable,
            io::ErrorKind::QuotaExceeded => Self::IoQuotaExceeded,
            io::ErrorKind::FileTooLarge => Self::IoFileTooLarge,
            io::ErrorKind::ResourceBusy => Self::IoResourceBusy,
            io::ErrorKind::ExecutableFileBusy => Self::IoExecutableFileBusy,
            io::ErrorKind::Deadlock => Self::IoDeadlock,
            io::ErrorKind::CrossesDevices => Self::IoCrossesDevices,
            io::ErrorKind::TooManyLinks => Self::IoTooManyLinks,
            io::ErrorKind::InvalidFilename => Self::IoInvalidFilename,
            io::ErrorKind::ArgumentListTooLong => Self::IoArgumentListTooLong,
            io::ErrorKind::Interrupted => Self::IoInterrupted,
            io::ErrorKind::Unsupported => Self::IoUnsupported,
            io::ErrorKind::UnexpectedEof => Self::IoUnexpectedEof,
            io::ErrorKind::OutOfMemory => Self::IoOutOfMemory,
            io::ErrorKind::Other => Self::IoOther,
            _ => Self::IoOther,
        }
    }
}

/// Represents errors that can occur in the transport layer
#[derive(Debug, Wire)]
#[wire(crate = upstream_wire)]
pub enum Error {
    /// A simple string message
    Message(String),
    /// An I/O error occurred
    Io(io::Error),
    /// Failed to serialize or deserialize data
    Wire(de::Error),
    /// The method is not supported
    MethodNotSupported(GlobalId),
    /// An application-level error
    Application(Application),
}

impl Error {
    pub fn message(msg: impl ToString) -> Self {
        Self::Message(msg.to_string())
    }

    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Message(_) => ErrorKind::Message,
            Self::Io(err) => err.kind().into(),
            Self::Wire(_) => ErrorKind::Wire,
            Self::MethodNotSupported(_) => ErrorKind::MethodNotSupported,
            Self::Application(_) => ErrorKind::Application,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Wire(err) => write!(f, "Serialization error: {err:?}"),
            Self::MethodNotSupported(id) => write!(f, "Method not supported: {id}"),
            Self::Application(err) => write!(f, "Application error: {err}"),
            Self::Message(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<Error> for io::Error {
    fn from(value: Error) -> Self {
        match value {
            Error::Io(error) => error,
            Error::Wire(deserialize_error) => {
                io::Error::new(io::ErrorKind::InvalidData, deserialize_error.to_string())
            }
            Error::MethodNotSupported(id) => io::Error::new(
                io::ErrorKind::Unsupported,
                format!("method {id} is not supported"),
            ),
            Error::Application(error) => io::Error::other(error),
            Error::Message(msg) => io::Error::other(msg),
        }
    }
}

impl From<de::Error> for Error {
    fn from(err: de::Error) -> Self {
        Self::Wire(err)
    }
}

impl From<upstream_time::Timeout> for Error {
    fn from(_: upstream_time::Timeout) -> Self {
        Self::Io(io::ErrorKind::TimedOut.into())
    }
}

impl<T: Into<Application>> From<T> for Error {
    fn from(err: T) -> Self {
        Self::Application(err.into())
    }
}
