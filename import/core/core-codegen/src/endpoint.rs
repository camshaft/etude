use crate::{emit, service::Service};
use quote::quote;
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug)]
pub struct Endpoint {
    pub name: String,
    pub consumes: BTreeMap<String, Service>,
    pub provides: BTreeMap<String, (Service, String)>,
}

impl Endpoint {
    pub fn new<N: AsRef<str>>(name: N) -> Self {
        Self {
            name: name.as_ref().to_string(),
            consumes: BTreeMap::new(),
            provides: BTreeMap::new(),
        }
    }

    pub fn consumes(mut self, service: Service) -> Self {
        self.consumes.insert(service.namespace.clone(), service);
        self
    }

    pub fn provides<P: AsRef<str>>(mut self, service: Service, impl_path: P) -> Self {
        self.provides.insert(
            service.namespace.clone(),
            (service, impl_path.as_ref().to_string()),
        );
        self
    }

    pub(crate) fn build<P: AsRef<Path>>(&self, dir: P) {
        let dir = dir.as_ref();

        let path = dir.join(format!("{}.rs", self.name));
        emit::rust(
            &path,
            quote! {
                pub fn todo() {}
            },
        )
        .unwrap();
    }
}
