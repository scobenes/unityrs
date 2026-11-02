//! The walks' watches keep no memory past their charge (invariant W2, plan
//! section 3.16): a walk's charge is released only after its entries left
//! both tables and every key list it grew shrank back to what the running
//! walks pay for (review round 5). A walk's log lists each flag a change
//! set once, whichever thread made the change (review round 14).

// The source scan behind "the core starts no thread and never waits"
// (`unityrs_testkit::waits`) takes only an item under `#[cfg(test)]` in its
// own file for test code, so every item here that names a thread, a channel
// or a lock carries one.
#[cfg(test)]
use std::sync::{Arc, Mutex, RwLock};

use super::{FILES, FIRED, FLAG, REGISTRY, Table, Under, WATCH_SHARE, Walk, Watches};
use crate::budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation};
use crate::env::held;
use crate::limits::{Meter, heap_cost};

/// A lock of the test's own that keys are published under, as the
/// registry's and the `files` listing's are.
#[cfg(test)]
#[derive(Debug, Default)]
struct Lock(RwLock<Table>);

#[cfg(test)]
impl Under for Lock {
    fn with_table(&self, f: &mut dyn FnMut(&mut Table)) {
        f(&mut held::write(&self.0));
    }
}

/// The two locks and the watches a walk starts on.
#[cfg(test)]
#[derive(Debug, Default)]
struct Rig {
    watches: Watches,
    locks: [Lock; 2],
}

#[cfg(test)]
impl Rig {
    fn start(&self, meter: &Arc<Meter>) -> Walk<'_> {
        let [registry, files] = &self.locks;
        self.watches.start(meter, [registry, files])
    }

    /// A change to the key hashed `hash`, published under lock `at`.
    fn publish(&self, at: usize, hash: u64) {
        self.locks[at].with_table(&mut |t| t.notify(hash));
    }

    /// Both tables' entries, and every key list's length and room.
    fn room(&self) -> (usize, Vec<(usize, usize)>) {
        let mut entries = 0;
        let mut lists = Vec::new();
        for lock in &self.locks {
            let (n, l) = held::read(&lock.0).room();
            entries += n;
            lists.extend(l);
        }
        (entries, lists)
    }
}

/// A budget that, at every release, records the walks running and the
/// tables' entries of the rig it is given.
#[cfg(test)]
#[derive(Debug)]
struct Spy(Arc<Home>);

#[cfg(test)]
#[derive(Debug, Default)]
struct Home {
    rig: Mutex<Option<Arc<Rig>>>,
    /// At each release: the walks running, the tables' entries, and the
    /// most room any key list kept past twice its length.
    seen: Mutex<Vec<(usize, usize, usize)>>,
}

#[cfg(test)]
impl MemoryBudget for Spy {
    fn try_reserve(&self, bytes: u64, _what: Reservation) -> Result<Permit, BudgetExceeded> {
        Ok(Permit::new(bytes, self.0.clone()))
    }
}

#[cfg(test)]
impl PermitHome for Home {
    fn release(&self, _bytes: u64) {
        let rig = self.rig.lock().unwrap().clone();
        if let Some(r) = rig {
            let (entries, lists) = r.room();
            let excess = lists
                .iter()
                .map(|&(len, cap)| cap.saturating_sub(2 * len))
                .max()
                .unwrap_or(0);
            self.seen
                .lock()
                .unwrap()
                .push((r.watches.walks(), entries, excess));
        }
    }
}

#[cfg(test)]
fn spied() -> (Arc<Rig>, Arc<Home>, Arc<Meter>) {
    let rig = Arc::new(Rig::default());
    let home = Arc::new(Home::default());
    *home.rig.lock().unwrap() = Some(rig.clone());
    let meter = Meter::new(u64::MAX, Arc::new(Spy(home.clone())), Reservation::Input);
    (rig, home, meter)
}

/// Walk `a` watches key `k` 10,000 times while walk `b` watches it once:
/// when `a` ends, `k`'s list keeps no more room than `b`'s one watch pays
/// for (two elements, and the four a list starts with), and `a`'s charge is
/// released only after its entries left (review round 5: the list kept
/// 16,384 elements' room, 128 KiB charged to no one, until `b` ended).
#[test]
fn a_finished_walk_leaves_no_room_in_a_shared_keys_list() {
    let (rig, home, meter) = spied();
    let mut b = rig.start(&meter);
    let k = b.hash(b"k");
    let kept = b.watch(&[(REGISTRY, k)]).unwrap();
    {
        let mut a: Walk<'_> = rig.start(&meter);
        for _ in 0..10_000 {
            drop(a.watch(&[(REGISTRY, k)]).unwrap());
        }
        assert!(a.charged() > 0);
        home.seen.lock().unwrap().clear();
    }
    let (entries, lists) = rig.room();
    assert_eq!(entries, 1);
    let &[(len, cap)] = lists.as_slice() else {
        panic!("{lists:?}");
    };
    assert_eq!(len, 1, "only b's watch is left");
    assert!(
        cap <= 2 * len + 4,
        "k's list kept room for {cap} after a ended"
    );
    let seen = home.seen.lock().unwrap().clone();
    assert_ne!(seen.len(), 0, "a's charge was released");
    for (walks, _, excess) in seen {
        assert_eq!(walks, 1, "a's charge was released while a still ran");
        assert!(
            excess <= 4,
            "a's charge was released before k's list shrank"
        );
    }
    drop(kept);
    drop(b);
    assert_eq!(rig.room(), (0, Vec::new()));
}

/// A walk's charge is released after its entries left both tables: every
/// release its walk makes sees no walk running and empty tables.
#[test]
fn a_walks_charge_outlives_its_entries() {
    let (rig, home, meter) = spied();
    {
        let mut walk = rig.start(&meter);
        let keys: Vec<u64> = (0..64_u32).map(|i| walk.hash(&i.to_le_bytes())).collect();
        let flags: Vec<_> = keys
            .iter()
            .map(|k| walk.watch(&[(REGISTRY, *k), (FILES, *k)]).unwrap())
            .collect();
        drop(flags);
        home.seen.lock().unwrap().clear();
    }
    let seen = home.seen.lock().unwrap().clone();
    assert_ne!(seen.len(), 0);
    assert!(
        seen.iter()
            .all(|&(walks, entries, _)| walks == 0 && entries == 0),
        "released while its entries were in a table: {seen:?}"
    );
}

/// A walk's log lists each flag a change set once, whichever key and lock
/// set it, and the list of its flags holds no more room than the walk's
/// charge pays for it ([`FIRED`] once, two elements a flag in [`FLAG`]):
/// `n` flags each watching a registry key and a `files` key, both keys
/// changed, the log taken half way and at the end (review round 13).
#[test]
fn a_walks_log_lists_each_flag_once_within_its_charge() {
    let (rig, _home, meter) = spied();
    for n in [1_usize, 3, 5, 64, 1000] {
        let mut walk = rig.start(&meter);
        let keys: Vec<[u64; 2]> = (0..n)
            .map(|i| {
                let i = u64::try_from(i).unwrap();
                [
                    walk.hash(&i.to_le_bytes()),
                    walk.hash(&((i + 1) << 32).to_le_bytes()),
                ]
            })
            .collect();
        let flags: Vec<_> = keys
            .iter()
            .map(|k| walk.watch(&[(REGISTRY, k[0]), (FILES, k[1])]).unwrap())
            .collect();
        let charged = walk.charged();
        let flags_n = u64::try_from(n).unwrap();
        assert_eq!(charged, FIRED + flags_n * (FLAG + 2 * WATCH_SHARE), "{n}");
        // What the charge holds past the flags' and the log's `Arc`s.
        let arcs = heap_cost(size_of::<super::Fired>() as u64 + 16)
            + flags_n * heap_cost(size_of::<super::Flag>() as u64 + 16);
        let list_share = FIRED + flags_n * FLAG - arcs;
        let room = u64::try_from(walk.flags_room()).unwrap();
        assert!(
            heap_cost(room * 8) <= list_share,
            "{n}: room {room} past {list_share}"
        );
        let mut seen = Vec::new();
        for (i, k) in keys.iter().enumerate() {
            // Odd flags see the `files` key change first.
            let order = if i % 2 == 0 {
                [(REGISTRY, k[0]), (FILES, k[1])]
            } else {
                [(FILES, k[1]), (REGISTRY, k[0])]
            };
            for (at, h) in order {
                rig.publish(at, h);
            }
            if i == n / 2 {
                seen.extend(walk.fired());
            }
        }
        seen.extend(walk.fired());
        seen.sort_unstable();
        assert_eq!(seen, (0..n).collect::<Vec<_>>(), "{n}: each flag once");
        assert!(flags.iter().all(|f| f.changed()), "{n}");
        assert_eq!(walk.fired().count(), 0, "{n}: taken");
        drop(flags);
    }
}

/// A change another thread publishes logs the flags it sets in the walk's
/// log, as one on the walk's own thread does (review round 14: only the
/// walk's own thread logged, and another thread's change took the flag
/// from its key, so the walk's own later change of the key logged
/// nothing): another thread changes `k`, then the walk's own thread
/// changes it again; the walk's log holds its flag once.
#[cfg(test)]
#[test]
fn another_threads_change_logs_the_walks_flag() {
    let (rig, _home, meter) = spied();
    let mut walk = rig.start(&meter);
    let k = walk.hash(b"k");
    let other = walk.hash(b"other");
    let flag = walk.watch(&[(REGISTRY, k)]).unwrap();
    let untouched = walk.watch(&[(REGISTRY, other)]).unwrap();
    std::thread::scope(|s| {
        s.spawn(|| rig.publish(REGISTRY, k));
    });
    assert!(flag.changed());
    rig.publish(REGISTRY, k);
    assert_eq!(walk.fired().collect::<Vec<_>>(), [flag.id()]);
    assert!(!untouched.changed());
    assert_eq!(walk.fired().count(), 0);
}

/// Four threads each change the keys of their own quarter of a walk's
/// flags, each key twice, while the walk's thread takes its log over and
/// over: every flag is in the walk's logs once, whatever the order the
/// threads' changes and the takes come in.
#[cfg(test)]
#[test]
fn the_log_lists_each_flag_once_under_changes_from_many_threads() {
    const N: usize = 4096;
    let (rig, _home, meter) = spied();
    let mut walk = rig.start(&meter);
    let keys: Vec<u64> = (0..N)
        .map(|i| walk.hash(&u64::try_from(i).unwrap().to_le_bytes()))
        .collect();
    let flags: Vec<_> = keys
        .iter()
        .map(|&k| walk.watch(&[(REGISTRY, k)]).unwrap())
        .collect();
    let mut seen = Vec::new();
    std::thread::scope(|s| {
        let rig = &rig;
        let keys = &keys;
        let threads: Vec<_> = (0..4)
            .map(|t| {
                s.spawn(move || {
                    for _ in 0..2 {
                        for k in keys.iter().skip(t).step_by(4) {
                            rig.publish(REGISTRY, *k);
                        }
                    }
                })
            })
            .collect();
        while !threads
            .iter()
            .all(std::thread::ScopedJoinHandle::is_finished)
        {
            seen.extend(walk.fired());
        }
    });
    seen.extend(walk.fired());
    seen.sort_unstable();
    assert_eq!(seen, (0..N).collect::<Vec<_>>());
    assert!(flags.iter().all(|f| f.changed()));
}
