//! The LRU's list and map stay one ([`super::DepCache`]'s type docs), its
//! work per use is constant, and an eviction ends, whatever the order in
//! which threads take their steps (review round 3: a use decided under one
//! lock whether its key was cached and acted under another, so a key
//! another thread evicted between the two was listed with no entry, the
//! dependencies before it fell off the list for good, and an eviction that
//! reached it spun). Every interleaving is forced with a hook
//! ([`super::Hook`]), never by repetition: each operation stopped at each
//! point between its steps while each other operation runs whole. A stress
//! run of random operations on several threads follows as a second check.
//! Every step a test build takes checks the list and the map
//! ([`super::Lru::check`]), so each test checks them at every step.

// The source scan behind "the core starts no thread and never waits"
// (`unityrs_testkit::waits`) takes only an item under `#[cfg(test)]` in its
// own file for test code (this file's `#[cfg(test)] mod tests;` is in
// another), so every item here that names a thread, a channel or a barrier
// carries one.
#[cfg(test)]
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
#[cfg(test)]
use std::thread::ThreadId;

use super::{DepCache, Hook, HookFn, LRU_SHARE, Lru, LruNode, lock, lru_key};
use crate::budget::Reservation;
use crate::env::registry::{Dependency, Open, open_bare};
use crate::env::{EnvOptions, Environment, Source};
use crate::files::File;
use crate::limits::Meter;
use crate::options::FileContext;
use crate::raw::RawStr;
use bytes::Bytes;
use unityrs_testkit::env::{Obj, bundle, res_bytes, serialized};
use unityrs_testkit::guard;
use unityrs_testkit::rng::Rng;

// Every wait for a thread the test stopped or let go goes through
// `unityrs_testkit::guard`: a deadlock guard, never a timing (every wait
// ends as soon as the thread gets there).

/// A dependency bundle: `cab`, holding one object, and with `pad` bytes
/// `cab.resS`.
fn bundle_with(cab: &str, pad: usize) -> Vec<u8> {
    let named = serialized(&[], &[Obj::named(1, cab, &[])]);
    if pad == 0 {
        return bundle(&[(cab, named)]);
    }
    bundle(&[(cab, named), (&format!("{cab}.resS"), res_bytes(pad, 1))])
}

fn source(cab: &str, pad: usize) -> Source {
    Source::Bytes {
        name: RawStr::from(format!("{cab}.bundle").as_str()),
        data: Bytes::from(bundle_with(cab, pad)),
    }
}

impl DepCache {
    /// Checks the list and the map now ([`Lru::check`]).
    pub(crate) fn check(&self) {
        lock(&self.lru).check();
    }

    /// The sum of the entries' meters as last read.
    pub(crate) fn total(&self) -> u64 {
        lock(&self.lru).total
    }

    /// How many dependencies the LRU holds.
    pub(crate) fn len(&self) -> usize {
        lock(&self.lru).nodes.len()
    }
}

/// Dependencies of one size, opened on their own meters, and an LRU whose
/// bound holds two and a half of them.
struct Rig {
    cache: Arc<DepCache>,
    deps: Vec<(Arc<Dependency>, File)>,
}

impl Rig {
    fn new(n: usize) -> Self {
        let ctx = Arc::new(FileContext::unitypy());
        let open = |i: usize| {
            let src = source(&format!("CAB-{i}"), 64);
            let name = RawStr::from(format!("d{i}").as_str());
            let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Dependency);
            let file = open_bare(&name, &src, &meter, &ctx).unwrap();
            (name, src, meter, file)
        };
        // What one holds while it is open (its charges go with the file).
        let (_, _, meter, file) = open(0);
        let one = meter.used() + LRU_SHARE;
        drop(file);
        let cache = Arc::new(DepCache::new(one * 5 / 2, true));
        let deps = (0..n)
            .map(|i| {
                let (name, source, meter, file) = open(i);
                let dep = Arc::new(Dependency {
                    name,
                    source,
                    open: Mutex::new(Open::of(&file, meter)),
                    ctx: ctx.clone(),
                    cache: Arc::downgrade(&cache),
                    slots: std::sync::atomic::AtomicUsize::new(0),
                    #[cfg(feature = "test-counters")]
                    load_id: crate::counters::LoadId::BARE,
                });
                (dep, file)
            })
            .collect();
        Self { cache, deps }
    }

    fn apply(&self, op: Op) {
        let dep = |i: usize| &self.deps[i].0;
        match op {
            Op::Touch(i) => self.cache.touch(dep(i), &self.deps[i].1).unwrap(),
            Op::Refresh(i) => self.cache.refresh(dep(i)),
            Op::EvictOthers(keep) => self.cache.evict_others(keep.map(dep)),
            Op::Forget(i) => self.cache.forget(dep(i)),
            Op::Bytes => {
                self.cache.bytes();
            }
        }
    }

    /// After every thread is done: the list and the map are one, the sum is
    /// the meters' as they stand, and one more use brings the sum within
    /// the bound (or leaves its own entry alone).
    fn settle(&self) {
        let cache = &self.cache;
        cache.check();
        assert_eq!(cache.bytes(), cache.total(), "the sum is the meters'");
        let last = self.deps.len() - 1;
        self.apply(Op::Touch(last));
        assert!(
            cache.total() <= cache.bound || cache.len() == 1,
            "one use evicts to the bound: {} > {} over {}",
            cache.total(),
            cache.bound,
            cache.len()
        );
        assert_eq!(cache.bytes(), cache.total());
    }
}

/// One operation of the LRU's, on a rig's dependency.
#[derive(Clone, Copy, Debug)]
enum Op {
    Touch(usize),
    Refresh(usize),
    EvictOthers(Option<usize>),
    Forget(usize),
    Bytes,
}

/// What a stopped thread tells the test.
#[derive(Debug, PartialEq, Eq)]
enum Event {
    /// It reached the point it was stopped at.
    Stopped,
    /// It finished without reaching it (or after it was let go).
    Done,
}

/// Stops the armed thread the `nth` time it reaches `at`, until the test
/// lets it go; every other thread, and every other point, passes.
#[cfg(test)]
struct Gate {
    at: Hook,
    nth: usize,
    armed: Mutex<Option<(ThreadId, usize)>>,
    events: Mutex<mpsc::Sender<Event>>,
    go: Mutex<mpsc::Receiver<()>>,
}

/// The test's side of a [`Gate`].
#[cfg(test)]
struct Held {
    gate: Arc<Gate>,
    events: mpsc::Receiver<Event>,
    go: mpsc::Sender<()>,
}

#[cfg(test)]
impl Held {
    fn install(cache: &DepCache, at: Hook, nth: usize) -> Self {
        let (events_tx, events) = mpsc::channel();
        let (go, go_rx) = mpsc::channel();
        let gate = Arc::new(Gate {
            at,
            nth,
            armed: Mutex::new(None),
            events: Mutex::new(events_tx),
            go: Mutex::new(go_rx),
        });
        let g = gate.clone();
        *lock(&cache.hook) = Some(HookFn(Arc::new(move |h| g.reach(h))));
        Self { gate, events, go }
    }

    /// Runs `f` on a thread of its own, armed.
    fn run(&self, f: impl FnOnce() + Send + 'static) -> std::thread::JoinHandle<()> {
        let gate = self.gate.clone();
        std::thread::spawn(move || {
            *lock(&gate.armed) = Some((std::thread::current().id(), 0));
            f();
            let _ = lock(&gate.events).send(Event::Done);
        })
    }

    /// What the armed thread did first: stopped, or finished.
    fn next(&self) -> Event {
        guard::recv(&self.events, "the armed thread to stop or finish")
    }

    fn release(&self) {
        self.go.send(()).unwrap();
    }
}

#[cfg(test)]
impl Gate {
    fn reach(&self, h: Hook) {
        if h != self.at {
            return;
        }
        let me = std::thread::current().id();
        let stop = {
            let mut armed = lock(&self.armed);
            match armed.as_mut() {
                Some((t, seen)) if *t == me => {
                    *seen += 1;
                    let stop = *seen == self.nth;
                    if stop {
                        *armed = None;
                    }
                    stop
                }
                _ => false,
            }
        };
        if stop {
            let _ = lock(&self.events).send(Event::Stopped);
            guard::hold(&lock(&self.go), "the test to let the stopped thread go");
        }
    }
}

/// Every operation stopped at every point between its steps (each
/// [`Hook`] the LRU has, its first to third time), on an LRU holding two
/// dependencies of a bound of two and a half, while each operation runs
/// whole: the list and the map stay one at every step (each step checks
/// them), the sum stays the meters', and one more use evicts to the bound.
/// The use stopped where round 3's touch had asked whether its key was
/// cached, while another thread evicts that key, is the case that listed a
/// key with no entry.
#[cfg(test)]
#[test]
fn every_operation_stopped_between_its_steps_while_another_runs_keeps_the_lru_one() {
    let stopped_ops = [
        Op::Touch(2),
        Op::Touch(0),
        Op::Refresh(1),
        Op::EvictOthers(None),
        Op::EvictOthers(Some(0)),
        Op::Forget(1),
    ];
    let points = [
        (Hook::Touching, 1),
        (Hook::Used, 1),
        (Hook::Evicting, 1),
        (Hook::Evicting, 2),
        (Hook::Evicting, 3),
    ];
    let others = [
        Op::Touch(3),
        Op::Touch(0),
        Op::Touch(1),
        Op::Touch(2),
        Op::Refresh(0),
        Op::Refresh(1),
        Op::EvictOthers(None),
        Op::EvictOthers(Some(1)),
        Op::EvictOthers(Some(2)),
        Op::Forget(0),
        Op::Forget(1),
        Op::Forget(2),
        Op::Bytes,
    ];
    let mut stopped = 0;
    for p in stopped_ops {
        for (at, nth) in points {
            for q in others {
                let rig = Arc::new(Rig::new(5));
                rig.apply(Op::Touch(0));
                rig.apply(Op::Touch(1));
                assert_eq!(rig.cache.len(), 2, "two fit the bound");
                let held = Held::install(&rig.cache, at, nth);
                let thread = {
                    let rig = rig.clone();
                    held.run(move || rig.apply(p))
                };
                if held.next() == Event::Stopped {
                    stopped += 1;
                    rig.apply(q);
                    rig.cache.check();
                    held.release();
                    assert_eq!(held.next(), Event::Done, "{p:?} at {at:?} {nth}, {q:?}");
                }
                thread.join().unwrap();
                *lock(&rig.cache.hook) = None;
                rig.settle();
            }
        }
    }
    // Each point an operation reaches, with each other operation: the
    // touches stop at 4 and 3 points, the refresh at 2, the evictions at 3
    // and 2, the forget at none.
    assert_eq!(stopped, (4 + 3 + 2 + 3 + 2) * others.len());
}

/// Round 3's case through the Environment (`EnvOptions::batch()`'s LRU, a
/// bound of two and a half dependencies): a `get` of A's CAB stops at its
/// use of A while another load evicts A. A is listed again with its entry,
/// a big dependency then evicts every other, and a `get` of B's CAB, which
/// opens B again, returns, with the LRU within its bound (round 3 listed A
/// with no entry, the list lost B and C, and that `get` spun forever).
#[cfg(test)]
#[test]
fn a_use_stopped_while_its_key_is_evicted_lists_no_key_without_an_entry() {
    let env_of = |bound: u64| {
        let mut o = EnvOptions::batch(Arc::new(FileContext::unitypy()));
        o.dep_cache_bytes = bound;
        Environment::new(o)
    };
    // Between what A and B and what A, B and C hold.
    let bound = {
        let env = env_of(u64::MAX);
        let mut held = Vec::new();
        for cab in ["CAB-A", "CAB-B", "CAB-C"] {
            env.load_dependency(source(cab, 64)).unwrap();
            held.push(env.stats().dependency_bytes);
        }
        u64::midpoint(held[1], held[2])
    };
    let env = env_of(bound);
    env.load_dependency(source("CAB-A", 64)).unwrap();
    env.load_dependency(source("CAB-B", 64)).unwrap();
    let held = Held::install(&env.inner.cache, Hook::Touching, 1);
    let thread = {
        let env = env.clone();
        held.run(move || {
            let m = Meter::unlimited();
            assert!(env.get_cab(&RawStr::from("cab-a"), &m).unwrap().is_some());
        })
    };
    assert_eq!(held.next(), Event::Stopped);
    env.load_dependency(source("CAB-C", 64)).unwrap();
    assert_eq!(env.stats().dependencies_evicted, 1, "C's load evicted A");
    held.release();
    assert_eq!(held.next(), Event::Done);
    thread.join().unwrap();
    env.inner.cache.check();
    env.load_dependency(source("CAB-D", 64 * 1024)).unwrap();
    let (tx, rx) = mpsc::channel();
    let get = {
        let env = env.clone();
        std::thread::spawn(move || {
            let m = Meter::unlimited();
            let found = env.get_cab(&RawStr::from("cab-b"), &m).unwrap().is_some();
            let _ = tx.send(found);
        })
    };
    let found = guard::recv(&rx, "a get of B to return (round 3's eviction spun here)");
    assert!(found);
    get.join().unwrap();
    env.inner.cache.check();
    let cache = &env.inner.cache;
    assert!(cache.total() <= bound || cache.len() == 1);
    assert_eq!(env.stats().dependency_bytes, cache.total());
}

/// An eviction ends on any state, a corrupt one too: a head no entry holds
/// (round 3's state) and a list that loops back on itself both stop an
/// eviction at once or within as many steps as the map holds, instead of
/// spinning.
#[cfg(test)]
#[test]
fn an_eviction_ends_on_a_corrupt_state() {
    let rig = Rig::new(3);
    let node = |i: usize, prev: Option<usize>, next: Option<usize>| {
        let (dep, file) = &rig.deps[i];
        LruNode {
            _dep: dep.clone(),
            file: file.clone(),
            seen: rig.cache.bound,
            prev,
            next,
            _charge: file.meter().charge(0).unwrap(),
        }
    };
    let k = |i: usize| lru_key(&rig.deps[i].0);
    let ghost = usize::MAX;
    // Each entry's `seen` is the bound, so the sum stays past it while two
    // are left.
    let corrupt = [
        // The head is a key without an entry (round 3's state).
        (ghost, vec![(0, None, Some(1)), (1, Some(0), None)]),
        // A loop: 0 and 1 point at each other both ways; 2 is not listed.
        (
            0,
            vec![
                (0, Some(1), Some(1)),
                (1, Some(0), Some(0)),
                (2, None, None),
            ],
        ),
    ];
    for (head, nodes) in corrupt {
        let mut lru = Lru {
            unchecked: true,
            ..Lru::default()
        };
        for (i, prev, next) in nodes {
            let n = node(i, prev.map(k), next.map(k));
            lru.total += n.seen;
            lru.nodes.insert(k(i), n);
        }
        lru.head = Some(if head == ghost { ghost } else { k(head) });
        lru.tail = Some(ghost);
        let cache = rig.cache.clone();
        *lock(&cache.lru) = lru;
        let (tx, rx) = mpsc::channel();
        let evict = std::thread::spawn(move || {
            // No entry is kept: 0 is no dependency's address.
            cache.evict_over(0);
            cache.evict_others(None);
            let _ = tx.send(());
        });
        guard::wait(&rx, "an eviction to end (round 3's eviction spun here)");
        evict.join().unwrap();
        *lock(&rig.cache.lru) = Lru::default();
    }
}

/// The stress run: threads applying random operations to a small LRU, the
/// list and the map checked at every step and by each thread after each
/// operation, then settled. Passes on any schedule.
#[cfg(test)]
#[test]
fn random_operations_on_many_threads_keep_the_lru_one() {
    const THREADS: u64 = 4;
    const OPS: usize = 4_000;
    let rig = Arc::new(Rig::new(6));
    let (tx, rx) = mpsc::channel();
    let threads: Vec<_> = (0..THREADS)
        .map(|t| {
            let (rig, tx) = (rig.clone(), tx.clone());
            std::thread::spawn(move || {
                let mut rng = Rng::new(0x5eed_0000 + t);
                for _ in 0..OPS {
                    let i = rng.below(5);
                    let op = match rng.below(10) {
                        0..=4 => Op::Touch(i),
                        5 => Op::Refresh(i),
                        6 => Op::EvictOthers((rng.below(2) == 0).then_some(i)),
                        7 | 8 => Op::Forget(i),
                        _ => Op::Bytes,
                    };
                    rig.apply(op);
                    rig.cache.check();
                }
                let _ = tx.send(());
            })
        })
        .collect();
    // A thread that panics drops its sender: once every thread has
    // finished or panicked the channel ends, and the joins report it.
    drop(tx);
    for _ in 0..THREADS {
        guard::hold(
            &rx,
            "every thread to finish (an eviction that spins never does)",
        );
    }
    for t in threads {
        t.join().unwrap();
    }
    rig.settle();
}

/// The LRU's work per use does not grow with the dependencies it holds
/// (review round 1: `touch` scanned and summed every entry, so `n` lookups
/// over `n` dependencies cost `n`²). Counted in entries visited, never by a
/// clock.
#[test]
fn the_lru_visits_a_constant_number_of_entries_per_use() {
    let visits = |n: usize| {
        let env = Environment::new(EnvOptions::batch(Arc::new(FileContext::unitypy())));
        for i in 0..n {
            env.load_dependency(source(&format!("CAB-{i}"), 0)).unwrap();
        }
        let cache: &DepCache = &env.inner.cache;
        let before = lock(&cache.lru).visits;
        for i in 0..n {
            let name = RawStr::from(format!("cab-{i}").as_str());
            assert!(env.get_cab(&name, &Meter::unlimited()).unwrap().is_some());
        }
        lock(&cache.lru).visits - before
    };
    let (small, large) = (visits(200), visits(800));
    // A lookup touches its dependency and refreshes it: two visits.
    assert!(
        large <= 2 * 800 && small <= 2 * 200,
        "visits: {small} for 200 lookups over 200 dependencies, {large} for 800 over 800"
    );
}

/// A `Lazy` dependency load charges its entry's share before its commit
/// and hands it to the LRU after ([`DepCache::touch_paid`]). Stopped
/// between the two, while another thread's `get` of one of its keys
/// caches it first ([`DepCache::touch`], through the dependency's handle),
/// the load finds its entry cached and returns its share after the lock:
/// the entry is read again without it, so the LRU's sum is the meter's,
/// and once the Environment is dropped the pool holds nothing (CR1's
/// review, round 2: no test saw the sum stay a share high when the share
/// was returned under the lock, after the entry was read with it, nor the
/// pool keep it when the share was forgotten).
#[cfg(test)]
#[test]
fn a_paid_touch_that_finds_its_entry_cached_reads_it_without_the_share() {
    let pool = Arc::new(crate::budget::Counting::new(u64::MAX));
    let mut ctx = FileContext::unitypy();
    ctx.budget = pool.clone();
    let mut options = EnvOptions::unitypy();
    options.context = Arc::new(ctx);
    options.load = crate::env::LoadMode::Lazy;
    let env = Environment::new(options);
    let held = Held::install(&env.inner.cache, Hook::LoadRegistered, 1);
    let load = {
        let env = env.clone();
        held.run(move || {
            env.load_dependency(source("CAB-A", 64)).unwrap();
        })
    };
    assert_eq!(held.next(), Event::Stopped);
    let m = Meter::unlimited();
    assert!(env.get_cab(&RawStr::from("cab-a"), &m).unwrap().is_some());
    assert_eq!(env.inner.cache.len(), 1, "the get cached the dependency");
    held.release();
    assert_eq!(held.next(), Event::Done);
    load.join().unwrap();
    let cache = &env.inner.cache;
    cache.check();
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.total(), env.stats().dependency_bytes);
    drop(env);
    assert_eq!(pool.used(), 0, "a charge was kept");
}
