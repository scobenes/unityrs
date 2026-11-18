//! Selection uses the same file walk as the inspection and replay crosswalk.
use super::{Arc, Discovery, ExportError, File, FileIndex, Meter, MeteredVec, ObjectRef, same};
use unityrs_anim::{ClipSelector, RigClips, SelectionError};
use unityrs_core::{files::SerializedFile, limits::ChargeError};
use unityrs_gltf::{ClipAddress, Clips, ObjectAddress};

/// Visit root and descendant files in depth-first inspection order.
pub fn walk_files<E: From<ChargeError> + From<unityrs_core::Error>>(
    root: File,
    meter: &Arc<Meter>,
    mut visit: impl FnMut(&File) -> Result<(), E>,
) -> Result<(), E> {
    let mut stack = MeteredVec::new(meter);
    stack.push((root, 0usize))?;
    while let Some((file, next)) = stack.as_mut_slice().last_mut() {
        if *next == 0 {
            meter.count_decode(1)?;
            visit(file)?;
        }
        if let Some(slot) = file.container_files().get(*next) {
            let child = file.container_entry(slot.dir_index)?;
            *next += 1;
            stack.push((child, 0))?;
        } else {
            stack.pop();
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) enum Addresses<'a> {
    Loaded(&'a [File]),
    Indexed(&'a FileIndex),
}

pub(super) fn clips<'a>(
    discovery: &'a Discovery,
    roots: Addresses<'_>,
    policy: &Clips,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<(&'a RigClips, &'a ObjectRef)>, ExportError> {
    let mut selected = MeteredVec::new(meter);
    match policy {
        Clips::Static => (),
        Clips::AllDiscovered => {
            for rig in discovery.rigs() {
                meter.count_decode(1 + rig.slots().len() as u64)?;
                for slot in rig.clips() {
                    selected.push((rig, slot.clip().map_err(unresolved)?))?;
                }
            }
        }
        Clips::Explicit(requests) => {
            let mut files = MeteredVec::new(meter);
            if let Addresses::Loaded(roots) = roots {
                for root in roots {
                    walk_files(root.clone(), meter, |file| -> Result<(), ExportError> {
                        files.push(match file {
                            File::Serialized(file) => Some(file.clone()),
                            _ => None,
                        })?;
                        Ok(())
                    })?;
                }
            }
            let object = |address: ObjectAddress| match roots {
                Addresses::Indexed(index) => {
                    address_from_discovery(discovery, index, address, meter)
                }
                Addresses::Loaded(_) => files
                    .as_slice()
                    .get(address.file_index as usize)
                    .and_then(Option::<Arc<SerializedFile>>::as_ref)
                    .and_then(|file| file.object(address.path_id))
                    .ok_or(ExportError::MissingClipSelection),
            };
            for request in requests.as_slice() {
                let root = object(request.root)?;
                let component = object(request.component)?;
                let target;
                let selector = match &request.clip {
                    ClipAddress::Resolved(address) => {
                        let inside = match roots {
                            Addresses::Loaded(_) => (address.file_index as usize) < files.len(),
                            Addresses::Indexed(index) => index.contains(address.file_index),
                        };
                        if !inside {
                            return Err(ExportError::OutsideSelectionView(*address));
                        }
                        target = object(*address)?;
                        ClipSelector::Clip(&target)
                    }
                    ClipAddress::Source {
                        owner,
                        field,
                        ordinal,
                    } => {
                        target = object(*owner)?;
                        ClipSelector::SourceAddress {
                            owner: &target,
                            field: field.as_bytes(),
                            ordinal: *ordinal as usize,
                        }
                    }
                };
                let slot = discovery
                    .select(&root, &component, selector, meter)
                    .map_err(|error| match error {
                        SelectionError::Missing => ExportError::MissingClipSelection,
                        SelectionError::Ambiguous => ExportError::AmbiguousClipSelection,
                        SelectionError::Unresolved(error) => unresolved(error),
                        SelectionError::Resource(error) => error.into(),
                    })?;
                let clip = slot.clip().map_err(unresolved)?;
                let mut owner = None;
                for rig in discovery.rigs() {
                    if same(&root, &rig.root, meter)? && same(&component, &rig.component, meter)? {
                        owner = Some(rig);
                        break;
                    }
                }
                let rig = owner.ok_or(ExportError::MissingClipSelection)?;
                // Deduplicate aliases to the same clip on the same component,
                // while preserving separate components on one Transform.
                let mut duplicate = false;
                for &(previous, candidate) in selected.as_slice() {
                    meter.count_decode(1)?;
                    if std::ptr::eq(previous, rig) && same(clip, candidate, meter)? {
                        duplicate = true;
                        break;
                    }
                }
                if !duplicate {
                    selected.push((rig, clip))?;
                }
            }
        }
    }
    Ok(selected)
}

fn unresolved(error: &unityrs_anim::SlotError) -> ExportError {
    ExportError::Unresolved(error.clone())
}

fn address_from_discovery(
    discovery: &Discovery,
    files: &FileIndex,
    address: ObjectAddress,
    meter: &Meter,
) -> Result<ObjectRef, ExportError> {
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
            if object.path_id() == address.path_id
                && files.matches(address.file_index, object, meter)?
            {
                return Ok(object.clone());
            }
        }
    }
    Err(ExportError::MissingClipSelection)
}
