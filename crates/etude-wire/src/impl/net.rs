use crate::{ByteVec, Deserialize, Serialize, Type, Wire, bytevec, de, type_of};
use std::net;

impl Deserialize for net::IpAddr {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, de::Error> {
        let tag = u8::deserialize(bytes)?;
        match tag {
            4 => Ok(net::IpAddr::V4(net::Ipv4Addr::deserialize(bytes)?)),
            6 => Ok(net::IpAddr::V6(net::Ipv6Addr::deserialize(bytes)?)),
            _ => Err(de::Error::new(
                de::ErrorKind::InvalidVariant,
                type_of::<Self>(),
            )),
        }
    }
}

crate::ser::impl_serialize_ref_copy!(net::IpAddr, net::Ipv4Addr, net::Ipv6Addr, net::SocketAddr);

impl Serialize for net::IpAddr {
    fn serialize<B: bytevec::Behavior>(self, out: &mut bytevec::Builder<B>) {
        match self {
            net::IpAddr::V4(addr) => {
                4u8.serialize(out); // V4 tag
                addr.serialize(out);
            }
            net::IpAddr::V6(addr) => {
                6u8.serialize(out); // V6 tag
                addr.serialize(out);
            }
        }
    }
}

impl Wire for std::net::IpAddr {
    const TYPE: Type = Type {
        name: "std::net::IpAddr",
        id: Type::ENUM,
        children: &[std::net::Ipv4Addr::TYPE, std::net::Ipv6Addr::TYPE],
    };
}

impl Deserialize for net::Ipv4Addr {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, de::Error> {
        let octets = <[u8; 4]>::deserialize(bytes)?;
        Ok(net::Ipv4Addr::from(octets))
    }
}

impl Serialize for net::Ipv4Addr {
    fn serialize<B: bytevec::Behavior>(self, out: &mut bytevec::Builder<B>) {
        self.octets().serialize(out);
    }
}

impl Wire for net::Ipv4Addr {
    const TYPE: Type = Type {
        name: "std::net::IpV4Addr",
        id: Type::STRUCT,
        children: &[<[u8; 4]>::TYPE],
    };
}

impl Deserialize for net::Ipv6Addr {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, de::Error> {
        let octets = <[u8; 16]>::deserialize(bytes)?;
        Ok(net::Ipv6Addr::from(octets))
    }
}

impl Serialize for net::Ipv6Addr {
    fn serialize<B: bytevec::Behavior>(self, out: &mut bytevec::Builder<B>) {
        self.octets().serialize(out);
    }
}

impl Wire for net::Ipv6Addr {
    const TYPE: Type = Type {
        name: "std::net::IpV6Addr",
        id: Type::STRUCT,
        children: &[<[u8; 16]>::TYPE],
    };
}

impl Deserialize for net::SocketAddr {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, de::Error> {
        let ip = net::IpAddr::deserialize(bytes)?;
        let port = u16::deserialize(bytes)?;
        Ok(net::SocketAddr::new(ip, port))
    }
}

impl Serialize for net::SocketAddr {
    fn serialize<B: bytevec::Behavior>(self, out: &mut bytevec::Builder<B>) {
        self.ip().serialize(out);
        self.port().serialize(out);
    }
}

impl Wire for net::SocketAddr {
    const TYPE: Type = Type {
        name: "std::net::SocketAddr",
        id: Type::STRUCT,
        children: &[net::IpAddr::TYPE, u16::TYPE],
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    generate_round_trip_test!(test_ipaddr_round_trip, net::IpAddr);
    generate_deserialize_fuzz_test!(fuzz_ipaddr_deserialize, net::IpAddr);
    generate_round_trip_test!(test_ipv4addr_round_trip, net::Ipv4Addr);
    generate_deserialize_fuzz_test!(fuzz_ipv4addr_deserialize, net::Ipv4Addr);
    generate_round_trip_test!(test_ipv6addr_round_trip, net::Ipv6Addr);
    generate_deserialize_fuzz_test!(fuzz_ipv6addr_deserialize, net::Ipv6Addr);
    generate_round_trip_test!(test_socketaddr_round_trip, net::SocketAddr);
    generate_deserialize_fuzz_test!(fuzz_socketaddr_deserialize, net::SocketAddr);

    // Network type snapshot tests
    generate_snapshot_test!(
        test_ipv4_snapshot,
        net::Ipv4Addr::new(127, 0, 0, 1),
        "ipv4_localhost"
    );
    generate_snapshot_test!(
        test_ipv6_snapshot,
        net::Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1),
        "ipv6_localhost"
    );
    generate_snapshot_test!(
        test_ipaddr_v4_snapshot,
        net::IpAddr::V4(net::Ipv4Addr::new(127, 0, 0, 1)),
        "ipaddr_v4"
    );
    generate_snapshot_test!(
        test_ipaddr_v6_snapshot,
        net::IpAddr::V6(net::Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)),
        "ipaddr_v6"
    );
    generate_snapshot_test!(
        test_socketaddr_v4_snapshot,
        net::SocketAddr::new(net::IpAddr::V4(net::Ipv4Addr::new(127, 0, 0, 1)), 8080),
        "socketaddr_v4"
    );
}
