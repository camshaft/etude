use crate::{ByteVec, Deserialize, Serialize, SerializeRef, Type, Wire, bytevec, de};
use std::io;

impl Deserialize for io::Error {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, de::Error> {
        let kind = io::ErrorKind::deserialize(bytes)?;
        let message = String::deserialize(bytes)?;
        Ok(io::Error::new(kind, message))
    }
}

impl Deserialize for io::ErrorKind {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, de::Error> {
        let tag = u8::deserialize(bytes)?;
        match tag {
            0 => Ok(io::ErrorKind::NotFound),
            1 => Ok(io::ErrorKind::PermissionDenied),
            2 => Ok(io::ErrorKind::ConnectionRefused),
            3 => Ok(io::ErrorKind::ConnectionReset),
            4 => Ok(io::ErrorKind::HostUnreachable),
            5 => Ok(io::ErrorKind::NetworkUnreachable),
            6 => Ok(io::ErrorKind::ConnectionAborted),
            7 => Ok(io::ErrorKind::NotConnected),
            8 => Ok(io::ErrorKind::AddrInUse),
            9 => Ok(io::ErrorKind::AddrNotAvailable),
            10 => Ok(io::ErrorKind::NetworkDown),
            11 => Ok(io::ErrorKind::BrokenPipe),
            12 => Ok(io::ErrorKind::AlreadyExists),
            13 => Ok(io::ErrorKind::WouldBlock),
            14 => Ok(io::ErrorKind::NotADirectory),
            15 => Ok(io::ErrorKind::IsADirectory),
            16 => Ok(io::ErrorKind::DirectoryNotEmpty),
            17 => Ok(io::ErrorKind::ReadOnlyFilesystem),
            18 => Ok(io::ErrorKind::StaleNetworkFileHandle),
            19 => Ok(io::ErrorKind::InvalidInput),
            20 => Ok(io::ErrorKind::InvalidData),
            21 => Ok(io::ErrorKind::TimedOut),
            22 => Ok(io::ErrorKind::WriteZero),
            23 => Ok(io::ErrorKind::StorageFull),
            24 => Ok(io::ErrorKind::NotSeekable),
            25 => Ok(io::ErrorKind::QuotaExceeded),
            26 => Ok(io::ErrorKind::FileTooLarge),
            27 => Ok(io::ErrorKind::ResourceBusy),
            28 => Ok(io::ErrorKind::ExecutableFileBusy),
            29 => Ok(io::ErrorKind::Deadlock),
            30 => Ok(io::ErrorKind::CrossesDevices),
            31 => Ok(io::ErrorKind::TooManyLinks),
            32 => Ok(io::ErrorKind::ArgumentListTooLong),
            33 => Ok(io::ErrorKind::Interrupted),
            34 => Ok(io::ErrorKind::Unsupported),
            35 => Ok(io::ErrorKind::UnexpectedEof),
            36 => Ok(io::ErrorKind::OutOfMemory),
            _ => Ok(io::ErrorKind::Other),
        }
    }
}

impl Wire for std::io::ErrorKind {
    const TYPE: Type = Type {
        name: "std::io::ErrorKind",
        id: Type::ENUM,
        children: &[u8::TYPE],
    };
}

impl Serialize for io::Error {
    fn serialize<B: bytevec::Behavior>(self, out: &mut bytevec::Builder<B>) {
        self.kind().serialize(out);
        self.to_string().serialize(out);
    }
}

impl SerializeRef for io::Error {
    fn serialize_ref<B: bytevec::Behavior>(&self, out: &mut bytevec::Builder<B>) {
        self.kind().serialize(out);
        self.to_string().serialize(out);
    }
}

crate::ser::impl_serialize_ref_copy!(io::ErrorKind);

impl Serialize for io::ErrorKind {
    fn serialize<B: bytevec::Behavior>(self, out: &mut bytevec::Builder<B>) {
        match self {
            Self::NotFound => 0u8,
            Self::PermissionDenied => 1,
            Self::ConnectionRefused => 2,
            Self::ConnectionReset => 3,
            Self::HostUnreachable => 4,
            Self::NetworkUnreachable => 5,
            Self::ConnectionAborted => 6,
            Self::NotConnected => 7,
            Self::AddrInUse => 8,
            Self::AddrNotAvailable => 9,
            Self::NetworkDown => 10,
            Self::BrokenPipe => 11,
            Self::AlreadyExists => 12,
            Self::WouldBlock => 13,
            Self::NotADirectory => 14,
            Self::IsADirectory => 15,
            Self::DirectoryNotEmpty => 16,
            Self::ReadOnlyFilesystem => 17,
            Self::StaleNetworkFileHandle => 18,
            Self::InvalidInput => 19,
            Self::InvalidData => 20,
            Self::TimedOut => 21,
            Self::WriteZero => 22,
            Self::StorageFull => 23,
            Self::NotSeekable => 24,
            Self::QuotaExceeded => 25,
            Self::FileTooLarge => 26,
            Self::ResourceBusy => 27,
            Self::ExecutableFileBusy => 28,
            Self::Deadlock => 29,
            Self::CrossesDevices => 30,
            Self::TooManyLinks => 31,
            Self::ArgumentListTooLong => 32,
            Self::Interrupted => 33,
            Self::Unsupported => 34,
            Self::UnexpectedEof => 35,
            Self::OutOfMemory => 36,
            _ => 255,
        }
        .serialize(out);
    }
}

impl Wire for std::io::Error {
    const TYPE: Type = Type {
        name: "std::io::Error",
        id: Type::STRUCT,
        children: &[std::io::ErrorKind::TYPE, String::TYPE],
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    generate_deserialize_fuzz_test!(fuzz_error_kind, io::ErrorKind);
    generate_deserialize_fuzz_test!(fuzz_error, io::Error);
}
