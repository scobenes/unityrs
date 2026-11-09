//! The walks' watches keep no memory past their charge (invariant W2, plan
//! section 3.16): a walk's charge is released only after its entries left
//! both tables, every key list it grew and each table's map shrank back to
//! what the running walks pay for (review rounds 5 and 16), and every block
//! the walk itself holds is freed (review round 16), whatever another
//! thread's change does meanwhile (review round 15). A walk's log lists each
//! flag a change set once, whichever thread made the change (review round
//! 14), however its push meets another push or a take (review round 15).
//! Each race is run by the module's hooks ([`super::Point`]) and the
//! testkit's handshakes, never by repetition or timing.

// The source scan behind "the core starts no thread and never waits"
// (`unityrs_testkit::waits`) takes only an item under `#[cfg(test)]` in its
// own file for test code, so every item here that names a thread, a channel
// or a lock carries one.
#[cfg(test)]
use std::sync::mpsc::{Receiver, Sender, channel};
#[cfg(test)]
use std::sync::{Arc, Mutex, RwLock};

use super::{
    FILES, FIRED, FLAG, Point, REGISTRY, Table, Under, WATCH_SHARE, Walk, Watches, set_hook,
};
use crate::budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation};
use crate::env::heap::{self, Noted};
use crate::env::held;
use crate::limits::{Meter, heap_cost};
#[cfg(test)]
use unityrs_testkit::guard;

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

/// Stops the calling thread the first time it reaches `at` ([`Point`]): it
/// tells `stopped`, then waits for `go`. Its hook is taken off when `run`
/// returns.
#[cfg(test)]
fn stopping_at<T>(at: Point, stopped: Sender<()>, go: Receiver<()>, run: impl FnOnce() -> T) -> T {
    let armed = std::cell::Cell::new(true);
    set_hook(Some(std::rc::Rc::new(move |p| {
        if p == at && armed.replace(false) {
            let _ = stopped.send(());
            guard::hold(&go, "the test to let the stopped thread go");
        }
    })));
    let out = run();
    set_hook(None);
    out
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
            let (n, _, l) = held::read(&lock.0).room();
            entries += n;
            lists.extend(l);
        }
        (entries, lists)
    }

    /// Each table's entries and the room its map holds.
    fn maps(&self) -> Vec<(usize, usize)> {
        self.locks
            .iter()
            .map(|lock| {
                let (n, room, _) = held::read(&lock.0).room();
                (n, room)
            })
            .collect()
    }
}

/// The most room a map of `len` entries keeps once a walk has ended: three
/// entries a key and eight more, the bound `Table::end` shrinks a map to
/// when it holds more (W2: no room the running walks' charges do not pay
/// for, each watch paying [`crate::limits::HASH_TABLE_SHARES`] buckets).
fn map_bound(len: usize) -> usize {
    3 * len + 8
}

/// A budget that, at every release, records the walks running, the tables'
/// entries and the room the rig it is given keeps.
#[cfg(test)]
#[derive(Debug)]
struct Spy(Arc<Home>);

#[cfg(test)]
#[derive(Debug, Default)]
struct Home {
    rig: Mutex<Option<Arc<Rig>>>,
    /// At each release: the walks running, the tables' entries, the most
    /// room any key list kept past twice its length, and the most room any
    /// map kept past [`map_bound`].
    seen: Mutex<Vec<(usize, usize, usize, usize)>>,
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
            let map_excess = r
                .maps()
                .iter()
                .map(|&(len, room)| room.saturating_sub(map_bound(len)))
                .max()
                .unwrap_or(0);
            self.seen
                .lock()
                .unwrap()
                .push((r.watches.walks(), entries, excess, map_excess));
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
    for (walks, _, excess, _) in seen {
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
            .all(|&(walks, entries, _, _)| walks == 0 && entries == 0),
        "released while its entries were in a table: {seen:?}"
    );
}

/// Walk `a` watches 10,000 keys under each lock while walk `b` watches one
/// under each: when `a` ends, each table's map keeps no more room than
/// [`map_bound`] allows for `b`'s one entry, and `a`'s charge is released
/// only after both maps shrank (W2; review round 16: no test failed when a
/// map kept, after the walk that grew it ended, the room of every key it had
/// watched, charged to no one).
#[test]
fn a_finished_walk_gives_back_the_maps_room() {
    let (rig, home, meter) = spied();
    let mut b = rig.start(&meter);
    let k = b.hash(b"k");
    let kept = b.watch(&[(REGISTRY, k), (FILES, k)]).unwrap();
    {
        let mut a = rig.start(&meter);
        let keys: Vec<(usize, u64)> = (0..10_000_u32)
            .flat_map(|i| {
                let h = a.hash(&i.to_le_bytes());
                [(REGISTRY, h), (FILES, h)]
            })
            .collect();
        drop(a.watch(&keys).unwrap());
        for (len, room) in rig.maps() {
            assert!(len >= 10_000 && room >= len, "a grew {len} keys, {room}");
        }
        home.seen.lock().unwrap().clear();
    }
    for (len, room) in rig.maps() {
        assert_eq!(len, 1, "only b's key is left");
        assert!(
            room <= map_bound(len),
            "the map kept room for {room} after a ended"
        );
    }
    let seen = home.seen.lock().unwrap().clone();
    assert_ne!(seen.len(), 0, "a's charge was released");
    for (walks, _, _, map_excess) in seen {
        assert_eq!(walks, 1, "a's charge was released while a still ran");
        assert_eq!(map_excess, 0, "a's charge was released before a map shrank");
    }
    drop(kept);
    drop(b);
    assert_eq!(rig.room(), (0, Vec::new()));
}

/// A walk's charge is released after every block the walk holds is freed:
/// its entries in both tables and the maps' room (its `Drop::drop`), and its
/// own `hashes`, `log` and `flags` (its fields, which drop before `charge`,
/// declared last). The test's thread counts its heap ([`heap`]): the heap
/// the charge's release sees is the heap the walk's drop leaves, and less
/// than before the drop (review round
/// 16: that order rested on the fields' declaration alone, and no test
/// failed with `charge` declared before `hashes`, `log` or `flags`, or one
/// of them after it).
#[test]
fn a_walks_charge_is_released_after_the_last_block_it_paid_for() {
    heap::count();
    let (meter, noted) = Noted::meter();
    let rig = Rig::default();
    let mut walk = rig.start(&meter);
    let keys: Vec<u64> = (0..64_u32).map(|i| walk.hash(&i.to_le_bytes())).collect();
    let flags: Vec<_> = keys
        .iter()
        .map(|k| walk.watch(&[(REGISTRY, *k), (FILES, *k)]).unwrap())
        .collect();
    drop(flags);
    let (before, from) = (heap::live(), noted.releases());
    drop(walk);
    let after = heap::live();
    assert!(before > after, "the walk's drop freed nothing");
    heap::released_last(&noted, from, after, "the walk's charge");
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

/// A walk's end takes out every entry of its own, by identity, while
/// another thread's change holds one of its flags (W2; review round 15: the
/// end kept every entry whose flag was still alive, so the one a notify
/// held stayed in the registry's table, dead, after the walk's charge was
/// released). The walk's flag watches a registry key and a `files` key;
/// another thread publishes the `files` key and is stopped right after its
/// notify upgraded the flag, its lock held; the walk ends on a third thread,
/// stopped right after it left the registry's table; the notify is let go,
/// then the end.
#[cfg(test)]
#[test]
fn a_walks_end_takes_out_its_entries_while_another_thread_holds_its_flag() {
    let (rig, home, meter) = spied();
    let mut walk = rig.start(&meter);
    let (r, f) = (walk.hash(b"r"), walk.hash(b"f"));
    drop(walk.watch(&[(REGISTRY, r), (FILES, f)]).unwrap());
    home.seen.lock().unwrap().clear();
    let rig = &*rig;
    std::thread::scope(|s| {
        let (notified, notify_stopped) = channel();
        let (notify_go, go) = channel();
        let notify = s.spawn(move || {
            stopping_at(Point::Upgraded, notified, go, || rig.publish(FILES, f));
        });
        guard::wait(&notify_stopped, "the notify to hold the walk's flag");
        let (ended, end_stopped) = channel();
        let (end_go, go) = channel();
        let end = s.spawn(move || {
            stopping_at(Point::Ended(REGISTRY), ended, go, || drop(walk));
        });
        guard::wait(&end_stopped, "the walk's end to leave the registry's table");
        notify_go.send(()).unwrap();
        notify.join().unwrap();
        end_go.send(()).unwrap();
        end.join().unwrap();
    });
    assert_eq!(rig.room(), (0, Vec::new()), "the walk left an entry");
    let seen = home.seen.lock().unwrap().clone();
    assert_ne!(seen.len(), 0, "the walk's charge was released");
    assert!(
        seen.iter()
            .all(|&(walks, entries, _, _)| walks == 0 && entries == 0),
        "released while its entries were in a table: {seen:?}"
    );
}

/// A flag's push that meets another flag's push of the same walk tries
/// again, so the log holds both (review round 15: a push that gave up on
/// a failed exchange was caught only by a test whose threads met by
/// chance): one flag's push, under the registry's lock, is stopped right
/// after it read the log's head; the other flag, under the `files` lock,
/// is pushed then; the first is let go.
#[cfg(test)]
#[test]
fn a_push_that_meets_another_push_tries_again() {
    let (rig, _home, meter) = spied();
    let mut walk = rig.start(&meter);
    let (a, b) = (walk.hash(b"a"), walk.hash(b"b"));
    let first = walk.watch(&[(REGISTRY, a)]).unwrap();
    let second = walk.watch(&[(FILES, b)]).unwrap();
    let rig = &*rig;
    std::thread::scope(|s| {
        let (read, stopped) = channel();
        let (go, go_rx) = channel();
        let push = s.spawn(move || {
            stopping_at(Point::Pushing, read, go_rx, || rig.publish(REGISTRY, a));
        });
        guard::wait(&stopped, "the first push to read the log's head");
        rig.publish(FILES, b);
        go.send(()).unwrap();
        push.join().unwrap();
    });
    assert_eq!(
        walk.fired().collect::<Vec<_>>(),
        [first.id(), second.id()],
        "the log lost a flag"
    );
}

/// A flag's push that meets its walk's take of the log tries again, so the
/// next take holds it (review round 15): a flag is logged; another flag's
/// push, under the registry's lock, is stopped right after it read the
/// log's head (the first flag); the walk takes the log then; the push is
/// let go.
#[cfg(test)]
#[test]
fn a_push_that_meets_a_take_tries_again() {
    let (rig, _home, meter) = spied();
    let mut walk = rig.start(&meter);
    let (a, c) = (walk.hash(b"a"), walk.hash(b"c"));
    let earlier = walk.watch(&[(REGISTRY, c)]).unwrap();
    let pushed = walk.watch(&[(REGISTRY, a)]).unwrap();
    rig.publish(REGISTRY, c);
    let rig = &*rig;
    let mut taken = Vec::new();
    std::thread::scope(|s| {
        let (read, stopped) = channel();
        let (go, go_rx) = channel();
        let push = s.spawn(move || {
            stopping_at(Point::Pushing, read, go_rx, || rig.publish(REGISTRY, a));
        });
        guard::wait(&stopped, "the push to read the log's head");
        taken.extend(walk.fired());
        go.send(()).unwrap();
        push.join().unwrap();
    });
    assert_eq!(taken, [earlier.id()]);
    assert!(pushed.changed());
    assert_eq!(
        walk.fired().collect::<Vec<_>>(),
        [pushed.id()],
        "the log lost a flag"
    );
}
