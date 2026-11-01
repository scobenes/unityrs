//! The dependency pool's refusals (plan section 3.13): every Environment
//! call that reads or registers through a dependency's meter, when the pool
//! refuses it, evicts the other cached dependencies and is tried once more,
//! whichever step of the call makes the refusal, under both modes (review
//! round 7: under `LoadMode::Lazy` the container pass's reach of an evicted
//! dependency's nested container opened it again with no retry, so a pool
//! with room for one open failed every `container()` and `container_path`
//! where `Eager` made room and succeeded; review round 8: a dereference or a
//! resource read from a dependency's file took no retry, nor did a lookup
//! whose key was charged to a dependency's meter).

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value as J, json};
use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation};
use unityrs_core::container::ContainerPath;
use unityrs_core::env::{Cab, CabResolver, EnvOptions, Environment, Fetch, LoadMode, PPtr, Source};
use unityrs_core::files::{File, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::object::ObjectRef;
use unityrs_core::value::Value;
use unityrs_core::{Bytes, Error, RawStr};
use unityrs_testkit::env::{
    DepScene, Scene, bundle, dependency_reads, dependency_scenes, res_bytes,
};

use crate::{DepPool, ctx_with};

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
/// (`tests/fixtures/d/env.json`, `tools/fixtures/env.py`): its steps, each
/// with its result and the Environment's state after it.
fn oracle_steps(name: &str) -> Vec<J> {
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
    scene["steps"].as_array().unwrap().clone()
}

/// The result of scene `name`'s step `op` (the container's rows,
/// `obj.container` of each object of the files it did not load as
/// dependencies, and each pointer and resource of the objects of those it
/// did).
fn oracle(name: &str, op: &str) -> J {
    let steps = oracle_steps(name);
    steps.iter().find(|s| s["op"][0] == op).unwrap()["result"].clone()
}

/// What scene `name`'s step `op` added to `env.files` (each entry's name
/// and `is_dependency`): the resolver's loads it listed, as an Environment
/// that keeps them lists them ([`EnvOptions::keep_resolver_loads`]).
fn oracle_listed(name: &str, op: &str) -> J {
    let steps = oracle_steps(name);
    let at = steps.iter().position(|s| s["op"][0] == op).unwrap();
    let names = |s: &J| -> Vec<J> {
        let files = s["files"].as_array().unwrap();
        files.iter().map(|f| json!([f[0], f[1]])).collect()
    };
    let before = names(&steps[at - 1]);
    let added: Vec<J> = names(&steps[at])
        .into_iter()
        .filter(|f| !before.contains(f))
        .collect();
    J::from(added)
}

/// Every entry `env.files` lists, by name and `is_dependency`.
fn listed(env: &Environment) -> Vec<J> {
    let files = env
        .files()
        .map(|f| json!([text(f.name.as_bytes()), f.is_dependency]));
    files.collect()
}

/// UnityPy's `env.path` for the scenes here: a file of exactly the name
/// asked (no scene's pointer or resource candidate is the end of another
/// resolver file's name), keyed `@/<name>` as `env.py` writes its folder.
#[derive(Debug)]
struct Files(Vec<(Vec<u8>, Bytes)>);

impl CabResolver for Files {
    fn resolve(&self, name: &[u8]) -> Option<Source> {
        let (n, data) = self.0.iter().find(|(n, _)| n == name)?;
        let mut key = b"@/".to_vec();
        key.extend_from_slice(n);
        Some(Source::Bytes {
            name: RawStr::new(key),
            data: data.clone(),
        })
    }
}

/// The options a sweep runs under: `batch()`'s, in `mode`, with a cache of
/// `cache` bytes, keeping the resolver's loads in `files` with `keep` (a
/// `find_file` load then lists its file, charged to its meter).
#[derive(Clone, Copy, Debug)]
struct Setup {
    mode: LoadMode,
    cache: u64,
    keep: bool,
}

/// Both modes, each with a cache that evicts a dependency at each
/// dependency load, one that never does, and the first keeping the
/// resolver's loads.
fn setups() -> Vec<Setup> {
    let mut out = Vec::new();
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        for (cache, keep) in [(1, false), (u64::MAX, false), (1, true)] {
            out.push(Setup { mode, cache, keep });
        }
    }
    out
}

/// An Environment of `s` on `budget`, the resolver serving `scene`'s files.
fn batch_env(s: Setup, budget: Arc<dyn MemoryBudget>, scene: &Scene) -> Environment {
    let mut options = EnvOptions::batch(ctx_with(budget));
    options.load = s.mode;
    options.dep_cache_bytes = s.cache;
    options.keep_resolver_loads = s.keep;
    let files = scene
        .resolver
        .iter()
        .map(|(n, d)| (n.clone(), Bytes::from(d.clone())))
        .collect();
    options.resolver = Some(Arc::new(Files(files)));
    Environment::new(options)
}

/// Loads scene `d`'s files in order, a dependency's through
/// `load_dependency`: the dependencies' handles, which the caller of a read
/// from a dependency's file holds.
fn load(env: &Environment, d: &DepScene) -> Result<Vec<File>, String> {
    let mut held = Vec::new();
    for (i, (name, data)) in d.scene.files.iter().enumerate() {
        let source = Source::Bytes {
            name: RawStr::new(name.clone()),
            data: Bytes::from(data.clone()),
        };
        if d.dependencies.contains(&i) {
            held.push(env.load_dependency(source).map_err(|e| e.to_string())?);
        } else {
            drop(env.load(source).map_err(|e| e.to_string())?);
        }
    }
    Ok(held)
}

/// Every object of the dependencies' files (`held`, in load order), as
/// `env.py`'s `dep_objects` walks UnityPy's: a bundle's entries in
/// directory order, a nested bundle's in place.
fn dep_objects(held: &[File]) -> Vec<ObjectRef> {
    fn walk(file: &File, out: &mut Vec<ObjectRef>) {
        match file {
            File::Serialized(sf) => out.extend(sf.objects()),
            File::Bundle(b) => {
                for i in 0..b.directory().len() {
                    walk(&b.entry(i).unwrap(), out);
                }
            }
            File::Web(_) | File::Raw(_) => {}
        }
    }
    let mut out = Vec::new();
    for f in held {
        walk(f, &mut out);
    }
    out
}

/// Every pointer of the dependencies' named objects, with its file.
fn pointers(held: &[File]) -> Vec<(Arc<SerializedFile>, PPtr)> {
    let mut out = Vec::new();
    for o in dep_objects(held).iter().filter(|o| o.class_id() == 1) {
        let p = o.parse_as_dict(true).unwrap();
        let Value::Dict(m) = p.value() else { continue };
        for (k, v) in p.entries(m) {
            let Value::Dict(fields) = v else { continue };
            if !k.starts_with(b"m_Ptr") {
                continue;
            }
            let int = |name: &[u8]| p.field(fields, name).unwrap().as_i64().unwrap();
            let file_id = i32::try_from(int(b"m_FileID")).unwrap();
            let path_id = int(b"m_PathID");
            out.push((o.file().clone(), PPtr { file_id, path_id }));
        }
    }
    out
}

/// A resource read from a dependency's file: the file, the path, the offset
/// and the size.
type Stream = (Arc<SerializedFile>, RawStr, u64, u64);

/// Every resource of the dependencies' textures.
fn streams(held: &[File]) -> Vec<Stream> {
    let mut out = Vec::new();
    for o in dep_objects(held).iter().filter(|o| o.class_id() == 28) {
        let p = o.parse_as_dict(true).unwrap();
        let Some(Value::Str(path)) = p.get(&[b"m_StreamData", b"path"]) else {
            panic!("no path");
        };
        let at = |k: &[u8]| p.get(&[b"m_StreamData", k]).unwrap().as_u64().unwrap();
        out.push((o.file().clone(), path.clone(), at(b"offset"), at(b"size")));
    }
    out
}

fn text(b: &[u8]) -> J {
    J::from(String::from_utf8_lossy(b).into_owned())
}

/// Every pointer of `ptrs` dereferenced.
fn derefs(
    env: &Environment,
    ptrs: &[(Arc<SerializedFile>, PPtr)],
) -> Vec<Result<ObjectRef, String>> {
    ptrs.iter()
        .map(|(f, p)| env.deref(f, *p).map_err(|e| e.to_string()))
        .collect()
}

/// Every resource of `streams` read.
fn reads(env: &Environment, streams: &[Stream]) -> Vec<Result<Bytes, String>> {
    streams
        .iter()
        .map(|(f, path, offset, size)| {
            env.resource(f, path, *offset, *size)
                .map_err(|e| e.to_string())
        })
        .collect()
}

/// What the dereferences reached, as `env.py` writes it: each target's file
/// name (UnityPy's basename), its path id, class id and `m_Name`.
fn targets(got: &[Result<ObjectRef, String>]) -> J {
    let target = |t: &Result<ObjectRef, String>| {
        let t = match t {
            Ok(t) => t,
            Err(e) => return J::from(format!("core: {e}")),
        };
        let name = t.file().name().as_bytes();
        let base = &name[name.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1)..];
        let p = t.parse_as_dict(true).unwrap();
        let Some(Value::Str(m_name)) = p.get(&[b"m_Name"]) else {
            panic!("no name");
        };
        json!([
            text(base),
            t.path_id(),
            t.class_id(),
            text(m_name.as_bytes())
        ])
    };
    J::from(got.iter().map(target).collect::<Vec<_>>())
}

/// The resource reads, as `env.py` writes them.
fn hex(got: &[Result<Bytes, String>]) -> J {
    let one = |r: &Result<Bytes, String>| match r {
        Ok(b) => J::from(b.iter().fold(String::new(), |mut s, x| {
            let _ = write!(s, "{x:02x}");
            s
        })),
        Err(e) => J::from(format!("core: {e}")),
    };
    J::from(got.iter().map(one).collect::<Vec<_>>())
}

/// What each key of the registry answers, looked up by name in upper case
/// (a key the lookup lowers, charged to `meter`) through `get_cab` and
/// `find_file`, and through the registry's own `get` with its key on an
/// unlimited meter: the registry holds no LRU, so its key's charge is the
/// caller's to retry (`get_cab` does), and only its slot's resolve, which
/// reads through a dependency's meter, makes room itself (review round 9:
/// no test refused that resolve outside a call that retries it again).
fn lookups(env: &Environment, keys: &[String], meter: &Arc<Meter>) -> Vec<String> {
    let answer = |k: &str, found: Result<Option<Cab>, _>| match found {
        Ok(Some(Cab::Serialized(sf))) => format!("{k}: {}", sf.name()),
        Ok(Some(Cab::Raw(_))) => format!("{k}: raw"),
        Ok(None) => format!("{k}: none"),
        Err(e) => format!("{k}: {e}"),
    };
    let mut out = Vec::new();
    for k in keys {
        let name = RawStr::from(k.to_uppercase().as_str());
        out.push(answer(k, env.get_cab(&name, meter)));
        out.push(answer(k, env.find_file(&name, meter)));
        out.push(answer(k, env.registry().get(&name, &Meter::unlimited())));
    }
    out
}

/// The container's rows and each of `objects`' path, as the fixture writes
/// them.
fn answers(env: &Environment, objects: &[ObjectRef]) -> J {
    let mut rows = Vec::new();
    let items = match env.container() {
        Ok(items) => items,
        Err(e) => return J::from(format!("core: {e}")),
    };
    for item in items {
        let r = match item {
            Ok(item) => item.row,
            Err(e) => return J::from(format!("core: {e}")),
        };
        let int = |v: Option<i128>| J::from(v.and_then(|v| i64::try_from(v).ok()));
        rows.push(json!([
            r.path.map_or(J::Null, |p| text(p.as_bytes())),
            int(r.preload_index),
            int(r.preload_size),
            int(r.file_id),
            int(r.path_id)
        ]));
    }
    let mut paths = Vec::new();
    for o in objects {
        paths.push(match env.container_path(o) {
            Ok(None) => J::Null,
            Ok(Some(ContainerPath::Str(s))) => text(s.as_bytes()),
            Ok(Some(ContainerPath::NotAString)) => J::from("<not a str>"),
            Err(e) => return J::from(format!("core: {e}")),
        });
    }
    json!([rows, paths])
}

/// Which calls a sweep refuses a reservation of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The scene's loads.
    Loads,
    /// A lookup by name of every key, charged to a dependency's meter.
    Lookups,
    /// The container pass and `container_path`.
    Pass,
    /// Every pointer of the dependencies' objects dereferenced.
    Derefs,
    /// Every resource of the dependencies' objects read.
    Resources,
}

const PHASES: [Phase; 5] = [
    Phase::Loads,
    Phase::Lookups,
    Phase::Pass,
    Phase::Derefs,
    Phase::Resources,
];

/// What a phase's calls returned, before it is written as the fixture
/// writes it.
enum Got {
    Json(J),
    Targets(Vec<Result<ObjectRef, String>>),
    Reads(Vec<Result<Bytes, String>>),
}

/// One run of `phase` on scene `d` under `s`, with the `k`-th dependency
/// reservation of the phase refused once (none for `None`): its answer,
/// the dependency reservations the phase made, and how many were refused.
/// What a phase reads first (the objects, their pointers and resources) is
/// read before the refusal is armed, and its answer is written after it is
/// disarmed, so the reservations swept are the Environment calls' alone. A
/// dereference's or a read's answer, when `s` keeps the resolver's loads,
/// is also what the phase added to `files`.
fn run(phase: Phase, s: Setup, d: &DepScene, keys: &[String], k: Option<u64>) -> (J, u64, u64) {
    let budget = Arc::new(RefuseOnce::default());
    let env = batch_env(s, budget.clone(), &d.scene);
    let arm = || budget.arm(k.unwrap_or(0));
    if phase == Phase::Loads {
        arm();
    }
    let at = budget.asked();
    let held = load(&env, d).unwrap_or_else(|e| panic!("{phase:?} {s:?} {k:?}: {e}"));
    let loads = budget.asked() - at;
    let objects: Vec<ObjectRef> = env.objects().map(Result::unwrap).collect();
    let (ptrs, res) = (pointers(&held), streams(&held));
    let meter = held[0].meter().clone();
    let before = listed(&env);
    if phase != Phase::Loads {
        arm();
    }
    let at = budget.asked();
    let got = match phase {
        Phase::Loads | Phase::Pass => Got::Json(answers(&env, &objects)),
        Phase::Lookups => Got::Json(json!(lookups(&env, keys, &meter))),
        Phase::Derefs => Got::Targets(derefs(&env, &ptrs)),
        Phase::Resources => Got::Reads(reads(&env, &res)),
    };
    let asked = match phase {
        Phase::Loads => loads,
        _ => budget.asked() - at,
    };
    budget.arm(0);
    let answer = match got {
        Got::Json(j) => j,
        Got::Targets(t) => targets(&t),
        Got::Reads(r) => hex(&r),
    };
    let answer = match phase {
        Phase::Derefs | Phase::Resources if s.keep => {
            let mut added = listed(&env);
            added.retain(|f| !before.contains(f));
            json!([answer, added])
        }
        _ => answer,
    };
    (answer, asked, budget.refused.load(Ordering::Acquire))
}

/// Every dependency reservation of each phase (the scene's loads, a lookup
/// by name of every key charged to a dependency's meter, the container pass
/// with `container_path`, and a dereference of every pointer and a read of
/// every resource of the dependencies' objects) refused once in turn, on
/// each dependency scene, under both modes, with a cache that evicts a
/// dependency at each dependency load and one that never does, and with the
/// resolver's loads kept in `files`: the loads and the pass end in UnityPy's
/// container rows and paths, the dereferences and the reads give UnityPy's
/// answers, and the lookups answer as they do with nothing refused (no
/// answer is an error), the dereferences and the reads that keep the
/// resolver's loads listing what UnityPy's listed. The `k`-th reservation
/// of a run whose first `k - 1` were granted is the `k`-th of the run that
/// counted them, so each is refused once. The reach's reopen without a retry fails the pass's first
/// reservation under `Lazy`; a retry that looks only for a bare refusal
/// fails an `Eager` pass whose reopen's entry parse is refused (the entry's
/// error wraps it); a dependency's table walk without one fails under both
/// modes; and a dereference, a resource read or a lookup without one fails
/// at its first reservation no inner retry covers, and a resolver load
/// whose listing is refused outside its retry leaves its file unlisted
/// (review round 8).
#[test]
fn a_pool_refusal_anywhere_evicts_the_others_and_retries_once() {
    let mut refused = 0;
    for (name, d) in dependency_scenes() {
        let want = |phase: Phase, s: Setup| {
            let step = match phase {
                Phase::Loads | Phase::Pass => {
                    return json!([oracle(name, "container"), oracle(name, "paths")]);
                }
                Phase::Derefs => "dep_derefs",
                Phase::Resources => "dep_resources",
                Phase::Lookups => return J::Null,
            };
            if s.keep {
                json!([oracle(name, step), oracle_listed(name, step)])
            } else {
                oracle(name, step)
            }
        };
        for s in setups() {
            let keys: Vec<String> = {
                let env = batch_env(s, Arc::new(RefuseOnce::default()), &d.scene);
                let _held = load(&env, &d).unwrap();
                env.registry()
                    .keys()
                    .map(|k| String::from_utf8_lossy(k.as_bytes()).into_owned())
                    .collect()
            };
            for phase in PHASES {
                let (first, n, _) = run(phase, s, &d, &keys, None);
                let want = match phase {
                    Phase::Lookups => first.clone(),
                    _ => want(phase, s),
                };
                assert_eq!(first, want, "{name} {s:?} {phase:?}");
                for k in 1..=n {
                    let what = format!("{name} {s:?} {phase:?}: {k} of {n}");
                    let (answer, _, refusals) = run(phase, s, &d, &keys, Some(k));
                    assert_eq!(answer, want, "{what}");
                    assert_eq!(refusals, 1, "{what}");
                    refused += 1;
                }
            }
        }
    }
    assert!(refused > 0, "no call made a dependency reservation");
}

/// An Environment in `mode` whose pool (its limit set by [`room`]) holds
/// scene `dependency_reads`, loaded, and the pad `p.bundle`, read once so
/// that its run stays decoded in the LRU's handle, and then held by nothing
/// but the LRU: the dependencies' handles, which the caller holds.
fn full_pool(mode: LoadMode) -> (Environment, Arc<DepPool>, Vec<File>) {
    let d = dependency_reads();
    let pool = Arc::new(DepPool::new(u64::MAX));
    let s = Setup {
        mode,
        cache: 512 << 20,
        keep: false,
    };
    let env = batch_env(s, pool.clone(), &d.scene);
    let held = load(&env, &d).unwrap();
    let p = env
        .load_dependency(Source::Bytes {
            name: RawStr::from("p.bundle"),
            data: Bytes::from(bundle(&[("P.resS", res_bytes(128 << 10, 3))])),
        })
        .unwrap();
    let File::Bundle(pb) = &p else {
        panic!("p.bundle is a bundle")
    };
    assert_eq!(pb.entry_range(0, 0..4).unwrap().len(), 4);
    drop(p);
    (env, pool, held)
}

/// Leaves `pool` 16 bytes of room over what it holds.
fn room(pool: &DepPool) {
    let used = pool.used.used.load(Ordering::Acquire);
    pool.limit.store(used + 16, Ordering::Release);
}

/// A pool whose cached dependencies leave no room for a dereference or a
/// resource read from a dependency's file: each call evicts them and
/// succeeds, every time (review round 8: with `p.bundle`'s decoded pad in
/// the LRU and 16 bytes to spare, every call failed and nothing was
/// evicted). The dereferences and the reads each start on a full pool of
/// their own, so neither runs in the room the other made. The answers are
/// UnityPy's for the scene `dependency_reads`.
#[test]
fn a_full_pool_makes_room_for_a_read_from_a_dependency() {
    for (op, reads_them) in [("dep_derefs", false), ("dep_resources", true)] {
        let want = oracle("dependency_reads", op);
        for mode in [LoadMode::Eager, LoadMode::Lazy] {
            let (env, pool, held) = full_pool(mode);
            let (ptrs, res) = (pointers(&held), streams(&held));
            room(&pool);
            for round in 0..3 {
                let got = if reads_them {
                    Got::Reads(reads(&env, &res))
                } else {
                    Got::Targets(derefs(&env, &ptrs))
                };
                // The targets' names are read with the pool's room back.
                pool.limit.store(u64::MAX, Ordering::Release);
                let got = match got {
                    Got::Reads(r) => hex(&r),
                    Got::Targets(t) => targets(&t),
                    Got::Json(j) => j,
                };
                assert_eq!(got, want, "{op} {mode:?} round {round}");
                room(&pool);
            }
            assert!(env.stats().dependencies_evicted > 0, "{op} {mode:?}");
        }
    }
}

/// A call's eviction never drops the bundle the call reads through: with
/// the dependencies' handles let go once their pointers and resources are
/// read, so that only the LRU holds each bundle, a dereference or a
/// resource read the full pool refuses evicts every dependency, its own
/// file's among them, and its second run still reads through that bundle,
/// which the call holds while it runs. Each call runs on an Environment of
/// its own (the eviction drops the bundle once the call returns), and
/// answers as UnityPy does.
#[test]
fn a_retry_keeps_the_bundle_the_call_reads_through() {
    let want = (
        oracle("dependency_reads", "dep_derefs"),
        oracle("dependency_reads", "dep_resources"),
    );
    let want: Vec<J> = [want.0, want.1]
        .iter()
        .flat_map(|w| w.as_array().unwrap().clone())
        .collect();
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let mut evicted = 0;
        for (i, one) in want.iter().enumerate() {
            let (env, pool, held) = full_pool(mode);
            let (mut ptrs, mut res) = (pointers(&held), streams(&held));
            assert_eq!(ptrs.len() + res.len(), want.len());
            // The one call, its file's bundle held by nothing but the LRU.
            let n = ptrs.len();
            let ptr = (i < n).then(|| ptrs.swap_remove(i));
            let stream = (i >= n).then(|| res.swap_remove(i - n));
            drop((held, ptrs, res));
            room(&pool);
            let got = match (ptr, stream) {
                (Some(ptr), _) => targets(&derefs(&env, &[ptr])),
                (_, Some(stream)) => hex(&reads(&env, &[stream])),
                (None, None) => unreachable!(),
            };
            pool.limit.store(u64::MAX, Ordering::Release);
            assert_eq!(got, J::from(vec![one.clone()]), "{mode:?} call {i}");
            evicted += env.stats().dependencies_evicted;
        }
        assert!(evicted > 0, "{mode:?}");
    }
}

/// A fetcher that refuses the pool itself (as one that reserves its bytes
/// from the pool would) `refusals` times, then serves `data`, counting its
/// runs.
#[derive(Debug)]
struct RefusingFetch {
    refusals: AtomicU64,
    runs: AtomicU64,
    data: Bytes,
}

impl RefusingFetch {
    fn new(refusals: u64) -> Arc<Self> {
        Arc::new(Self {
            refusals: AtomicU64::new(refusals),
            runs: AtomicU64::new(0),
            data: Bytes::from(bundle(&[("CAB-Q.resS", res_bytes(8, 1))])),
        })
    }
}

impl Fetch for RefusingFetch {
    fn fetch(&self) -> Result<Bytes, Error> {
        self.runs.fetch_add(1, Ordering::AcqRel);
        let left = self
            .refusals
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
        if left.is_ok() {
            return Err(Error::Budget(BudgetExceeded {
                requested: 64,
                what: Reservation::Dependency,
            }));
        }
        Ok(self.data.clone())
    }
}

/// `load_dependency` of `fetch` on `env`: whether it loaded, and how many
/// times the fetcher ran.
fn fetch_load(env: &Environment, name: &str, fetch: &Arc<RefusingFetch>) -> (bool, u64) {
    let source = Source::Fetch {
        name: RawStr::from(name),
        fetch: fetch.clone(),
    };
    let loaded = env.load_dependency(source).is_ok();
    (loaded, fetch.runs.load(Ordering::Acquire))
}

/// A fetcher's own refusal of the pool is retried once whatever the thread
/// ran before: a load whose fetcher refuses once loads, alone and after a
/// load on the same thread and Environment whose fetcher always refuses
/// (review round 9: the failed load's mark stayed, the second refusal
/// moved no count and matched it, and the second load failed with its
/// fetcher run once). Each case runs on a thread of its own, so no earlier
/// test leaves anything on it.
#[test]
fn a_fetchers_refusal_is_retried_whatever_the_thread_ran_before() {
    let options = || {
        let mut o = EnvOptions::batch(ctx_with(Arc::new(RefuseOnce::default())));
        o.load = LoadMode::Lazy;
        o
    };
    let alone = std::thread::spawn(move || {
        let env = Environment::new(options());
        fetch_load(&env, "once.bundle", &RefusingFetch::new(1))
    });
    assert_eq!(alone.join().unwrap(), (true, 2), "alone");
    let after = std::thread::spawn(move || {
        let env = Environment::new(options());
        let failed = fetch_load(&env, "never.bundle", &RefusingFetch::new(u64::MAX));
        (
            failed,
            fetch_load(&env, "once.bundle", &RefusingFetch::new(1)),
        )
    });
    let (failed, once) = after.join().unwrap();
    assert_eq!(failed, (false, 2), "the load whose fetcher always refuses");
    assert_eq!(once, (true, 2), "after a failed load on the same thread");
}
