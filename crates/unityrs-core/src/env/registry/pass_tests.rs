//! The container pass reaches a dropped slot of an evicted dependency,
//! opening the dependency again (plan section 9, Q13, revised after review
//! round 6), whatever the order in which another pass, a pass whose reopen
//! the pool refuses once (which then evicts the others and retries, review
//! round 7), an eviction, a lookup by name and the key listing take their
//! steps around it. Every
//! interleaving is forced with the Environment's hooks
//! ([`crate::env::gate`]), as in `reach_tests.rs`: each operation stopped at
//! each point between its steps while each other operation runs whole, or
//! stops at a point of its own and the two are let go in either order.

// The source scan behind "the core starts no thread and never waits"
// (`unityrs_testkit::waits`) takes only an item under `#[cfg(test)]` in its
// own file for test code, so every item here that names a thread carries
// one.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::{Dependency, Hook, Open, Slot, open_source};
use crate::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation};
use crate::container::ContainerPath;
#[cfg(test)]
use crate::env::gate::{Event, Gate, Run};
use crate::env::{Cab, EnvOptions, Environment, LoadMode, Source};
use crate::limits::Meter;
use crate::options::FileContext;
use crate::raw::RawStr;
use bytes::Bytes;
use unityrs_testkit::env::{Obj, bundle, nested_between_loads, nested_dropped_evicted, serialized};

/// A budget that grants every reservation but, once armed, the next
/// dependency reservation, which it refuses: a pool with no room left until
/// something is evicted.
#[derive(Debug, Default)]
struct RefuseNext(AtomicBool);

impl MemoryBudget for RefuseNext {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        if what == Reservation::Dependency && self.0.swap(false, Ordering::AcqRel) {
            return Err(BudgetExceeded {
                requested: bytes,
                what,
            });
        }
        Ok(Permit::detached(bytes))
    }
}

/// The scene `nested_dropped_evicted` loaded under `batch()` with a one-byte
/// pool: `f`, the dependency `a` (whose `INNER` holds `CAB-D`), the
/// dependency `b` (which evicts `a`), then `p`, whose raw `INNER` drops
/// `a`'s unreached; its load's `settle` finds `a` evicted and leaves the
/// slot to the pass.
struct Rig {
    env: Environment,
    /// `a`'s `INNER`.
    dropped: Arc<Slot>,
    /// The bytes an eviction's load reads, and how many it made.
    other: Bytes,
    loads: Arc<AtomicUsize>,
    /// The pool, which a refused pass arms.
    pool: Arc<RefuseNext>,
}

impl Rig {
    fn new() -> Self {
        let scene = nested_dropped_evicted();
        let pool = Arc::new(RefuseNext::default());
        let mut ctx = FileContext::unitypy();
        ctx.budget = pool.clone();
        let mut options = EnvOptions::batch(Arc::new(ctx));
        options.dep_cache_bytes = 1;
        assert_eq!(options.load, LoadMode::Lazy);
        let env = Environment::new(options);
        let src = |i: usize| {
            let (name, data) = &scene.scene.files[i];
            Source::Bytes {
                name: RawStr::new(name.clone()),
                data: Bytes::from(data.clone()),
            }
        };
        let mut dropped = None;
        for i in 0..scene.scene.files.len() {
            if scene.dependencies.contains(&i) {
                drop(env.load_dependency(src(i)).unwrap());
            } else {
                drop(env.load(src(i)).unwrap());
            }
            if i == 1 {
                dropped = env.registry().peek("inner");
            }
        }
        let dropped = dropped.expect("a's INNER");
        assert!(dropped.unreached(), "the drop was reached at the load");
        assert!(
            env.registry().evicted_waiting(),
            "the drop waits for the pass"
        );
        let other = Bytes::from(bundle(&[(
            "CAB-C",
            serialized(&[], &[Obj::named(1, "c1", &[])]),
        )]));
        Self {
            env,
            dropped,
            other,
            loads: Arc::new(AtomicUsize::new(0)),
            pool,
        }
    }

    /// R1's tables and the lists' counts; R3: the dropped slot, once marked
    /// reached, has its container's entry registered.
    fn check(&self) {
        let reg = self.env.registry();
        reg.check();
        if !self.dropped.unreached() {
            assert!(reg.has("cab-d"), "R3: a reached container lacks cab-d");
        }
    }

    /// The path `obj.container` gives `CAB-F`'s object 1.
    fn path_of_f1(&self) -> Option<String> {
        let meter = Meter::unlimited();
        let Some(Cab::Serialized(sf)) = self.env.get_cab(&RawStr::from("CAB-F"), &meter).unwrap()
        else {
            panic!("CAB-F");
        };
        let f1 = sf.objects().find(|o| o.path_id() == 1).expect("f1");
        match self.env.container_path(&f1).unwrap() {
            Some(ContainerPath::Str(s)) => Some(String::from_utf8_lossy(s.as_bytes()).into()),
            Some(ContainerPath::NotAString) => Some("<not a str>".into()),
            None => None,
        }
    }
}

/// An operation around the pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    /// The container pass.
    Pass,
    /// The container pass, the pool armed to refuse the next dependency
    /// reservation (its reopen's, when no other operation's comes first).
    RefusedPass,
    /// A dependency load, which evicts whatever the pool holds.
    Evict,
    /// A lookup by name of the dropped container's entry (no reopen, Q13).
    GetD,
    /// The key listing.
    Keys,
}

const OPS: [Op; 5] = [Op::Pass, Op::RefusedPass, Op::Evict, Op::GetD, Op::Keys];

/// The points an operation may be stopped at: a `settle` that took a slot,
/// a reach that registered, and the LRU's steps a reopen and an eviction
/// take.
const POINTS: [Hook; 5] = [
    Hook::SettleTook,
    Hook::Reached,
    Hook::Touching,
    Hook::Used,
    Hook::Evicting,
];

fn apply(rig: &Rig, op: Op) {
    let env = &rig.env;
    match op {
        Op::Pass => drop(env.container().unwrap().count()),
        Op::RefusedPass => {
            rig.pool.0.store(true, Ordering::Release);
            env.container().unwrap().count();
        }
        Op::Evict => {
            let n = rig.loads.fetch_add(1, Ordering::Relaxed);
            let name = format!("c{n}.bundle");
            drop(
                env.load_dependency(Source::Bytes {
                    name: RawStr::from(name.as_str()),
                    data: rig.other.clone(),
                })
                .unwrap(),
            );
        }
        Op::GetD => drop(
            env.get_cab(&RawStr::from("CAB-D"), &Meter::unlimited())
                .unwrap(),
        ),
        Op::Keys => drop(env.registry().keys().count()),
    }
}

/// What `op` must leave when it returns: a pass, every slot listed on
/// either list when it started reached (R4 for the pass), `CAB-D` among
/// them.
fn after(rig: &Rig, op: Op, listed: &[Arc<Slot>]) {
    rig.check();
    if matches!(op, Op::Pass | Op::RefusedPass) {
        for slot in listed {
            assert!(!slot.unreached(), "R4: the pass left a listed slot");
        }
        assert!(rig.env.registry().has("cab-d"), "the pass missed cab-d");
    }
}

/// Once both returned and a pass ran, nothing is listed and `CAB-F`'s
/// object 1 has the path UnityPy gives it (`tests/fixtures/d/env.json`,
/// scene `nested_dropped_evicted`).
fn quiet(rig: &Rig) {
    rig.check();
    rig.env.container().unwrap().count();
    rig.check();
    assert!(
        rig.env.registry().listed().is_empty(),
        "a slot stayed listed"
    );
    assert!(!rig.dropped.unreached());
    assert_eq!(rig.path_of_f1().as_deref(), Some("assets/d.prefab"));
}

#[cfg(test)]
fn start(gate: &Gate, rig: &Arc<Rig>, op: Op, at: Option<Hook>) -> (Run<()>, Vec<Arc<Slot>>) {
    let listed = rig.env.registry().listed();
    let r = rig.clone();
    (gate.run(at.map(|h| (h, 1)), move || apply(&r, op)), listed)
}

#[cfg(test)]
fn finish(rig: &Rig, op: Op, (run, listed): (Run<()>, Vec<Arc<Slot>>)) {
    run.finish();
    after(rig, op, &listed);
}

/// Every operation stopped at every point (its first time there), while
/// each operation runs whole, or stops at each point of its own and the two
/// are let go in either order: at each step the invariants hold, at each
/// return its own, and once both returned a pass gives UnityPy's path.
#[test]
fn the_pass_reaches_an_evicted_drop_under_every_interleaving() {
    let mut cases = 0;
    for a in OPS {
        for p in POINTS {
            for b in OPS {
                for q in std::iter::once(None).chain(POINTS.map(Some)) {
                    for a_first in [true, false] {
                        if q.is_none() && !a_first {
                            continue;
                        }
                        cases += 1;
                        let rig = Arc::new(Rig::new());
                        let gate = Gate::install(&rig.env);
                        let ra = start(&gate, &rig, a, Some(p));
                        if ra.0.next() == Event::Stopped {
                            rig.check();
                        }
                        let rb = start(&gate, &rig, b, q);
                        if q.is_none() {
                            finish(&rig, b, rb);
                            finish(&rig, a, ra);
                        } else {
                            if rb.0.next() == Event::Stopped {
                                rig.check();
                            }
                            if a_first {
                                finish(&rig, a, ra);
                                finish(&rig, b, rb);
                            } else {
                                finish(&rig, b, rb);
                                finish(&rig, a, ra);
                            }
                        }
                        quiet(&rig);
                    }
                }
            }
        }
    }
    assert_eq!(cases, 5 * 5 * 5 * 11);
}

/// A pass whose reopen of the evicted dependency the pool refuses evicts
/// the other dependencies and opens it again, so it reaches the drop and
/// gives UnityPy's path, where it failed (plan section 3.13; review round
/// 7: the reopen had no retry, so every pass failed while the pool had no
/// room).
#[test]
fn a_pass_whose_reopen_the_pool_refuses_makes_room_and_reaches_the_drop() {
    let rig = Rig::new();
    let evicted = rig.env.stats().dependencies_evicted;
    apply(&rig, Op::RefusedPass);
    assert!(!rig.pool.0.load(Ordering::Acquire), "nothing was refused");
    assert!(!rig.dropped.unreached(), "the pass left the drop");
    assert!(
        rig.env.stats().dependencies_evicted > evicted,
        "the retry evicted nothing"
    );
    quiet(&rig);
}

/// Without a pass, nothing reopens the evicted dependency for its drop: a
/// lookup by name and the key listing leave the slot listed for the pass,
/// and the pass then reaches it (plan section 9, Q13: lookups by name keep
/// the rule; only the pass reopens).
#[test]
fn only_the_pass_reopens_an_evicted_drop() {
    let rig = Rig::new();
    apply(&rig, Op::GetD);
    apply(&rig, Op::Keys);
    assert!(rig.dropped.unreached(), "a lookup reopened the dependency");
    assert!(rig.env.registry().evicted_waiting());
    quiet(&rig);
    assert!(!rig.env.registry().evicted_waiting());
}

/// UnityPy's keys at the end of the scene `nested_between_loads`
/// (`tests/fixtures/d/env.json`, `tools/fixtures/env.py`).
fn between_keys() -> Vec<String> {
    // Read at run time: no file is compiled into the core
    // (`crates/unityrs-tpk/tests/no_tpk.rs`).
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/d/env.json");
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).expect("env.json")).expect("env.json");
    let scene = doc["scenes"]
        .as_array()
        .expect("scenes")
        .iter()
        .find(|s| s["name"] == "nested_between_loads")
        .expect("the scene is recorded");
    let last = scene["steps"]
        .as_array()
        .expect("steps")
        .last()
        .expect("a step");
    last["cabs"]
        .as_array()
        .expect("keys")
        .iter()
        .map(|k| k.as_str().expect("a key").to_owned())
        .collect()
}

/// The scene `nested_between_loads` with `c.bundle` a dependency whose
/// `n`, ranked between the key's position (`a.bundle`'s) and its value
/// (`b.bundle`'s), changes no key and is registered after a container pass
/// was built, `c.bundle`'s first key `e0` before it (review round 16). The
/// load lists the dropped slot and starts a new container index generation
/// in that step; its `settle` finds the dependency evicted, so the slot
/// waits for the next container pass, the only reader that opens an
/// evicted dependency again (plan section 9, Q13). Here the dependency is
/// evicted before its `n` is registered, which stands for the `settle` of
/// a load whose reach of the slot the pool refused, twice, and an eviction
/// after it. The next container pass is run, opens the dependency and
/// reaches the slot: `CAB-Z` is registered, its table walked, and the keys
/// are UnityPy's. With no new generation the pass built before was kept,
/// and `CAB-Z` was never registered.
#[test]
fn a_drop_ranked_between_a_keys_position_and_value_is_reached_by_the_next_pass() {
    let scene = nested_between_loads();
    let ctx = Arc::new(FileContext::unitypy());
    let env = Environment::new(EnvOptions::batch(ctx.clone()));
    let src = |i: usize| Source::Bytes {
        name: RawStr::new(scene.files[i].0.clone()),
        data: Bytes::from(scene.files[i].1.clone()),
    };
    let reg = env.registry();
    drop(env.load(src(0)).unwrap());
    let load = reg.next_load();
    let name = RawStr::new(scene.files[1].0.clone());
    let source = src(1);
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Dependency);
    let file = open_source(&name, &source, &meter, &ctx).unwrap();
    let dep = Arc::new(Dependency {
        name,
        source,
        open: std::sync::Mutex::new(Open::of(&file, meter.clone())),
        ctx: ctx.clone(),
        cache: Arc::downgrade(&env.inner.cache),
        slots: AtomicUsize::new(0),
    });
    reg.register_lazy_dependency(&RawStr::from("e0"), &dep, load, &[0], &meter)
        .unwrap();
    drop(env.load(src(2)).unwrap());
    env.container().unwrap().count();
    // The pass opened the dependency through the LRU; nothing holds it now.
    drop(file);
    env.inner.cache.evict_others(None);
    assert!(dep.alive().is_none(), "the dependency is still open");
    reg.register_lazy_dependency(&RawStr::from("n"), &dep, load, &[1], &meter)
        .unwrap();
    assert!(reg.evicted_waiting(), "the drop does not wait for the pass");
    assert!(
        !reg.has("cab-z"),
        "the drop was reached at its registration"
    );
    env.container().unwrap().count();
    let keys: Vec<String> = reg
        .keys()
        .map(|k| String::from_utf8_lossy(k.as_bytes()).into_owned())
        .collect();
    assert_eq!(keys, between_keys(), "the keys");
    let z = reg.peek("cab-z").expect("CAB-Z is registered");
    assert!(z.walked(), "CAB-Z's table was not walked");
    reg.check();
    assert!(!reg.evicted_waiting());
}
