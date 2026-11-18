//! Graph-owned dependency container handles keep resolved siblings readable.
use std::sync::Arc;
use unityrs_core::{
    ObjectRef,
    env::{Environment, PPtr},
    files::File,
    limits::{Meter, MeteredVec},
};

use crate::SceneError;

#[derive(Debug)]
pub(crate) struct Held(MeteredVec<File>);
impl Held {
    pub(crate) fn new(meter: &Arc<Meter>) -> Self {
        Self(MeteredVec::new(meter))
    }
    pub(crate) fn resolve(
        &mut self,
        env: &Environment,
        from: &ObjectRef,
        pointer: PPtr,
        meter: &Meter,
    ) -> Result<ObjectRef, SceneError> {
        let (object, owner) = env.deref_held(from.file(), pointer, meter)?;
        if let Some(owner) = owner {
            meter.count_decode(self.0.len() as u64)?;
            if !self.0.as_slice().iter().any(|old| same_handle(old, &owner)) {
                self.0.push(owner)?;
            }
        }
        Ok(object)
    }
}

fn same_handle(a: &File, b: &File) -> bool {
    match (a, b) {
        (File::Bundle(a), File::Bundle(b)) => Arc::ptr_eq(a, b),
        (File::Web(a), File::Web(b)) => Arc::ptr_eq(a, b),
        (File::Serialized(a), File::Serialized(b)) => Arc::ptr_eq(a, b),
        (File::Raw(a), File::Raw(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}
