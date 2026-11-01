//! Lookups and dereference: `get_cab`, `find_file` and `PPtr.deref`, by
//! the rules of [`super::pptr`]. "Module docs" below are [`super`]'s.

use std::sync::Arc;

use super::container_index::Siblings;
use super::resource_data::top_bundle;
use super::{Cab, DerefError, Environment, Found, HeldCab, PPtr, RawCab, pptr};
use crate::error::Error;
use crate::files::{File, ParentKind, SerializedFile};
use crate::limits::Meter;
use crate::object::ObjectRef;
use crate::pystr::{clean_external_path, lower_eq, simplify_name};
use crate::raw::RawStr;

/// `objects[m_PathID]` of a target file (rule 6).
fn object_in(sf: &Arc<SerializedFile>, path_id: i64) -> Result<ObjectRef, DerefError> {
    sf.object(path_id).ok_or_else(|| DerefError::MissingObject {
        file: sf.name().clone(),
        path_id,
    })
}

/// How [`Environment::external`] reached the file it found, which says how
/// long a container preload walk may keep the answer (UnityPy keeps none:
/// each dereference looks again, `classes/PPtr.py:57-85`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reached {
    /// A sibling in the asking file's bundle, whose names never change.
    Sibling,
    /// This Environment's `files` or registry, which a load may change.
    Lookup,
    /// The resolver's file, loaded now: under the name it was asked for or
    /// another, which only the next lookup tells.
    Load,
}

impl Environment {
    /// `get_cab`: the registry under `simplify_name(name)`, an entry parsed
    /// now if it was registered lazily. The lowered key a lookup builds is
    /// charged to `meter` while the lookup runs (the meter of the file whose
    /// input chose `name`; a caller's own string may use
    /// [`Meter::unlimited`]), and a name already in its simplified form is
    /// not copied ([`Registry::get`]). A dependency's CAB may outlive its
    /// bundle in the LRU: [`Self::get_cab_held`] returns the handle that
    /// keeps it. A pool refusal anywhere in the lookup, the key's charge to
    /// a dependency's meter included, evicts the others and retries once
    /// (plan section 3.13).
    ///
    /// [`Registry::get`]: super::Registry::get
    pub fn get_cab(&self, name: &RawStr, meter: &Arc<Meter>) -> Result<Option<Cab>, Error> {
        Ok(self.get_cab_held(name, meter)?.map(|h| h.cab))
    }

    /// [`Self::get_cab`], with the handle that keeps a dependency's CAB's
    /// container alive while the caller holds it ([`HeldCab`]): an eviction
    /// drops only the LRU's handle, so the CAB's pointers into its sibling
    /// entries keep resolving (rather than failing with
    /// [`DerefError::ContainerDropped`]).
    pub fn get_cab_held(
        &self,
        name: &RawStr,
        meter: &Arc<Meter>,
    ) -> Result<Option<HeldCab>, Error> {
        self.room(|| self.inner.registry.get_held(name, meter))
    }

    /// `find_file(name, is_dependency=True)`: [`Self::get_cab`], else the
    /// resolver, loading what it returns as a dependency: with
    /// [`EnvOptions::keep_resolver_loads`] also a [`FileEntry`] with
    /// `is_dependency` set, under the resolved source's name, never evicted
    /// (module docs). `meter` as for [`Self::get_cab`]. A container the
    /// resolver loaded is no CAB (`None`).
    ///
    /// [`EnvOptions::keep_resolver_loads`]: super::EnvOptions::keep_resolver_loads
    /// [`FileEntry`]: super::FileEntry
    pub fn find_file(&self, name: &RawStr, meter: &Arc<Meter>) -> Result<Option<Cab>, Error> {
        Ok(match self.find(name, meter)?.map(|(found, _)| found) {
            Some(Found::Cab(cab)) => Some(cab),
            Some(Found::File(File::Serialized(sf))) => Some(Cab::Serialized(sf)),
            Some(Found::File(File::Raw(raw))) => Some(Cab::Raw(RawCab::File(raw))),
            Some(Found::File(File::Bundle(_) | File::Web(_))) | None => None,
        })
    }

    /// [`Self::find_file`]'s lookup, and how it reached what it found. A
    /// pool refusal in the registry's lookup evicts the others and retries
    /// it once; the resolver's load takes its own (plan section 3.13).
    fn find(&self, name: &RawStr, meter: &Arc<Meter>) -> Result<Option<(Found, Reached)>, Error> {
        // UnityPy environment.py:326-329: `get_cab(simplify_name(name))`,
        // which simplifies again.
        let found = self.room(|| {
            let simple = simplify_name(name, meter)?;
            self.inner.registry.get(&simple, meter)
        })?;
        if let Some(cab) = found {
            return Ok(Some((Found::Cab(cab), Reached::Lookup)));
        }
        // :330-354: the resolver stands for the walk of `env.path`.
        let Some(resolver) = self.inner.options.resolver.clone() else {
            return Ok(None);
        };
        let Some(source) = resolver.resolve(name.as_bytes()) else {
            return Ok(None);
        };
        let file = self.load_resolved(&source, None)?;
        Ok(Some((Found::File(file), Reached::Load)))
    }

    /// `PPtr.deref` from an object in `from` (`classes/PPtr.py:43-90`), by
    /// the rules of [`pptr`]: `from`'s container is its bundle
    /// ([`crate::files::ParentKind::Bundle`]'s handle), or, for a file loaded
    /// at top level, this Environment, whose [`Self::files`] keys the sibling
    /// scan compares. `from` need not have been loaded here (a batch job's
    /// own bundle handle): its bundle siblings come from its parent, every
    /// other CAB from this Environment. When `from`'s bundle has been
    /// dropped (its weak handle no longer upgrades) the dereference fails
    /// with [`DerefError::ContainerDropped`] rather than guess: UnityPy
    /// cannot reach that state, since an object keeps its file's parent
    /// alive, so the bindings and a batch engine keep a strong bundle handle
    /// beside every `SerializedFile` they hand out. A pool refusal anywhere
    /// in the dereference (reading or looking up from a dependency's file)
    /// evicts the others and retries once, `from`'s top-level bundle held
    /// meanwhile (plan section 3.13; review round 8: it had no retry).
    pub fn deref(&self, from: &Arc<SerializedFile>, pptr: PPtr) -> Result<ObjectRef, DerefError> {
        // UnityPy classes/PPtr.py:49-50
        if pptr.path_id == 0 {
            return Err(DerefError::Null);
        }
        // :53-54
        if pptr.file_id == 0 {
            return object_in(from, pptr.path_id);
        }
        let _top = top_bundle(from);
        // Nothing after a resolver's load charges anything, so a retry never
        // loads again what the first run loaded.
        self.room(|| {
            self.reach_chain(from)?;
            let (target, _) = self.external(from, pptr.file_id, None)?;
            Self::object_of(&target, pptr.path_id)
        })
    }

    /// The file `from`'s external `file_id` names (`classes/PPtr.py:57-85`):
    /// a sibling in `from`'s container (the bundle's `files`, or for a
    /// top-level file this Environment's), else `find_file`; and how it was
    /// reached ([`Reached`]). `siblings` is a container pass's index of
    /// bundle names; without it the bundle's names are scanned.
    pub(super) fn external(
        &self,
        from: &Arc<SerializedFile>,
        file_id: i32,
        siblings: Option<&mut Siblings>,
    ) -> Result<(Found, Reached), DerefError> {
        // :57-60
        let externals = from.externals();
        let external = match pptr::external_index(file_id, externals.len()) {
            Ok(i) => externals.get(i),
            Err(pptr::ExternalIndexError::PastEnd) => {
                return Err(DerefError::FileIdPastEnd { file_id });
            }
            Err(pptr::ExternalIndexError::BelowStart) => None,
        };
        let Some(external) = external else {
            return Err(DerefError::FileIdBelowStart { file_id });
        };
        let meter = from.meter();
        // :68-73
        let clean = clean_external_path(&external.path, meter).map_err(Error::from)?;
        // :63-78: the container's `files`, each key lower-cased, the first
        // equal one wins.
        let sibling = match from.parent() {
            Some(parent) => match &parent.kind {
                ParentKind::Bundle { bundle, .. } => {
                    let Some(bundle) = bundle.upgrade() else {
                        return Err(DerefError::ContainerDropped {
                            name: parent.name.clone(),
                        });
                    };
                    let hit = match siblings {
                        Some(index) => index.find(&bundle, &clean)?,
                        None => bundle
                            .files()
                            .iter()
                            .find(|s| lower_eq(s.name.as_bytes(), clean.as_bytes()))
                            .map(|s| s.dir_index),
                    };
                    match hit {
                        Some(i) => {
                            let f = bundle.entry(i)?;
                            // Named as a sibling, a nested container is
                            // reached, as a `get` of its name would reach it.
                            if let File::Bundle(nested) = &f {
                                self.reach_sibling(from, &bundle, i, nested)?;
                            }
                            Some((f, Reached::Sibling))
                        }
                        None => None,
                    }
                }
                ParentKind::Web => None,
            },
            None => self.files_match(&clean).map(|f| (f, Reached::Lookup)),
        };
        match sibling {
            Some((f, reached)) => Ok((Found::File(f), reached)),
            // :80-85
            None => match self.find(&clean, meter)? {
                Some(t) => Ok(t),
                None => Err(DerefError::NotFound {
                    path: external.path.clone(),
                }),
            },
        }
    }

    /// The object holding `path_id` in what a dereference reached
    /// (`classes/PPtr.py:90`, `assetsfile_dst.objects[m_PathID]`).
    pub(super) fn object_of(target: &Found, path_id: i64) -> Result<ObjectRef, DerefError> {
        match target.clone() {
            Found::Cab(Cab::Serialized(sf)) | Found::File(File::Serialized(sf)) => {
                object_in(&sf, path_id)
            }
            Found::Cab(Cab::Raw(RawCab::InBundle { bundle, dir_index })) => {
                Err(DerefError::RawTarget {
                    name: bundle
                        .directory()
                        .get(dir_index)
                        .map(|d| d.path.clone())
                        .unwrap_or_default(),
                })
            }
            Found::Cab(Cab::Raw(RawCab::File(raw))) | Found::File(File::Raw(raw)) => {
                Err(DerefError::RawTarget {
                    name: raw.name.clone(),
                })
            }
            Found::File(File::Bundle(b)) => Err(DerefError::RawTarget {
                name: b.name().clone(),
            }),
            Found::File(File::Web(w)) => Err(DerefError::RawTarget {
                name: w.name().clone(),
            }),
        }
    }
}
