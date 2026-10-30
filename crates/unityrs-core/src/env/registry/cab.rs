//! What the registry answers a lookup with.

use std::sync::Arc;

use crate::files::{BundleFile, File, RawFile, SerializedFile};

/// A registered CAB: what `get_cab` returns.
#[derive(Clone, Debug)]
pub enum Cab {
    /// A `SerializedFile`.
    Serialized(Arc<SerializedFile>),
    /// A raw reader (`.resS`, `.resource`, name-rule files).
    Raw(RawCab),
}

/// A raw CAB's bytes.
#[derive(Clone, Debug)]
pub enum RawCab {
    /// A bundle entry, read through the bundle (no copy of the whole entry).
    InBundle {
        /// The bundle.
        bundle: Arc<BundleFile>,
        /// Its directory index.
        dir_index: usize,
    },
    /// A file loaded on its own.
    File(Arc<RawFile>),
}

/// A CAB with the handle that keeps its container alive: for a dependency's
/// CAB, the dependency's top-level file as the lookup reached it (an
/// eviction drops only the LRU's handle, so while this one is held the CAB's
/// bundle, and with it the pointers that need its siblings, stay
/// reachable); `None` for a CAB of a file the caller loaded, which is never
/// evicted. What [`crate::env::Environment::get_cab_held`] returns.
#[derive(Clone, Debug)]
pub struct HeldCab {
    /// The CAB.
    pub cab: Cab,
    /// The dependency's top-level file, when the CAB is a dependency's.
    pub hold: Option<File>,
}

const _: () = crate::send_sync::<Cab>();
const _: () = crate::send_sync::<RawCab>();
const _: () = crate::send_sync::<HeldCab>();
