//! Watches on the answers a container preload walk keeps
//! (`super::container_index`'s `Targets`). A walk keeps an external's answer
//! (found or not) until a key its lookup read changes: the registry key
//! `find_file` asks, and a top-level file's lowered `files` key. Each lookup
//! sets a [`Flag`] under those keys before it reads them, and every change to
//! a key, by any thread (a load's registration, a resolver load's, under
//! `LoadMode::Lazy` the entries a reach registers, a `files` entry), sets
//! the flags under it ([`Table::notify`]), so a walk asks again exactly the
//! answers UnityPy's next lookup could find changed, and each external is
//! looked up once, and once more per change to one of its keys (review
//! round 4: every change, any thread's, expired every answer, so a table of
//! pointers to long names cost the table times the names).
//!
//! **The invariant** (plan section 3.16, W1): a key's change and the
//! setting of every flag under it are one step, made under the lock the key
//! is published under (the registry's for a registry key, the `files`
//! listing's for a `files` key), which keeps that key's [`Table`]; a walk
//! sets up its flag under that lock before its lookup reads the key under
//! it. So a lookup that reads the key before a change has its flag set by
//! the change, and every step of a walk that comes after a change (a lookup
//! that finds it, a kept answer whose flag it set, the log a walk takes
//! next) sees the change and every flag it set, whatever the threads'
//! timing (review round 14: the change was published, and its flags set
//! after the lock, so between the two a walk's lookup could find the change
//! while its kept answer under the same key still held). A key is watched
//! as a 64-bit hash under the Environment's random state, so a collision
//! only asks again an answer that still held, and an input cannot aim one.
//!
//! **What a walk dereferences again** (review rounds 13 and 14): a flag's
//! first setting, by whichever thread's change, logs it in its walk's
//! [`Fired`] list in that same step, without a lock (a stack linked through
//! the flags, [`Flag::next`]), so the walk learns which of its answers
//! changed without reading every flag it holds, and puts the table indexes
//! dereferenced under them back (`super::container_index`'s `Targets`).
//! What the walk does is then UnityPy's walk with each key another
//! thread's load changes changed at the point of the walk at which it was
//! published (a load publishes its keys one at a time, plan section 3.16
//! item 6): the step that comes after it sees it, through a lookup, a kept
//! answer or the log (review round 14: another thread's change set a flag
//! without logging it and took it from its key, so a load of the walk's own
//! that changed the key later put back nothing, and which indexes a later
//! row dereferenced again depended on which thread changed the key first).
//! A key such a load adds does not fail the pass, where UnityPy's loop over
//! `env.cabs` raises (`PARITY.md`'s `parse_preload_table` row).
//!
//! **Memory** (W2): each watch is charged to its walk's meter, by a charge
//! the walk itself holds ([`Walk`]), so the charge is released only after
//! the walk's entries have left both tables and every key list it grew has
//! shrunk back to what the running walks pay for (review round 5: the
//! charge was released before the entries left the map, and a key list
//! kept the room a finished walk had paid for). A walk's end takes out of
//! each table every entry that is its own, by identity (its flag's log is
//! the walk's), and only then lets go of its flags, so no table holds a dead
//! flag: an end that told its entries by whether their flags were still
//! alive kept the one a change another thread was publishing under the other
//! lock held, which stayed there, dead, after the walk's charge was released
//! (review round 15).

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};

use crate::error::Error;
use crate::limits::{Charge, HASH_TABLE_SHARES, Meter, heap_cost};

/// No flag: the end of a walk's [`Fired`] list.
const NONE: usize = usize::MAX;

/// The lock a key is published under: the registry's ([`REGISTRY`]) or the
/// `files` listing's ([`FILES`]), an index into a walk's locks.
pub(crate) const REGISTRY: usize = 0;
/// The `files` listing's lock ([`REGISTRY`]).
pub(crate) const FILES: usize = 1;

/// Set when a key it watches changes.
#[derive(Debug)]
pub(crate) struct Flag {
    set: AtomicBool,
    /// Its number among its walk's flags, from 0 in the order they were
    /// set up.
    id: usize,
    /// The number of the flag its walk's log held before it was logged
    /// ([`NONE`]: none).
    next: AtomicUsize,
    /// Its walk's log.
    log: Arc<Fired>,
}

impl Flag {
    /// Whether a key it watches changed since it was set up.
    pub(crate) fn changed(&self) -> bool {
        self.set.load(Ordering::Acquire)
    }

    /// Its number among its walk's flags.
    pub(crate) fn id(&self) -> usize {
        self.id
    }

    /// Sets it; the first time, by whichever thread, pushes it on its
    /// walk's [`Fired`] list (module docs). A flag is pushed once, so its
    /// `next` is written before it is reachable from the list and never
    /// after, and a walk that takes the list reads every `next` it follows
    /// after the push that wrote it (the head's release sequence).
    fn fire(&self) {
        if self.set.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut head = self.log.head.load(Ordering::Acquire);
        loop {
            self.next.store(head, Ordering::Relaxed);
            #[cfg(test)]
            hook(Point::Pushing);
            match self.log.head.compare_exchange_weak(
                head,
                self.id,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(now) => head = now,
            }
        }
    }
}

/// A walk's log: the number of the flag set last since the walk last took
/// it ([`Walk::fired`]), the others linked from it through [`Flag::next`],
/// each once (module docs).
#[derive(Debug)]
pub(crate) struct Fired {
    head: AtomicUsize,
}

/// What one flag costs its walk: its `Arc` in a glibc chunk, and its
/// element in the walk's list of its flags, twice (the list doubles).
pub(crate) const FLAG: u64 = heap_cost((size_of::<Flag>() + 2 * size_of::<usize>()) as u64)
    + 2 * size_of::<Arc<Flag>>() as u64;

/// What a walk's [`Fired`] list costs it, made with its first flag: its
/// `Arc` in a glibc chunk, and the four elements the list of its flags holds
/// room for at its first push, in a chunk.
pub(crate) const FIRED: u64 = heap_cost((size_of::<Fired>() + 2 * size_of::<usize>()) as u64)
    + heap_cost(4 * size_of::<Arc<Flag>>() as u64);

/// What one watch of one key costs its walk: its share of its lock's
/// table, and its element in the key's list and in the walk's own list,
/// each twice (the lists double, and a key list shrinks back when a walk
/// ends, [`Walk`]).
pub(crate) const WATCH_SHARE: u64 = HASH_TABLE_SHARES
    * (size_of::<u64>() + size_of::<Vec<Weak<Flag>>>() + 1) as u64
    + 2 * (size_of::<Weak<Flag>>() + size_of::<u64>()) as u64;

/// The running walks' flags under the keys one lock publishes, by key hash,
/// kept under that lock (module docs).
#[derive(Debug, Default)]
pub(crate) struct Table {
    map: HashMap<u64, Vec<Weak<Flag>>>,
}

impl Table {
    /// Notes a change to the key whose hash ([`Watches::hash`]) is `hash`,
    /// in the step that publishes it, under its lock: every flag under it
    /// is set (and logged, module docs), and the key is watched no more
    /// until a lookup watches it again.
    pub(crate) fn notify(&mut self, hash: u64) {
        let Some(flags) = self.map.remove(&hash) else {
            return;
        };
        for flag in flags {
            if let Some(flag) = flag.upgrade() {
                #[cfg(test)]
                hook(Point::Upgraded);
                flag.fire();
            }
        }
    }

    /// Puts `flag` under each of `hashes`.
    fn add(&mut self, hashes: &[u64], flag: &Arc<Flag>) {
        for &h in hashes {
            self.map.entry(h).or_default().push(Arc::downgrade(flag));
        }
    }

    /// A walk's end: its entries (those whose flag's log is `mine`) leave
    /// the lists of `hashes` (each once), by identity, and each list and the
    /// map keep no room the running walks do not pay for.
    fn end(&mut self, hashes: &[u64], mine: &Arc<Fired>) {
        for h in hashes {
            if let Some(flags) = self.map.get_mut(h) {
                flags.retain(|f| f.upgrade().is_some_and(|f| !Arc::ptr_eq(&f.log, mine)));
                if flags.is_empty() {
                    self.map.remove(h);
                } else if flags.capacity() > 2 * flags.len() + 4 {
                    // The running walks pay for two elements a watch: a
                    // list keeps no room a finished walk paid for.
                    flags.shrink_to(2 * flags.len());
                }
            }
        }
        // The map keeps no room the running walks' charges do not pay for
        // (a key's share covers a growing table's).
        let len = self.map.len();
        if self.map.capacity() > 3 * len + 8 {
            self.map.shrink_to(len);
        }
    }

    /// The table's entries and each key list's length and room, for a
    /// test.
    #[cfg(test)]
    pub(crate) fn room(&self) -> (usize, Vec<(usize, usize)>) {
        let lists = self.map.values().map(|l| (l.len(), l.capacity())).collect();
        (self.map.len(), lists)
    }
}

/// A lock keys are published under, which keeps the [`Table`] of the
/// running walks' flags under those keys (module docs): the registry and
/// the `files` listing.
pub(crate) trait Under: Sync {
    /// Runs `f` on the table, under the lock.
    fn with_table(&self, f: &mut dyn FnMut(&mut Table));
}

/// The Environment's hashing of watched keys, and, for a test, its count of
/// running walks.
#[derive(Debug, Default)]
pub(crate) struct Watches {
    state: RandomState,
    /// Walks running, which only tests read.
    #[cfg(test)]
    walks: AtomicUsize,
}

impl Watches {
    /// Starts a walk's watches under `locks` (the registry's and the
    /// `files` listing's, in that order: [`REGISTRY`], [`FILES`]), charged
    /// to `meter`; they end when it drops.
    pub(crate) fn start<'a>(&'a self, meter: &Arc<Meter>, locks: [&'a dyn Under; 2]) -> Walk<'a> {
        #[cfg(test)]
        self.walks.fetch_add(1, Ordering::SeqCst);
        Walk {
            watches: self,
            locks,
            hashes: [Vec::new(), Vec::new()],
            log: None,
            flags: Vec::new(),
            charge: Charge::zero(meter),
        }
    }

    /// The hash `key` (a registry key, or a `files` key lowered) is watched
    /// under, computed before the lock its change is published under.
    pub(crate) fn hash(&self, key: &[u8]) -> u64 {
        self.state.hash_one(key)
    }

    /// Walks running, for a test.
    #[cfg(test)]
    pub(crate) fn walks(&self) -> usize {
        self.walks.load(Ordering::SeqCst)
    }
}

/// One walk's watches, with the charge that pays for them. It holds its
/// flags, so a change sets one until the walk ends, however the answer it
/// guards was kept; its entries leave both tables when it drops, and its
/// charge is a field of its own, released after [`Drop::drop`] has taken
/// them out (W2, module docs).
pub(crate) struct Walk<'a> {
    watches: &'a Watches,
    /// The registry's lock and the `files` listing's ([`REGISTRY`],
    /// [`FILES`]).
    locks: [&'a dyn Under; 2],
    /// Every hash it watched under each lock, once per watch.
    hashes: [Vec<u64>; 2],
    /// Its flags' log, made with the first.
    log: Option<Arc<Fired>>,
    /// Its flags, by number.
    flags: Vec<Arc<Flag>>,
    /// [`FIRED`] once, [`FLAG`] per flag and [`WATCH_SHARE`] per watch of a
    /// key.
    charge: Charge,
}

impl Walk<'_> {
    /// The hash `key` is watched under.
    pub(crate) fn hash(&self, key: &[u8]) -> u64 {
        self.watches.hash(key)
    }

    /// A flag set at the next change to any of `keys` (each the lock it is
    /// published under, [`REGISTRY`] or [`FILES`], and its hash), to be set
    /// up before the lookup that reads them, charged first ([`FLAG`], and
    /// [`WATCH_SHARE`] per key); a refusal sets up nothing. Its number
    /// ([`Flag::id`]) is the count of the walk's flags set up before it.
    pub(crate) fn watch(&mut self, keys: &[(usize, u64)]) -> Result<Arc<Flag>, Error> {
        let n = u64::try_from(keys.len()).unwrap_or(u64::MAX);
        let first = if self.log.is_none() { FIRED } else { 0 };
        self.charge.grow(
            WATCH_SHARE
                .saturating_mul(n)
                .saturating_add(FLAG)
                .saturating_add(first),
        )?;
        let log = self
            .log
            .get_or_insert_with(|| {
                Arc::new(Fired {
                    head: AtomicUsize::new(NONE),
                })
            })
            .clone();
        let flag = Arc::new(Flag {
            set: AtomicBool::new(false),
            id: self.flags.len(),
            next: AtomicUsize::new(NONE),
            log,
        });
        self.flags.push(flag.clone());
        for (at, (lock, mine)) in self.locks.iter().zip(&mut self.hashes).enumerate() {
            let start = mine.len();
            mine.extend(keys.iter().filter(|(l, _)| *l == at).map(|(_, h)| *h));
            if let Some(added) = mine.get(start..).filter(|a| !a.is_empty()) {
                lock.with_table(&mut |t| t.add(added, &flag));
            }
        }
        Ok(flag)
    }

    /// The numbers of its flags set since the last call, by any thread,
    /// each once over the walk, the latest first (module docs); taken as
    /// the call is made, read as the iterator runs.
    pub(crate) fn fired(&self) -> impl Iterator<Item = usize> + '_ {
        let mut at = self
            .log
            .as_ref()
            .map_or(NONE, |l| l.head.swap(NONE, Ordering::AcqRel));
        std::iter::from_fn(move || {
            let id = at;
            at = self.flags.get(id)?.next.load(Ordering::Acquire);
            Some(id)
        })
    }

    /// What the walk's charge holds, for a test.
    #[cfg(test)]
    pub(crate) fn charged(&self) -> u64 {
        self.charge.bytes()
    }

    /// The room its list of flags holds, in elements, for a test.
    #[cfg(test)]
    pub(crate) fn flags_room(&self) -> usize {
        self.flags.capacity()
    }
}

impl Drop for Walk<'_> {
    fn drop(&mut self) {
        // Its entries leave each table by identity, not by whether their
        // flags are alive, and its flags outlive them: `flags` drops after
        // this body (module docs, W2). A walk with no log set up no flag.
        if let Some(log) = &self.log {
            for at in [REGISTRY, FILES] {
                let (Some(lock), Some(hashes)) = (self.locks.get(at), self.hashes.get_mut(at))
                else {
                    continue;
                };
                if hashes.is_empty() {
                    continue;
                }
                hashes.sort_unstable();
                hashes.dedup();
                lock.with_table(&mut |t| t.end(hashes, log));
                #[cfg(test)]
                hook(Point::Ended(at));
            }
        }
        #[cfg(test)]
        self.watches.walks.fetch_sub(1, Ordering::SeqCst);
        // `flags` drops after this body, then `charge`: what it paid for is
        // freed first.
    }
}

/// Where a test stops a thread in this module ([`set_hook`]).
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Point {
    /// A notify upgraded a flag under its key, before it sets it (the
    /// publishing lock held).
    Upgraded,
    /// A flag's first setting read its walk's log's head and linked the
    /// flag to it, before the exchange that pushes it.
    Pushing,
    /// A walk's end took its entries out of the table of lock `at`
    /// ([`REGISTRY`], [`FILES`]), before the next.
    Ended(usize),
}

/// A test's hook on this module's points ([`Point`]).
#[cfg(test)]
pub(crate) type PointFn = std::rc::Rc<dyn Fn(Point)>;

#[cfg(test)]
thread_local! {
    /// The hook of the thread that runs it, so tests on other threads are
    /// never stopped.
    static HOOK: std::cell::RefCell<Option<PointFn>> = const { std::cell::RefCell::new(None) };
}

/// Sets this thread's hook ([`Point`]); `None` takes it off.
#[cfg(test)]
pub(crate) fn set_hook(f: Option<PointFn>) {
    HOOK.with(|h| *h.borrow_mut() = f);
}

/// Runs this thread's hook at `at`.
#[cfg(test)]
fn hook(at: Point) {
    let f = HOOK.with(|h| h.borrow().clone());
    if let Some(f) = f {
        f(at);
    }
}

#[cfg(test)]
mod tests;
