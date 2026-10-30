//! The registry's reach and settle keep their invariants (plan section 3.16,
//! R1, R3 and R4) whatever the order in which threads take their steps
//! (review round 5: a `settle` that popped a slot let a concurrent
//! registration return before the slot was reached). Every interleaving is
//! forced with the Environment's hooks ([`crate::env::gate`]), never by
//! repetition: each operation stopped at each point between its steps,
//! while each other operation runs whole, or stops at a point of its own
//! and the two are let go in either order. A stress run of random
//! operations on several threads follows as a second check, the invariants
//! checked when each step returns.

// The source scan behind "the core starts no thread and never waits"
// (`unityrs_testkit::waits`) takes only an item under `#[cfg(test)]` in its
// own file for test code, so every item here that names a thread, a channel
// or a barrier carries one.
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

use super::pending::List;
use super::{Hook, Registry, Slot};
#[cfg(test)]
use crate::env::gate::{Event, Gate, Run};
use crate::env::{Cab, EnvOptions, Environment, LoadMode, RawCab, Source};
use crate::limits::Meter;
use crate::raw::RawStr;
use bytes::Bytes;
use unityrs_testkit::env::{Obj, bundle, res_bytes, serialized};
use unityrs_testkit::rng::Rng;

impl Registry {
    /// The slot under `key` (already simplified), without settling.
    pub(super) fn peek(&self, key: &str) -> Option<Arc<Slot>> {
        self.read()
            .slots
            .get(key.as_bytes())
            .map(|e| e.slot.clone())
    }

    /// Whether `key` (already simplified) is registered, without settling.
    pub(super) fn has(&self, key: &str) -> bool {
        self.read().slots.contains_key(key.as_bytes())
    }

    /// The slots listed now, on either list.
    pub(crate) fn listed(&self) -> Vec<Arc<Slot>> {
        let inner = self.read();
        inner.pending.all().into_iter().map(|(_, s)| s).collect()
    }

    /// R1's tables agree (every key of the order is in the table at its
    /// position, and no other; a key's value ranks at or above its
    /// position), and the list is in its order and counted right.
    pub(crate) fn check(&self) {
        let inner = self.read();
        assert_eq!(inner.order.len(), inner.slots.len(), "order and table");
        for (rank, key) in &inner.order {
            let e = inner.slots.get(key.as_bytes()).expect("a key of the order");
            assert_eq!(&e.first, rank, "a key's position");
            assert!(e.slot.rank >= e.first, "a key's value ranks below it");
        }
        let listed = inner.pending.all();
        assert!(listed.windows(2).all(|w| w[0].0 < w[1].0), "list order");
        let len = |l| inner.pending.len(l);
        let ordering = std::sync::atomic::Ordering::Acquire;
        assert_eq!(
            self.pending_len.load(ordering),
            len(List::Listed),
            "the list's count"
        );
        assert_eq!(
            self.evicted_len.load(ordering),
            len(List::Evicted),
            "the evicted list's count"
        );
    }
}

fn cab(name: &str) -> Vec<u8> {
    serialized(&[], &[Obj::named(1, name, &[])])
}

/// The bundles: `o` holds `INNER` (a container of `CAB-I` and its
/// `.resS`), which `p`'s `INNER` (a CAB) drops; `q` holds `X` (a container
/// of `CAB-X`), which `r`'s `X` drops; `s` holds `Y`, a container nothing
/// drops.
#[derive(Clone)]
struct Scene {
    o: Bytes,
    p: Bytes,
    q: Bytes,
    r: Bytes,
    s: Bytes,
}

impl Scene {
    fn new() -> Self {
        let inner = bundle(&[("CAB-I", cab("i")), ("CAB-I.resS", res_bytes(8, 1))]);
        let x = bundle(&[("CAB-X", cab("x"))]);
        let y = bundle(&[("CAB-Y", cab("y"))]);
        Self {
            o: Bytes::from(bundle(&[("CAB-O", cab("o")), ("INNER", inner)])),
            p: Bytes::from(bundle(&[("INNER", cab("p-inner"))])),
            q: Bytes::from(bundle(&[("CAB-Q", cab("q")), ("X", x)])),
            r: Bytes::from(bundle(&[("X", cab("r-x"))])),
            s: Bytes::from(bundle(&[("Y", y)])),
        }
    }
}

fn src(name: &str, data: &Bytes) -> Source {
    Source::Bytes {
        name: RawStr::from(name),
        data: data.clone(),
    }
}

/// The containers, and the keys their reach registers.
const SUBTREES: [(&str, &[&str]); 3] = [
    ("inner", &["cab-i", "cab-i.ress"]),
    ("x", &["cab-x"]),
    ("y", &["cab-y"]),
];

/// A `Lazy` Environment with `o`, `q` and `s` loaded, and the slots of
/// their containers.
struct Rig {
    scene: Scene,
    env: Environment,
    containers: Vec<(Arc<Slot>, &'static [&'static str])>,
}

impl Rig {
    fn new(scene: &Scene) -> Self {
        let mut o = EnvOptions::unitypy();
        o.load = LoadMode::Lazy;
        let env = Environment::new(o);
        for (name, data) in [
            ("o.bundle", &scene.o),
            ("q.bundle", &scene.q),
            ("s.bundle", &scene.s),
        ] {
            env.load(src(name, data)).unwrap();
        }
        let reg = env.registry();
        let containers = SUBTREES
            .iter()
            .map(|(k, sub)| (reg.peek(k).expect("a container's key"), *sub))
            .collect();
        Self {
            scene: scene.clone(),
            env,
            containers,
        }
    }

    /// The invariants that hold at any moment: R1's tables (and the list's
    /// order), and R3: a container's slot marked reached has every key of
    /// its subtree registered.
    fn check(&self) {
        let reg = self.env.registry();
        reg.check();
        for (slot, sub) in &self.containers {
            if !slot.unreached() {
                for k in *sub {
                    assert!(reg.has(k), "R3: a reached container lacks {k}");
                }
            }
        }
    }
}

/// An operation that touches the registry's slots or its list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    /// A load whose entry drops `o`'s `INNER` unreached.
    LoadP,
    /// A load whose entry drops `q`'s `X` unreached.
    LoadR,
    /// A reload of `o` on the same `Bytes`, whose `INNER` stands in for
    /// the key's value when that is unreached (plan section 9, Q14).
    LoadO2,
    /// A lookup by name of an entry inside `INNER`.
    GetI,
    /// A lookup of `Y`, a live container, which reaches it.
    GetY,
    /// The key listing.
    Keys,
    /// The container pass.
    Pass,
}

const OPS: [Op; 7] = [
    Op::LoadP,
    Op::LoadR,
    Op::LoadO2,
    Op::GetI,
    Op::GetY,
    Op::Keys,
    Op::Pass,
];

/// Whether `op` is a load.
fn is_load(op: Op) -> bool {
    matches!(op, Op::LoadP | Op::LoadR | Op::LoadO2)
}

/// The file a load op loads.
fn loaded(scene: &Scene, op: Op) -> (&'static str, &Bytes) {
    match op {
        Op::LoadP => ("p.bundle", &scene.p),
        Op::LoadR => ("r.bundle", &scene.r),
        _ => ("o2.bundle", &scene.o),
    }
}

/// The points an operation may be stopped at.
const POINTS: [Hook; 4] = [
    Hook::Published,
    Hook::Deferred,
    Hook::SettleTook,
    Hook::Reached,
];

/// Runs `op` on `env`; whether a lookup by name found its CAB.
fn apply(env: &Environment, scene: &Scene, op: Op) -> Option<bool> {
    let meter = Meter::unlimited();
    match op {
        Op::LoadP | Op::LoadR | Op::LoadO2 => {
            let (name, data) = loaded(scene, op);
            drop(env.load(src(name, data)).unwrap());
        }
        Op::GetI => {
            let found = env.get_cab(&RawStr::from("CAB-I"), &meter).unwrap();
            return Some(found.is_some());
        }
        Op::GetY => drop(env.get_cab(&RawStr::from("Y"), &meter).unwrap()),
        Op::Keys => drop(env.registry().keys().count()),
        Op::Pass => drop(env.container().unwrap().count()),
    }
    None
}

/// What `op` must leave when it returns, given the slots listed when it
/// started (R4: each is reached) and what it found.
fn after(rig: &Rig, op: Op, listed: &[Arc<Slot>], found: Option<bool>) {
    rig.check();
    for slot in listed {
        // A slot the key's value stands in for is never listed, so every
        // listed one is reached.
        assert!(
            !slot.unreached(),
            "R4: {op:?} returned with a slot listed before it unreached"
        );
    }
    let [inner, x, y] = [0, 1, 2].map(|i| &rig.containers[i].0);
    let registered = |sub: &[&str], what: &str| {
        let reg = rig.env.registry();
        for k in sub {
            assert!(reg.has(k), "{what}: {k} is not registered");
        }
    };
    match op {
        // A registration that drops a slot returns with its subtree
        // registered: the slot reached, or (`INNER`, after the reload) the
        // reload's slot, which stands in for the first load's, reached.
        Op::LoadP => registered(SUBTREES[0].1, "R4: LoadP's own drop"),
        Op::LoadR => assert!(!x.unreached(), "R4: LoadR's own drop"),
        // A lookup that resolves a container's slot reaches it.
        Op::GetY => assert!(!y.unreached(), "a lookup of Y"),
        Op::Pass => {
            for (_, sub) in &rig.containers {
                registered(sub, "the pass reaches every container");
            }
            assert!(!x.unreached() && !y.unreached(), "the pass");
        }
        Op::LoadO2 | Op::GetI | Op::Keys => {}
    }
    if op == Op::GetI && listed.iter().any(|s| Arc::ptr_eq(s, inner)) {
        assert_eq!(found, Some(true), "a lookup after INNER's drop");
    }
}

/// What each name finds, as a comparable answer; `inner` left out when
/// `own_key` (its own key is (b)'s difference: a reload's container after
/// the last load of `p`'s CAB of its name holds it under `Lazy`, where
/// `Eager` keeps the CAB).
fn answers(env: &Environment, own_key: bool) -> Vec<String> {
    let meter = Meter::unlimited();
    let names = [
        "cab-o",
        "cab-i",
        "cab-i.ress",
        "inner",
        "cab-q",
        "cab-x",
        "x",
        "cab-y",
    ];
    names
        .iter()
        .filter(|n| !(own_key && **n == "inner"))
        .map(|n| match env.get_cab(&RawStr::from(*n), &meter).unwrap() {
            None => format!("{n}: none"),
            Some(Cab::Serialized(sf)) => format!("{n}: {}", sf.name()),
            Some(Cab::Raw(RawCab::InBundle { bundle, dir_index })) => {
                format!("{n}: raw {}", bundle.directory()[dir_index].path)
            }
            Some(Cab::Raw(RawCab::File(f))) => format!("{n}: raw {}", f.name),
        })
        .collect()
}

/// The keys in order, the containers' names out: `Lazy` keys a
/// container's own name at its own position, where `Eager` keys it only as
/// a later CAB's (plan 3.13, (b)).
fn keys(env: &Environment) -> Vec<String> {
    env.registry()
        .keys()
        .map(|k| String::from_utf8_lossy(k.as_bytes()).into_owned())
        .filter(|k| !SUBTREES.iter().any(|(c, _)| c == k))
        .collect()
}

/// Once every operation returned, nothing is listed, and after a container
/// pass the registry answers as an eager load of the same loads in the same
/// order does (R1: whatever order the registrations arrived in).
fn quiet(rig: &Rig, loads: &[Op]) {
    rig.check();
    rig.env.container().unwrap().count();
    rig.check();
    assert!(
        rig.env.registry().listed().is_empty(),
        "a slot stayed listed"
    );
    let eager = Environment::new(EnvOptions::unitypy());
    let scene = &rig.scene;
    let mut order = vec![
        ("o.bundle", &scene.o),
        ("q.bundle", &scene.q),
        ("s.bundle", &scene.s),
    ];
    for op in loads {
        order.push(loaded(scene, *op));
    }
    for (name, data) in order {
        eager.load(src(name, data)).unwrap();
    }
    let at = |op| loads.iter().rposition(|o| *o == op);
    let own_key = matches!((at(Op::LoadP), at(Op::LoadO2)), (Some(p), Some(o2)) if p < o2);
    assert_eq!(
        answers(&rig.env, own_key),
        answers(&eager, own_key),
        "R1: values"
    );
    assert_eq!(keys(&rig.env), keys(&eager), "R1: positions");
}

/// Starts `op` on its own thread, stopped at `at` (its first time there),
/// with the slots listed as it starts.
#[cfg(test)]
fn start(gate: &Gate, rig: &Rig, op: Op, at: Option<Hook>) -> (Run<Option<bool>>, Vec<Arc<Slot>>) {
    let listed = rig.env.registry().listed();
    let env = rig.env.clone();
    let scene = rig.scene.clone();
    let run = gate.run(at.map(|h| (h, 1)), move || apply(&env, &scene, op));
    (run, listed)
}

/// Lets `run` (operation `op`) go, and checks what it leaves.
#[cfg(test)]
fn finish(rig: &Rig, op: Op, (run, listed): (Run<Option<bool>>, Vec<Arc<Slot>>)) {
    let found = run.finish();
    after(rig, op, &listed, found);
}

/// Every operation stopped at every point (its first time there), while
/// each operation runs whole, or stops at each point of its own and the two
/// are let go in either order: at each step the invariants hold, at each
/// return its own, and once both returned the registry answers as an eager
/// load does.
#[test]
fn reach_and_settle_keep_their_invariants_under_every_interleaving() {
    let scene = Scene::new();
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
                        let rig = Rig::new(&scene);
                        let gate = Gate::install(&rig.env);
                        let ra = start(&gate, &rig, a, Some(p));
                        if ra.0.next() == Event::Stopped {
                            rig.check();
                        }
                        let rb = start(&gate, &rig, b, q);
                        if q.is_none() {
                            // `b` runs whole while `a` is stopped.
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
                        // The loads in the order they took their sequence
                        // numbers: `a` took its own before it stopped.
                        let loads: Vec<Op> = [a, b].into_iter().filter(|o| is_load(*o)).collect();
                        quiet(&rig, &loads);
                    }
                }
            }
        }
    }
    assert_eq!(cases, 7 * 4 * 7 * 9);
}

/// Review round 5's interleaving, by name: a load stopped after it listed
/// the slot it dropped, a lookup stopped after its `settle` picked that
/// slot; the load is let go first and returns with the slot reached, so a
/// lookup by name right after it finds `CAB-I` (a `settle` that popped the
/// slot left the load's own `settle` an empty list).
#[test]
fn a_load_returns_with_its_drop_reached_while_another_settle_holds_it() {
    let scene = Scene::new();
    let rig = Rig::new(&scene);
    let gate = Gate::install(&rig.env);
    let load = start(&gate, &rig, Op::LoadP, Some(Hook::Deferred));
    load.0.stopped();
    let get = start(&gate, &rig, Op::GetI, Some(Hook::SettleTook));
    get.0.stopped();
    finish(&rig, Op::LoadP, load);
    let inner = &rig.containers[0].0;
    assert!(!inner.unreached(), "the load returned with INNER unreached");
    let found = rig
        .env
        .get_cab(&RawStr::from("CAB-I"), &Meter::unlimited())
        .unwrap();
    assert!(found.is_some(), "a lookup right after the load");
    finish(&rig, Op::GetI, get);
    quiet(&rig, &[Op::LoadP]);
}

/// A standing slot that is the key's value learns a lower position from a
/// registration that ranks below it (two loads of `o` with a `CAB-Z` after
/// `INNER`, on the same `Bytes`, the later one's `INNER` registered first:
/// `CAB-Z`'s key then sits between the two positions) while a lookup on a third
/// thread is reaching it, stopped after its registrations: the reach reads
/// the position again after it marks the slot reached and registers once
/// more there, so the entries take the first load's positions, as an eager
/// load gives them (`reach.rs`'s module docs, "Stand-ins"; plan section 9,
/// Q14, revised after review round 6). Run as well with the reach done
/// before the registration, which then finds the slot reached and lists
/// its own slot, which its load reaches before it returns.
#[test]
fn a_reach_that_learns_a_lower_position_registers_there() {
    let inner = bundle(&[("CAB-I", cab("i")), ("CAB-I.resS", res_bytes(8, 1))]);
    let o = Bytes::from(bundle(&[
        ("CAB-O", cab("o")),
        ("INNER", inner),
        ("CAB-Z", cab("z")),
    ]));
    for reach_first in [false, true] {
        let mut options = EnvOptions::unitypy();
        options.load = LoadMode::Lazy;
        let env = Environment::new(options);
        let gate = Gate::install(&env);
        let load = |name: &'static str, at: Option<(Hook, usize)>| {
            let (env, data) = (env.clone(), o.clone());
            gate.run(at, move || drop(env.load(src(name, &data)).unwrap()))
        };
        // The first load stops after its first key, `CAB-O`; the second
        // registers `INNER` first, at its own position.
        let first = load("o.bundle", Some((Hook::Published, 1)));
        first.stopped();
        load("o2.bundle", None).finish();
        let standing = env.registry().peek("inner").expect("INNER");
        assert_eq!(standing.position(), standing.rank.load);
        let get = {
            let env = env.clone();
            let at = (!reach_first).then_some((Hook::Reached, 1));
            gate.run(at, move || {
                let meter = Meter::unlimited();
                drop(env.get_cab(&RawStr::from("INNER"), &meter).unwrap());
            })
        };
        if reach_first {
            get.finish();
            first.finish();
        } else {
            get.stopped();
            assert!(env.registry().has("cab-i"), "the reach registered");
            first.finish();
            assert!(standing.position() < standing.rank.load);
            assert!(standing.unreached(), "the reach is stopped");
            get.finish();
        }
        assert!(!standing.unreached(), "reach first: {reach_first}");
        env.registry().check();
        assert!(env.registry().listed().is_empty());
        let eager = Environment::new(EnvOptions::unitypy());
        for name in ["o.bundle", "o2.bundle"] {
            eager.load(src(name, &o)).unwrap();
        }
        let what = format!("reach first: {reach_first}");
        assert_eq!(answers(&env, false), answers(&eager, false), "{what}");
        assert_eq!(keys(&env), keys(&eager), "{what}: the positions");
    }
}

/// A listed slot is reached by every load's registration, whether or not
/// it drops a slot itself, and by the key listing, as by every other
/// reader (R3; review round 5: only a registration that dropped a slot
/// settled, and `keys` never did, so a refused reach waited for a lookup
/// by name). The slot is listed here directly, as a refused reach leaves
/// it.
#[test]
fn every_load_and_the_key_listing_reach_what_is_listed() {
    let scene = Scene::new();
    let other = Bytes::from(bundle(&[("CAB-C", cab("c"))]));
    for by_keys in [true, false] {
        let rig = Rig::new(&scene);
        let reg = rig.env.registry();
        let y = rig.containers[2].0.clone();
        {
            let mut inner = reg.write();
            inner.pending.push(y.clone());
            reg.store_lens(&inner.pending);
        }
        if by_keys {
            let _ = reg.keys().count();
        } else {
            drop(rig.env.load(src("c.bundle", &other)).unwrap());
        }
        assert!(!y.unreached(), "keys {by_keys}: Y was not reached");
        assert!(reg.has("cab-y"), "keys {by_keys}");
        assert!(reg.listed().is_empty(), "keys {by_keys}");
        rig.check();
    }
}

/// The test's own lock, held across an operation, so not one of the
/// Environment's (`super::super::held`).
#[cfg(test)]
fn plain<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Random operations on four threads, each step's invariants checked when
/// it returns (R1, R3, and R4 for what was listed when it started); once
/// all are done, the registry answers as an eager load of the same loads
/// in the order they were made. The loads are made in turn (a lock records
/// their order); every other operation races them.
#[cfg(test)]
#[test]
fn reach_and_settle_keep_their_invariants_under_a_random_stress() {
    let scene = Scene::new();
    for seed in 0..24_u64 {
        let rig = Arc::new(Rig::new(&scene));
        let loads = Arc::new(Mutex::new(Vec::new()));
        let threads: Vec<_> = (0..4_u64)
            .map(|t| {
                let rig = rig.clone();
                let loads = loads.clone();
                std::thread::spawn(move || {
                    let mut rng = Rng::new(seed * 16 + t);
                    for _ in 0..12 {
                        let op = OPS[rng.below(OPS.len())];
                        let listed = rig.env.registry().listed();
                        let found = if is_load(op) {
                            let mut order = plain(&loads);
                            let found = apply(&rig.env, &rig.scene, op);
                            order.push(op);
                            found
                        } else {
                            apply(&rig.env, &rig.scene, op)
                        };
                        after(&rig, op, &listed, found);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let order = plain(&loads).clone();
        quiet(&rig, &order);
    }
}
