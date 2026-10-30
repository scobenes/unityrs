//! A registry slot (module docs, "What a slot holds").

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{Cab, DepSlot, Rank};
use crate::files::BundleFile;
use crate::limits::Charge;

/// One registry slot, with the charge for its registration (module docs).
#[derive(Debug)]
pub(crate) struct Slot {
    pub(crate) kind: SlotKind,
    /// Its rank (module docs).
    pub(crate) rank: Rank,
    /// A `Lazy` load's registration of a top-level bundle's entry, unread:
    /// it may be a nested container (module docs).
    pub(super) lazy: bool,
    /// For a lazy slot, whether it was reached: known to be no container,
    /// or every entry of the container it is registered (set after the
    /// last one is).
    pub(super) reached: AtomicBool,
    /// Whether a parse through the slot was counted (`EnvStats`).
    pub(super) counted: AtomicBool,
    /// Whether the container preload walked the table of the file the slot
    /// holds (`parse_preload_table` sets `_preload_table = None` when it
    /// ends). Kept on the slot, which outlives an eviction, so a
    /// dependency reopened after one is not walked again, as UnityPy, which
    /// never evicts, walks each file once.
    pub(super) walked: AtomicBool,
    /// The slot's own charge (its `Arc`, its rank's path), returned when
    /// the last handle on the slot drops.
    pub(super) _charge: Charge,
}

/// What a slot reaches.
#[derive(Debug)]
pub(crate) enum SlotKind {
    /// A CAB of a file the caller loaded, parsed at registration (`Eager`):
    /// never evicted.
    Ready(Cab),
    /// An entry of a bundle the caller loaded `Lazy`, parsed on first use
    /// through the bundle's own entry cell: never evicted.
    Entry {
        /// The bundle.
        bundle: Arc<BundleFile>,
        /// Its directory index.
        dir_index: usize,
    },
    /// A dependency's CAB, reached from the LRU's handle on its top-level
    /// file along the slot's rank's path, and reopened from its source after
    /// an eviction. Holds no strong handle on the file; its handle on the
    /// dependency is counted, so the LRU lets go of a dependency no slot
    /// reaches ([`DepSlot`]).
    Dependency {
        /// The dependency.
        dep: DepSlot,
    },
}

impl Slot {
    /// Whether the slot is a dependency's (`is_dependency` in UnityPy).
    pub(crate) fn is_dependency(&self) -> bool {
        matches!(self.kind, SlotKind::Dependency { .. })
    }

    /// Whether the container preload walked its file's table.
    pub(crate) fn walked(&self) -> bool {
        self.walked.load(Ordering::Acquire)
    }

    /// Marks its file's table walked.
    pub(crate) fn set_walked(&self) {
        self.walked.store(true, Ordering::Release);
    }

    /// Whether it is a lazy slot not yet reached (module docs).
    pub(super) fn unreached(&self) -> bool {
        self.lazy && !self.reached.load(Ordering::Acquire)
    }

    /// Marks it reached.
    pub(super) fn mark_reached(&self) {
        if self.lazy {
            self.reached.store(true, Ordering::Release);
        }
    }
}
