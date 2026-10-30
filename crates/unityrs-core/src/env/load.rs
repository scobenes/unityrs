//! Loads: [`Environment::load`], [`Environment::load_dependency`] and the
//! files the resolver returns, and the registration of their CABs.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(test)]
use super::registry::Hook;
use super::registry::{Dependency, WeakFile, open_source};
use super::{Cab, Environment, LoadMode, RawCab, Source, source_name};
use crate::budget::Reservation;
use crate::error::{Error, Unsupported};
use crate::files::{BundleFile, File};
use crate::limits::Meter;
use crate::raw::RawStr;

/// Whose CABs a registration makes: a caller's file (slots that may hold
/// it), or a dependency's (slots that reopen it).
enum Target<'a> {
    Caller,
    Dependency(&'a Arc<Dependency>),
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
    /// CABs registered as a caller's, and a `files` entry under `name`,
    /// charged to the file's meter (a resolver's name copied into its key
    /// first; a caller's own name is shared, not copied).
    fn load_listed(&self, name: &RawStr, source: &Source, resolved: bool) -> Result<File, Error> {
        let ctx = self.ctx();
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        // UnityPy environment.py:139-160
        let file = open_source(name, source, &meter, ctx)?;
        self.register_file(name, &file, &Target::Caller)?;
        let key = if resolved {
            RawStr::charged(name.as_bytes(), &meter)?
        } else {
            name.clone()
        };
        self.list(key, file.clone(), false, &meter)?;
        Ok(file)
    }

    /// Registers the CABs of a top-level `file` loaded under `name`: the
    /// file itself, or a bundle's entries (`Eager`: as UnityPy parses them;
    /// `Lazy`: every directory entry's name now, parsed on first use).
    /// Every registration is one load's, ranked by its directory path
    /// ([`registry`]).
    ///
    /// [`registry`]: super::registry
    fn register_file(&self, name: &RawStr, file: &File, target: &Target<'_>) -> Result<(), Error> {
        let reg = &self.inner.registry;
        let load = reg.next_load();
        match file {
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
                    for (i, d) in b.directory().iter().enumerate() {
                        match target {
                            Target::Caller => {
                                reg.register_entry(&d.path, b.clone(), i, load, &[i], b.meter())?;
                            }
                            Target::Dependency(dep) => {
                                reg.register_lazy_dependency(&d.path, dep, load, &[i], b.meter())?;
                            }
                        }
                    }
                    Ok(())
                }
            },
            File::Web(_) => Err(Error::Unsupported(Unsupported::WebFile)),
        }
    }

    fn register_cab(
        &self,
        name: &RawStr,
        cab: Cab,
        meter: &Arc<Meter>,
        target: &Target<'_>,
        load: u64,
        path: &[usize],
    ) -> Result<(), Error> {
        let reg = &self.inner.registry;
        match target {
            Target::Caller => reg.register_ready(name, cab, load, path, meter),
            Target::Dependency(dep) => reg.register_dependency(name, dep, load, path, meter),
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
        target: &Target<'_>,
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
    // The source is handed over (the frozen signature) and kept by a clone.
    #[allow(clippy::needless_pass_by_value)]
    pub fn load_dependency(&self, source: Source) -> Result<File, Error> {
        let name = source_name(&source);
        self.load_dep(&name, &source)
    }

    fn load_dep(&self, name: &RawStr, source: &Source) -> Result<File, Error> {
        match self.try_load_dep(name, source) {
            // A pool refusal evicts the others and retries once (plan 3.13);
            // a second refusal fails naming the pool, and nothing of the
            // dependency is kept (its first attempt's charges returned
            // with its handle).
            Err(Error::Budget(b)) if b.what == Reservation::Dependency => {
                self.inner.cache.evict_others(None);
                self.try_load_dep(name, source)
            }
            other => other,
        }
    }

    /// One attempt of [`Self::load_dep`], on a meter of its own.
    fn try_load_dep(&self, name: &RawStr, source: &Source) -> Result<File, Error> {
        let ctx = self.ctx();
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Dependency);
        let file = open_source(name, source, &meter, ctx)?;
        let dep = Arc::new(Dependency {
            name: name.clone(),
            source: source.clone(),
            meter: Mutex::new(meter),
            open: Mutex::new(WeakFile::of(&file)),
            ctx: ctx.clone(),
            cache: Arc::downgrade(&self.inner.cache),
            slots: AtomicUsize::new(0),
        });
        self.register_file(&dep.name, &file, &Target::Dependency(&dep))?;
        #[cfg(test)]
        self.inner.cache.hook(Hook::LoadRegistered);
        self.inner.cache.loaded.fetch_add(1, Ordering::Relaxed);
        self.inner.cache.touch(&dep, &file)?;
        // Another load registered every key this one did meanwhile: no slot
        // reaches it, and the LRU lets go of it ([`registry`]). The last
        // slot to go before the touch told an LRU that did not hold it yet;
        // one that goes after it tells the LRU itself (`DepSlot`).
        if dep.unreachable() {
            self.inner.cache.forget(&dep);
        }
        #[cfg(test)]
        self.inner.cache.hook(Hook::LoadChecked);
        Ok(file)
    }

    /// A file the resolver returned: `find_file`'s (`asked` is `None`) is a
    /// dependency, listed in `files` under the source's name with
    /// `keep_resolver_loads`; a resource fallback's (`asked` is the name it
    /// asked for) is a caller's load under that name with
    /// `keep_resolver_loads`, and a dependency without it.
    pub(super) fn load_resolved(
        &self,
        source: &Source,
        asked: Option<&RawStr>,
    ) -> Result<File, Error> {
        let keep = self.inner.options.keep_resolver_loads;
        match asked {
            Some(name) if keep => self.load_listed(name, source, true),
            Some(name) => self.load_dep(name, source),
            None => {
                let name = source_name(source);
                let file = self.load_dep(&name, source)?;
                if keep {
                    let meter = file.meter().clone();
                    let key = RawStr::charged(name.as_bytes(), &meter)?;
                    self.list(key, file.clone(), true, &meter)?;
                }
                Ok(file)
            }
        }
    }
}
