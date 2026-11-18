//! File addresses for a sealed view. Entries keep no file or bundle alive.
use std::sync::Arc;
use unityrs_core::{
    ObjectRef,
    files::File,
    limits::{ChargeError, Meter, MeteredVec},
};

use super::{ExportError, walk_files};

struct Occurrence {
    identity: usize,
    path: MeteredVec<usize>,
    // A dependency's identity belongs to its memory account across reopens.
    // Keep the account alive so its address cannot be reused.
    _account: Arc<Meter>,
}
impl Occurrence {
    fn matches(&self, object: &ObjectRef, meter: &Meter) -> Result<bool, ChargeError> {
        let (identity, path) = object.file().occurrence_key();
        meter.count_decode(1 + self.path.len().min(path.len()) as u64)?;
        Ok(identity == self.identity && path == self.path.as_slice())
    }
}

/// A private view has the staged input first and the dependencies in id order.
/// The registered copy of the input has the same file indices as the own open.
pub struct FileIndex {
    own: MeteredVec<Option<Occurrence>>,
    registered_own: MeteredVec<Option<Occurrence>>,
    dependencies: MeteredVec<Option<Occurrence>>,
}
impl FileIndex {
    #[must_use]
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            own: MeteredVec::new(meter),
            registered_own: MeteredVec::new(meter),
            dependencies: MeteredVec::new(meter),
        }
    }

    pub fn add(
        &mut self,
        root: File,
        own: bool,
        registered: bool,
        meter: &Arc<Meter>,
    ) -> Result<(), ExportError> {
        let rows = match (own, registered) {
            (true, false) => &mut self.own,
            (true, true) => &mut self.registered_own,
            (false, _) => &mut self.dependencies,
        };
        walk_files(root, meter, |file| {
            let entry = if let File::Serialized(serialized) = file {
                let (identity, path) = serialized.occurrence_key();
                let mut stored = MeteredVec::with_capacity(meter, path.len())?;
                stored.extend_from_slice(path)?;
                Some(Occurrence {
                    identity,
                    path: stored,
                    _account: serialized.meter().clone(),
                })
            } else {
                None
            };
            rows.push(entry)?;
            Ok(())
        })
    }

    pub(super) fn contains(&self, index: u32) -> bool {
        let index = index as usize;
        if let Some(dep) = index.checked_sub(self.own.len()) {
            dep < self.dependencies.len()
        } else {
            index < self.own.len()
        }
    }

    pub(super) fn matches(
        &self,
        index: u32,
        object: &ObjectRef,
        meter: &Meter,
    ) -> Result<bool, ChargeError> {
        let index = index as usize;
        if let Some(dep) = index.checked_sub(self.own.len()) {
            return Self::at(&self.dependencies, dep, object, meter);
        }
        Ok(Self::at(&self.own, index, object, meter)?
            || Self::at(&self.registered_own, index, object, meter)?)
    }

    fn at(
        rows: &MeteredVec<Option<Occurrence>>,
        index: usize,
        object: &ObjectRef,
        meter: &Meter,
    ) -> Result<bool, ChargeError> {
        rows.as_slice()
            .get(index)
            .and_then(Option::as_ref)
            .map_or(Ok(false), |file| file.matches(object, meter))
    }
}
