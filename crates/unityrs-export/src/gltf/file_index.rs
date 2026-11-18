//! File addresses in inspection order, opened only for an explicit selection.
use std::sync::Arc;

use unityrs_anim::Discovery;
use unityrs_core::{
    ObjectRef,
    env::{DependencyFile, FileOccurrence},
    files::{BundleFile, File, SerializedFile},
    limits::{ChargeError, Meter, MeteredVec},
};
use unityrs_gltf::ObjectAddress;

use super::ExportError;

/// The input comes first, followed by view members in ID order. Keep only
/// reopen handles so collecting one prefab never pins the rest of the view.
pub struct FileIndex {
    registered_own: Option<DependencyFile>,
    dependencies: MeteredVec<DependencyFile>,
}

impl FileIndex {
    #[must_use]
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            registered_own: None,
            dependencies: MeteredVec::new(meter),
        }
    }

    pub fn add(&mut self, handle: DependencyFile, own: bool) -> Result<(), ChargeError> {
        if own {
            self.registered_own = Some(handle);
            Ok(())
        } else {
            self.dependencies.push(handle)
        }
    }

    pub(super) fn object(
        &self,
        own: &Arc<BundleFile>,
        discovery: &Discovery,
        address: ObjectAddress,
        meter: &Arc<Meter>,
    ) -> Result<ObjectRef, ExportError> {
        let mut index = address.file_index as usize;
        let own_location = locate(File::Bundle(own.clone()), &mut index, meter)?;
        let (location, alias) = if matches!(own_location, Location::Beyond) {
            (self.dependency_location(index, meter)?, None)
        } else {
            (own_location, self.registered_own.as_ref())
        };
        let Location::Serialized { file, path } = location else {
            return Err(if matches!(location, Location::Beyond) {
                ExportError::OutsideSelectionView(address)
            } else {
                ExportError::MissingClipSelection
            });
        };
        let occurrence = FileOccurrence::of(&file);
        for rig in discovery.rigs() {
            let objects = [&rig.root, &rig.component]
                .into_iter()
                .chain(rig.slots().iter().flat_map(|slot| {
                    [
                        Some(&slot.source.owner),
                        slot.replacement.as_ref().map(|source| &source.owner),
                        slot.clip().ok(),
                    ]
                    .into_iter()
                    .flatten()
                }))
                .chain(rig.issues().iter().map(|issue| &issue.source.owner));
            for object in objects {
                meter.count_decode(1)?;
                if object.path_id() != address.path_id {
                    continue;
                }
                let candidate = FileOccurrence::of(object.file());
                meter.count_decode(1 + path.len().min(candidate.path_len()) as u64)?;
                if candidate == occurrence
                    || alias.is_some_and(|own| {
                        own.owns(object.file())
                            && object.file().occurrence_key().1 == path.as_slice()
                    })
                {
                    return Ok(object.clone());
                }
            }
        }
        Err(ExportError::MissingClipSelection)
    }

    fn dependency_location(
        &self,
        mut index: usize,
        meter: &Arc<Meter>,
    ) -> Result<Location, ExportError> {
        for dep in self.dependencies.as_slice() {
            let start = index;
            let (location, remaining) = dep.read(|root| {
                // An eviction retry must restart at the same index.
                let mut remaining = start;
                match locate(root, &mut remaining, meter) {
                    Err(ExportError::Input(error)) => Err(error),
                    result => Ok(result.map(|location| (location, remaining))),
                }
            })??;
            if !matches!(location, Location::Beyond) {
                return Ok(location);
            }
            index = remaining;
        }
        Ok(Location::Beyond)
    }
}

enum Location {
    Beyond,
    Other,
    Serialized {
        file: Arc<SerializedFile>,
        path: MeteredVec<usize>,
    },
}

struct Frame {
    file: File,
    next: usize,
    dir_index: usize,
}

fn locate(root: File, index: &mut usize, meter: &Arc<Meter>) -> Result<Location, ExportError> {
    meter.count_decode(1)?;
    if *index == 0 {
        return Ok(Location::Other);
    }
    *index -= 1;
    let mut stack = MeteredVec::new(meter);
    stack.push(Frame {
        file: root,
        next: 0,
        dir_index: 0,
    })?;
    while let Some(frame) = stack.as_mut_slice().last_mut() {
        let Some(slot) = frame.file.container_files().get(frame.next) else {
            stack.pop();
            continue;
        };
        meter.count_decode(1)?;
        let dir_index = slot.dir_index;
        if *index == 0 {
            let File::Serialized(file) = frame.file.container_entry(dir_index)? else {
                return Ok(Location::Other);
            };
            let mut path = MeteredVec::with_capacity(meter, stack.len())?;
            for ancestor in stack.as_slice().iter().skip(1) {
                path.push(ancestor.dir_index)?;
            }
            path.push(dir_index)?;
            return Ok(Location::Serialized { file, path });
        }
        *index -= 1;
        frame.next += 1;
        // A preceding leaf contributes one position even if its data is
        // corrupt. Descend only when its children can change the index.
        if let Some(file) = frame.file.child_container(dir_index)? {
            stack.push(Frame {
                file,
                next: 0,
                dir_index,
            })?;
        }
    }
    Ok(Location::Beyond)
}
