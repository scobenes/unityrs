//! Reaching a lazy slot (module docs, "Nested containers under
//! `LoadMode::Lazy`"): [`Registry::reach`], the one routine that registers
//! a nested container's entries; [`Registry::settle`], which reaches the
//! lazy slots registrations dropped; and [`entry_of`], the one rule that
//! tells, without reading, which entry of which input a slot is.
//!
//! **The lists of dropped slots** (plan section 3.16, invariants R3 and R4;
//! `pending.rs`). A dropped lazy slot stays listed until it is reached (or
//! found to hold nothing to register), and every `settle` reaches every
//! slot on the list, one after another, those another thread is reaching
//! too: a reach is safe for concurrent callers, so `settle` never waits for
//! another thread and never returns while a slot listed before it started
//! is unreached, but for a refusal (review round 5: a `settle` popped a
//! slot, and a concurrent one found the list empty and returned while the
//! first was still reaching it, so a load returned with the dropped
//! container's entries unregistered). The list is ordered by when each slot
//! was listed, and a `settle` walks it by that order, so a slot it leaves (a
//! refusal) is not asked again by the same call. A reach's own
//! registrations never settle: the slots they drop are listed, and the
//! `settle` that runs the reach, or the one [`Registry::reach`] ends with,
//! takes them in the same loop, so a chain of drops is reached one after
//! another, never nested on the stack. A slot of a dependency the LRU has
//! evicted is not opened again by a `settle` (plan section 9, Q13): it moves
//! to a second list, which only the container pass reaches, opening the
//! dependency again ([`Registry::settle_for_pass`]; Q13 revised after
//! review round 6, whose pass missed the paths that container's tables set
//! in other files). Each step a list takes counts as work of its slot's
//! input (`pending.rs`; review round 6: removing a slot from the front of
//! a `Vec` made settling M slots O(M²), uncounted).
//!
//! **Stand-ins** (plan section 9, Q14). A registration that drops a lazy
//! slot of the same entry of the same input as the slot that stays (a
//! reload on the same `Bytes`, which every handle on the dropped entry can
//! tell without reading: [`stands_in`]) does not list it: the slot that
//! stays stands in for it, and learns the least load among the slots it
//! stands in for ([`Slot::position`]), whose ranks its container's entries
//! take as their positions when it is reached, with its own as their values
//! (review round 6: they took the reload's positions, so the container
//! pass walked them later and `container_path` gave another row's path). So
//! one reach, of the standing slot, registers what an eager load of both
//! registers, the dropped entry is never read, and a lookup from inside the
//! dropped entry reaches the standing slot (`super::super::resource_data`).
//! The registration tells the stand-in before its write lock, with no lock
//! held, and applies it in the lock's step only when the key's value is
//! still the slot it told it against. A standing slot that was already the
//! key's value may be reaching on another thread: the reach reads the
//! position, registers there, and closes the position only if it is still
//! the one it registered at, registering again at a lower one it finds,
//! before it marks the slot reached ([`Slot::close_at`]); the registration
//! lowers the position only while it is not closed ([`Slot::stand_for`]),
//! one atomic word for both, so either the reach registers at the lowered
//! position before any reader sees the slot reached, or the registration
//! finds it closed and lists the dropped slot as any other (review round 7:
//! the reach marked the slot reached and then moved its entries, so a
//! container pass in between walked them at the reload's positions).
//!
//! **Work** (R5). A reach is a lazy computation of the input whose slot it
//! is: what its registrations were granted when the input's own meter
//! refused them counts as that open's work, and once the work has passed
//! its allowance a reach is refused before it registers anything
//! ([`crate::LimitKind::DecodeWork`], which is not transient), so lookups
//! cannot repeat a refused reach without bound (review round 5: each lookup
//! by name registered the container again up to the refusal, and nothing
//! counted it).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::super::Source;
#[cfg(test)]
use super::Hook;
use super::pending::{List, STEP};
use super::{By, Cab, DepSlot, Dependency, Loads, RawCab, Registry, Slot, SlotKind};
use crate::detect::{FileType, check_file_type};
use crate::error::Error;
use crate::files::{BundleFile, File};
use crate::limits::{GrantScope, Limits, Meter};
use crate::once::CellError;

/// The bytes of an entry read to tell a bundle. `check_file_type` calls the
/// entry one by the NUL-terminated string in its first 20 bytes, which it
/// reads alike from these 21 and from the whole entry (`py_string_to_null`
/// stops at the 20th byte without a NUL only when a 21st follows), and a
/// prefix this short never reaches the gzip test, the only one that reads
/// the `Limits` it is given.
const PEEK: u64 = 21;

/// Whether entry `i` of `b` is a bundle, from its first [`PEEK`] bytes
/// (their blocks decoded, as any read decodes them). A read that fails other
/// than transiently says no: such an entry cannot be parsed, so it holds
/// nothing to register, and its reads fail with the error.
fn is_bundle_entry(b: &BundleFile, i: usize) -> Result<bool, Error> {
    match b.entry_range(i, 0..PEEK) {
        Ok(head) => Ok(matches!(
            check_file_type(&head, &Limits::default()),
            Ok(FileType::BundleFile)
        )),
        Err(e) if e.is_transient() => Err(e),
        Err(_) => Ok(false),
    }
}

/// How a slot is an entry of the input a handle was opened on, told without
/// reading anything ([`entry_of`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Through {
    /// The slot reads that very handle: its bundle is the handle, or it is a
    /// dependency's whose open handle is the handle or whose `Bytes` the
    /// handle was opened on. The asker's handle may read the entry for it,
    /// and a reach from it keeps nothing of the handle (a dependency's slots
    /// hold their path, an entry slot the very bundle it already holds).
    Own,
    /// The slot is the same entry of the same input (the same `Bytes`,
    /// parentless) through another handle: a reload's slot, which stands in
    /// for the earlier load's (plan section 9, Q14). A reach goes through
    /// the slot's own handle, so the registry keeps no handle of the
    /// asker's.
    Slot,
}

/// The directory index of the entry of `fb` that `slot` is, and how
/// ([`Through`]), when the core can tell without reading anything (plan
/// section 3.13): the slot's bundle is `fb`, or is parentless on the very
/// `Bytes` `fb` is; or the slot is a dependency's top-level entry whose open
/// handle is `fb` or whose [`Source::Bytes`] are the bytes `fb` was opened
/// on. A [`Source::Path`] or [`Source::Fetch`] slot on another handle cannot
/// be told without reading it again.
pub(crate) fn entry_of(fb: &Arc<BundleFile>, slot: &Slot) -> Option<(usize, Through)> {
    match &slot.kind {
        SlotKind::Entry { bundle, dir_index } if Arc::ptr_eq(bundle, fb) => {
            Some((*dir_index, Through::Own))
        }
        SlotKind::Entry { bundle, dir_index } => {
            same_input(bundle, fb).then_some((*dir_index, Through::Slot))
        }
        SlotKind::Ready(Cab::Raw(RawCab::InBundle { bundle, dir_index }))
            if Arc::ptr_eq(bundle, fb) =>
        {
            Some((*dir_index, Through::Own))
        }
        SlotKind::Dependency { dep } => {
            let &[i] = slot.rank.path() else {
                return None;
            };
            let same_handle = matches!(dep.alive(), Some(File::Bundle(b)) if Arc::ptr_eq(&b, fb));
            let same_bytes = fb.parent().is_none()
                && matches!(&dep.source, Source::Bytes { data, .. }
                    if same_bytes(data, fb.source()));
            (same_handle || same_bytes).then_some((i, Through::Own))
        }
        SlotKind::Ready(_) => None,
    }
}

/// Whether `standing`, a key's value, stands in for `dropped`, which it
/// replaced or beat: the same entry of the same input, as every handle on
/// `dropped`'s entry tells it ([`entry_of`] from `dropped`'s own handle), so
/// a lookup from inside `dropped`'s container reaches `standing` instead
/// (`super::super::resource_data`'s `reach_chain`), and `standing`'s reach
/// registers its entries at `dropped`'s positions (module docs,
/// "Stand-ins"; plan section 9, Q14). Any other drop is reached
/// ([`Registry::settle`]): a reload a lookup could not tell, two
/// [`Source::Path`] dependencies of one path among them (review round 5:
/// the reload's slot stood in for the earlier one's, but nothing reached it
/// from inside the earlier load's container).
pub(super) fn stands_in(standing: &Slot, dropped: &Slot) -> bool {
    if standing.rank.path() != dropped.rank.path() {
        return false;
    }
    let (top, i) = match &dropped.kind {
        SlotKind::Entry { bundle, dir_index } => (bundle.clone(), *dir_index),
        SlotKind::Dependency { dep } => {
            let (&[i], Some(File::Bundle(top))) = (dropped.rank.path(), dep.alive()) else {
                return false;
            };
            (top, i)
        }
        SlotKind::Ready(_) => return false,
    };
    entry_of(&top, standing).is_some_and(|(j, _)| j == i)
}

/// Two parentless bundles opened on one allocation and length.
fn same_input(one: &BundleFile, other: &BundleFile) -> bool {
    one.parent().is_none() && other.parent().is_none() && same_bytes(one.source(), other.source())
}

/// One allocation and length.
fn same_bytes(data: &[u8], data2: &[u8]) -> bool {
    data.as_ptr() == data2.as_ptr() && data.len() == data2.len()
}

/// Why a registration of a nested container's subtree failed: reading an
/// entry, or registering one.
enum Failed {
    Read(Error),
    Register(Error),
}

/// What a reach did with a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reached {
    /// The slot is reached: its container's subtree registered, or it holds
    /// none.
    Done,
    /// The slot is an evicted dependency's, which a reach that does not
    /// reopen left unread (plan section 9, Q13).
    Evicted,
}

/// What a lazy slot's entry is ([`Registry::lazy_container`]).
enum Entry {
    /// A nested container, parsed.
    Container(Arc<BundleFile>),
    /// No container: nothing to register.
    Nothing,
    /// An evicted dependency's entry, not opened again.
    Evicted,
}

impl Registry {
    /// Reaches `slot` (module docs): for a lazy slot not yet reached, reads
    /// the entry's first bytes and, when they are a bundle's, parses it and
    /// registers its whole subtree at the ranks below the slot's, as the
    /// slot's own kind registers (entries of the parsed bundle for a
    /// caller's, dependency slots for a dependency's), a container inside it
    /// with no key of its own, as an eager load registers it, and at the
    /// positions of the least dropped slot it stands in for
    /// ([`Slot::position`], plan section 9, Q14); then closes the position
    /// there and marks the slot reached, and reaches the slots its
    /// registrations dropped
    /// ([`Self::settle`]). `nested` is the container when the caller holds
    /// it: the slot's own parse, or the asking file's handle on the very
    /// entry the slot is ([`Through::Own`]), so nothing is read through the
    /// slot's handle (plan section 3.13). Every caller that finds the slot
    /// unreached registers the subtree, and a second registration of an
    /// entry changes nothing, so each returns with every entry registered.
    /// A refusal is returned, and the slot stays unreached for the next
    /// caller; past its input's work allowance the refusal is
    /// [`crate::LimitKind::DecodeWork`] (module docs, "Work").
    pub(crate) fn reach(
        &self,
        slot: &Arc<Slot>,
        nested: Option<&Arc<BundleFile>>,
    ) -> Result<(), Error> {
        let reached = self.reach_one(slot, nested, true).map(|_| ());
        // A refusal there is left for the next call, as a lookup leaves it.
        self.settle().ok();
        reached
    }

    /// [`Self::reach`] without the settle; with `reopen` false, a dependency
    /// the LRU evicted is not opened again to read the entry, and the slot
    /// is left unreached ([`Reached::Evicted`], plan section 9, Q13).
    fn reach_one(
        &self,
        slot: &Arc<Slot>,
        nested: Option<&Arc<BundleFile>>,
        reopen: bool,
    ) -> Result<Reached, Error> {
        // A dependency's reach (its reopen, its reads and its registrations,
        // all charged to the dependency's meter) the pool refuses evicts the
        // others and is tried once more, as every dependency read is
        // (`with_room`); a reach tried again registers what it registered
        // already at the same ranks, which changes nothing.
        match &slot.kind {
            SlotKind::Dependency { dep } => dep.with_room(|| self.reach_once(slot, nested, reopen)),
            SlotKind::Entry { .. } | SlotKind::Ready(_) => self.reach_once(slot, nested, reopen),
        }
    }

    /// One try of [`Self::reach_one`].
    fn reach_once(
        &self,
        slot: &Arc<Slot>,
        nested: Option<&Arc<BundleFile>>,
        reopen: bool,
    ) -> Result<Reached, Error> {
        if !slot.unreached() {
            return Ok(Reached::Done);
        }
        let parsed = match nested {
            Some(_) => None,
            None => match self.lazy_container(slot, reopen)? {
                Entry::Container(b) => Some(b),
                Entry::Nothing => None,
                Entry::Evicted => return Ok(Reached::Evicted),
            },
        };
        let Some(nested) = nested.or(parsed.as_ref()) else {
            slot.mark_reached();
            return Ok(Reached::Done);
        };
        let mut at = slot.position();
        self.register_counted(slot, nested, at)?;
        // A stand-in learned while the registration ran: the entries move
        // to its position before the slot is closed there and marked
        // (module docs, "Stand-ins"). Each turn lowers the position.
        while let Err(lower) = slot.close_at(at) {
            at = lower;
            self.register_counted(slot, nested, at)?;
        }
        #[cfg(test)]
        self.hook(Hook::Reached);
        slot.mark_reached();
        Ok(Reached::Done)
    }

    /// The entry a lazy slot is: its first bytes read through the slot's
    /// own handle (an evicted dependency's opened again only with
    /// `reopen`), and the entry parsed only when they are a bundle's.
    fn lazy_container(&self, slot: &Slot, reopen: bool) -> Result<Entry, Error> {
        let (bundle, i) = match &slot.kind {
            SlotKind::Entry { bundle, dir_index } => (bundle.clone(), *dir_index),
            SlotKind::Dependency { dep } => {
                let &[i] = slot.rank.path() else {
                    return Ok(Entry::Nothing);
                };
                let top = if reopen {
                    dep.handle()?
                } else {
                    let Some(top) = dep.alive() else {
                        return Ok(Entry::Evicted);
                    };
                    top
                };
                let File::Bundle(b) = top else {
                    return Ok(Entry::Nothing);
                };
                (b, i)
            }
            SlotKind::Ready(_) => return Ok(Entry::Nothing),
        };
        if !is_bundle_entry(&bundle, i)? {
            return Ok(Entry::Nothing);
        }
        match bundle.entry(i) {
            Ok(File::Bundle(nested)) => {
                self.count(slot);
                Ok(Entry::Container(nested))
            }
            Err(e) if e.is_transient() => Err(e),
            Ok(_) | Err(_) => Ok(Entry::Nothing),
        }
    }

    /// Registers `nested`'s subtree for `slot`, its positions at load
    /// `position` ([`Slot::position`]), charged to the slot's own input
    /// (plan section 3.16, R2): a dependency's slots to the dependency's
    /// meter, whatever handle read the entry (review round 5: a batch job's
    /// read through its own handle charged the shared registry's entries to
    /// the job's meter, which they outlived), an entry slot's to its
    /// bundle's. As a lazy computation of that input (module docs, "Work"):
    /// past its work allowance nothing is registered, and what a
    /// registration the meter refused was granted counts as work.
    fn register_counted(
        &self,
        slot: &Slot,
        nested: &Arc<BundleFile>,
        position: u64,
    ) -> Result<(), Error> {
        let dep = match &slot.kind {
            SlotKind::Dependency { dep } => Some(&**dep),
            SlotKind::Entry { .. } | SlotKind::Ready(_) => None,
        };
        let meter = match dep {
            Some(dep) => dep.meter(),
            None => nested.meter().clone(),
        };
        if let Some(limit) = meter.work_used_up() {
            return Err(Error::Limit(limit));
        }
        let mut path = slot.rank.path().to_vec();
        let loads = Loads {
            value: slot.rank.load,
            position,
        };
        let scope = GrantScope::enter(&meter);
        let registered = self.register_tree(nested, dep, loads, &mut path, &meter);
        let own = scope.finish(&meter);
        // The dependency's meter grew: the LRU's sum follows it.
        if let Some(dep) = dep
            && let Some(cache) = dep.cache.upgrade()
        {
            cache.refresh(dep);
        }
        match registered {
            Ok(()) => Ok(()),
            // Past the allowance the refusal is `DecodeWork`, kept.
            Err(Failed::Register(e)) if e.is_meter_refusal() => {
                meter.add_work(own);
                Err(meter.work_used_up().map_or(e, Error::Limit))
            }
            Err(Failed::Register(e) | Failed::Read(e)) => Err(e),
        }
    }

    /// Registers every entry of `nested`, at `path` and below, as an eager
    /// load does: a bundle entry's own entries in its place, every other
    /// entry as a slot of the kind `dep` says, known to be no container,
    /// charged to `meter`. An entry that fails other than transiently is
    /// registered as it is, so its reads fail with its error (difference
    /// (a)). Bounded by [`Limits::max_container_depth`], as the parse is.
    fn register_tree(
        &self,
        nested: &Arc<BundleFile>,
        dep: Option<&Arc<Dependency>>,
        loads: Loads,
        path: &mut Vec<usize>,
        meter: &Arc<Meter>,
    ) -> Result<(), Failed> {
        for (j, d) in nested.directory().iter().enumerate() {
            path.push(j);
            let r = self.register_tree_entry(nested, j, &d.path, dep, loads, path, meter);
            path.pop();
            r?;
        }
        Ok(())
    }

    // An entry's place and what it is registered for, each a term above.
    #[allow(clippy::too_many_arguments)]
    fn register_tree_entry(
        &self,
        nested: &Arc<BundleFile>,
        j: usize,
        name: &crate::raw::RawStr,
        dep: Option<&Arc<Dependency>>,
        loads: Loads,
        path: &mut Vec<usize>,
        meter: &Arc<Meter>,
    ) -> Result<(), Failed> {
        if is_bundle_entry(nested, j).map_err(Failed::Read)? {
            match nested.entry(j) {
                Ok(File::Bundle(inner)) => {
                    self.parsed.fetch_add(1, Ordering::Relaxed);
                    return self.register_tree(&inner, dep, loads, path, meter);
                }
                Err(e) if e.is_transient() => return Err(Failed::Read(e)),
                Ok(_) | Err(_) => {}
            }
        }
        let kind = match dep {
            Some(dep) => SlotKind::Dependency {
                dep: DepSlot::new(dep),
            },
            None => SlotKind::Entry {
                bundle: nested.clone(),
                dir_index: j,
            },
        };
        self.insert(name, kind, loads, path, meter, By::Reach, false)
            .map_err(Failed::Register)
    }

    /// The first slot of `list` after `after` in the order, and its place:
    /// a cursor that slots listed or taken off meanwhile do not disturb.
    /// The search's steps count as `charge_to`'s input's work, or as the
    /// slot's it finds.
    fn listed_after(
        &self,
        list: List,
        after: Option<u64>,
        charge_to: Option<&Slot>,
    ) -> Option<(u64, Arc<Slot>)> {
        let (found, steps) = self.read().pending.after(list, after);
        let payer = charge_to.or(found.as_ref().map(|(_, s)| &**s));
        if let Some(payer) = payer {
            payer.input_meter().add_work(steps.saturating_mul(STEP));
        }
        found
    }

    /// Takes `slot`, listed at `seq` on `list`, off it (another thread may
    /// have already), its steps counted as its input's work (`pending.rs`).
    /// The handle is dropped after the lock, since it may be the slot's
    /// last, which may tell the LRU (plan section 3.16: no lock is taken
    /// while another is held).
    pub(super) fn unlist(&self, list: List, seq: u64, slot: &Slot) {
        let (taken, steps) = {
            let mut inner = self.write();
            let out = inner.pending.take(list, seq);
            self.store_lens(&inner.pending);
            out
        };
        drop(taken);
        slot.input_meter().add_work(steps.saturating_mul(STEP));
    }

    /// Moves `slot`, listed at `seq`, to the list only the container pass
    /// reaches (plan section 9, Q13), its steps counted as its input's
    /// work.
    fn defer(&self, seq: u64, slot: &Slot) {
        let steps = {
            let mut inner = self.write();
            let steps = inner.pending.defer(seq);
            self.store_lens(&inner.pending);
            steps
        };
        slot.input_meter().add_work(steps.saturating_mul(STEP));
    }

    /// Whether a dropped slot of an evicted dependency waits for the
    /// container pass (plan section 9, Q13).
    pub(super) fn evicted_waiting(&self) -> bool {
        self.evicted_len.load(Ordering::Acquire) > 0
    }

    /// Reaches every lazy slot a registration dropped unreached (module
    /// docs), each listed slot once per call, in the order they were
    /// listed, the slots those reaches drop included. A dropped slot is no
    /// key's value, so nothing could reach it by name afterwards; an eager
    /// load registered its entries at load. A dependency the LRU has evicted
    /// is not opened again for it: its slot moves to the list only the
    /// container pass reaches ([`Self::settle_for_pass`], plan section 9,
    /// Q13). A slot stays listed until it is reached; a refusal leaves it
    /// for the next call (every load's registration, every lookup by name,
    /// [`Registry::keys`] and every container pass calls this), and the
    /// first one is returned once every other listed slot was tried: a
    /// registration or a lookup goes on without it, as without any container
    /// not yet reached, and a container pass fails with it. Any other
    /// failure means the dropped entry cannot be parsed, or its input's work
    /// is used up (module docs, "Work"): nothing more is registered.
    pub(crate) fn settle(&self) -> Result<(), Error> {
        let mut refused = None;
        if self.pending_len.load(Ordering::Acquire) > 0 {
            self.settle_list(List::Listed, false, &mut refused);
        }
        refused.map_or(Ok(()), Err)
    }

    /// [`Self::settle`] for the container pass: every listed slot, and every
    /// dropped slot of an evicted dependency, the dependency opened again to
    /// reach it (plan section 9, Q13, revised after review round 6: an
    /// eager load registered the dropped container's entries at load, and
    /// their tables set paths in other files, as an eager pass reopens an
    /// evicted dependency to walk its table).
    pub(crate) fn settle_for_pass(&self) -> Result<(), Error> {
        let mut refused = None;
        if self.pending_len.load(Ordering::Acquire) > 0 {
            self.settle_list(List::Listed, true, &mut refused);
        }
        if self.evicted_waiting() {
            self.settle_list(List::Evicted, true, &mut refused);
        }
        refused.map_or(Ok(()), Err)
    }

    /// Reaches every slot of `list`, as [`Self::settle`] says, an evicted
    /// dependency's opened again with `reopen`; the first refusal into
    /// `refused`.
    fn settle_list(&self, list: List, reopen: bool, refused: &mut Option<Error>) {
        let mut after = None;
        let mut prev: Option<Arc<Slot>> = None;
        while let Some((seq, slot)) = self.listed_after(list, after, prev.as_deref()) {
            after = Some(seq);
            #[cfg(test)]
            self.hook(Hook::SettleTook);
            match self.reach_one(&slot, None, reopen) {
                Ok(Reached::Done) => self.unlist(list, seq, &slot),
                Ok(Reached::Evicted) => self.defer(seq, &slot),
                Err(e) if e.is_transient() => {
                    refused.get_or_insert(e);
                }
                Err(_) => {
                    slot.mark_reached();
                    self.unlist(list, seq, &slot);
                }
            }
            prev = Some(slot);
        }
    }
}
