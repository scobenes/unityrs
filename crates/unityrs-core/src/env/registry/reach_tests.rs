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

use super::{Hook, Registry, Slot, lock};
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
    fn peek(&self, key: &str) -> Option<Arc<Slot>> {
        self.read()
            .slots
            .get(key.as_bytes())
            .map(|e| e.slot.clone())
    }

    /// Whether `key` (already simplified) is registered, without settling.
    fn has(&self, key: &str) -> bool {
        self.read().slots.contains_key(key.as_bytes())
    }

    /// The slots listed now.
    pub(crate) fn listed(&self) -> Vec<Arc<Slot>> {
        let inner = self.read();
        inner.pending.slots.iter().map(|(_, s)| s.clone()).collect()
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
        let listed = &inner.pending.slots;
        assert!(listed.windows(2).all(|w| w[0].0 < w[1].0), "list order");
        let len = self.pending_len.load(std::sync::atomic::Ordering::Acquire);
        assert_eq!(len, listed.len(), "the list's count");
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
    /// A lookup by name of an entry inside `INNER`.
    GetI,
    /// A lookup of `Y`, a live container, which reaches it.
    GetY,
    /// The key listing.
    Keys,
    /// The container pass.
    Pass,
}

const OPS: [Op; 6] = [Op::LoadP, Op::LoadR, Op::GetI, Op::GetY, Op::Keys, Op::Pass];

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
        Op::LoadP => drop(env.load(src("p.bundle", &scene.p)).unwrap()),
        Op::LoadR => drop(env.load(src("r.bundle", &scene.r)).unwrap()),
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
    let reg = rig.env.registry();
    for slot in listed {
        // A slot is listed with the change that drops it, and taken off
        // again when the key's value stands in for it (R4's exemption).
        let covered = || {
            let inner = reg.read();
            let standing = inner.slots.values().map(|e| &e.slot);
            standing.clone().any(|s| super::reach::stands_in(s, slot))
                && !inner
                    .pending
                    .slots
                    .iter()
                    .any(|(_, l)| Arc::ptr_eq(l, slot))
        };
        assert!(
            !slot.unreached() || covered(),
            "R4: {op:?} returned with a slot listed before it unreached"
        );
    }
    let [inner, x, y] = [0, 1, 2].map(|i| &rig.containers[i].0);
    match op {
        // A registration that drops a slot returns with it reached.
        Op::LoadP => assert!(!inner.unreached(), "R4: LoadP's own drop"),
        Op::LoadR => assert!(!x.unreached(), "R4: LoadR's own drop"),
        // A lookup that resolves a container's slot reaches it.
        Op::GetY => assert!(!y.unreached(), "a lookup of Y"),
        Op::Pass => {
            for (slot, _) in &rig.containers {
                assert!(!slot.unreached(), "the pass reaches every container");
            }
        }
        Op::GetI | Op::Keys => {}
    }
    if op == Op::GetI && listed.iter().any(|s| Arc::ptr_eq(s, inner)) {
        assert_eq!(found, Some(true), "a lookup after INNER's drop");
    }
}

/// What each name finds, as a comparable answer.
fn answers(env: &Environment) -> Vec<String> {
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
        order.push(match op {
            Op::LoadP => ("p.bundle", &scene.p),
            _ => ("r.bundle", &scene.r),
        });
    }
    for (name, data) in order {
        eager.load(src(name, data)).unwrap();
    }
    assert_eq!(answers(&rig.env), answers(&eager), "R1: values");
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
                        let loads: Vec<Op> = [a, b]
                            .into_iter()
                            .filter(|o| matches!(o, Op::LoadP | Op::LoadR))
                            .collect();
                        quiet(&rig, &loads);
                    }
                }
            }
        }
    }
    assert_eq!(cases, 6 * 4 * 6 * 9);
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
            let len = inner.pending.slots.len();
            reg.pending_len
                .store(len, std::sync::atomic::Ordering::Release);
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
                        let found = if matches!(op, Op::LoadP | Op::LoadR) {
                            let mut order = lock(&loads);
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
        let order = lock(&loads).clone();
        quiet(&rig, &order);
    }
}
