use crate::{Deserialize, Serialize, Wire, type_of};
use core::time::Duration;

impl Wire for Duration {
    const TYPE: crate::Type = crate::Type {
        name: "std::time::Duration",
        id: crate::Type::STRUCT,
        children: &[*type_of::<u64>(), *type_of::<u32>()],
    };
}

impl Deserialize for Duration {
    fn deserialize(bytes: &mut etude_bytevec::ByteVec) -> Result<Self, crate::DeserializeError> {
        let secs = u64::deserialize(bytes)?;
        let subsecs = u32::deserialize(bytes)?;
        Ok(Duration::new(secs, subsecs))
    }
}

crate::ser::impl_serialize_ref_copy!(Duration);

impl Serialize for Duration {
    fn serialize<B: etude_bytevec::Behavior>(self, out: &mut etude_bytevec::Builder<B>) {
        self.as_secs().serialize(out);
        self.subsec_nanos().serialize(out);
    }
}
