//! A registry slot (module docs, "What a slot holds").

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::{Cab, DepSlot, Rank};
use crate::files::BundleFile;
use crate::limits::{Charge, Meter};

/// The top bit of [`Slot::stands_for`]: the slot's position is closed.
pub(super) const CLOSED: u64 = 1 << 63;

/// [`Slot::stands_for`] of a slot that stands in for none: past every load.
pub(super) const NONE: u64 = CLOSED - 1;

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
    /// or every entry of the container it is registered at the slot's
    /// position, closed (set after the last one is, [`Self::close_at`]).
    pub(super) reached: AtomicBool,
    /// For a lazy slot that stands in for dropped slots of the same entry
    /// of the same input (plan section 9, Q14), the least load among them:
    /// its container's entries take their positions at that load's ranks,
    /// as an eager load of the dropped one registered them
    /// ([`Self::position`]); [`NONE`] for none. Its top bit, [`CLOSED`],
    /// says the position will not move again: a reach sets it once its
    /// entries are registered at the position ([`Self::close_at`]), before
    /// it marks the slot reached. Lowered only under the registry's write
    /// lock, in the step that drops a slot, and only while not closed
    /// ([`Self::stand_for`]); one atomic word, so a lowering and a close are
    /// one before the other.
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

    /// Marks it reached, its position closed first ([`Self::close_at`]): a
    /// slot found to hold no container has no entries to move.
    pub(super) fn mark_reached(&self) {
        if self.lazy {
            self.stands_for.fetch_or(CLOSED, Ordering::SeqCst);
            self.reached.store(true, Ordering::SeqCst);
        }
    }

    /// The load whose ranks its container's entries take as their
    /// positions: its own, or the least of the dropped slots it stands in
    /// for ([`Self::stands_for`]).
    pub(super) fn position(&self) -> u64 {
        Self::position_of(self.stands_for.load(Ordering::SeqCst), self.rank.load)
    }

    fn position_of(stands_for: u64, own: u64) -> u64 {
        (stands_for & !CLOSED).min(own)
    }

    /// Lowers [`Self::stands_for`] to `load`, under the registry's write
    /// lock, unless the position is closed: whether it did. A slot whose
    /// position is closed no longer stands in (`reach.rs`'s module docs,
    /// "Stand-ins").
    pub(super) fn stand_for(&self, load: u64) -> bool {
        let load = load.min(NONE);
        self.stands_for
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |s| {
                (s & CLOSED == 0).then_some(s.min(load))
            })
            .is_ok()
    }

    /// Closes the position at `at`, the one a reach registered the
    /// container's entries at, unless a stand-in lowered it since: then
    /// the lower one, which the reach registers at and closes again. Of a
    /// lowering and a close, one comes first in the word's order, so either
    /// the reach registers at the lowered position before the slot is
    /// marked reached, or the registration finds the position closed and
    /// lists the slot it drops (review round 7: the reach marked the slot
    /// reached and only then read the position again, so a container pass
    /// in between walked entries that moved after it). A slot closed
    /// already (another reach's close) is closed at its final position.
    pub(super) fn close_at(&self, at: u64) -> Result<(), u64> {
        let own = self.rank.load;
        self.stands_for
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |s| {
                (Self::position_of(s, own) >= at).then_some(s | CLOSED)
            })
            .map(|_| ())
            .map_err(|s| Self::position_of(s, own))
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
