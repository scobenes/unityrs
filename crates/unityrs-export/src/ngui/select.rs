//! Shared own-row traversal for metadata and extraction (selection section 3).
use super::{Failure, Hooks, classify, seam, table};
use std::sync::Arc;
use unityrs_core::{
    RawStr,
    files::{BundleFile, File, SerializedFile},
    limits::{Meter, MeteredVec},
};

/// Open every directory row first, including shadowed and unsupported rows,
/// before returning selected `SerializedFile`s in `files()` order. Never runs
/// an Environment container pass. `nested` observes every nested row in order.
pub fn select_own(
    own: &BundleFile,
    meter: &Arc<Meter>,
    hooks: &dyn Hooks,
    nested: &mut dyn FnMut(&RawStr) -> Result<(), Failure>,
) -> Result<MeteredVec<Arc<SerializedFile>>, Failure> {
    for (i, row) in own.directory().iter().enumerate() {
        if hooks.stopped() {
            return Err(Failure::Stopped);
        }
        let entry = seam::probe(hooks, || {
            own.entry(i)
                .map_err(|e| classify::entry_error(&e, &row.path))
        })?;
        if let File::Bundle(_) = entry {
            nested(&row.path)?;
        }
    }
    let mut files = MeteredVec::new(meter);
    for slot in own.files() {
        if hooks.stopped() {
            return Err(Failure::Stopped);
        }
        let entry = seam::probe(hooks, || {
            own.entry(slot.dir_index)
                .map_err(|e| classify::entry_error(&e, &slot.name))
        })?;
        if let File::Serialized(sf) = entry {
            seam::probe(hooks, || table::push(&mut files, sf))?;
        }
    }
    Ok(files)
}
