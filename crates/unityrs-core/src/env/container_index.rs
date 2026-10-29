//! The container: [`Environment::container`], [`Environment::container_path`]
//! and the pass that builds their index (UnityPy's
//! `_build_container_index`), the walk of each file's preload table.

use std::collections::HashMap;
use std::fmt;
use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::registry::{Rank, load_keys_here};
use super::{Cab, DerefError, Environment, Found, PPtr, never_evicts};
use crate::budget::Reservation;
use crate::container::{Container, ContainerPath, ContainerRow};
use crate::error::{Error, Unsupported};
use crate::files::{BundleFile, File, SerializedFile};
use crate::limits::{Charge, HASH_TABLE_SHARES, Meter, MeteredVec};
use crate::object::ObjectRef;
use crate::pystr::py_lower;
use crate::raw::RawStr;

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

/// The skip list of a preload walk: `next[i]` leads to the first table
/// index from `i` not yet dereferenced (a disjoint-set forest whose finds
/// compress their paths), so each index is dereferenced once however many
/// rows' spans cover it.
struct Undone(MeteredVec<usize>);

impl Undone {
    fn new(len: usize, meter: &Arc<Meter>) -> Result<Self, Error> {
        let mut next = MeteredVec::with_capacity(meter, len.saturating_add(1))?;
        for i in 0..=len {
            next.push(i)?;
        }
        Ok(Self(next))
    }

    /// The first index from `i` not yet done (the table's length past it).
    fn find(&mut self, i: usize) -> usize {
        let next = self.0.as_mut_slice();
        let mut root = i;
        while let Some(&n) = next.get(root) {
            if n == root {
                break;
            }
            root = n;
        }
        let mut at = i;
        while let Some(n) = next.get_mut(at) {
            let up = *n;
            if up == root {
                break;
            }
            *n = root;
            at = up;
        }
        root
    }

    /// Marks `i` done.
    fn done(&mut self, i: usize) {
        if let Some(n) = self.0.as_mut_slice().get_mut(i) {
            *n = i.saturating_add(1);
        }
    }
}

/// What one table entry of a preload walk costs the walk's map of the
/// file's externals: [`HASH_TABLE_SHARES`] buckets.
const TARGET_SHARE: u64 =
    HASH_TABLE_SHARES * (size_of::<i32>() + size_of::<Option<Found>>() + 1) as u64;

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
    /// registry order, after the preload index is built (`environment.py:208-227`),
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

    /// `_build_container_index` (`environment.py:208-216`): once until the
    /// next load's registration, every registered `SerializedFile`'s preload
    /// table, dependencies' included, in registry order. UnityPy sets its
    /// flag before the loop and each registration clears it, so a pass that
    /// fails leaves the index built unless the pass itself registered
    /// something; here a pass marks built the registry generation it started
    /// at, whether it ends or fails, and any load's registration since starts
    /// a new generation. A slot's table is marked walked only when its
    /// walk ends (UnityPy clears `_preload_table` then). A pass whose own
    /// dereferences make a new key fails as UnityPy's loop over
    /// `cabs.values()` does (`RuntimeError: dictionary changed size during
    /// iteration`), at the step after; a key a nested container's entries
    /// make under [`LoadMode::Lazy`] is not one (an eager load made it at
    /// load), nor is one another thread's load makes. Two passes may run at
    /// once: each walks every table not yet marked, and each `path_dict`
    /// entry keeps the least stamp ([`crate::container`]), so both leave
    /// what one after the other would.
    ///
    /// [`LoadMode::Lazy`]: super::LoadMode::Lazy
    fn build_container_index(&self) -> Result<(), Error> {
        let reg = &self.inner.registry;
        let generation = reg.generation();
        if reg.built(generation) {
            return Ok(());
        }
        let result = self.container_pass(generation);
        reg.publish_built(generation);
        result
    }

    fn container_pass(&self, generation: u64) -> Result<(), Error> {
        let reg = &self.inner.registry;
        let changed = || Error::Malformed {
            what: "container",
            detail: "dictionary changed size during iteration".into(),
        };
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
                let container = sf.container()?;
                let before = load_keys_here();
                self.preload(sf, container, (generation, &rank), &mut siblings)?;
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
    /// Linear in the rows, the table and the externals (plan decision 11),
    /// where UnityPy's loop is quadratic (every row walks its whole span):
    /// each table index is dereferenced once ([`Undone`]), at the first row
    /// whose span reaches it, which is the row whose `setdefault` UnityPy
    /// keeps; each external is resolved once ([`Self::external`]), each
    /// bundle's names indexed once per pass ([`Siblings`]), and each row's
    /// path copied once, charged to this file's meter. A repeated
    /// dereference answers as the first did: the one load it can make is
    /// `find_file`'s, which UnityPy registers under the path it found, whose
    /// simplified basename is the name asked, so the next lookup finds it (a
    /// container it loads, which registers other names, raises at its first
    /// element). A resolver that names its source otherwise, which UnityPy's
    /// walk cannot, is loaded once per walk.
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
        let mut undone: Option<Undone> = None;
        let mut targets: HashMap<i32, Option<Found>> = HashMap::new();
        let mut targets_charge = Charge::zero(meter);
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
            let undone = match &mut undone {
                Some(u) => u,
                None => undone.insert(Undone::new(table.len(), meter)?),
            };
            let mut path: Option<Option<RawStr>> = None;
            let mut j = undone.find(s);
            while j < e {
                undone.done(j);
                // An element that is no pointer raises (PARITY.md 19.3).
                let Some(p) = table.get(j).copied().flatten() else {
                    return Err(malformed("a preload table element is not a pointer"));
                };
                if !p.is_set() {
                    j = undone.find(j.saturating_add(1));
                    continue;
                }
                self.inner.preload_derefs.fetch_add(1, Ordering::Relaxed);
                if let Some(target) =
                    self.preload_target(sf, p, &mut targets, &mut targets_charge, siblings)?
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
                j = undone.find(j.saturating_add(1));
            }
        }
        Ok(())
    }

    /// `pptr.deref()` for a preload element: the object, or `None` where the
    /// preload skips (`FileNotFoundError`, `KeyError`). Each external is
    /// resolved once per walk, in `targets` (charged to `charge`).
    fn preload_target(
        &self,
        sf: &Arc<SerializedFile>,
        p: PPtr,
        targets: &mut HashMap<i32, Option<Found>>,
        charge: &mut Charge,
        siblings: &mut Siblings,
    ) -> Result<Option<ObjectRef>, Error> {
        let found = if p.file_id == 0 {
            Some(Found::File(File::Serialized(sf.clone())))
        } else if let Some(t) = targets.get(&p.file_id) {
            t.clone()
        } else {
            let t = match self.external(sf, p.file_id, Some(siblings)) {
                Ok(t) => Some(t),
                Err(DerefError::FileIdPastEnd { .. } | DerefError::NotFound { .. }) => None,
                Err(e) => return Err(deref_error(e)),
            };
            charge.grow(TARGET_SHARE)?;
            targets.insert(p.file_id, t.clone());
            t
        };
        let Some(found) = found else {
            return Ok(None);
        };
        match Self::object_of(&found, p.path_id) {
            Ok(o) => Ok(Some(o)),
            Err(DerefError::MissingObject { .. }) => Ok(None),
            Err(e) => Err(deref_error(e)),
        }
    }
}
