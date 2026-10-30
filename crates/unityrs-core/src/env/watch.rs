//! Watches on the answers a container preload walk keeps
//! (`super::container_index`'s `Targets`). A walk keeps an external's answer
//! (found or not) until a key its lookup read changes: the registry key
//! `find_file` asks, and a top-level file's lowered `files` key. Each lookup
//! sets a [`Flag`] under those keys before it reads them, and every change to
//! a key while a walk runs, by any thread (a load's registration, a resolver
//! load's, under `LoadMode::Lazy` the entries a reach registers, a `files`
//! entry), sets the flags under it ([`Watches::notify`]), so a walk asks again
//! exactly the answers UnityPy's next lookup could find changed, and each
//! external is looked up once, and once more per change to one of its keys
//! (review round 4: every change, any thread's, expired every answer, so a
//! table of pointers to long names cost the table times the names).
//!
//! The flag is set before the lookup reads, so a change made while it reads
//! is never missed: a change published before the reader's lock is seen by
//! the lookup, and one published after it finds the flag (the change's
//! `notify` follows its publication; a `notify` that finds no walk running
//! comes after a walk's start only if the walk's lookup took the lock
//! first). A key is watched as a 64-bit hash under the Environment's random
//! state, so a collision only asks again an answer that still held, and an
//! input cannot aim one. Each watch is charged to its walk's meter until the
//! walk ends, when its entries leave the map.

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use crate::limits::{HASH_TABLE_SHARES, heap_cost};

/// Set when a key it watches changes.
#[derive(Debug, Default)]
pub(crate) struct Flag(AtomicBool);

impl Flag {
    /// Whether a key it watches changed since it was set up.
    pub(crate) fn changed(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// What one flag costs its walk: its `Arc` in a glibc chunk.
pub(crate) const FLAG: u64 = heap_cost((size_of::<Flag>() + 2 * size_of::<usize>()) as u64);

/// What one watch of one key costs its walk: its share of the map, and its
/// element in the key's list and in the walk's own list, each twice (the
/// lists double).
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

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Watches {
    /// Whether a walk runs, so a change must be noted.
    pub(crate) fn active(&self) -> bool {
        self.walks.load(Ordering::SeqCst) > 0
    }

    /// Starts a walk's watches; they end when it drops.
    pub(crate) fn start(&self) -> Walk<'_> {
        self.walks.fetch_add(1, Ordering::SeqCst);
        Walk {
            watches: self,
            hashes: Vec::new(),
        }
    }

    /// Notes a change to `key` (a registry key, or a `files` key lowered),
    /// after the change is published: every flag under it is set, and the
    /// key is watched no more until a lookup watches it again.
    pub(crate) fn notify(&self, key: &[u8]) {
        if !self.active() {
            return;
        }
        let hash = self.state.hash_one(key);
        let flags = lock(&self.map).remove(&hash);
        for flag in flags.into_iter().flatten() {
            if let Some(flag) = flag.upgrade() {
                flag.0.store(true, Ordering::Release);
            }
        }
    }
}

/// One walk's watches. Its flags must drop before it does (`Targets` holds
/// them in a field before this one), so its entries leave the map.
pub(crate) struct Walk<'a> {
    watches: &'a Watches,
    /// Every hash it watched, once per watch.
    hashes: Vec<u64>,
}

impl Walk<'_> {
    /// The hash `key` is watched under.
    pub(crate) fn hash(&self, key: &[u8]) -> u64 {
        self.watches.state.hash_one(key)
    }

    /// A flag set at the next change to any of `hashes`, to be set up before
    /// the lookup that reads them. The caller charged [`FLAG`], and
    /// [`WATCH_SHARE`] per hash.
    pub(crate) fn watch(&mut self, hashes: &[u64]) -> Arc<Flag> {
        let flag = Arc::new(Flag::default());
        {
            let mut map = lock(&self.watches.map);
            for &h in hashes {
                map.entry(h).or_default().push(Arc::downgrade(&flag));
            }
        }
        self.hashes.extend_from_slice(hashes);
        flag
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
    }
}
