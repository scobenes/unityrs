// Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py, UnityPy/files/ObjectReader.py, UnityPy/helpers/ContainerHelper.py. See THIRD_PARTY_LICENSES.
//! The container: [`Environment::container`], [`Environment::container_path`]
//! and the pass that builds their index (UnityPy's
//! `_build_container_index`), the walk of each file's preload table.

use std::collections::HashMap;
use std::fmt;
use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::deref::Reached;
use super::registry::{Rank, SlotKind, load_keys_here};
use super::revisit::Pending;
use super::watch::{Flag, Walk};
use super::{Cab, DerefError, Environment, Found, LoadMode, PPtr, never_evicts, pptr};
use crate::budget::Reservation;
use crate::container::{Container, ContainerPath, ContainerRow};
use crate::error::{Error, Unsupported};
use crate::files::{BundleFile, File, SerializedFile};
use crate::limits::{Charge, GrantScope, HASH_TABLE_SHARES, Meter, MeteredVec};
use crate::object::ObjectRef;
use crate::once::CellError;
use crate::pystr::{clean_external_path, py_lower, simplify_name};
use crate::raw::RawStr;
use crate::walk::STEP_WORK;

/// The iterator [`Environment::container`] returns, CABs in registry order.
pub struct ContainerItems<'a> {
    env: &'a Environment,
    after: Option<Rank>,
    current: Option<(Arc<SerializedFile>, usize)>,
}

const _: () = crate::send_sync::<ContainerItems<'static>>();

impl Iterator for ContainerItems<'_> {
    type Item = Result<ContainerItem, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((sf, k)) = self.current.take() {
                match sf.container() {
                    Ok(c) => {
                        if let Some(row) = c.rows().get(k) {
                            let row = row.clone();
                            self.current = Some((sf.clone(), k + 1));
                            return Some(Ok(ContainerItem { row, file: sf }));
                        }
                    }
                    Err(e) => return Some(Err(e)),
                }
            }
            // UnityPy environment.py:221-224: every non-dependency CAB.
            let (rank, slot) = self.env.registry().slot_after(self.after.as_ref())?;
            self.after = Some(rank);
            if slot.is_dependency() {
                continue;
            }
            match self.env.registry().resolve(&slot) {
                Ok(Some(Cab::Serialized(sf))) => self.current = Some((sf, 0)),
                Ok(_) => {}
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

impl fmt::Debug for ContainerItems<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContainerItems").finish_non_exhaustive()
    }
}

/// A container row with the object it points at, as `env.container` yields.
#[derive(Clone, Debug)]
pub struct ContainerItem {
    /// The row.
    pub row: ContainerRow,
    /// The file whose container it is.
    pub file: Arc<SerializedFile>,
}

const _: () = crate::send_sync::<ContainerItem>();

/// A container pass's index of each bundle's entry names, lower-cased, to
/// the entry the first of them names (rule 4's scan of a bundle's `files`
/// as one lookup): built once for each bundle the pass meets, charged to the
/// bundle's meter, and dropped with the pass.
#[derive(Default)]
pub(super) struct Siblings {
    bundles: HashMap<usize, SiblingNames>,
}

struct SiblingNames {
    /// Keeps the bundle, and so the address that keys it, for the pass.
    _bundle: Arc<BundleFile>,
    names: HashMap<RawStr, usize>,
    _charge: Charge,
}

impl Siblings {
    /// The directory index of `bundle`'s first `files` entry whose name
    /// lowers to `clean`.
    pub(super) fn find(
        &mut self,
        bundle: &Arc<BundleFile>,
        clean: &RawStr,
    ) -> Result<Option<usize>, Error> {
        let key = Arc::as_ptr(bundle).addr();
        let entry = match self.bundles.entry(key) {
            std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                let meter = bundle.meter();
                let slots = bundle.files();
                let n = u64::try_from(slots.len()).unwrap_or(u64::MAX);
                let charge = meter.charge(
                    HASH_TABLE_SHARES
                        .saturating_mul((size_of::<RawStr>() + size_of::<usize>() + 1) as u64)
                        .saturating_mul(n.saturating_add(1)),
                )?;
                let mut names = HashMap::with_capacity(slots.len());
                for s in slots {
                    names
                        .entry(py_lower(&s.name, meter)?)
                        .or_insert(s.dir_index);
                }
                v.insert(SiblingNames {
                    _bundle: bundle.clone(),
                    names,
                    _charge: charge,
                })
            }
        };
        Ok(entry.names.get(clean.as_bytes()).copied())
    }
}

/// What one external costs a preload walk's map of answers:
/// [`HASH_TABLE_SHARES`] buckets.
const TARGET_SHARE: u64 = HASH_TABLE_SHARES * (size_of::<i32>() + size_of::<Known>() + 1) as u64;

/// No table index: the end of a list in [`Targets::next`].
const NONE: usize = usize::MAX;

/// A preload walk's answers for its file's externals, each kept only while
/// UnityPy's next dereference would give the same one: a bundle sibling, or
/// a file id past the externals' end, for the walk; an answer of `files` or
/// the registry (found or not) until a change to a key its lookup read
/// ([`super::watch`]: a load's registration, any thread's, the resolver's
/// included, and under [`LoadMode::Lazy`] the entries a reach registers),
/// so the answer UnityPy's next lookup could find changed is asked again
/// and every other is kept; and the resolver's load not at all (the next
/// dereference looks again, and finds it if the load registered the name
/// asked for, or loads again if not, as UnityPy's `find_file` does when its
/// walk matched a longer file name by suffix). An external is looked up
/// once, and once more per change to one of its keys, so a walk costs its
/// table and its externals' names (plan decision 11), plus a lookup per
/// change to a key it asked, each counted as work on the walking file's
/// meter (`STEP_WORK` and the external's path's bytes; review round 4:
/// every change, any thread's, expired every answer, so a table of pointers
/// to long names cost the table times the names).
///
/// Each answer with a flag (one of `files`, the registry or the resolver's
/// load) lists the table indexes dereferenced under it (`answers`, linked
/// through `next`), so when a load of the walk's own sets its flag
/// ([`Walk::fired`]) those indexes are dereferenced again at the next row
/// whose span reaches them (review round 13: a walk that dereferenced each
/// index once never gave a later row's path to the file a load in between
/// put under the key the index's external resolves through).
///
/// [`LoadMode::Lazy`]: super::LoadMode::Lazy
///
/// Each charge is released after the memory it pays for (plan section 3.15):
/// fields drop in order, so `known` and `answers` (with the flags) go
/// first, then the walk's watches with their own charge, then `known`'s
/// charge (review round 5: the watches' charge was released before their
/// entries left the map).
struct Targets<'a> {
    known: HashMap<i32, Known>,
    /// Every flag the walk set up, by its number, with the head of the list
    /// of table indexes dereferenced under its answer.
    answers: MeteredVec<Answer>,
    /// Each table index's next in its answer's list, made at the first
    /// link: an index is in one list at most, the one of the answer it last
    /// got, and leaves it when the list is taken.
    next: Option<MeteredVec<usize>>,
    /// After `known` and `answers`, so the flags drop before the watches
    /// end.
    walk: Walk<'a>,
    /// `known`'s table, after it.
    charge: Charge,
}

/// One external's answer in [`Targets`].
struct Known {
    found: Option<Found>,
    /// Whether the walk keeps it: not the resolver's load, which the next
    /// dereference asks again.
    kept: bool,
    /// Set when a key its lookup read changes; `None`: kept for the walk (a
    /// sibling, a file id past the end).
    flag: Option<Arc<Flag>>,
}

/// The table indexes dereferenced under one flag's answer: the head of a
/// list linked through [`Targets::next`].
struct Answer {
    /// Kept alive while the walk runs, so a change sets it: a resolver
    /// load's flag lives here only.
    _flag: Arc<Flag>,
    head: usize,
}

/// The work a preload walk counts on its file's meter for each index it
/// puts back to be dereferenced again ([`Targets::reopen`]): a step for the
/// dereference, and one per level of the [`Pending`] set for putting the
/// index back (the find and the removal that dereference makes read no
/// more words than a first dereference's).
fn reopen_work(pending: &Pending) -> u64 {
    STEP_WORK.saturating_mul(1 + u64::try_from(pending.levels()).unwrap_or(u64::MAX))
}

impl Targets<'_> {
    /// The answer kept for `file_id`, if it holds.
    fn kept(&self, file_id: i32) -> Option<&Known> {
        self.known
            .get(&file_id)
            .filter(|k| k.kept && k.flag.as_ref().is_none_or(|f| !f.changed()))
    }

    /// Records `found` for `file_id`, kept (`kept`) while `flag` is unset
    /// (`None`: for the walk).
    fn keep(
        &mut self,
        file_id: i32,
        found: Option<Found>,
        kept: bool,
        flag: Option<Arc<Flag>>,
    ) -> Result<(), Error> {
        if !self.known.contains_key(&file_id) {
            self.charge.grow(TARGET_SHARE)?;
        }
        self.known.insert(file_id, Known { found, kept, flag });
        Ok(())
    }

    /// Lists table index `i` (of a table of `len`) under the answer of flag
    /// `id`.
    fn link(&mut self, i: usize, id: usize, len: usize, meter: &Arc<Meter>) -> Result<(), Error> {
        if self.next.is_none() {
            let mut n = MeteredVec::with_capacity(meter, len)?;
            for _ in 0..len {
                n.push(NONE)?;
            }
            self.next = Some(n);
        }
        if let (Some(answer), Some(slot)) = (
            self.answers.as_mut_slice().get_mut(id),
            self.next.as_mut().and_then(|n| n.as_mut_slice().get_mut(i)),
        ) {
            *slot = answer.head;
            answer.head = i;
        }
        Ok(())
    }

    /// Puts back into `pending` every table index listed under an answer
    /// whose flag a load of the walk's own set since the last call, each
    /// counted on `meter` first ([`reopen_work`]); the lists are taken, so
    /// an index is put back once per dereference.
    fn reopen(&mut self, pending: &mut Pending, meter: &Meter) -> Result<(), Error> {
        for id in self.walk.fired() {
            let Some(answer) = self.answers.as_mut_slice().get_mut(id) else {
                continue;
            };
            let mut i = std::mem::replace(&mut answer.head, NONE);
            let links = self.next.as_ref().map(MeteredVec::as_slice);
            while i != NONE {
                meter.count_work(reopen_work(pending))?;
                pending.insert(i);
                i = links.and_then(|l| l.get(i)).copied().unwrap_or(NONE);
            }
        }
        Ok(())
    }
}

/// A dereference failure as a file-level error (the container preload's
/// raise): a load failure as it is, any other as [`Error::Deref`].
fn deref_error(e: DerefError) -> Error {
    match e {
        DerefError::Load(e) => e,
        other => Error::Deref(Arc::new(other)),
    }
}

impl Environment {
    /// `container`: every non-dependency CAB's container rows, CABs in
    /// registry order, after the preload index is built (`environment.py:208-226`),
    /// as an iterator. It walks the registry, as UnityPy walks `env.cabs`, so
    /// a CAB a dependency's replaced under its key is not listed.
    pub fn container(&self) -> Result<ContainerItems<'_>, Error> {
        self.build_container_index()?;
        Ok(ContainerItems {
            env: self,
            after: None,
            current: None,
        })
    }

    /// `obj.container` (`files/ObjectReader.py:199-204`): the container index
    /// built first ([`Self::container`]'s pass, whose failure is this call's),
    /// then the path the object's file's `path_dict` holds for its path id
    /// ([`crate::container`]): a row of the file's own container whose asset
    /// is the object, the last such row winning, else the first row whose
    /// preload span reached it. The paths live in the file the object was
    /// read from, as UnityPy's live in its `assets_file`, so a second handle
    /// a caller opened on the same bytes holds only its own rows'.
    ///
    /// Where a dependency may be evicted (every option but
    /// [`EnvOptions::unitypy`]'s, [`EnvOptions::dep_cache_bytes`]), an
    /// object of a dependency's file is refused with
    /// [`Unsupported::EvictableContainerPath`]: an eviction drops the paths
    /// the preload set in that file and its reopen does not walk again, so
    /// the answer would depend on when the LRU evicted (`PARITY.md` section
    /// 19.3). The preload sets no path in such a file.
    ///
    /// [`EnvOptions::unitypy`]: super::EnvOptions::unitypy
    /// [`EnvOptions::dep_cache_bytes`]: super::EnvOptions::dep_cache_bytes
    pub fn container_path(&self, obj: &ObjectRef) -> Result<Option<ContainerPath>, Error> {
        let sf = obj.file();
        if self.may_reopen(sf) {
            return Err(Error::Unsupported(Unsupported::EvictableContainerPath));
        }
        self.build_container_index()?;
        sf.container()?.path_of(obj.path_id(), sf.meter())
    }

    /// Whether `sf` is a dependency's file the LRU may evict and the
    /// Environment open again.
    fn may_reopen(&self, sf: &SerializedFile) -> bool {
        !never_evicts(&self.inner.options) && sf.meter().reservation() == Reservation::Dependency
    }

    /// `_build_container_index` (`environment.py:208-215`): once until the
    /// next load's registration, every registered `SerializedFile`'s preload
    /// table, dependencies' included, in registry order. UnityPy sets its
    /// flag before the loop and each registration clears it, so a pass that
    /// fails leaves the index built unless the pass itself registered
    /// something; here a pass marks built the registry generation it started
    /// at when it ends or fails with an error the input decides, and any
    /// load's registration since starts a new generation. A refusal (a
    /// budget, meter or I/O failure, [`crate::once::CellError::is_transient`])
    /// is never kept (plan 4.1): the pass is not marked built, and the next
    /// ask walks again (review round 5: a pool refused once left the preload
    /// unwalked for the generation). What a walk the file's own meter refused
    /// was granted counts as that open's work, so a refusal the next ask
    /// would repeat stops at [`crate::LimitKind::DecodeWork`], which is kept
    /// ([`Self::walk_counted`]). A slot's table is marked walked only when its
    /// walk ends (UnityPy clears `_preload_table` then). A pass whose own
    /// dereferences make a new key fails as UnityPy's loop over
    /// `cabs.values()` does (`RuntimeError: dictionary changed size during
    /// iteration`), at the step after; a key a nested container's entries
    /// make under [`LoadMode::Lazy`] is not one (an eager load made it at
    /// load), nor is one another thread's load makes. Under `Lazy` the pass
    /// first reaches every registered entry and every dropped one, an
    /// evicted dependency's opened again (the registry's module docs; plan
    /// section 9, Q13), so it walks what an eager load registered; a slot
    /// dropped after a pass is a later load's, which starts a new
    /// generation, so a pass finished at the current one leaves none
    /// unreached but for a kept refusal's. Two passes may run at once: each walks every table not yet marked, and each `path_dict`
    /// entry keeps the least stamp ([`crate::container`]), so both leave
    /// what one after the other would.
    ///
    /// [`LoadMode::Lazy`]: super::LoadMode::Lazy
    fn build_container_index(&self) -> Result<(), Error> {
        let reg = &self.inner.registry;
        // Every slot dropped before this call is reached first, as by every
        // other reader (plan section 3.16, R3); a refusal is the walk's to
        // meet, or the next call's.
        reg.settle().ok();
        let generation = reg.generation();
        if reg.built(generation) {
            return Ok(());
        }
        let result = self.container_pass(generation);
        if result.as_ref().err().is_none_or(|e| !e.is_transient()) {
            reg.publish_built(generation);
        }
        result
    }

    /// [`Self::preload`] of `sf`'s table, as a lazy computation of `sf`'s
    /// open (`crate::once`'s rule on work): when `sf`'s own meter refuses
    /// it, what the meter granted it on this thread counts as the open's
    /// work, and once that work has passed the allowance the refusal is
    /// [`crate::LimitKind::DecodeWork`], which the input decides, so the pass
    /// keeps it.
    fn walk_counted(
        &self,
        sf: &Arc<SerializedFile>,
        container: &Container,
        at: (u64, &Rank),
        siblings: &mut Siblings,
    ) -> Result<(), Error> {
        let meter = sf.meter();
        let scope = GrantScope::enter(meter);
        let walked = self.preload(sf, container, at, siblings);
        let own = scope.finish(meter);
        match walked {
            Err(e) if e.is_meter_refusal() => {
                meter.add_work(own);
                Err(meter.work_used_up().map_or(e, Error::Limit))
            }
            other => other,
        }
    }

    fn container_pass(&self, generation: u64) -> Result<(), Error> {
        let reg = &self.inner.registry;
        let changed = || Error::Malformed {
            what: "container",
            detail: "dictionary changed size during iteration".into(),
        };
        // Under `Lazy`, every registered entry is reached before the walk
        // (the registry's module docs), so a nested container's entries are
        // walked at the positions and with the values an eager load gives
        // them, whichever container the walk would meet first (review round
        // 4: a later container's entry replaced the value of a key the walk
        // had passed, and the walk had read the earlier one).
        if self.inner.options.load == LoadMode::Lazy {
            let mut after: Option<Rank> = None;
            while let Some((rank, slot)) = reg.slot_after(after.as_ref()) {
                reg.reach(&slot, None)?;
                after = Some(rank);
            }
            // Every slot a registration dropped, those these reaches dropped
            // included, and those of an evicted dependency, opened again
            // (plan section 9, Q13); a refusal there is the pass's.
            reg.settle_for_pass()?;
        }
        let mut siblings = Siblings::default();
        let mut after: Option<Rank> = None;
        let mut grew = false;
        while let Some((rank, slot)) = reg.slot_after(after.as_ref()) {
            if grew {
                return Err(changed());
            }
            // The dependency's file is held while its table is walked, so
            // an eviction the walk makes cannot drop the CAB's bundle.
            let Some(held) = reg.resolve_held(&slot)? else {
                after = Some(rank);
                continue;
            };
            let Cab::Serialized(sf) = &held.cab else {
                after = Some(rank);
                continue;
            };
            if !slot.walked() {
                let before = load_keys_here();
                let mut walk = || {
                    let container = sf.container()?;
                    self.walk_counted(sf, container, (generation, &rank), &mut siblings)
                };
                // A dependency's table is walked on its meter: a walk the
                // pool refuses evicts the others and walks again
                // (`registry::with_room`; review round 7: under either mode
                // a pool with no room for the walk failed every pass). A
                // walk is idempotent: each `path_dict` entry keeps the
                // least stamp.
                match &slot.kind {
                    SlotKind::Dependency { dep } => dep.with_room(walk)?,
                    SlotKind::Entry { .. } | SlotKind::Ready(_) => walk()?,
                }
                slot.set_walked();
                grew = load_keys_here() != before;
            }
            after = Some(rank);
            drop(held);
        }
        if grew {
            return Err(changed());
        }
        Ok(())
    }

    /// `ContainerHelper.parse_preload_table`
    /// (`helpers/ContainerHelper.py:31-49`): every row's preload pointers
    /// dereferenced, and each target's path id given the row's path in its
    /// file's `path_dict` unless it has one (`setdefault`);
    /// `FileNotFoundError` and `KeyError` skipped, any other failure raised.
    ///
    /// UnityPy's loop is quadratic (every row walks its whole span). Here a
    /// table index is dereferenced at the first row whose span reaches it,
    /// and at a later row only once a load of the walk's own has changed a
    /// key its external's lookup read since its last dereference
    /// ([`Pending`], and the lists of [`Targets`]): UnityPy's dereference
    /// that gives the file the last one gave sets nothing (`setdefault`), so
    /// each file a dereference reaches gets the path of the first row that
    /// reaches it there, as in UnityPy (review round 13: each index was
    /// dereferenced once, so the file a load in between put under its
    /// external's key got no later row's path). Each external's answer is
    /// kept while it holds ([`Targets`]), so an external is looked up once,
    /// and once more per change to a key its lookup read, under every option
    /// set and whatever other keys other threads change meanwhile
    /// (`EnvStats::preload_lookups` counts them); each bundle's names indexed
    /// once per pass ([`Siblings`]), and each row's path copied once, charged
    /// to this file's meter. So a walk no load changes is linear in the
    /// rows, the table and the externals (plan decision 11, counting no
    /// work), and what changes make it do again (an index put back, an
    /// external looked up again) is counted as work on this file's meter,
    /// whose allowance bounds a table whose own loads change its answers
    /// over and over ([`crate::LimitKind::DecodeWork`]).
    ///
    /// A dereference of an external answered before answers as UnityPy's
    /// would: `find_file` registers what it loads under the path it found,
    /// whose basename is the name asked when the walk found that name, and a
    /// longer one when it matched a file name by suffix (`ab.assets` finds
    /// `xab.assets`, `environment.py:345-354`), so every dereference of that
    /// external loads it again and the last copy keeps its key. The index
    /// that loaded such a copy is dereferenced again only once a change to a
    /// key its lookup read makes the name it asked findable; until then,
    /// where rows' spans overlap, the external is loaded once per index, not
    /// once per row and index (`PARITY.md` section 19.3; plan section 9,
    /// Q11).
    fn preload(
        &self,
        sf: &Arc<SerializedFile>,
        container: &Container,
        at: (u64, &Rank),
        siblings: &mut Siblings,
    ) -> Result<(), Error> {
        if matches!(container.preload_table(), Ok(None)) {
            return Ok(());
        }
        let malformed = |detail: &'static str| Error::Malformed {
            what: "container",
            detail: detail.into(),
        };
        let meter = sf.meter();
        let mut pending: Option<Pending> = None;
        let mut targets = Targets {
            known: HashMap::new(),
            answers: MeteredVec::new(meter),
            next: None,
            walk: self.inner.registry.watches().start(meter),
            charge: Charge::zero(meter),
        };
        for (r, row) in container.rows().iter().enumerate() {
            // `start = info.preloadIndex`, `size = info.preloadSize`: a
            // member the AssetInfo lacks raises; one that is no `int` raises
            // here too (PARITY.md section 19.3).
            let (Some(start), Some(size)) = (row.preload_index, row.preload_size) else {
                return Err(malformed("an AssetInfo index is not an int"));
            };
            if start < 0 || size <= 0 {
                continue;
            }
            // `len(self._preload_table)`: a table that is not a list raises.
            let table = container.preload_table()?.unwrap_or_default();
            if start + size > i128::try_from(table.len()).unwrap_or(i128::MAX) {
                continue;
            }
            let (Ok(s), Ok(e)) = (usize::try_from(start), usize::try_from(start + size)) else {
                continue;
            };
            let pending = match &mut pending {
                Some(u) => u,
                None => pending.insert(Pending::full(table.len(), meter)?),
            };
            let mut path: Option<Option<RawStr>> = None;
            let mut next = pending.first_from(s);
            while let Some(j) = next.filter(|&j| j < e) {
                pending.remove(j);
                next = pending.first_from(j.saturating_add(1));
                // An element that is no pointer raises (PARITY.md 19.3).
                let Some(p) = table.get(j).copied().flatten() else {
                    return Err(malformed("a preload table element is not a pointer"));
                };
                if !p.is_set() {
                    continue;
                }
                self.inner.preload_derefs.fetch_add(1, Ordering::Relaxed);
                let (target, answer) = self.preload_target(sf, p, &mut targets, siblings)?;
                if let Some(id) = answer {
                    targets.link(j, id, table.len(), meter)?;
                }
                // What this dereference's loads changed, this index's own
                // answer included, is dereferenced again from the next row
                // on, or later in this one past `j`.
                targets.reopen(pending, meter)?;
                next = pending.first_from(j.saturating_add(1));
                if let Some(target) = target
                    && !self.may_reopen(target.file())
                {
                    let tf = target.file();
                    let path = match &path {
                        Some(p) => p.clone(),
                        None => path
                            .insert(
                                row.path
                                    .as_ref()
                                    .map(|p| RawStr::charged(p.as_bytes(), meter))
                                    .transpose()?,
                            )
                            .clone(),
                    };
                    let stamp = (
                        at.0,
                        at.1.clone(),
                        u32::try_from(r).unwrap_or(u32::MAX),
                        u32::try_from(j).unwrap_or(u32::MAX),
                    );
                    tf.container()?
                        .set_default(p.path_id, path.as_ref(), &stamp, tf.meter())?;
                }
            }
        }
        Ok(())
    }

    /// `pptr.deref()` for a preload element: the object, or `None` where the
    /// preload skips (`FileNotFoundError`, `KeyError`); and the number of
    /// the flag whose setting changes the answer (`None`: an answer for the
    /// walk, the own file's, a sibling's or a file id past the end). An
    /// external's answer is kept in `targets` while it holds (charged to its
    /// charge); a lookup of an external the walk looked up before is
    /// counted as work on `sf`'s meter first (`STEP_WORK` and the bytes of
    /// the external's path, which the lookup cleans, simplifies and hashes).
    fn preload_target(
        &self,
        sf: &Arc<SerializedFile>,
        p: PPtr,
        targets: &mut Targets,
        siblings: &mut Siblings,
    ) -> Result<(Option<ObjectRef>, Option<usize>), Error> {
        let (found, answer) = if p.file_id == 0 {
            (Some(Found::File(File::Serialized(sf.clone()))), None)
        } else if let Some(k) = targets.kept(p.file_id) {
            (k.found.clone(), k.flag.as_ref().map(|f| f.id()))
        } else {
            if targets.known.contains_key(&p.file_id) {
                let bytes = Self::external_path(sf, p.file_id).map_or(0, <[u8]>::len);
                sf.meter().count_work(
                    STEP_WORK.saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX)),
                )?;
            }
            self.inner.preload_lookups.fetch_add(1, Ordering::Relaxed);
            // Watched before the lookup reads, so no change it misses is
            // missed by the flag; its answer's list made first, so the flag's
            // number is its place there.
            let keys = Self::lookup_keys(sf, p.file_id, &targets.walk)?;
            targets.answers.reserve(1)?;
            let flag = targets.walk.watch(&keys)?;
            let id = flag.id();
            targets.answers.push(Answer {
                _flag: flag.clone(),
                head: NONE,
            })?;
            #[cfg(test)]
            self.inner.registry.hook(super::registry::Hook::Watched);
            // What to record: kept until a change, kept for the walk, or
            // not kept (the resolver's load).
            let (found, kept, flag) = match self.external(sf, p.file_id, Some(siblings)) {
                Ok((t, Reached::Sibling)) => (Some(t), true, None),
                Ok((t, Reached::Lookup)) => (Some(t), true, Some(flag)),
                Ok((t, Reached::Load)) => (Some(t), false, Some(flag)),
                Err(DerefError::FileIdPastEnd { .. }) => (None, true, None),
                Err(DerefError::NotFound { .. }) => (None, true, Some(flag)),
                Err(e) => return Err(deref_error(e)),
            };
            let answer = flag.is_some().then_some(id);
            targets.keep(p.file_id, found.clone(), kept, flag)?;
            (found, answer)
        };
        let Some(found) = found else {
            return Ok((None, answer));
        };
        match Self::object_of(&found, p.path_id) {
            Ok(o) => Ok((Some(o), answer)),
            Err(DerefError::MissingObject { .. }) => Ok((None, answer)),
            Err(e) => Err(deref_error(e)),
        }
    }

    /// External `file_id`'s stored path, if `sf` has that external.
    fn external_path(sf: &SerializedFile, file_id: i32) -> Option<&[u8]> {
        let externals = sf.externals();
        pptr::external_index(file_id, externals.len())
            .ok()
            .and_then(|i| externals.get(i))
            .map(|e| e.path.as_bytes())
    }

    /// The keys external `file_id`'s lookup read past `sf`'s bundle names
    /// ([`Self::external`]): the registry key `find_file` asks
    /// (`simplify_name` twice), and for a top-level file the lowered `files`
    /// key its sibling scan matches.
    fn lookup_keys(sf: &SerializedFile, file_id: i32, walk: &Walk) -> Result<Vec<u64>, Error> {
        let externals = sf.externals();
        let Some(external) = pptr::external_index(file_id, externals.len())
            .ok()
            .and_then(|i| externals.get(i))
        else {
            return Ok(Vec::new());
        };
        let meter = sf.meter();
        let clean = clean_external_path(&external.path, meter)?;
        let simple = simplify_name(&simplify_name(&clean, meter)?, meter)?;
        let mut keys = vec![walk.hash(simple.as_bytes())];
        if sf.parent().is_none() {
            keys.push(walk.hash(clean.as_bytes()));
        }
        Ok(keys)
    }
}
