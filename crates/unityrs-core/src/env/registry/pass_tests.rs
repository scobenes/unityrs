//! The container pass reaches a dropped slot of an evicted dependency,
//! opening the dependency again (plan section 9, Q13, revised after review
//! round 6), whatever the order in which another pass, an eviction, a
//! lookup by name and the key listing take their steps around it. Every
//! interleaving is forced with the Environment's hooks
//! ([`crate::env::gate`]), as in `reach_tests.rs`: each operation stopped at
//! each point between its steps while each other operation runs whole, or
//! stops at a point of its own and the two are let go in either order.

// The source scan behind "the core starts no thread and never waits"
// (`unityrs_testkit::waits`) takes only an item under `#[cfg(test)]` in its
// own file for test code, so every item here that names a thread carries
// one.
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{Hook, Slot};
use crate::container::ContainerPath;
#[cfg(test)]
use crate::env::gate::{Event, Gate, Run};
use crate::env::{Cab, EnvOptions, Environment, LoadMode, Source};
use crate::limits::Meter;
use crate::options::FileContext;
use crate::raw::RawStr;
use bytes::Bytes;
use unityrs_testkit::env::{Obj, bundle, nested_dropped_evicted, serialized};

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
}

impl Rig {
    fn new() -> Self {
        let scene = nested_dropped_evicted();
        let mut options = EnvOptions::batch(Arc::new(FileContext::unitypy()));
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
    /// A dependency load, which evicts whatever the pool holds.
    Evict,
    /// A lookup by name of the dropped container's entry (no reopen, Q13).
    GetD,
    /// The key listing.
    Keys,
}

const OPS: [Op; 4] = [Op::Pass, Op::Evict, Op::GetD, Op::Keys];

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
    if op == Op::Pass {
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
    assert_eq!(cases, 4 * 5 * 4 * 11);
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
