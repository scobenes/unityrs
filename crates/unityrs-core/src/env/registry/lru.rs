//! The dependency LRU ([`DepCache`]): the strong handle on each cached
//! dependency, bounded by the sum of their meters.

use std::collections::HashMap;
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
/// An eviction allocates nothing: it takes one entry out per lock and drops
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
    /// Entries visited, for the tests that count the LRU's work.
    #[cfg(test)]
    steps: AtomicU64,
    /// Where a test stops a thread ([`Hook`]).
    #[cfg(test)]
    pub(crate) hook: Mutex<Option<HookFn>>,
}

/// A point at which a test may stop a thread, to run two threads through
/// the LRU's decisions in an order of its choosing.
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

#[derive(Debug, Default)]
struct Lru {
    nodes: HashMap<usize, LruNode>,
    /// The least recently used.
    head: Option<usize>,
    /// The most recently used.
    tail: Option<usize>,
    /// The sum of every node's `seen`.
    total: u64,
}

/// What one cached dependency costs the map: [`HASH_TABLE_SHARES`] buckets.
const LRU_SHARE: u64 = HASH_TABLE_SHARES * (size_of::<usize>() + size_of::<LruNode>() + 1) as u64;

/// The LRU's key for `dep`: its address, which the entry's `Arc` keeps.
fn lru_key(dep: &Arc<Dependency>) -> usize {
    Arc::as_ptr(dep).addr()
}

impl Lru {
    /// Takes `k` out of the list (it stays in the map).
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

    /// Puts `k` (in the map, out of the list) at the most recent end.
    fn push_back(&mut self, k: usize) {
        let old = self.tail;
        if let Some(n) = self.nodes.get_mut(&k) {
            n.prev = old;
            n.next = None;
        }
        match old.and_then(|t| self.nodes.get_mut(&t)) {
            Some(t) => t.next = Some(k),
            None => self.head = Some(k),
        }
        self.tail = Some(k);
    }

    /// Takes `k` out of the list and the map.
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
            steps: AtomicU64::new(0),
            #[cfg(test)]
            hook: Mutex::new(None),
        }
    }

    /// How many dependencies the LRU holds.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        lock(&self.lru).nodes.len()
    }

    #[cfg_attr(not(test), allow(clippy::unused_self))]
    fn step(&self) {
        #[cfg(test)]
        self.steps.fetch_add(1, Ordering::Relaxed);
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
    /// `file`'s meter first; a refusal caches nothing and is the caller's
    /// error.
    pub(crate) fn touch(&self, dep: &Arc<Dependency>, file: &File) -> Result<(), Error> {
        let k = lru_key(dep);
        let known = lock(&self.lru).nodes.contains_key(&k);
        let charge = if known {
            None
        } else {
            Some(file.meter().charge(LRU_SHARE)?)
        };
        {
            let mut lru = lock(&self.lru);
            self.step();
            // Out of the list if it is in it; in the map if it is new (a
            // node another thread put there meanwhile is kept, and this
            // one's charge returned).
            lru.unlink(k);
            if let Some(charge) = charge {
                lru.nodes.entry(k).or_insert_with(|| LruNode {
                    _dep: dep.clone(),
                    file: file.clone(),
                    seen: 0,
                    prev: None,
                    next: None,
                    _charge: charge,
                });
            }
            lru.push_back(k);
            lru.reread(k);
        }
        self.evict_over(k);
        Ok(())
    }

    /// Re-reads `dep`'s meter after a read through it (the read's parse
    /// charged after the touch), then evicts as [`Self::touch`] does. Does
    /// nothing for a dependency the LRU does not hold.
    pub(crate) fn refresh(&self, dep: &Arc<Dependency>) {
        let k = lru_key(dep);
        {
            let mut lru = lock(&self.lru);
            if !lru.nodes.contains_key(&k) {
                return;
            }
            self.step();
            lru.reread(k);
        }
        self.evict_over(k);
    }

    /// Evicts from the least recently used end, never `keep`, while the sum
    /// passes the bound; each candidate's meter is re-read first. One entry
    /// per lock, dropped after it (type docs).
    fn evict_over(&self, keep: usize) {
        loop {
            let node = {
                let mut lru = lock(&self.lru);
                if lru.total <= self.bound || lru.nodes.len() <= 1 {
                    return;
                }
                let Some(h) = lru.head.filter(|&h| h != keep) else {
                    return;
                };
                self.step();
                lru.reread(h);
                if lru.total <= self.bound {
                    return;
                }
                lru.remove(h)
            };
            self.evicted(node);
        }
    }

    /// Drops the LRU's handle on every dependency but `keep` (a pool
    /// refusal's one retry, plan section 3.13): at most as many as it held
    /// when called, least recently used first, one per lock (type docs).
    pub(crate) fn evict_others(&self, keep: Option<&Arc<Dependency>>) {
        if !self.evicts {
            return;
        }
        let keep = keep.map(lru_key);
        let held = lock(&self.lru).nodes.len();
        for _ in 0..held {
            let node = {
                let mut lru = lock(&self.lru);
                let Some(k) = lru.head.filter(|&h| Some(h) != keep).or_else(|| {
                    lru.head
                        .and_then(|h| lru.nodes.get(&h))
                        .and_then(|n| n.next)
                }) else {
                    return;
                };
                self.step();
                lru.remove(k)
            };
            self.evicted(node);
        }
    }

    /// Drops the LRU's handle on `dep`, which nothing can reach any more
    /// (its last registry slot went): not an eviction.
    pub(crate) fn forget(&self, dep: &Arc<Dependency>) {
        let gone = lock(&self.lru).remove(lru_key(dep));
        drop(gone);
    }

    /// Counts an entry an eviction took out, and drops it (with no lock held).
    fn evicted(&self, node: Option<LruNode>) {
        if node.is_some() {
            self.evicted.fetch_add(1, Ordering::Relaxed);
        }
        drop(node);
    }

    /// The bytes the cached dependencies' meters hold now (a report: every
    /// meter is read).
    pub(crate) fn bytes(&self) -> u64 {
        lock(&self.lru)
            .nodes
            .values()
            .map(|n| n.file.meter().used())
            .fold(0, u64::saturating_add)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use super::DepCache;
    use crate::env::{EnvOptions, Environment, Source};
    use crate::raw::RawStr;
    use bytes::Bytes;

    fn bundle_with(cab: &str) -> Vec<u8> {
        use unityrs_testkit::env::{Obj, bundle, serialized};
        bundle(&[(cab, serialized(&[], &[Obj::named(1, cab, &[])]))])
    }

    /// The LRU's work per use does not grow with the dependencies it holds
    /// (review round 1: `touch` scanned and summed every entry, so `n`
    /// lookups over `n` dependencies cost `n`²). Counted in entries
    /// visited, never by a clock.
    #[test]
    fn the_lru_visits_a_constant_number_of_entries_per_use() {
        let steps = |n: usize| {
            let env = Environment::new(EnvOptions::batch(Arc::new(
                crate::options::FileContext::unitypy(),
            )));
            for i in 0..n {
                let name = format!("CAB-{i}");
                env.load_dependency(Source::Bytes {
                    name: RawStr::from(format!("{name}.bundle").as_str()),
                    data: Bytes::from(bundle_with(&name)),
                })
                .unwrap();
            }
            let cache: &DepCache = &env.inner.cache;
            let before = cache.steps.load(Ordering::Relaxed);
            for i in 0..n {
                let name = RawStr::from(format!("cab-{i}").as_str());
                assert!(
                    env.get_cab(&name, &crate::limits::Meter::unlimited())
                        .unwrap()
                        .is_some()
                );
            }
            cache.steps.load(Ordering::Relaxed) - before
        };
        let (small, large) = (steps(200), steps(800));
        // A lookup touches its dependency and refreshes it: two visits.
        assert!(
            large <= 2 * 800 && small <= 2 * 200,
            "visits: {small} for 200 lookups over 200 dependencies, {large} for 800 over 800"
        );
    }
}
