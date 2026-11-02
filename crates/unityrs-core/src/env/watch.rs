//! Watches on the answers a container preload walk keeps
//! (`super::container_index`'s `Targets`). A walk keeps an external's answer
//! (found or not) until a key its lookup read changes: the registry key
//! `find_file` asks, and a top-level file's lowered `files` key. Each lookup
//! sets a [`Flag`] under those keys before it reads them, and every change to
//! a key, by any thread (a load's registration, a resolver load's, under
//! `LoadMode::Lazy` the entries a reach registers, a `files` entry), calls
//! [`Watches::notify`] after it is published, which sets the flags under it
//! while a walk runs, so a walk asks again exactly the answers UnityPy's next
//! lookup could find changed, and each external is looked up once, and once
//! more per change to one of its keys (review round 4: every change, any
//! thread's, expired every answer, so a table of pointers to long names cost
//! the table times the names).
//!
//! **The invariant** (plan section 3.16, W1): a walk that read a key sees
//! every change to it published after the read, by any thread. One
//! mechanism keeps it: the flag is set before the lookup reads, and every
//! publication calls `notify` after it publishes, unconditionally, and
//! `notify` decides whether a walk runs only then. A change published before
//! the reader's lock is seen by the lookup; one published after it finds the
//! flag, since the walk's start and its flag happen before its read, which
//! happens before the publication, which happens before `notify` reads the
//! walk count (review round 5: the `files` listing decided before it
//! published whether to notify, so a walk that started in between kept a
//! stale miss). A key is watched as a 64-bit hash under the Environment's
//! random state, so a collision only asks again an answer that still held,
//! and an input cannot aim one.
//!
//! **Memory** (W2): each watch is charged to its walk's meter, by a charge
//! the walk itself holds ([`Walk`]), so the charge is released only after
//! the walk's entries have left the map and every key list it grew has
//! shrunk back to what the running walks pay for (review round 5: the
//! charge was released before the entries left, and a key list kept the
//! room a finished walk had paid for).
//!
//! **What a walk dereferences again** (review round 13): a flag a change
//! made on the walk's own thread sets (its own lookups' loads, which are
//! UnityPy's whole walk) is logged, once, in the walk's [`Fired`] list, so
//! the walk learns which of its answers its own loads changed without
//! reading every flag it holds (`super::container_index`'s `Revisits`). A
//! change another thread makes sets the flag and logs nothing: the walk's
//! next lookup of that external sees it, and no table index the walk has
//! dereferenced is dereferenced again for it, so the walk's work never
//! depends on what other threads load.

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::held::lock;
use crate::error::Error;
use crate::limits::{Charge, HASH_TABLE_SHARES, Meter, heap_cost};

/// Set when a key it watches changes.
#[derive(Debug)]
pub(crate) struct Flag {
    set: AtomicBool,
    /// Its number among its walk's flags, from 0 in the order they were
    /// set up.
    id: usize,
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

    /// Sets it; the first time, on its walk's own thread, logs it in its
    /// walk's [`Fired`] list (module docs).
    fn fire(&self) {
        if !self.set.swap(true, Ordering::AcqRel) && self.log.thread == thread_here() {
            lock(&self.log.ids).push(self.id);
        }
    }
}

/// A walk's log of its flags a change on the walk's own thread set, by
/// number, each once (module docs).
#[derive(Debug)]
pub(crate) struct Fired {
    /// The walk's thread ([`thread_here`]).
    thread: usize,
    ids: Mutex<Vec<usize>>,
}

thread_local! {
    /// Its address names the thread that holds it while that thread runs.
    static HERE: u8 = const { 0 };
}

/// A number no other running thread has: the address of this thread's
/// [`HERE`] (a walk's flags live on its thread's stack, so its thread runs
/// while any of them can be set).
fn thread_here() -> usize {
    HERE.with(|h| std::ptr::from_ref(h).addr())
}

/// What one flag costs its walk: its `Arc` in a glibc chunk, and its
/// element in the walk's [`Fired`] list and in the list the walk takes it
/// out into, each of which doubles.
pub(crate) const FLAG: u64 =
    heap_cost((size_of::<Flag>() + 2 * size_of::<usize>()) as u64) + 4 * size_of::<usize>() as u64;

/// What a walk's [`Fired`] list costs it, made with its first flag: its
/// `Arc` in a glibc chunk.
pub(crate) const FIRED: u64 = heap_cost((size_of::<Fired>() + 2 * size_of::<usize>()) as u64);

/// What one watch of one key costs its walk: its share of the map, and its
/// element in the key's list and in the walk's own list, each twice (the
/// lists double, and a key list shrinks back when a walk ends, [`Walk`]).
pub(crate) const WATCH_SHARE: u64 = HASH_TABLE_SHARES
    * (size_of::<u64>() + size_of::<Vec<Weak<Flag>>>() + 1) as u64
    + 2 * (size_of::<Weak<Flag>>() + size_of::<u64>()) as u64;

/// The flags of the running walks, by key hash.
#[derive(Debug, Default)]
pub(crate) struct Watches {
    state: RandomState,
    /// Walks running.
    walks: AtomicUsize,
    map: Mutex<HashMap<u64, Vec<Weak<Flag>>>>,
}

impl Watches {
    /// Whether a walk runs.
    pub(crate) fn active(&self) -> bool {
        self.walks.load(Ordering::SeqCst) > 0
    }

    /// Starts a walk's watches, charged to `meter`; they end when it drops.
    pub(crate) fn start(&self, meter: &Arc<Meter>) -> Walk<'_> {
        self.walks.fetch_add(1, Ordering::SeqCst);
        Walk {
            watches: self,
            hashes: Vec::new(),
            fired: None,
            flags: 0,
            charge: Charge::zero(meter),
        }
    }

    /// The hash `key` (a registry key, or a `files` key lowered) is watched
    /// under. Computed before a publication, so the `notify` after it needs
    /// no copy of the key.
    pub(crate) fn hash(&self, key: &[u8]) -> u64 {
        self.state.hash_one(key)
    }

    /// Notes a change to `key`, after the change is published (module docs).
    pub(crate) fn notify(&self, key: &[u8]) {
        self.notify_hash(self.hash(key));
    }

    /// [`Self::notify`] of the key whose [`Self::hash`] is `hash`: when a
    /// walk runs, every flag under it is set (and logged, on its walk's own
    /// thread: module docs), and the key is watched no more until a lookup
    /// watches it again.
    pub(crate) fn notify_hash(&self, hash: u64) {
        if !self.active() {
            return;
        }
        let flags = lock(&self.map).remove(&hash);
        for flag in flags.into_iter().flatten() {
            if let Some(flag) = flag.upgrade() {
                flag.fire();
            }
        }
    }

    /// The map's entries and each key list's length and room, for a test.
    #[cfg(test)]
    pub(crate) fn room(&self) -> (usize, Vec<(usize, usize)>) {
        let map = lock(&self.map);
        let lists = map.values().map(|l| (l.len(), l.capacity())).collect();
        (map.len(), lists)
    }

    /// Walks running, for a test.
    #[cfg(test)]
    pub(crate) fn walks(&self) -> usize {
        self.walks.load(Ordering::SeqCst)
    }
}

/// One walk's watches, with the charge that pays for them. Its flags must
/// drop before it does (`Targets` holds them in a field before this one), so
/// its entries leave the map; its charge is a field of its own, released
/// after [`Drop::drop`] has taken them out (W2, module docs).
pub(crate) struct Walk<'a> {
    watches: &'a Watches,
    /// Every hash it watched, once per watch.
    hashes: Vec<u64>,
    /// Its flags' log, made with the first.
    fired: Option<Arc<Fired>>,
    /// The flags it has set up: the next one's number.
    flags: usize,
    /// [`FIRED`] once, [`FLAG`] per flag and [`WATCH_SHARE`] per watch of a
    /// key.
    charge: Charge,
}

impl Walk<'_> {
    /// The hash `key` is watched under.
    pub(crate) fn hash(&self, key: &[u8]) -> u64 {
        self.watches.hash(key)
    }

    /// A flag set at the next change to any of `hashes`, to be set up before
    /// the lookup that reads them, charged first ([`FLAG`], and
    /// [`WATCH_SHARE`] per hash); a refusal sets up nothing.
    /// Its number ([`Flag::id`]) is the count of the walk's flags set up
    /// before it.
    pub(crate) fn watch(&mut self, hashes: &[u64]) -> Result<Arc<Flag>, Error> {
        let n = u64::try_from(hashes.len()).unwrap_or(u64::MAX);
        let first = if self.fired.is_none() { FIRED } else { 0 };
        self.charge.grow(
            WATCH_SHARE
                .saturating_mul(n)
                .saturating_add(FLAG)
                .saturating_add(first),
        )?;
        let log = self
            .fired
            .get_or_insert_with(|| {
                Arc::new(Fired {
                    thread: thread_here(),
                    ids: Mutex::new(Vec::new()),
                })
            })
            .clone();
        let flag = Arc::new(Flag {
            set: AtomicBool::new(false),
            id: self.flags,
            log,
        });
        self.flags += 1;
        {
            let mut map = lock(&self.watches.map);
            for &h in hashes {
                map.entry(h).or_default().push(Arc::downgrade(&flag));
            }
        }
        self.hashes.extend_from_slice(hashes);
        Ok(flag)
    }

    /// The numbers of its flags a change on its own thread set since the
    /// last call, each once over the walk (module docs).
    pub(crate) fn fired(&self) -> Vec<usize> {
        self.fired
            .as_ref()
            .map(|f| std::mem::take(&mut *lock(&f.ids)))
            .unwrap_or_default()
    }

    /// What the walk's charge holds, for a test.
    #[cfg(test)]
    pub(crate) fn charged(&self) -> u64 {
        self.charge.bytes()
    }
}

impl Drop for Walk<'_> {
    fn drop(&mut self) {
        self.hashes.sort_unstable();
        self.hashes.dedup();
        {
            let mut map = lock(&self.watches.map);
            for h in &self.hashes {
                if let Some(flags) = map.get_mut(h) {
                    flags.retain(|f| f.strong_count() > 0);
                    if flags.is_empty() {
                        map.remove(h);
                    } else if flags.capacity() > 2 * flags.len() + 4 {
                        // The running walks pay for two elements a watch:
                        // a list keeps no room a finished walk paid for.
                        flags.shrink_to(2 * flags.len());
                    }
                }
            }
            // The map keeps no room the running walks' charges do not pay
            // for (a key's share covers a growing table's).
            let len = map.len();
            if map.capacity() > 3 * len + 8 {
                map.shrink_to(len);
            }
        }
        self.watches.walks.fetch_sub(1, Ordering::SeqCst);
        // `charge` drops after this body, once what it paid for is freed.
    }
}

#[cfg(test)]
mod tests;
