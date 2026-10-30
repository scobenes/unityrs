//! The dependency pool's refusals (plan section 3.13): a dependency read or
//! registration the pool refuses evicts the other cached dependencies and
//! is tried once more, whichever step of a call makes it, under both modes
//! (review round 7: under `LoadMode::Lazy` the container pass's reach of an
//! evicted dependency's nested container opened it again with no retry, so
//! a pool with room for one open failed every `container()` and
//! `container_path` where `Eager` made room and succeeded).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value as J;
use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation};
use unityrs_core::container::ContainerPath;
use unityrs_core::env::{Cab, EnvOptions, Environment, LoadMode, Source};
use unityrs_core::object::ObjectRef;
use unityrs_core::{Bytes, RawStr};
use unityrs_testkit::env::{DepScene, dependency_scenes};

use crate::{ctx_with, unlimited};

/// A budget that grants every reservation but one: the `n`-th dependency
/// reservation after it is armed with `n` ([`Self::arm`]), refused once.
/// It counts the dependency reservations it is asked for.
#[derive(Debug, Default)]
struct RefuseOnce {
    countdown: AtomicU64,
    asked: AtomicU64,
    refused: AtomicU64,
}

impl RefuseOnce {
    fn arm(&self, n: u64) {
        self.countdown.store(n, Ordering::Release);
    }

    fn asked(&self) -> u64 {
        self.asked.load(Ordering::Acquire)
    }
}

impl MemoryBudget for RefuseOnce {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        if what == Reservation::Dependency {
            self.asked.fetch_add(1, Ordering::AcqRel);
            let left = self
                .countdown
                .try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
            if left == Ok(1) {
                self.refused.fetch_add(1, Ordering::AcqRel);
                return Err(BudgetExceeded {
                    requested: bytes,
                    what,
                });
            }
        }
        Ok(Permit::detached(bytes))
    }
}

/// What UnityPy 1.25.2 recorded for dependency scene `name`
/// (`tests/fixtures/d/env.json`, `tools/fixtures/env.py`): the container's
/// rows, and `obj.container` of each object of the files it did not load
/// as dependencies.
fn oracle(name: &str) -> (J, J) {
    let doc: J = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/d/env.json"
    )))
    .unwrap();
    let scene = doc["dependency_scenes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == name)
        .unwrap();
    let step = |op: &str| {
        scene["steps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["op"][0] == op)
            .unwrap()["result"]
            .clone()
    };
    (step("container"), step("paths"))
}

/// An Environment with `batch()` options on `budget`, in `mode`, with a
/// dependency cache of `cache` bytes.
fn batch_env(mode: LoadMode, cache: u64, budget: &Arc<RefuseOnce>) -> Environment {
    let mut options = EnvOptions::batch(ctx_with(budget.clone()));
    options.load = mode;
    options.dep_cache_bytes = cache;
    Environment::new(options)
}

/// Loads scene `d`'s files in order, a dependency's through
/// `load_dependency`.
fn load(env: &Environment, d: &DepScene) -> Result<(), String> {
    for (i, (name, data)) in d.scene.files.iter().enumerate() {
        let source = Source::Bytes {
            name: RawStr::new(name.clone()),
            data: Bytes::from(data.clone()),
        };
        let loaded = if d.dependencies.contains(&i) {
            env.load_dependency(source)
        } else {
            env.load(source)
        };
        drop(loaded.map_err(|e| e.to_string())?);
    }
    Ok(())
}

/// The objects of `CAB-F`, the scenes' one file whose objects `env.objects`
/// lists.
fn objects(env: &Environment) -> Vec<ObjectRef> {
    let Some(Cab::Serialized(sf)) = env.get_cab(&RawStr::from("CAB-F"), &unlimited()).unwrap()
    else {
        panic!("CAB-F");
    };
    sf.objects().collect()
}

/// What each key of the registry answers, looked up by name.
fn lookups(env: &Environment, keys: &[String]) -> Vec<String> {
    keys.iter()
        .map(
            |k| match env.get_cab(&RawStr::from(k.as_str()), &unlimited()) {
                Ok(Some(Cab::Serialized(sf))) => format!("{k}: {}", sf.name()),
                Ok(Some(Cab::Raw(_))) => format!("{k}: raw"),
                Ok(None) => format!("{k}: none"),
                Err(e) => format!("{k}: {e}"),
            },
        )
        .collect()
}

/// The container's rows and each object's path, as the fixture writes them.
fn answers(env: &Environment, objects: &[ObjectRef]) -> Result<(J, J), String> {
    let text = |b: &[u8]| J::from(String::from_utf8_lossy(b).into_owned());
    let mut rows = Vec::new();
    for item in env.container().map_err(|e| e.to_string())? {
        let r = item.map_err(|e| e.to_string())?.row;
        let int = |v: Option<i128>| J::from(v.and_then(|v| i64::try_from(v).ok()));
        rows.push(J::from(vec![
            r.path.map_or(J::Null, |p| text(p.as_bytes())),
            int(r.preload_index),
            int(r.preload_size),
            int(r.file_id),
            int(r.path_id),
        ]));
    }
    let mut paths = Vec::new();
    for o in objects {
        paths.push(match env.container_path(o).map_err(|e| e.to_string())? {
            None => J::Null,
            Some(ContainerPath::Str(s)) => text(s.as_bytes()),
            Some(ContainerPath::NotAString) => J::from("<not a str>"),
        });
    }
    Ok((J::from(rows), J::from(paths)))
}

/// Which calls a sweep refuses a reservation of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The scene's loads.
    Loads,
    /// A lookup by name of every key.
    Lookups,
    /// The container pass and `container_path`.
    Pass,
}

/// Every dependency reservation of each phase (the scene's loads, a lookup
/// by name of every key, the container pass with `container_path`) refused
/// once in turn, on each dependency scene, under both modes, with a cache
/// that evicts a dependency at each dependency load and one that never
/// does: the loads and the pass end in UnityPy's container rows and paths,
/// and the lookups answer as they do with nothing refused (no answer is an
/// error). The `k`-th reservation of a run whose first `k - 1` were granted
/// is the `k`-th of the run that counted them, so each is refused once. The
/// reach's reopen without a retry fails the pass's first reservation under
/// `Lazy`; a retry that looks only for a bare refusal fails an `Eager` pass
/// whose reopen's entry parse is refused (the entry's error wraps it); a
/// dependency's table walk without one fails under both modes.
#[test]
fn a_pool_refusal_anywhere_evicts_the_others_and_retries_once() {
    let mut refused = 0;
    for (name, d) in dependency_scenes() {
        let (rows, paths) = oracle(name);
        let want = Ok((rows, paths));
        for mode in [LoadMode::Eager, LoadMode::Lazy] {
            for cache in [1, u64::MAX] {
                let budget = Arc::new(RefuseOnce::default());
                let env = batch_env(mode, cache, &budget);
                let at_load = budget.asked();
                load(&env, &d).unwrap();
                let keys: Vec<String> = env
                    .registry()
                    .keys()
                    .map(|k| String::from_utf8_lossy(k.as_bytes()).into_owned())
                    .collect();
                let at_lookups = budget.asked();
                let found = lookups(&env, &keys);
                // The pass on an Environment of its own, with no lookup
                // before it.
                let budget2 = Arc::new(RefuseOnce::default());
                let env2 = batch_env(mode, cache, &budget2);
                load(&env2, &d).unwrap();
                let objects2 = objects(&env2);
                let at_pass = budget2.asked();
                assert_eq!(answers(&env2, &objects2), want, "{name} {mode:?} {cache}");
                let counts = [
                    (Phase::Loads, at_lookups - at_load),
                    (Phase::Lookups, budget.asked() - at_lookups),
                    (Phase::Pass, budget2.asked() - at_pass),
                ];
                for (phase, n) in counts {
                    for k in 1..=n {
                        let what = format!("{name} {mode:?} cache {cache} {phase:?}: {k} of {n}");
                        let budget = Arc::new(RefuseOnce::default());
                        let env = batch_env(mode, cache, &budget);
                        if phase == Phase::Loads {
                            budget.arm(k);
                        }
                        load(&env, &d).unwrap_or_else(|e| panic!("{what}: {e}"));
                        match phase {
                            Phase::Loads => {
                                let objects = objects(&env);
                                assert_eq!(answers(&env, &objects), want, "{what}");
                            }
                            Phase::Lookups => {
                                budget.arm(k);
                                assert_eq!(lookups(&env, &keys), found, "{what}");
                            }
                            Phase::Pass => {
                                let objects = objects(&env);
                                budget.arm(k);
                                assert_eq!(answers(&env, &objects), want, "{what}");
                            }
                        }
                        assert_eq!(budget.refused.load(Ordering::Acquire), 1, "{what}");
                        refused += 1;
                    }
                }
            }
        }
    }
    assert!(refused > 0, "no call made a dependency reservation");
}
