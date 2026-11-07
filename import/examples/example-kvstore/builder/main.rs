use upstream_codegen::{Builder, Endpoint};

mod kvstore;

fn main() {
    Builder::new("kvstore")
        .with(Endpoint::new("server").provides(kvstore::kvstore(), "crate::server::KVStore"))
        .with(Endpoint::new("client").consumes(kvstore::kvstore()))
        .build("src/generated.rs");
}
