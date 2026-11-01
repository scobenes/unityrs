//! Every public call of the Environment, and how each meets a refusal of
//! the dependency pool (plan section 3.13): the calls are read from the
//! source, so a new one fails [`every_public_call_is_classified`] until it
//! is listed in [`CALLS`] with what it charges and the tests that refuse
//! each of its reservations in turn (review round 9: `object_tree`'s class
//! tree compile charged a dependency's meter outside the retry that every
//! other step of the call took, and no list of the calls showed it).

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value as J;
use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation};
use unityrs_core::env::{Cab, EnvOptions, Environment, LoadMode, Source};
use unityrs_core::files::File;
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, RawStr};
use unityrs_testkit::env::{DepScene, dependency_scenes, nested_dropped_evicted};

use crate::ctx_with;

/// How a public call meets the dependency pool.
#[derive(Clone, Copy, Debug)]
enum Pool {
    /// It charges no dependency's meter and asks the pool for nothing.
    Never,
    /// It takes the retry itself (`Environment::room` or `with_room`
    /// around each step that reads or registers through a dependency's
    /// meter): a refusal evicts the other dependencies and the step runs
    /// once more.
    Own,
    /// It charges a dependency's meter only inside the registry's own
    /// retries: a reach of a dependency's nested container (the `settle`
    /// a registration and a lookup run, the container pass's reaches), a
    /// dependency slot's resolve, a walk of a dependency's table. A
    /// refusal a `settle` meets twice is left for the next call.
    Inner,
    /// [`Self::Inner`], but the key a lookup builds is charged to the
    /// meter the caller gives, and returned as it is when refused: the
    /// registry holds no LRU (`Environment::get_cab` is the lookup that
    /// retries it).
    CallersKey,
}

/// Every public call (each `pub fn` of an `impl Environment` or `impl
/// Registry` block under `src/env/`, and each iterator a listing returns),
/// how it meets the pool, and the tests that refuse its dependency
/// reservations (or, for [`Pool::Never`], count them) and fail on a mutant
/// that removes its retry.
const CALLS: &[(&str, Pool, &[&str])] = &[
    ("Environment::new", Pool::Never, &[]),
    ("Environment::options", Pool::Never, &[NEVER]),
    ("Environment::registry", Pool::Never, &[NEVER]),
    ("Environment::stats", Pool::Never, &[NEVER]),
    ("Environment::files", Pool::Never, &[NEVER]),
    ("FileEntries::next", Pool::Never, &[NEVER]),
    ("Environment::objects", Pool::Never, &[NEVER]),
    ("Objects::next", Pool::Never, &[NEVER]),
    ("Registry::len", Pool::Never, &[NEVER]),
    ("Registry::is_empty", Pool::Never, &[NEVER]),
    ("Keys::next", Pool::Never, &[NEVER]),
    ("Environment::load_dependency", Pool::Own, &[SWEEP]),
    ("Environment::get_cab", Pool::Own, &[SWEEP]),
    ("Environment::get_cab_held", Pool::Own, &[SWEEP]),
    ("Environment::find_file", Pool::Own, &[SWEEP]),
    ("Environment::deref", Pool::Own, &[SWEEP, FULL, HOLD]),
    ("Environment::resource", Pool::Own, &[SWEEP, FULL, HOLD]),
    ("Environment::object_tree", Pool::Own, &[TREE, TREE_FULL]),
    ("Environment::load", Pool::Inner, &[SETTLE]),
    ("Registry::register", Pool::Inner, &[SETTLE]),
    ("Registry::register_lazy", Pool::Inner, &[SETTLE]),
    ("Registry::keys", Pool::Inner, &[KEYS]),
    ("Environment::container", Pool::Inner, &[SWEEP]),
    ("Environment::container_path", Pool::Inner, &[SWEEP]),
    ("ContainerItems::next", Pool::Inner, &[SWEEP, SETTLE]),
    ("Registry::get", Pool::CallersKey, &[SWEEP]),
];

const NEVER: &str = "calls_that_never_charge_the_pool_make_no_dependency_reservation";
const SWEEP: &str = "a_pool_refusal_anywhere_evicts_the_others_and_retries_once";
const FULL: &str = "a_full_pool_makes_room_for_a_read_from_a_dependency";
const HOLD: &str = "a_retry_keeps_the_bundle_the_call_reads_through";
const TREE: &str = "a_pool_refusal_in_object_tree_evicts_and_retries_once";
const TREE_FULL: &str = "a_full_pool_makes_room_for_object_tree";
const SETTLE: &str = "a_registration_that_drops_a_dependencys_container_makes_room_to_reach_it";
const KEYS: &str = "the_key_listing_makes_room_to_reach_a_dropped_container";

/// The `.rs` files under `dir`, recursively, with their text.
fn sources(dir: &Path, out: &mut Vec<(String, String)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&p).unwrap();
            out.push((p.display().to_string(), text));
        }
    }
}

/// The public calls `text` defines: `Type::name` for each `pub fn` of an
/// `impl Environment` or `impl Registry` block (rustfmt's layout: the block
/// opens and closes at column 0, its methods at four spaces), and
/// `Type::next` for each `Iterator` it implements.
fn public_calls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut block: Option<&str> = None;
    for line in text.lines() {
        if let Some(ty) = line
            .strip_prefix("impl ")
            .and_then(|r| r.strip_suffix(" {"))
            .filter(|ty| matches!(*ty, "Environment" | "Registry"))
        {
            block = Some(ty);
        } else if let Some(rest) = line
            .strip_prefix("impl Iterator for ")
            .or_else(|| line.strip_prefix("impl<'a> Iterator for "))
        {
            let ty: String = rest
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            out.push(format!("{ty}::next"));
        } else if line == "}" {
            block = None;
        } else if let Some(ty) = block
            && let Some(rest) = line.strip_prefix("    pub fn ")
        {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            out.push(format!("{ty}::{name}"));
        }
    }
    out
}

/// Every public call of the Environment, its registry and its listings is
/// in [`CALLS`], and nothing else is; each test a call names exists. A new
/// call that charges a dependency's meter is listed with the retry it
/// takes and the test that refuses each of its reservations in turn.
#[test]
fn every_public_call_is_classified() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut src = Vec::new();
    sources(&root.join("src/env"), &mut src);
    let found: BTreeSet<String> = src.iter().flat_map(|(_, t)| public_calls(t)).collect();
    let listed: BTreeSet<String> = CALLS.iter().map(|(n, _, _)| (*n).to_owned()).collect();
    assert_eq!(listed.len(), CALLS.len(), "a call is listed twice");
    let unlisted: Vec<_> = found.difference(&listed).collect();
    let gone: Vec<_> = listed.difference(&found).collect();
    assert!(
        unlisted.is_empty() && gone.is_empty(),
        "public calls not in CALLS: {unlisted:?}; listed calls that no longer exist: {gone:?}"
    );
    assert!(found.len() > 20, "the scan found {} calls", found.len());
    let mut tests = Vec::new();
    sources(&root.join("tests"), &mut tests);
    sources(&root.join("src"), &mut tests);
    for (call, pool, names) in CALLS {
        if !matches!(pool, Pool::Never) || *call != "Environment::new" {
            assert!(!names.is_empty(), "{call} names no test");
        }
        for name in *names {
            let def = format!("fn {name}(");
            assert!(
                tests.iter().any(|(_, t)| t.contains(&def)),
                "{call}: no test {name}"
            );
        }
    }
}

/// A budget that refuses the next `left` dependency reservations, and
/// counts the dependency reservations it is asked for.
#[derive(Debug, Default)]
struct RefuseNext {
    left: AtomicU64,
    asked: AtomicU64,
    refused: AtomicU64,
}

impl RefuseNext {
    fn refuse(&self, n: u64) {
        self.left.store(n, Ordering::Release);
    }

    fn asked(&self) -> u64 {
        self.asked.load(Ordering::Acquire)
    }

    fn refused(&self) -> u64 {
        self.refused.load(Ordering::Acquire)
    }
}

impl MemoryBudget for RefuseNext {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        if what == Reservation::Dependency {
            self.asked.fetch_add(1, Ordering::AcqRel);
            let left = self
                .left
                .try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
            if left.is_ok() {
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

/// `batch()`'s options in `mode` on `budget`, with a cache of `cache` bytes.
fn batch_env(mode: LoadMode, cache: u64, budget: Arc<RefuseNext>) -> Environment {
    let mut o = EnvOptions::batch(ctx_with(budget));
    o.load = mode;
    o.dep_cache_bytes = cache;
    Environment::new(o)
}

fn source(d: &DepScene, i: usize) -> Source {
    let (name, data) = &d.scene.files[i];
    Source::Bytes {
        name: RawStr::new(name.clone()),
        data: Bytes::from(data.clone()),
    }
}

/// Loads files `range` of scene `d`, a dependency's through
/// `load_dependency`: the dependencies' handles.
fn load(env: &Environment, d: &DepScene, range: std::ops::Range<usize>) -> Vec<File> {
    let mut held = Vec::new();
    for i in range {
        if d.dependencies.contains(&i) {
            held.push(env.load_dependency(source(d, i)).unwrap());
        } else {
            drop(env.load(source(d, i)).unwrap());
        }
    }
    held
}

/// The calls [`CALLS`] lists as [`Pool::Never`] ask the pool for nothing,
/// on every dependency scene, loaded under both modes with a cache that
/// evicts a dependency at each dependency load and one that never does:
/// `options`, `registry`, `stats`, the `files` and `objects` listings (a
/// file a caller loaded is on the caller's meter), the registry's `len`
/// and `is_empty`, and its key listing once a first listing settled what
/// the loads dropped.
#[test]
fn calls_that_never_charge_the_pool_make_no_dependency_reservation() {
    for (name, d) in dependency_scenes() {
        for mode in [LoadMode::Eager, LoadMode::Lazy] {
            for cache in [1, u64::MAX] {
                let what = format!("{name} {mode:?} cache {cache}");
                let budget = Arc::new(RefuseNext::default());
                let env = batch_env(mode, cache, budget.clone());
                let _held = load(&env, &d, 0..d.scene.files.len());
                let settled = env.registry().keys().count();
                let at = budget.asked();
                let _ = (env.options(), env.registry(), env.stats());
                let files = env.files().count();
                for o in env.objects() {
                    o.unwrap_or_else(|e| panic!("{what}: {e}"));
                }
                let reg = env.registry();
                let keys = reg.keys().count();
                assert_eq!((reg.len(), reg.is_empty()), (keys, keys == 0), "{what}");
                assert_eq!(keys, settled, "{what}");
                let callers = d.scene.files.len() - d.dependencies.len();
                assert_eq!(files, callers, "{what}: the caller's loads are listed");
                assert_eq!(budget.asked() - at, 0, "{what}");
            }
        }
    }
}

/// UnityPy's keys once scene `nested_dropped_evicted`'s loads are done
/// (`tests/fixtures/d/env.json`), sorted: under `LoadMode::Lazy` a nested
/// container's own name is a key at its rank (plan 3.13 (b)), which moves
/// `inner` but adds no key here, since `p.bundle`'s entry `INNER` is one.
fn oracle_keys() -> Vec<String> {
    let doc: J = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/d/env.json"
    )))
    .unwrap();
    let scene = doc["dependency_scenes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "nested_dropped_evicted")
        .unwrap();
    let step = scene["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["op"] == serde_json::json!(["load", 3]))
        .unwrap();
    let mut keys: Vec<String> = step["cabs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k.as_str().unwrap().to_owned())
        .collect();
    keys.sort();
    keys
}

fn sorted_keys(env: &Environment) -> Vec<String> {
    let mut keys: Vec<String> = env
        .registry()
        .keys()
        .map(|k| String::from_utf8_lossy(k.as_bytes()).into_owned())
        .collect();
    keys.sort();
    keys
}

/// How a test drops the dependency `a.bundle`'s unread nested container
/// `INNER`: a caller's load of `p.bundle`, whose entry `INNER` takes the
/// key, or a registration of that entry through the registry.
#[derive(Clone, Copy, Debug)]
enum Dropper {
    Load,
    Register,
}

/// Scene `nested_dropped_evicted` under `LoadMode::Lazy`, with a cache
/// that evicts nothing at a load, loaded up to `p.bundle`; then `p.bundle`'s
/// `INNER` registered by `how`, with the next `refused` dependency
/// reservations refused. The registration drops `a.bundle`'s slot unread,
/// and its `settle` reaches it, reading `a.bundle` through its meter.
fn drop_inner(how: Dropper, refused: u64) -> (Environment, Arc<RefuseNext>, Vec<File>) {
    let d = nested_dropped_evicted();
    let budget = Arc::new(RefuseNext::default());
    let env = batch_env(LoadMode::Lazy, u64::MAX, budget.clone());
    let held = load(&env, &d, 0..3);
    let inner = match how {
        Dropper::Load => None,
        Dropper::Register => {
            let side = Environment::new(EnvOptions::unitypy());
            drop(side.load(source(&d, 3)).unwrap());
            let cab = side.get_cab(&RawStr::from("INNER"), &Meter::unlimited());
            let Ok(Some(cab @ Cab::Raw(_))) = cab else {
                panic!("p.bundle's INNER is a raw entry: {cab:?}")
            };
            Some(cab)
        }
    };
    budget.refuse(refused);
    match inner {
        None => drop(env.load(source(&d, 3)).unwrap()),
        Some(cab) => env
            .registry()
            .register(&RawStr::from("INNER"), cab, &Meter::unlimited())
            .unwrap(),
    }
    budget.refuse(0);
    (env, budget, held)
}

/// A registration that drops a dependency's unread nested container (a
/// caller's load, or the registry's own `register`) reaches it before it
/// returns, as an eager load registered its entries at load; when the pool
/// refuses that reach, the reach evicts the other dependencies and runs
/// once more (`settle`'s reach takes the registry's retry, plan section
/// 3.13), so the registration returns with UnityPy's keys registered. The
/// registry's size is read before anything else settles, so a reach left
/// for the next call shows as a missing key.
#[test]
fn a_registration_that_drops_a_dependencys_container_makes_room_to_reach_it() {
    let want = oracle_keys();
    for how in [Dropper::Load, Dropper::Register] {
        let (env, budget, _held) = drop_inner(how, 1);
        assert_eq!(budget.refused(), 1, "{how:?}: nothing was refused");
        assert_eq!(
            env.registry().len(),
            want.len(),
            "{how:?}: a key is missing"
        );
        assert!(
            env.stats().dependencies_evicted > 0,
            "{how:?}: nothing was evicted"
        );
        assert_eq!(sorted_keys(&env), want, "{how:?}");
    }
}

/// A dropped container whose reach the pool refused twice (the
/// registration's try and its retry) is left for the next call: the key
/// listing reaches it first, and when the pool refuses that reach once,
/// it evicts the other dependencies and runs once more, so the listing has
/// UnityPy's keys.
#[test]
fn the_key_listing_makes_room_to_reach_a_dropped_container() {
    let want = oracle_keys();
    let (env, budget, _held) = drop_inner(Dropper::Load, 2);
    assert_eq!(budget.refused(), 2);
    assert_eq!(
        env.registry().len(),
        want.len() - 1,
        "the reach was not left"
    );
    budget.refuse(1);
    let keys = sorted_keys(&env);
    budget.refuse(0);
    assert_eq!(budget.refused(), 3, "the listing's reach was not refused");
    assert_eq!(keys, want);
}
