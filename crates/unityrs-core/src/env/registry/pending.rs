//! The lists of dropped lazy slots waiting to be reached (`reach.rs`'s
//! module docs, "The lists of dropped slots"; plan section 3.16, R3 and
//! R4): every slot a registration dropped unreached, and apart from it the
//! slots of a dependency the LRU had evicted when a `settle` took them,
//! which only the container pass opens again (plan section 9, Q13).
//!
//! **Cost** (review round 6: the list was a `Vec` whose `remove` shifted
//! every later slot, so settling M dropped slots took O(M²) steps that
//! nothing counted). Each list is a B-tree keyed by the order the slots were
//! listed in, so a `settle`'s cursor ([`Pending::after`]) and a slot's
//! removal ([`Pending::take`]) each take O(log n) steps for n listed slots,
//! and settling M slots takes O(M log M). Every step counts as work of the
//! slot's input, [`STEP`] bytes a step, [`search_steps`]`(n)` steps a
//! search (the unit a bundle read's searches count in, plan section 3.6),
//! so the steps are bounded by `Limits::max_decode_work` as every other
//! input-driven step is, and a test sees them through the input's meter
//! (plan section 9, "How a test sees bounded work"). A step count worked
//! out from a list's length is what a B-tree takes, and not what a list
//! that shifts its slots on a removal takes, which would count the same:
//! so the lists' type is pinned by this module's test, beside the count
//! (review round 7: with `Vec` lists back, every test still passed).
//!
//! **Memory.** A lazy slot pays for its place in a list when it is made
//! (`super::PENDING_SHARE`): its share of a B-tree node, as a key pays for
//! its share of the registry's `order` tree (std's `BTreeMap` keeps every
//! node but the root at least five elements full, after a removal too, and
//! every internal node but the root at least six children, so a fifth of a
//! leaf and a thirtieth of an internal node per element cover them). A
//! list's root is one node, a leaf or an internal node, and a list that
//! empties is replaced by an empty one, which allocates nothing, so the
//! roots are at most two nodes per registry at any time: a constant of the
//! registry's tables, as the key table's control tail is (plan section
//! 3.13). A slot is in at most one of the lists.

use std::collections::BTreeMap;
use std::mem::size_of;
use std::ops::Bound;
use std::sync::Arc;

use super::Slot;
use crate::limits::heap_cost;

/// The work one step counts: the bytes of one key a search of a list reads
/// (`files::bundle`'s unit).
pub(super) const STEP: u64 = 8;

/// The steps a search of a B-tree of `n` slots takes at most: ⌈log2 n⌉ + 1
/// for `n > 0` (one node per level, each searched within its eleven keys,
/// counted as a binary search of the whole), and one for none (the empty
/// tree's check).
pub(super) fn search_steps(n: usize) -> u64 {
    n.checked_sub(1).map_or(1, |m| u64::from(m.bit_width()) + 1)
}

/// An element of a list: its place in the order and the slot.
const ELEMENT: usize = size_of::<u64>() + size_of::<Arc<Slot>>();

/// A leaf of a list's B-tree: its header, 11 keys and 11 values; an
/// internal node adds 12 edges (std's `BTreeMap`, `B = 6`).
const LEAF: u64 = heap_cost((16 + 11 * ELEMENT) as u64);
const INTERNAL: u64 = heap_cost((16 + 11 * ELEMENT + 12 * size_of::<usize>()) as u64);

/// What a lazy slot pays for its place in a list (module docs): a fifth of
/// a leaf and a thirtieth of an internal node.
pub(super) const SHARE: u64 = LEAF / 5 + 1 + INTERNAL / 30 + 1;

/// Which list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum List {
    /// Dropped slots every `settle` reaches.
    Listed,
    /// Dropped slots of an evicted dependency, which only the container
    /// pass reaches (plan section 9, Q13).
    Evicted,
}

/// The two lists, kept under the registry's lock, so a slot leaves the key
/// table and joins a list in one step: no `settle` can find it in neither
/// (plan section 3.16, R4).
#[derive(Debug, Default)]
pub(super) struct Pending {
    listed: BTreeMap<u64, Arc<Slot>>,
    evicted: BTreeMap<u64, Arc<Slot>>,
    /// The next slot's place in the order.
    next: u64,
}

impl Pending {
    fn list(&self, list: List) -> &BTreeMap<u64, Arc<Slot>> {
        match list {
            List::Listed => &self.listed,
            List::Evicted => &self.evicted,
        }
    }

    fn list_mut(&mut self, list: List) -> &mut BTreeMap<u64, Arc<Slot>> {
        match list {
            List::Listed => &mut self.listed,
            List::Evicted => &mut self.evicted,
        }
    }

    /// Lists `slot` (on [`List::Listed`]); its place in the order, and the
    /// steps the insertion took.
    pub(super) fn push(&mut self, slot: Arc<Slot>) -> (u64, u64) {
        let seq = self.next;
        self.next = seq.saturating_add(1);
        let steps = search_steps(self.listed.len());
        self.listed.insert(seq, slot);
        (seq, steps)
    }

    /// The first slot of `list` after `after` in the order (the first of
    /// all for `None`) and its place, and the steps the search took: a
    /// cursor that slots listed or taken off meanwhile do not disturb.
    pub(super) fn after(&self, list: List, after: Option<u64>) -> (Option<(u64, Arc<Slot>)>, u64) {
        let slots = self.list(list);
        let lower = after.map_or(Bound::Unbounded, Bound::Excluded);
        let found = slots
            .range((lower, Bound::Unbounded))
            .next()
            .map(|(s, slot)| (*s, slot.clone()));
        (found, search_steps(slots.len()))
    }

    /// Takes the slot at `seq` off `list` (another thread may have already),
    /// and the steps it took. A list left empty is replaced by a new one,
    /// which frees its root (module docs).
    pub(super) fn take(&mut self, list: List, seq: u64) -> (Option<Arc<Slot>>, u64) {
        let slots = self.list_mut(list);
        let steps = search_steps(slots.len());
        let taken = slots.remove(&seq);
        if slots.is_empty() {
            *slots = BTreeMap::new();
        }
        (taken, steps)
    }

    /// Moves the slot at `seq` from [`List::Listed`] to [`List::Evicted`],
    /// at the same place in the order; the steps it took.
    pub(super) fn defer(&mut self, seq: u64) -> u64 {
        let (taken, steps) = self.take(List::Listed, seq);
        let Some(slot) = taken else {
            return steps;
        };
        let more = search_steps(self.evicted.len());
        self.evicted.insert(seq, slot);
        steps.saturating_add(more)
    }

    /// How many slots `list` holds.
    pub(super) fn len(&self, list: List) -> usize {
        self.list(list).len()
    }

    /// Every slot listed on either list, in the order (for tests).
    #[cfg(test)]
    pub(super) fn all(&self) -> Vec<(u64, Arc<Slot>)> {
        let mut all: Vec<_> = self
            .listed
            .iter()
            .chain(&self.evicted)
            .map(|(s, slot)| (*s, slot.clone()))
            .collect();
        all.sort_by_key(|(s, _)| *s);
        all
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use super::{List, Pending, search_steps};
    use crate::env::registry::Slot;

    /// The lists are the B-trees their steps are counted for (module docs,
    /// "Cost"): a list of another structure must change this test, and
    /// [`search_steps`] with it. And each operation counts the steps of a
    /// search of the list's length at its start, the cursor's included.
    #[test]
    fn the_lists_are_the_b_trees_their_steps_are_counted_for() {
        let mut p = Pending::default();
        let lists: [&BTreeMap<u64, Arc<Slot>>; 2] = [&p.listed, &p.evicted];
        assert!(lists.iter().all(|l| l.is_empty()));
        assert_eq!(p.after(List::Listed, None).1, search_steps(0));
        assert_eq!(p.take(List::Listed, 0).1, search_steps(0));
        assert_eq!(p.defer(0), search_steps(0));
        // ⌈log2 n⌉ + 1 for n > 0, as a search down a B-tree of n keys.
        let steps: Vec<u64> = [1, 2, 3, 4, 5, 8, 9, 1024, 1025]
            .into_iter()
            .map(search_steps)
            .collect();
        assert_eq!(steps, [1, 2, 3, 3, 4, 4, 5, 11, 12]);
    }
}
