//! A registry slot (module docs, "What a slot holds").

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::{Cab, DepSlot, Rank};
use crate::files::BundleFile;
use crate::limits::{Charge, Meter};

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
    /// last one is). Read and set sequentially consistent, with
    /// [`Self::stands_for`] (`Slot::position`).
    pub(super) reached: AtomicBool,
    /// For a lazy slot that stands in for dropped slots of the same entry
    /// of the same input (plan section 9, Q14), the least load among them:
    /// its container's entries take their positions at that load's ranks,
    /// as an eager load of the dropped one registered them
    /// ([`Self::position`]); `u64::MAX` for none. Only lowered, and only
    /// under the registry's write lock, in the step that drops a slot.
    pub(super) stands_for: AtomicU64,
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
    pub(super) charge: Charge,
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
        self.lazy && !self.reached.load(Ordering::SeqCst)
    }

    /// Marks it reached.
    pub(super) fn mark_reached(&self) {
        if self.lazy {
            self.reached.store(true, Ordering::SeqCst);
        }
    }

    /// The load whose ranks its container's entries take as their
    /// positions: its own, or the least of the dropped slots it stands in
    /// for ([`Self::stands_for`]). A reach reads it before it registers and
    /// again after it marks the slot reached, and registers again at a
    /// lower one it then finds; a registration that lowers it reads
    /// [`Self::unreached`] after (both sequentially consistent), so a
    /// position learned while a reach runs is never missed by both
    /// (`reach.rs`'s module docs).
    pub(super) fn position(&self) -> u64 {
        self.stands_for.load(Ordering::SeqCst).min(self.rank.load)
    }

    /// Lowers [`Self::stands_for`] to `load`, under the registry's write
    /// lock.
    pub(super) fn stand_for(&self, load: u64) {
        self.stands_for.fetch_min(load, Ordering::SeqCst);
    }

    /// The meter of the input the slot belongs to (plan section 3.16, R2):
    /// a dependency's current open's, an entry's bundle's, else the one
    /// its registration was charged to. What the work done for it counts
    /// on.
    pub(super) fn input_meter(&self) -> Arc<Meter> {
        match &self.kind {
            SlotKind::Dependency { dep } => dep.meter(),
            SlotKind::Entry { bundle, .. } => bundle.meter().clone(),
            SlotKind::Ready(_) => self.charge.meter().clone(),
        }
    }
}
