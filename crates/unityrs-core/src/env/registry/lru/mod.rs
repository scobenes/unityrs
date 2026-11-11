//! The dependency LRU ([`DepCache`]): the strong handle on each cached
//! dependency, bounded by the sum of their meters.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::convert::Infallible;
use std::mem::size_of;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::{Dependency, lock};
use crate::error::Error;
use crate::files::File;
use crate::limits::{Charge, HASH_TABLE_SHARES};

/// The dependency LRU: the strong handle on each cached dependency, most
/// recently used last, bounded by the sum of their meters
/// (`EnvOptions::dep_cache_bytes`). Locked only to reorder, to insert or to
/// take handles out; a handle it drops is dropped after the lock.
///
/// Every operation is O(1) but an eviction's, which is O(1) per dependency
/// it evicts: the entries are a doubly linked list threaded through a hash
/// map (keyed by the dependency's address, which its `Arc` in the entry
/// keeps), and the bound is checked against the sum of each entry's meter
/// as it stood when the entry was last used (touched, or refreshed after a
/// read through it), re-read for the least recently used entry before it is
/// evicted. A meter that grew or shrank since its entry's last use is not
/// re-read until then, so the sum can lag a use behind; the pool's own
/// refusal (an eviction and one retry, plan section 3.13) is what bounds the
/// memory, and the LRU only orders what goes first.
///
/// **One lock, one step.** The list and the map are one value ([`Lru`])
/// behind one lock, changed only by its own methods, and each step an
/// operation takes under the lock (a use's reorder or insertion, one
/// eviction) decides and acts in that lock. Every step leaves every key the
/// map holds listed once, nothing else listed, and the sum equal to the
/// entries' meters as last read. Between two steps an operation holds no
/// lock and carries nothing it decided: another thread's steps may run
/// there, and the next step decides again (review round 3: a use decided
/// under one lock whether its key was cached and acted under a second, so a
/// key evicted between the two was listed again without an entry; the
/// entries before it fell off the list, past the bound for good, and an
/// eviction that reached it spun). An eviction takes at most as many steps
/// as the LRU held at its first, so it ends on any state.
///
/// An eviction allocates nothing: it takes one entry out per step and drops
/// it after the lock, so no list of the entries it evicts is built, however
/// many there are (review round 2: an eviction collected them, and a pool
/// refusal every key, in vectors no meter paid for).
#[derive(Debug)]
pub(crate) struct DepCache {
    bound: u64,
    /// False when nothing is ever evicted, a pool refusal included
    /// ([`crate::env::EnvOptions::dep_cache_bytes`]).
    evicts: bool,
    lru: Mutex<Lru>,
    pub(crate) loaded: AtomicU64,
    pub(crate) evicted: AtomicU64,
    /// Where a test stops a thread ([`Hook`]).
    #[cfg(test)]
    pub(crate) hook: Mutex<Option<HookFn>>,
}

/// A point at which a test may stop a thread, to run two threads through
/// the Environment's decisions in an order of its choosing, with no lock
/// held. The LRU's are between two of its steps (type docs), and run
/// [`DepCache::hook`]; the registry's, the `files` listing's and a preload
/// walk's (plan section 3.16's invariants R1-R5 and W1-W2) run the
/// registry's hook.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hook {
    /// A slot's handle on its dependency was counted out, before the LRU
    /// is told whether it was the last.
    SlotReleased,
    /// A dependency load registered its CABs, before it touches the LRU.
    LoadRegistered,
    /// A dependency load touched the LRU and asked whether a slot reaches
    /// the dependency, before it returns.
    LoadChecked,
    /// A use ([`DepCache::touch`]) of a dependency the caller holds open,
    /// before its step (round 3's touch had already asked, under a lock of
    /// its own, whether its key was cached).
    Touching,
    /// A use ([`DepCache::touch`], [`DepCache::refresh`]) took its step,
    /// before it evicts.
    Used,
    /// An eviction (a use's, [`DepCache::evict_others`]), before its next
    /// step.
    Evicting,
    /// A registration published its change (and, in the same step, set
    /// the walks' flags under its key and, for a load's, started a new
    /// container index generation), before it settles.
    Published,
    /// A registration listed the lazy slot it dropped (and, in the same
    /// step, for a load's, started a new container index generation),
    /// before it settles.
    Deferred,
    /// A `settle` picked a listed slot, before it reaches it.
    SettleTook,
    /// A reach registered its container's entries, before it marks the
    /// slot reached.
    Reached,
    /// The `files` listing lowered its key, before it publishes.
    Listing,
    /// The `files` listing published its entry (and, in the same step, set
    /// the walks' flags under its key), before it returns.
    Listed,
    /// A preload walk's lookup set up its flag, before it reads.
    Watched,
    /// A preload walk is about to take what changed since its last search
    /// and look for the next table index of container row `row` from
    /// `index` on (`index` is the row's span's end when the row is done):
    /// UnityPy's walk before it looks at element `index` of the row's span.
    Search {
        /// The row, by its position in the container.
        row: usize,
        /// The table index.
        index: usize,
    },
    /// A preload walk found table index `index` of container row `row` to
    /// dereference, before it does: UnityPy's walk before it looks at that
    /// element.
    Step {
        /// The row, by its position in the container.
        row: usize,
        /// The table index.
        index: usize,
    },
}

/// A test's hook ([`Hook`]).
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct HookFn(pub(crate) Arc<dyn Fn(Hook) + Send + Sync>);

#[cfg(test)]
impl std::fmt::Debug for HookFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HookFn")
    }
}

/// One cached dependency.
#[derive(Debug)]
struct LruNode {
    /// Keeps the dependency, and so the address that keys the node.
    _dep: Arc<Dependency>,
    file: File,
    /// Its meter as it stood when it was last used.
    seen: u64,
    prev: Option<usize>,
    next: Option<usize>,
    /// Its share of the map, charged to its meter.
    _charge: Charge,
}

/// The list and the map, which only these methods change, each leaving
/// every key the map holds listed once, nothing else listed, and `total`
/// the sum of the entries' `seen` ([`DepCache`]'s type docs).
#[derive(Debug, Default)]
struct Lru {
    nodes: HashMap<usize, LruNode>,
    /// The least recently used.
    head: Option<usize>,
    /// The most recently used.
    tail: Option<usize>,
    /// The sum of every node's `seen`.
    total: u64,
    /// Entries visited, for the tests that count the LRU's work.
    #[cfg(test)]
    visits: u64,
    /// A test's corrupt state, which [`Self::check`] lets by, to show that
    /// an eviction ends on any state.
    #[cfg(test)]
    unchecked: bool,
}

/// What one cached dependency costs the map: [`HASH_TABLE_SHARES`] buckets.
const LRU_SHARE: u64 = HASH_TABLE_SHARES * (size_of::<usize>() + size_of::<LruNode>() + 1) as u64;

/// The LRU's key for `dep`: its address, which the entry's `Arc` keeps.
fn lru_key(dep: &Arc<Dependency>) -> usize {
    Arc::as_ptr(dep).addr()
}

impl Lru {
    /// Counts an entry visited (tests only).
    #[cfg_attr(not(test), allow(clippy::unused_self))]
    fn visit(&mut self) {
        #[cfg(test)]
        {
            self.visits += 1;
        }
    }

    /// Takes `k` out of the list (it stays in the map); nothing for a key
    /// the map does not hold.
    fn unlink(&mut self, k: usize) {
        let Some((prev, next)) = self.nodes.get(&k).map(|n| (n.prev, n.next)) else {
            return;
        };
        match prev.and_then(|p| self.nodes.get_mut(&p)) {
            Some(p) => p.next = next,
            None => self.head = next,
        }
        match next.and_then(|n| self.nodes.get_mut(&n)) {
            Some(n) => n.prev = prev,
            None => self.tail = prev,
        }
        if let Some(n) = self.nodes.get_mut(&k) {
            n.prev = None;
            n.next = None;
        }
    }

    /// Lists `k` (in the map, out of the list) at the most recent end; a
    /// key the map does not hold is never listed.
    fn link_back(&mut self, k: usize) {
        let old = self.tail;
        let Some(n) = self.nodes.get_mut(&k) else {
            return;
        };
        n.prev = old;
        n.next = None;
        match old.and_then(|t| self.nodes.get_mut(&t)) {
            Some(t) => t.next = Some(k),
            None => self.head = Some(k),
        }
        self.tail = Some(k);
    }

    /// Makes `k` the most recently used and re-reads its meter: the map's
    /// entry if it holds one, else the one `make` builds, whose refusal
    /// changes nothing. Whether `k` is cached is asked and acted on here,
    /// in one step.
    fn use_key<E>(&mut self, k: usize, make: impl FnOnce() -> Result<LruNode, E>) -> Result<(), E> {
        self.visit();
        let cached = match self.nodes.entry(k) {
            Entry::Occupied(_) => true,
            Entry::Vacant(v) => {
                v.insert(make()?);
                false
            }
        };
        if cached {
            self.unlink(k);
        }
        self.link_back(k);
        self.reread(k);
        Ok(())
    }

    /// Takes `k` out of the list and the map; `None`, changing nothing, for
    /// a key the map does not hold.
    fn remove(&mut self, k: usize) -> Option<LruNode> {
        self.unlink(k);
        let node = self.nodes.remove(&k)?;
        self.total = self.total.saturating_sub(node.seen);
        Some(node)
    }

    /// Sets `k`'s `seen` to its meter now.
    fn reread(&mut self, k: usize) {
        if let Some(n) = self.nodes.get_mut(&k) {
            let now = n.file.meter().used();
            self.total = self.total.saturating_sub(n.seen).saturating_add(now);
            n.seen = now;
        }
    }

    /// One step of [`DepCache::evict_over`]: the least recently used entry
    /// but `keep`, taken out if the sum still passes `bound` once its meter
    /// is re-read; `None` when there is nothing to evict.
    fn take_over(&mut self, bound: u64, keep: usize) -> Option<LruNode> {
        if self.total <= bound || self.nodes.len() <= 1 {
            return None;
        }
        let h = self.head.filter(|&h| h != keep)?;
        self.visit();
        self.reread(h);
        if self.total <= bound {
            return None;
        }
        self.remove(h)
    }

    /// One step of [`DepCache::evict_others`]: the least recently used
    /// entry but `keep`, taken out.
    fn take_other(&mut self, keep: Option<usize>) -> Option<LruNode> {
        let h = self.head?;
        let k = if Some(h) == keep {
            self.nodes.get(&h)?.next?
        } else {
            h
        };
        self.visit();
        self.remove(k)
    }

    /// Panics unless every key the map holds is listed once, in a list
    /// whose links agree both ways and end at `tail`, nothing else is
    /// listed, and `total` is the sum of the entries' `seen`.
    #[cfg(test)]
    fn check(&self) {
        if self.unchecked {
            return;
        }
        let (mut at, mut prev, mut listed) = (self.head, None, 0_usize);
        while let Some(k) = at {
            let Some(n) = self.nodes.get(&k) else {
                panic!("the list holds {k:#x}, which the map does not");
            };
            assert_eq!(n.prev, prev, "the back link of {k:#x}");
            listed += 1;
            assert!(listed <= self.nodes.len(), "a key listed twice");
            prev = Some(k);
            at = n.next;
        }
        assert_eq!(self.tail, prev, "the tail is the last key listed");
        assert_eq!(
            listed,
            self.nodes.len(),
            "every key the map holds is listed"
        );
        let sum = self.nodes.values().map(|n| n.seen).sum::<u64>();
        assert_eq!(self.total, sum, "the total is the entries' sum");
    }
}

impl DepCache {
    pub(crate) fn new(bound: u64, evicts: bool) -> Self {
        Self {
            bound,
            evicts,
            lru: Mutex::new(Lru::default()),
            loaded: AtomicU64::new(0),
            evicted: AtomicU64::new(0),
            #[cfg(test)]
            hook: Mutex::new(None),
        }
    }

    /// One step: `f` on the list and the map under the lock (type docs),
    /// their state checked before the lock goes in a test build.
    fn step<R>(&self, f: impl FnOnce(&mut Lru) -> R) -> R {
        let mut lru = lock(&self.lru);
        let r = f(&mut lru);
        #[cfg(test)]
        lru.check();
        r
    }

    /// Runs the test's hook at `at`, with no lock held.
    #[cfg(test)]
    pub(crate) fn hook(&self, at: Hook) {
        let hook = lock(&self.hook).clone();
        if let Some(HookFn(f)) = hook {
            f(at);
        }
    }

    /// Marks `dep` (open as `file`) the most recently used, then evicts the
    /// least recently used others while the sum of the meters passes the
    /// bound (type docs). A new entry's share of the map is charged to
    /// `file`'s meter in the step that inserts it (a budget may be called
    /// under a core lock, [`crate::budget::MemoryBudget`]); a refusal
    /// caches nothing and is the caller's error.
    pub(crate) fn touch(&self, dep: &Arc<Dependency>, file: &File) -> Result<(), Error> {
        self.use_dep(dep, file, || Self::room(file))
    }

    /// A new entry's share of the map for `file`, charged to its meter
    /// ahead of [`Self::touch_paid`]: a dependency load under `Lazy`
    /// charges it before it publishes a registration (the registry's module
    /// docs, "A failed dependency load").
    pub(crate) fn room(file: &File) -> Result<Charge, Error> {
        Ok(file.meter().charge(LRU_SHARE)?)
    }

    /// [`Self::touch`] with the new entry's share already charged
    /// ([`Self::room`]), so nothing in it can be refused; the share is
    /// returned when `dep` is cached already.
    pub(crate) fn touch_paid(&self, dep: &Arc<Dependency>, file: &File, room: Charge) {
        let Ok(()) = self.use_dep::<Infallible>(dep, file, || Ok(room));
    }

    /// [`Self::touch`], the new entry's share from `share`, asked for in
    /// the step that inserts the entry and only then.
    fn use_dep<E>(
        &self,
        dep: &Arc<Dependency>,
        file: &File,
        share: impl FnOnce() -> Result<Charge, E>,
    ) -> Result<(), E> {
        let k = lru_key(dep);
        #[cfg(test)]
        self.hook(Hook::Touching);
        self.step(|lru| {
            lru.use_key(k, || {
                Ok(LruNode {
                    _dep: dep.clone(),
                    file: file.clone(),
                    seen: 0,
                    prev: None,
                    next: None,
                    _charge: share()?,
                })
            })
        })?;
        #[cfg(test)]
        self.hook(Hook::Used);
        self.evict_over(k);
        Ok(())
    }

    /// Re-reads `dep`'s meter after a read through it (the read's parse
    /// charged after the touch), then evicts as [`Self::touch`] does. Does
    /// nothing for a dependency the LRU does not hold.
    pub(crate) fn refresh(&self, dep: &Arc<Dependency>) {
        let k = lru_key(dep);
        let held = self.step(|lru| {
            if !lru.nodes.contains_key(&k) {
                return false;
            }
            lru.visit();
            lru.reread(k);
            true
        });
        if held {
            #[cfg(test)]
            self.hook(Hook::Used);
            self.evict_over(k);
        }
    }

    /// Evicts from the least recently used end, never `keep`, while the sum
    /// passes the bound; each candidate's meter is re-read first. One entry
    /// per step, dropped after it, and at most as many as the LRU held at
    /// the first step (type docs).
    fn evict_over(&self, keep: usize) {
        let mut left = None;
        loop {
            #[cfg(test)]
            self.hook(Hook::Evicting);
            let node = self.step(|lru| {
                let n = left.get_or_insert(lru.nodes.len());
                *n = n.checked_sub(1)?;
                lru.take_over(self.bound, keep)
            });
            let Some(node) = node else {
                return;
            };
            self.evicted(node);
        }
    }

    /// Drops the LRU's handle on every dependency but `keep` (a pool
    /// refusal's one retry, plan section 3.13): at most as many as it held
    /// at the first step, least recently used first, one per step (type
    /// docs).
    pub(crate) fn evict_others(&self, keep: Option<&Arc<Dependency>>) {
        if !self.evicts {
            return;
        }
        let keep = keep.map(lru_key);
        let mut left = None;
        loop {
            #[cfg(test)]
            self.hook(Hook::Evicting);
            let node = self.step(|lru| {
                let n = left.get_or_insert(lru.nodes.len());
                *n = n.checked_sub(1)?;
                lru.take_other(keep)
            });
            let Some(node) = node else {
                return;
            };
            self.evicted(node);
        }
    }

    /// Drops the LRU's handle on `dep`, which nothing can reach any more
    /// (its last registry slot went): not an eviction.
    pub(crate) fn forget(&self, dep: &Arc<Dependency>) {
        let gone = self.step(|lru| lru.remove(lru_key(dep)));
        drop(gone);
    }

    /// Counts an entry an eviction took out, and drops it (with no lock held).
    fn evicted(&self, node: LruNode) {
        self.evicted.fetch_add(1, Ordering::Relaxed);
        drop(node);
    }

    /// The bytes the cached dependencies' meters hold now (a report: every
    /// meter is read).
    pub(crate) fn bytes(&self) -> u64 {
        self.step(|lru| {
            lru.nodes
                .values()
                .map(|n| n.file.meter().used())
                .fold(0, u64::saturating_add)
        })
    }
}

#[cfg(test)]
mod tests;
