//! The walks' watches keep no memory past their charge (invariant W2, plan
//! section 3.16): a walk's charge is released only after its entries left
//! the map and every key list it grew shrank back to what the running walks
//! pay for (review round 5).

use std::sync::{Arc, Mutex};

use super::{FIRED, FLAG, WATCH_SHARE, Walk, Watches};
use crate::budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation};
use crate::limits::{Meter, heap_cost};

/// A budget that, at every release, records the walks running and the
/// map's entries of the watches it is given.
#[derive(Debug)]
struct Spy(Arc<Home>);

#[derive(Debug, Default)]
struct Home {
    watches: Mutex<Option<Arc<Watches>>>,
    /// At each release: the walks running, the map's entries, and the most
    /// room any key list kept past twice its length.
    seen: Mutex<Vec<(usize, usize, usize)>>,
}

impl MemoryBudget for Spy {
    fn try_reserve(&self, bytes: u64, _what: Reservation) -> Result<Permit, BudgetExceeded> {
        Ok(Permit::new(bytes, self.0.clone()))
    }
}

impl PermitHome for Home {
    fn release(&self, _bytes: u64) {
        let watches = self.watches.lock().unwrap().clone();
        if let Some(w) = watches {
            let (entries, lists) = w.room();
            let excess = lists
                .iter()
                .map(|&(len, cap)| cap.saturating_sub(2 * len))
                .max()
                .unwrap_or(0);
            self.seen.lock().unwrap().push((w.walks(), entries, excess));
        }
    }
}

fn spied() -> (Arc<Watches>, Arc<Home>, Arc<Meter>) {
    let watches = Arc::new(Watches::default());
    let home = Arc::new(Home::default());
    *home.watches.lock().unwrap() = Some(watches.clone());
    let meter = Meter::new(u64::MAX, Arc::new(Spy(home.clone())), Reservation::Input);
    (watches, home, meter)
}

/// Walk `a` watches key `k` 10,000 times while walk `b` watches it once:
/// when `a` ends, `k`'s list keeps no more room than `b`'s one watch pays
/// for (two elements, and the four a list starts with), and `a`'s charge is
/// released only after its entries left (review round 5: the list kept
/// 16,384 elements' room, 128 KiB charged to no one, until `b` ended).
#[test]
fn a_finished_walk_leaves_no_room_in_a_shared_keys_list() {
    let (watches, home, meter) = spied();
    let mut b = watches.start(&meter);
    let k = b.hash(b"k");
    let kept = b.watch(&[k]).unwrap();
    {
        let mut a: Walk<'_> = watches.start(&meter);
        for _ in 0..10_000 {
            // The flag drops at once, as an unkept resolver load's does.
            drop(a.watch(&[k]).unwrap());
        }
        assert!(a.charged() > 0);
        home.seen.lock().unwrap().clear();
    }
    let (entries, lists) = watches.room();
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
    assert_eq!(watches.room(), (0, Vec::new()));
}

/// A walk's charge is released after its entries left the map: every
/// release its walk makes sees no walk running and an empty map.
#[test]
fn a_walks_charge_outlives_its_entries() {
    let (watches, home, meter) = spied();
    {
        let mut walk = watches.start(&meter);
        let keys: Vec<u64> = (0..64_u32).map(|i| walk.hash(&i.to_le_bytes())).collect();
        let flags: Vec<_> = keys.iter().map(|k| walk.watch(&[*k]).unwrap()).collect();
        drop(flags);
        home.seen.lock().unwrap().clear();
    }
    let seen = home.seen.lock().unwrap().clone();
    assert_ne!(seen.len(), 0);
    assert!(
        seen.iter()
            .all(|&(walks, entries, _)| walks == 0 && entries == 0),
        "released while its entries were in the map: {seen:?}"
    );
}

/// A walk's log of its flags set on its own thread lists each once, in the
/// order they were set, and its vector holds no more room than the walk's
/// charge pays for it ([`FIRED`] once, two elements a flag in [`FLAG`]):
/// `n` flags each watching two keys, both keys changed, the list taken
/// half way and at the end (review round 13).
#[test]
fn a_walks_log_lists_each_flag_once_within_its_charge() {
    let (watches, _home, meter) = spied();
    for n in [1_usize, 3, 5, 64, 1000] {
        let mut walk = watches.start(&meter);
        let keys: Vec<[u64; 2]> = (0..n)
            .map(|i| {
                let i = u64::try_from(i).unwrap();
                [
                    walk.hash(&i.to_le_bytes()),
                    walk.hash(&((i + 1) << 32).to_le_bytes()),
                ]
            })
            .collect();
        let flags: Vec<_> = keys.iter().map(|k| walk.watch(k).unwrap()).collect();
        let charged = walk.charged();
        let flags_n = u64::try_from(n).unwrap();
        assert_eq!(charged, FIRED + flags_n * (FLAG + 2 * WATCH_SHARE), "{n}");
        // What the charge holds past the flags' and the list's `Arc`s.
        let arcs = heap_cost(size_of::<super::Fired>() as u64 + 16)
            + flags_n * heap_cost(size_of::<super::Flag>() as u64 + 16);
        let log_share = FIRED + flags_n * FLAG - arcs;
        let mut seen = Vec::new();
        for (i, k) in keys.iter().enumerate() {
            watches.notify_hash(k[0]);
            watches.notify_hash(k[1]);
            let room = u64::try_from(walk.log_room()).unwrap();
            assert!(
                heap_cost(room * 8) <= log_share,
                "{n}: room {room} past {log_share}"
            );
            if i == n / 2 {
                seen.extend(walk.fired());
            }
        }
        seen.extend(walk.fired());
        assert_eq!(
            seen,
            (0..n).collect::<Vec<_>>(),
            "{n}: each flag once, in order"
        );
        assert!(flags.iter().all(|f| f.changed()), "{n}");
        assert_eq!(walk.fired(), Vec::<usize>::new(), "{n}: taken");
        drop(flags);
    }
}
