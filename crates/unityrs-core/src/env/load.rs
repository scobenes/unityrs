// Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py, UnityPy/files/BundleFile.py, UnityPy/files/File.py. See THIRD_PARTY_LICENSES.
//! Loads: [`Environment::load`], [`Environment::load_dependency`] and the
//! files the resolver returns, and the registration of their CABs.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(test)]
use super::registry::Hook;
use super::registry::{DepCache, Dependency, Open, Stage, open_source, with_room};
use super::{Cab, Environment, LoadMode, RawCab, Source, source_name};
use crate::budget::Reservation;
use crate::error::{Error, Unsupported};
use crate::files::{BundleFile, File};
use crate::limits::{Charge, Meter};
use crate::raw::RawStr;

/// Whose CABs a registration makes: a caller's file (slots that may hold
/// it), or a dependency's (slots that reopen it), registered one at a time
/// or staged, every one charged before any is published (the registry's
/// module docs, "A failed dependency load").
enum Target<'a> {
    Caller,
    Dependency(&'a Arc<Dependency>),
    Staged(&'a Arc<Dependency>, &'a mut Stage),
}

/// Who asked the resolver for the file [`Environment::load_resolved`]
/// loads, which decides how it is loaded and keyed.
#[derive(Clone, Copy, Debug)]
pub(super) enum Asked<'a> {
    /// `find_file(name, is_dependency)`: keyed by the source's name (UnityPy
    /// keys the path it found, `load_file` keying a path by itself).
    Find {
        /// UnityPy's flag (a dereference's `find_file` takes its default,
        /// `True`).
        is_dependency: bool,
    },
    /// A resource read's fallback, `load_file(name, True)`: keyed by the
    /// name it asked for.
    Fallback(&'a RawStr),
}

impl Environment {
    /// `load_file`: detect, parse, register the CABs, add to `files` under the
    /// source's name (a repeated name replaces the value in place, as a dict).
    // The source is handed over (the frozen signature); a caller's load
    // keeps nothing of it but its name.
    #[allow(clippy::needless_pass_by_value)]
    pub fn load(&self, source: Source) -> Result<File, Error> {
        let name = source_name(&source);
        let file = self.load_listed(&name, &source, false)?;
        self.inner.loaded.fetch_add(1, Ordering::Relaxed);
        Ok(file)
    }

    /// A load UnityPy's `load_file` makes (the caller's, or a resource
    /// fallback's under `keep_resolver_loads`): its own input meter, its
    /// CABs registered as a caller's, and a `files` entry under `name` at
    /// the load's number, as its registrations are ranked, charged to the
    /// file's meter (a resolver's name copied into its key first; a
    /// caller's own name is shared, not copied).
    fn load_listed(&self, name: &RawStr, source: &Source, resolved: bool) -> Result<File, Error> {
        let ctx = self.ctx();
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        // UnityPy environment.py:139-160
        let file = open_source(name, source, &meter, ctx)?;
        let load = self.register_file(name, &file, &mut Target::Caller)?;
        let key = if resolved {
            RawStr::charged(name.as_bytes(), &meter)?
        } else {
            name.clone()
        };
        self.list(key, file.clone(), false, load, &meter)?;
        Ok(file)
    }

    /// Registers the CABs of a top-level `file` loaded under `name`: the
    /// file itself, or a bundle's entries (`Eager`: as UnityPy parses them;
    /// `Lazy`: every directory entry's name now, parsed on first use), or
    /// stages them ([`Target::Staged`]: room for a lazy bundle's directory
    /// reserved first). Every registration is one load's, ranked by its
    /// directory path ([`registry`]); returns the load's number, which its
    /// `files` entry takes ([`Self::list`]).
    ///
    /// [`registry`]: super::registry
    fn register_file(
        &self,
        name: &RawStr,
        file: &File,
        target: &mut Target<'_>,
    ) -> Result<u64, Error> {
        let reg = &self.inner.registry;
        let load = reg.next_load();
        let registered = match file {
            File::Serialized(sf) => self.register_cab(
                name,
                Cab::Serialized(sf.clone()),
                file.meter(),
                target,
                load,
                &[],
            ),
            File::Raw(raw) => self.register_cab(
                name,
                Cab::Raw(RawCab::File(raw.clone())),
                file.meter(),
                target,
                load,
                &[],
            ),
            File::Bundle(b) => match self.inner.options.load {
                LoadMode::Eager => self.register_eager(b, target, load, &mut Vec::new()),
                LoadMode::Lazy => {
                    if let Target::Staged(_, stage) = target {
                        stage.reserve(b.directory().len())?;
                    }
                    for (i, d) in b.directory().iter().enumerate() {
                        match target {
                            Target::Caller => {
                                reg.register_entry(&d.path, b.clone(), i, load, &[i], b.meter())?;
                            }
                            Target::Dependency(dep) => {
                                reg.register_lazy_dependency(&d.path, dep, load, &[i], b.meter())?;
                            }
                            Target::Staged(dep, stage) => {
                                reg.stage(stage, &d.path, dep, load, &[i], b.meter(), true)?;
                            }
                        }
                    }
                    Ok(())
                }
            },
            File::Web(_) => Err(Error::Unsupported(Unsupported::WebFile)),
        };
        registered.map(|()| load)
    }

    fn register_cab(
        &self,
        name: &RawStr,
        cab: Cab,
        meter: &Arc<Meter>,
        target: &mut Target<'_>,
        load: u64,
        path: &[usize],
    ) -> Result<(), Error> {
        let reg = &self.inner.registry;
        match target {
            Target::Caller => reg.register_ready(name, cab, load, path, meter),
            Target::Dependency(dep) => reg.register_dependency(name, dep, load, path, meter),
            Target::Staged(dep, stage) => reg.stage(stage, name, dep, load, path, meter, false),
        }
    }

    /// UnityPy's eager bundle: every block decoded, then every entry parsed
    /// in directory order and each `SerializedFile` and raw entry registered
    /// as it is parsed (`files/BundleFile.py:168-186`, `files/File.py:76-90`),
    /// a nested bundle's entries in place. A failing entry fails the bundle
    /// with the CABs before it registered; a block that fails registers
    /// nothing (plan section 3.13). [`BundleFile::load_all`] decodes and
    /// parses first, so this walk only reads its cells: when it failed with
    /// an entry's error, the blocks all decoded and the entries before the
    /// failing one are registered; any other failure is the decode's.
    fn register_eager(
        &self,
        b: &Arc<BundleFile>,
        target: &mut Target<'_>,
        load: u64,
        path: &mut Vec<usize>,
    ) -> Result<(), Error> {
        let all = b.load_all();
        if let Err(e) = &all
            && !matches!(e, Error::Entry { .. })
        {
            return all;
        }
        for (i, d) in b.directory().iter().enumerate() {
            path.push(i);
            let step = b.entry(i).and_then(|f| {
                self.inner.eager_parsed.fetch_add(1, Ordering::Relaxed);
                match f {
                    File::Bundle(nested) => self.register_eager(&nested, target, load, path),
                    File::Serialized(sf) => self.register_cab(
                        &d.path,
                        Cab::Serialized(sf),
                        b.meter(),
                        target,
                        load,
                        path,
                    ),
                    File::Raw(_) => {
                        let cab = Cab::Raw(RawCab::InBundle {
                            bundle: b.clone(),
                            dir_index: i,
                        });
                        self.register_cab(&d.path, cab, b.meter(), target, load, path)
                    }
                    File::Web(_) => Ok(()),
                }
            });
            path.pop();
            if let Err(e) = step {
                return Err(all.err().unwrap_or(e));
            }
        }
        all
    }

    /// Loads `source` as a dependency, as `find_file` loads one through the
    /// resolver: its own meter under [`crate::budget::Reservation::Dependency`]
    /// (so a refusal names the dependency pool), its CABs registered (lazily
    /// under [`LoadMode::Lazy`]) without a strong handle, kept in the LRU and
    /// evictable, and not added to [`Self::files`] (UnityPy has no such call;
    /// a batch engine registers its inputs this way). The returned handle is
    /// the caller's: an eviction drops only the LRU's, so while the caller
    /// holds it the slots reach the same open file through their weak handle.
    ///
    /// Under [`LoadMode::Lazy`] a load that fails publishes nothing: every
    /// registration is charged, with the LRU's entry, before the first is
    /// published, so a refusal at any of them leaves the registry as it was
    /// (the registry's module docs, "A failed dependency load"). Under
    /// [`LoadMode::Eager`] the CABs are registered as they are parsed, as
    /// UnityPy's `load_file` registers them, so a load that fails keeps
    /// those registered before its failure (the partial registration of
    /// the module docs).
    // The source is handed over (the frozen signature) and kept by a clone.
    #[allow(clippy::needless_pass_by_value)]
    pub fn load_dependency(&self, source: Source) -> Result<File, Error> {
        let name = source_name(&source);
        self.load_dep(&name, &source)
    }

    fn load_dep(&self, name: &RawStr, source: &Source) -> Result<File, Error> {
        // A pool refusal evicts the others and retries once (plan 3.13); a
        // second refusal fails naming the pool, and nothing of the
        // dependency is kept (its first attempt's charges returned with its
        // handle; under `Lazy` neither attempt published a key).
        with_room(Some(&self.inner.cache), None, || {
            self.try_load_dep(name, source).map(|(file, _)| file)
        })
    }

    /// One attempt of [`Self::load_dep`], on a meter of its own: the file
    /// and the load's number. Under `Lazy` its registrations are staged,
    /// every one charged, and the LRU's entry charged, before the first is
    /// published ([`Self::register_staged`]); under `Eager` each is
    /// published as it is registered.
    fn try_load_dep(&self, name: &RawStr, source: &Source) -> Result<(File, u64), Error> {
        let ctx = self.ctx();
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Dependency);
        let file = open_source(name, source, &meter, ctx)?;
        let dep = Arc::new(Dependency {
            name: name.clone(),
            source: source.clone(),
            open: Mutex::new(Open::of(&file, meter)),
            ctx: ctx.clone(),
            cache: Arc::downgrade(&self.inner.cache),
            slots: AtomicUsize::new(0),
        });
        let (load, room) = match self.inner.options.load {
            LoadMode::Lazy => {
                let (load, room) = self.register_staged(&dep, &file)?;
                (load, Some(room))
            }
            LoadMode::Eager => {
                let load = self.register_file(&dep.name, &file, &mut Target::Dependency(&dep))?;
                (load, None)
            }
        };
        #[cfg(test)]
        self.inner.cache.hook(Hook::LoadRegistered);
        self.inner.cache.loaded.fetch_add(1, Ordering::Relaxed);
        match room {
            Some(room) => self.inner.cache.touch_paid(&dep, &file, room),
            None => self.inner.cache.touch(&dep, &file)?,
        }
        // Another load registered every key this one did meanwhile: no slot
        // reaches it, and the LRU lets go of it ([`registry`]). The last
        // slot to go before the touch told an LRU that did not hold it yet;
        // one that goes after it tells the LRU itself (`DepSlot`).
        if dep.unreachable() {
            self.inner.cache.forget(&dep);
        }
        #[cfg(test)]
        self.inner.cache.hook(Hook::LoadChecked);
        Ok((file, load))
    }

    /// A dependency's registrations under `Lazy`, staged ([`Stage`]: each
    /// key, slot, new key's copy and share, and position's path charged to
    /// the dependency's meter, with the list that holds them), then the
    /// LRU's entry charged ([`DepCache::room`]), then every registration
    /// published in the order it was staged ([`super::registry::Registry::commit`]).
    /// A refusal before the commit drops the stage, which returns every
    /// charge and was never in the registry, so no lookup, on this thread
    /// or another, can have found any of it; nothing after the commit's
    /// start can be refused. The load's number, and the LRU entry's charge
    /// for [`DepCache::touch_paid`].
    fn register_staged(&self, dep: &Arc<Dependency>, file: &File) -> Result<(u64, Charge), Error> {
        let mut stage = Stage::new(file.meter());
        let load = self.register_file(&dep.name, file, &mut Target::Staged(dep, &mut stage))?;
        let room = DepCache::room(file)?;
        self.inner.registry.commit(stage)?;
        Ok((load, room))
    }

    /// A file the resolver returned (module docs, "Resolver loads in
    /// `files`"). With `keep_resolver_loads`: `find_file`'s
    /// ([`Asked::Find`]) with `is_dependency` is a dependency, listed in
    /// `files` under the source's name (flagged a dependency unless it is a
    /// raw reader, which UnityPy gives no flag); without `is_dependency` a caller's
    /// load under that name (UnityPy's `load_file(fp, name=name,
    /// is_dependency=False)`, `environment.py:332`, `:354`); a resource
    /// fallback's ([`Asked::Fallback`]) is a caller's load under the name it
    /// asked for. Without `keep_resolver_loads` every one is a dependency in
    /// the LRU only (`PARITY.md` section 19.3's row "resolver loads under
    /// `keep_resolver_loads` false", and section 19.2's
    /// `SerializedFile.load_dependencies` row).
    ///
    /// Every one of these loads, whatever its listing, is one run of
    /// [`with_room`]: a pool refusal anywhere in it (the source's fetcher's
    /// own included, counted as it returns: `registry::from_callers`)
    /// evicts the other dependencies and loads once more, as
    /// [`Self::load_dep`] does (review round 12: a load listed as no
    /// dependency, `find_file`'s with `is_dependency` false or a resource
    /// fallback's under `keep_resolver_loads`, returned its fetcher's
    /// refusal untried, unless the fallback's candidate step around it
    /// retried it). A listing is charged to the loaded file's meter inside
    /// that run, so a second run loads and lists again (review round 8: a
    /// refused listing failed the load, whose file then stayed registered
    /// and unlisted).
    pub(super) fn load_resolved(&self, source: &Source, asked: Asked<'_>) -> Result<File, Error> {
        let keep = self.inner.options.keep_resolver_loads;
        let resolved;
        let name = match asked {
            Asked::Fallback(name) => name,
            Asked::Find { .. } => {
                resolved = source_name(source);
                &resolved
            }
        };
        with_room(Some(&self.inner.cache), None, || match asked {
            _ if !keep => self.try_load_dep(name, source).map(|(file, _)| file),
            Asked::Fallback(_)
            | Asked::Find {
                is_dependency: false,
            } => self.load_listed(name, source, true),
            Asked::Find {
                is_dependency: true,
            } => {
                let (file, load) = self.try_load_dep(name, source)?;
                let meter = file.meter().clone();
                let key = RawStr::charged(name.as_bytes(), &meter)?;
                // A reader has no `is_dependency`: `parse_file` returns the
                // reader itself (`helpers/ImportHelper.py:136-156`), which
                // `getattr(f, "is_dependency", False)` reads false.
                let flag = !matches!(file, File::Raw(_));
                self.list(key, file.clone(), flag, load, &meter)?;
                Ok(file)
            }
        })
    }
}
