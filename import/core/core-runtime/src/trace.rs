use bolero_generator::TypeGenerator;
use upstream_buffer::writer::Storage;
use upstream_bytevec::{Behavior, Builder, ByteVec};
use upstream_wire::{self as wire, Deserialize, Serialize, Wire, type_of};
use std::fmt;

const LEN: usize = 16;

/// The ID of the entire request, end-to-end.
///
/// Renders as a fixed-width 32-character lower-hex string via `Display`,
/// matching the W3C / X-Ray trace-id convention. Use this exact form for
/// log queries (`| filter trace_id = "<32-hex>"`).
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Trace([u8; LEN]);

impl fmt::Debug for Trace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Trace(0x{self})")
    }
}

impl Trace {
    pub fn new(id: [u8; LEN]) -> Self {
        Self(id)
    }

    pub fn empty() -> Self {
        Self([0; LEN])
    }

    /// The raw 16-byte trace id, for recording into fixed-layout sinks (e.g.
    /// backbeat events) that need the bytes rather than the hex `Display` form.
    pub fn to_bytes(self) -> [u8; LEN] {
        self.0
    }
}

impl fmt::Display for Trace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl TypeGenerator for Trace {
    fn generate<D: bolero_generator::Driver>(driver: &mut D) -> Option<Self> {
        Some(Self(driver.produce()?))
    }
}

/// The ID of a single hop within a request, equivalent to an X-Ray segment id.
///
/// Renders as a fixed-width 16-character lower-hex string via `Display`,
/// matching the wire encoding (`u64::to_be_bytes`) and what we set in the
/// `x-upstream-parent-span` HTTP header. Use this for span-id log queries.
#[derive(Copy, Clone, Default, PartialEq, Eq)]
pub struct Span(u64);

impl Span {
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn into_u64(self) -> u64 {
        self.0
    }
}

impl From<u64> for Span {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Span(0x{self})")
    }
}

impl TypeGenerator for Span {
    fn generate<D: bolero_generator::Driver>(driver: &mut D) -> Option<Self> {
        Some(Self(driver.produce()?))
    }
}

impl Wire for Trace {
    const TYPE: wire::Type = wire::Type {
        name: "Trace",
        id: type_of::<u128>().id,
        children: type_of::<u128>().children,
    };
}

impl Serialize for Trace {
    fn serialize<B: Behavior>(self, out: &mut Builder<B>) {
        out.put_slice(&self.0);
    }
}

impl Deserialize for Trace {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, wire::DeserializeError> {
        let value = Deserialize::deserialize(bytes).map_err(|_| {
            wire::DeserializeError::new(wire::de::ErrorKind::InvalidLength, type_of::<Self>())
        })?;
        Ok(Self(value))
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags(u8);

impl Flags {
    pub const NONE: Self = Self(0);
    pub const SAMPLED: Self = Self(0x01);

    pub const fn is_sampled(&self) -> bool {
        (self.0 & Self::SAMPLED.0) != 0
    }
}

impl Wire for Flags {
    const TYPE: wire::Type = wire::Type {
        name: "TraceFlags",
        id: type_of::<u8>().id,
        children: type_of::<u8>().children,
    };
}

impl Serialize for Flags {
    fn serialize<B: Behavior>(self, out: &mut Builder<B>) {
        out.put_slice(&[self.0]);
    }
}

impl Deserialize for Flags {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, wire::DeserializeError> {
        let value = Deserialize::deserialize(bytes).map_err(|_| {
            wire::DeserializeError::new(wire::de::ErrorKind::InvalidLength, type_of::<Self>())
        })?;
        Ok(Self(value))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Wire)]
#[wire(crate = upstream_wire)]
pub struct Header {
    pub trace: Trace,
    pub flags: Flags,
    pub span_id: u64,
}

impl Header {
    pub const WIRE_SIZE: usize = LEN + 1 + 8;
}

/// The parent span id — the sender's span_id from the inbound wire header.
/// Wraps `Option<Span>` with a `Display` impl that renders as hex or empty.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ParentSpan(pub Option<Span>);

impl fmt::Display for ParentSpan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(s) => s.fmt(f),
            None => Ok(()),
        }
    }
}
