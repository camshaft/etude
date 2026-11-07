use crate::{ByteVec, Deserialize, Serialize, SerializeRef, Type, Wire, bytevec, de, type_of};
use std::ops::{Bound, Range, RangeInclusive};

impl<T: Deserialize> Deserialize for Range<T> {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, de::Error> {
        let start = T::deserialize(bytes)?;
        let end = T::deserialize(bytes)?;
        Ok(start..end)
    }
}

impl<T: Serialize> Serialize for Range<T> {
    fn serialize<B: bytevec::Behavior>(self, out: &mut bytevec::Builder<B>) {
        self.start.serialize(out);
        self.end.serialize(out);
    }
}

impl<T: SerializeRef> SerializeRef for Range<T> {
    fn serialize_ref<B: bytevec::Behavior>(&self, out: &mut bytevec::Builder<B>) {
        self.start.serialize_ref(out);
        self.end.serialize_ref(out);
    }
}

impl<T: Wire> Wire for Range<T> {
    const TYPE: Type = Type {
        name: "std::ops::Range",
        id: Type::STRUCT,
        children: &[T::TYPE, T::TYPE],
    };
}

impl<T: Deserialize> Deserialize for RangeInclusive<T> {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, de::Error> {
        let start = T::deserialize(bytes)?;
        let end = T::deserialize(bytes)?;
        Ok(start..=end)
    }
}

impl<T: Serialize> Serialize for RangeInclusive<T> {
    fn serialize<B: bytevec::Behavior>(self, out: &mut bytevec::Builder<B>) {
        let (start, end) = self.into_inner();
        start.serialize(out);
        end.serialize(out);
    }
}

impl<T: SerializeRef> SerializeRef for RangeInclusive<T> {
    fn serialize_ref<B: bytevec::Behavior>(&self, out: &mut bytevec::Builder<B>) {
        self.start().serialize_ref(out);
        self.end().serialize_ref(out);
    }
}

impl<T: Wire> Wire for RangeInclusive<T> {
    const TYPE: Type = Type {
        name: "std::ops::RangeInclusive",
        id: Type::STRUCT,
        children: &[T::TYPE, T::TYPE],
    };
}

impl<T: Deserialize> Deserialize for Bound<T> {
    fn deserialize(bytes: &mut ByteVec) -> Result<Self, de::Error> {
        let tag = u8::deserialize(bytes)?;
        Ok(match tag {
            0 => Bound::Unbounded,
            1 => Bound::Excluded(T::deserialize(bytes)?),
            2 => Bound::Included(T::deserialize(bytes)?),
            _ => {
                return Err(de::Error::new(
                    de::ErrorKind::InvalidVariant,
                    type_of::<Self>(),
                ));
            }
        })
    }
}

impl<T: Serialize> Serialize for Bound<T> {
    fn serialize<B: bytevec::Behavior>(self, out: &mut bytevec::Builder<B>) {
        match self {
            Bound::Unbounded => 0u8.serialize(out),
            Bound::Excluded(value) => {
                1u8.serialize(out);
                value.serialize(out);
            }
            Bound::Included(value) => {
                2u8.serialize(out);
                value.serialize(out);
            }
        }
    }
}

impl<T: SerializeRef> SerializeRef for Bound<T> {
    fn serialize_ref<B: bytevec::Behavior>(&self, out: &mut bytevec::Builder<B>) {
        match self {
            Bound::Unbounded => 0u8.serialize(out),
            Bound::Excluded(value) => {
                1u8.serialize(out);
                value.serialize_ref(out);
            }
            Bound::Included(value) => {
                2u8.serialize(out);
                value.serialize_ref(out);
            }
        }
    }
}

impl<T: Wire> Wire for Bound<T> {
    const TYPE: Type = Type {
        name: "std::ops::Bound",
        id: Type::ENUM,
        children: &[
            Type {
                name: "Unbounded",
                id: 0,
                children: &[],
            },
            Type {
                name: "Excluded",
                id: 1,
                children: &[T::TYPE],
            },
            Type {
                name: "Included",
                id: 2,
                children: &[T::TYPE],
            },
        ],
    };
}
