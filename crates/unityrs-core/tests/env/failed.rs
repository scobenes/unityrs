//! A dependency load that fails publishes nothing (plan section 3.13,
//! "Registry"; `docs/plans/batch.md` section 6, CR1): under
//! `LoadMode::Lazy` every registration of the load is charged, with the
//! LRU's entry and a `find_file` load's `files` entry, before the first is
//! published, so a load refused at any of them leaves the registry as it
//! was, returns every charge, and every lookup after it answers as before
//! the load. Under `Eager`, `EnvOptions::unitypy()`'s mode, a bundle's CABs
//! are registered as they are parsed, as UnityPy's `load_file` registers
//! them, so a failed load keeps those registered before its failure.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

use unityrs_core::budget::{
    BudgetExceeded, Counting, MemoryBudget, Permit, Reservation, Unbounded,
};
use unityrs_core::detect::{FileType, check_file_type};
use unityrs_core::env::{
    Cab, CabResolver, EnvOptions, Environment, LoadMode, PPtr, RawCab, Source,
};
use unityrs_core::files::{File, RawFile, SerializedFile, parse_file};
use unityrs_core::limits::Meter;
use unityrs_core::object::ObjectRef;
use unityrs_core::value::Value;
use unityrs_core::{Bytes, Error, FileContext, LimitKind, RawStr};
use unityrs_testkit::env::{Obj, bundle, res_bytes, serialized};
use unityrs_testkit::guard;

use crate::{ctx_with, key, keys, src, unlimited};

/// How a [`Pool`] treats the dependency reservations it is asked for
/// after it is armed.
#[derive(Clone, Copy, Debug)]
enum Arm {
    /// Refuses the `n`-th and every one after it: a load refused there is
    /// refused again in the retry `with_room` makes after it evicts the
    /// others, so the load fails.
    From(u64),
    /// Refuses the `n`-th alone: the retry succeeds.
    Only(u64),
    /// Refuses none.
    Grant,
}

/// The test's dependency pool: a [`Counting`] budget for the dependency
/// reservations, which it counts and, once armed, refuses as [`Arm`] says
/// (inputs' and providers' are granted, detached); it can record each
/// dependency reservation's running total.
#[derive(Debug)]
struct Pool {
    deps: Counting,
    /// The `n` of an `Arm::From` or `Arm::Only`; 0 grants every one.
    n: AtomicU64,
    only: AtomicBool,
    /// The dependency reservations asked for since it was armed.
    asked: AtomicU64,
    /// With `record`: what `deps` held when it was armed, and, for each
    /// dependency reservation, what it holds above that once the
    /// reservation is made.
    record: AtomicBool,
    base: AtomicU64,
    totals: Mutex<Vec<u64>>,
}

impl Default for Pool {
    fn default() -> Self {
        Self {
            deps: Counting::new(u64::MAX),
            n: AtomicU64::new(0),
            only: AtomicBool::new(false),
            asked: AtomicU64::new(0),
            record: AtomicBool::new(false),
            base: AtomicU64::new(0),
            totals: Mutex::new(Vec::new()),
        }
    }
}

impl Pool {
    fn arm(&self, arm: Arm) {
        self.asked.store(0, Ordering::Release);
        let (n, only) = match arm {
            Arm::From(n) => (n, false),
            Arm::Only(n) => (n, true),
            Arm::Grant => (0, false),
        };
        self.only.store(only, Ordering::Release);
        self.n.store(n, Ordering::Release);
    }

    fn disarm(&self) {
        self.n.store(0, Ordering::Release);
        self.record.store(false, Ordering::Release);
    }

    fn asked(&self) -> u64 {
        self.asked.load(Ordering::Acquire)
    }

    /// The dependency pool's bytes reserved and not returned.
    fn used(&self) -> u64 {
        self.deps.used()
    }

    /// Records each dependency reservation's running total from now on.
    fn record(&self) {
        self.totals.lock().unwrap().clear();
        self.base.store(self.deps.used(), Ordering::Release);
        self.record.store(true, Ordering::Release);
    }

    fn totals(&self) -> Vec<u64> {
        self.totals.lock().unwrap().clone()
    }
}

impl MemoryBudget for Pool {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        if what != Reservation::Dependency {
            return Ok(Permit::detached(bytes));
        }
        let asked = self.asked.fetch_add(1, Ordering::AcqRel) + 1;
        let n = self.n.load(Ordering::Acquire);
        let refused = n != 0
            && if self.only.load(Ordering::Acquire) {
                asked == n
            } else {
                asked >= n
            };
        if refused {
            return Err(BudgetExceeded {
                requested: bytes,
                what,
            });
        }
        if self.record.load(Ordering::Acquire) {
            let base = self.base.load(Ordering::Acquire);
            let held = self.deps.used().saturating_sub(base) + bytes;
            self.totals.lock().unwrap().push(held);
        }
        self.deps.try_reserve(bytes, what)
    }
}

/// `A`, a dependency loaded first: `CAB-S`, its `.resS`, and `CAB-A`.
fn a() -> Source {
    src(
        "a.bundle",
        &bundle(&[
            ("CAB-S", serialized(&[], &[Obj::named(1, "a-s", &[])])),
            ("CAB-S.resS", res_bytes(64, 1)),
            ("CAB-A", serialized(&[], &[Obj::named(1, "a-a", &[])])),
        ]),
    )
}

/// `B`'s entries, as many as its load registers.
const B_ENTRIES: u64 = 4;

/// `B`, the dependency whose load is refused: a new key, the two keys `A`
/// registered, and another new key, in that order, so a refusal at its
/// k-th entry comes after it staged (before CR1, published) a new key,
/// the value of a key `A` holds, or both.
fn b() -> Source {
    src(
        "b.bundle",
        &bundle(&[
            ("CAB-B", serialized(&[], &[Obj::named(1, "b-b", &[])])),
            ("CAB-S", serialized(&[], &[Obj::named(1, "b-s", &[])])),
            ("CAB-S.resS", res_bytes(64, 101)),
            ("CAB-B2", serialized(&[], &[Obj::named(1, "b-b2", &[])])),
        ]),
    )
}

/// `C`, a caller's file: its object points into `CAB-S`, `CAB-A` and
/// `CAB-B`, and its texture reads `CAB-S.resS`.
fn c() -> Source {
    src(
        "c.assets",
        &serialized(
            &[
                "archive:/CAB-S/CAB-S",
                "archive:/CAB-A/CAB-A",
                "archive:/CAB-B/CAB-B",
            ],
            &[
                Obj::named(1, "c1", &[(1, 1), (2, 1), (3, 1)]),
                Obj::streamed(2, "tex", "archive:/CAB-S/CAB-S.resS", 3, 5),
            ],
        ),
    )
}

/// Every name the scene registers, and a lower-case one.
const NAMES: [&str; 6] = ["CAB-S", "CAB-S.resS", "CAB-A", "CAB-B", "CAB-B2", "cab-s"];

fn m_name(o: &ObjectRef) -> String {
    let p = o.parse_as_dict(true).unwrap();
    let Some(Value::Str(name)) = p.get(&[b"m_Name"]) else {
        panic!("no name");
    };
    String::from_utf8_lossy(name.as_bytes()).into_owned()
}

fn hex(b: &[u8]) -> String {
    b.iter().fold(String::new(), |mut s, x| {
        let _ = write!(s, "{x:02x}");
        s
    })
}

/// A CAB as its first object's name, or its first bytes.
fn cab(found: Result<Option<Cab>, Error>) -> String {
    match found {
        Ok(Some(Cab::Serialized(sf))) => {
            format!("serialized {}", m_name(&sf.objects().next().unwrap()))
        }
        Ok(Some(Cab::Raw(RawCab::InBundle { bundle, dir_index }))) => {
            format!("raw {}", hex(&bundle.entry_bytes(dir_index).unwrap()[..8]))
        }
        Ok(Some(Cab::Raw(RawCab::File(raw)))) => format!("raw {}", hex(&raw.data[..8])),
        Ok(None) => "none".to_owned(),
        Err(e) => format!("error {e}"),
    }
}

/// A file as [`cab`] writes its CAB.
fn file(found: Result<Option<File>, Error>) -> String {
    match found {
        Ok(Some(File::Serialized(sf))) => cab(Ok(Some(Cab::Serialized(sf)))),
        Ok(Some(File::Raw(raw))) => cab(Ok(Some(Cab::Raw(RawCab::File(raw))))),
        Ok(Some(File::Bundle(b))) => format!("bundle {}", b.name()),
        Ok(Some(File::Web(_))) => "web".to_owned(),
        Ok(None) => "none".to_owned(),
        Err(e) => format!("error {e}"),
    }
}

/// What the lookups by name answer: the keys in order, and each name's
/// `get_cab` and `find_file`.
fn by_name(env: &Environment) -> Vec<String> {
    let m = unlimited();
    let mut out = vec![format!("keys {:?}", keys(env))];
    for name in NAMES {
        out.push(format!(
            "get_cab {name}: {}",
            cab(env.get_cab(&key(name), &m))
        ));
        let found = env.find_file(&key(name), true, &m);
        out.push(format!("find_file {name}: {}", file(found)));
    }
    out
}

/// What every lookup answers: [`by_name`]'s; each of `c`'s pointers
/// dereferenced; `c`'s texture's resource read; and the files listed.
fn answers(env: &Environment, c: &Arc<SerializedFile>) -> Vec<String> {
    let mut out = by_name(env);
    for file_id in 1..=3 {
        let got = env.deref(
            c,
            PPtr {
                file_id,
                path_id: 1,
            },
        );
        let got = got.map_or_else(|e| format!("error {e}"), |o| m_name(&o));
        out.push(format!("deref {file_id}: {got}"));
    }
    let read = env.resource(c, &RawStr::from("archive:/CAB-S/CAB-S.resS"), 3, 5);
    out.push(format!(
        "resource: {}",
        read.map_or_else(|e| format!("error {e}"), |b| hex(&b))
    ));
    let files = env.files().map(|f| f.name.to_string()).collect::<Vec<_>>();
    out.push(format!("files {files:?}"));
    out
}

/// A resolver that answers every name with `B` while it serves, and
/// nothing otherwise, so the lookups around the load the test makes
/// through it load nothing.
#[derive(Debug, Default)]
struct ServesB(AtomicBool);

impl CabResolver for ServesB {
    fn resolve(&self, _: &[u8]) -> Option<Source> {
        self.0.load(Ordering::Acquire).then(b)
    }
}

/// How the test loads `B`.
#[derive(Clone, Copy, Debug)]
enum Load {
    /// `load_dependency`.
    Dependency,
    /// `find_file("CAB-B", is_dependency=True)`, which the registry cannot
    /// answer, through the resolver: a resolver load of `try_load_dep`,
    /// listed in `files` under `keep_resolver_loads`.
    Resolved,
}

/// An Environment of `options` on a [`Pool`], with `A` and `C` loaded and
/// [`ServesB`] as its resolver, not serving.
fn scene(mut options: EnvOptions) -> (Environment, Arc<Pool>, Arc<ServesB>, Arc<SerializedFile>) {
    let pool = Arc::new(Pool::default());
    let resolver = Arc::new(ServesB::default());
    options.context = ctx_with(pool.clone());
    options.resolver = Some(resolver.clone());
    let env = Environment::new(options);
    drop(env.load_dependency(a()).unwrap());
    let File::Serialized(c) = env.load(c()).unwrap() else {
        panic!("C is a SerializedFile");
    };
    (env, pool, resolver, c)
}

/// The options of the sweeps: `batch()`'s with a cache that evicts and
/// with one that never does (and lists resolver loads), and `unitypy()`'s,
/// each in `mode` (each scene puts its own budget and resolver in them).
/// Whether each never evicts goes with it: then a pool refusal's retry
/// evicts nothing.
fn options(mode: LoadMode) -> Vec<(&'static str, EnvOptions, bool)> {
    let mut evicting = EnvOptions::batch(Arc::new(FileContext::unitypy()));
    evicting.load = mode;
    let mut keeping = evicting.clone();
    keeping.dep_cache_bytes = u64::MAX;
    keeping.keep_resolver_loads = true;
    let mut unitypy = EnvOptions::unitypy();
    unitypy.load = mode;
    vec![
        ("batch(), evicting", evicting, false),
        ("batch(), never evicting", keeping, true),
        ("unitypy()", unitypy, true),
    ]
}

/// What one armed load of `B` did.
struct Outcome {
    /// Whether it failed (with the pool's refusal).
    failed: bool,
    /// What every lookup answered before it and after it.
    before: Vec<String>,
    after: Vec<String>,
    /// The dependency reservations it asked for.
    asked: u64,
    /// The dependency pool's bytes just before it and just after it.
    used_before: u64,
    used_after: u64,
    /// The dependencies it evicted (a pool refusal's retry evicts `A` when
    /// the cache evicts).
    evicted: u64,
}

/// Loads `B` as `load` says, on a fresh scene, with the pool armed as
/// `arm` says.
fn armed(options: &EnvOptions, load: Load, arm: Arm) -> Outcome {
    let (env, pool, resolver, c) = scene(options.clone());
    let before = answers(&env, &c);
    let used_before = pool.used();
    let evicted = env.stats().dependencies_evicted;
    pool.arm(arm);
    let loaded = match load {
        Load::Dependency => env.load_dependency(b()).map(Some),
        Load::Resolved => {
            resolver.0.store(true, Ordering::Release);
            let found = env.find_file(&key("CAB-B"), true, &unlimited());
            resolver.0.store(false, Ordering::Release);
            found
        }
    };
    pool.disarm();
    let asked = pool.asked();
    let failed = match loaded {
        Ok(found) => {
            assert!(found.is_some(), "{load:?}: B was not found");
            false
        }
        Err(e) => {
            assert!(pool_refused(&e), "{arm:?}: not the pool's refusal: {e}");
            true
        }
    };
    let used_after = pool.used();
    let evicted = env.stats().dependencies_evicted - evicted;
    Outcome {
        failed,
        before,
        after: answers(&env, &c),
        asked,
        used_before,
        used_after,
        evicted,
    }
}

/// Whether `e` is the pool's refusal, as it is or as an entry's error
/// (`Eager` parses every entry at load).
fn pool_refused(e: &Error) -> bool {
    match e {
        Error::Budget(b) => b.what == Reservation::Dependency,
        Error::Entry { source, .. } => pool_refused(source),
        _ => false,
    }
}

/// The dependency reservations a bare open of `B` makes: `B` parsed on a
/// dependency's meter, as a load opens it, and nothing registered.
fn open_reservations() -> u64 {
    let pool = Arc::new(Pool::default());
    let ctx = ctx_with(pool.clone());
    let meter = Meter::for_input(&ctx.limits, pool.clone(), Reservation::Dependency);
    let Source::Bytes { name, data } = b() else {
        panic!("B is bytes");
    };
    let ty = check_file_type(&data, &ctx.limits).unwrap();
    pool.arm(Arm::Grant);
    let opened = parse_file(name, data, ty, None, &meter, &ctx).unwrap();
    let asked = pool.asked();
    drop(opened);
    asked
}

/// CR1's test: `B`'s load is refused at each of its dependency
/// reservations in turn (the open's; then the list of staged
/// registrations, reserved once for the whole directory before any entry;
/// then each entry's: its lowered key, its slot, a new key's share then its
/// copy, its position's path; then the LRU's entry, and a listed resolver
/// load's `files` entry), after `A` registered two keys of the same names,
/// so the registration is refused at its k-th entry for every k, and at
/// each charge of that entry. Under `Lazy`, after every failed load, every
/// lookup answers as before it, and the pool holds what it held before it
/// (with a cache that evicts, what it holds after the load refused at its
/// first reservation: the retry evicted `A` alike), under `batch()`'s
/// options with a cache that evicts `A` for the retry and one that never
/// does, and under `unitypy()`'s options made lazy, for a load by
/// `load_dependency` and one through the resolver. The load that is
/// refused nowhere changes the answers, so the sweep tells the two apart,
/// and its registrations ask for at least four reservations per entry and
/// two more beyond a bare open's (CR1's review, round 1: a stage charged to
/// another meter passed the sweep); under `keep_resolver_loads`, the load
/// through the resolver asks for at least three more than
/// `load_dependency`'s, its listing's: its key's copy (the bytes, then
/// their owner) and its share and lowered key, at least one reservation
/// (CR1's review, round 2: a listing charged to another meter passed the
/// sweep).
#[test]
fn a_failed_load_leaves_every_lookup_as_it_was() {
    let open = open_reservations();
    for (what, options, keeps) in options(LoadMode::Lazy) {
        let mut whole = [0; 2];
        for (i, load) in [Load::Dependency, Load::Resolved].into_iter().enumerate() {
            let first = armed(&options, load, Arm::From(1));
            assert!(first.failed, "{what}, {load:?}: refused at its open");
            for n in 1.. {
                let o = armed(&options, load, Arm::From(n));
                if !o.failed {
                    // Refused at each of its reservations in turn, it failed
                    // each time.
                    assert_eq!(o.asked, n - 1, "{what}, {load:?}: passed reservation {n}");
                    assert_ne!(o.after, o.before, "{what}, {load:?}: it changed nothing");
                    let registered = o.asked - open;
                    assert!(
                        registered >= 4 * B_ENTRIES + 2,
                        "{what}, {load:?}: {registered} reservations beyond the open's {open}"
                    );
                    whole[i] = o.asked;
                    break;
                }
                assert_eq!(
                    o.after, o.before,
                    "{what}, {load:?}: refused at reservation {n}"
                );
                let returned = if keeps {
                    o.used_before
                } else {
                    first.used_after
                };
                assert_eq!(
                    o.used_after, returned,
                    "{what}, {load:?}: refused at reservation {n}, it kept a charge"
                );
            }
        }
        if options.keep_resolver_loads {
            let [by_dependency, resolved] = whole;
            assert!(
                resolved >= by_dependency + 3,
                "{what}: the resolved load asked for {resolved}, load_dependency {by_dependency}"
            );
        }
    }
}

/// A load refused at one reservation alone succeeds. Where the refusal
/// fails its first run (at the open, an entry's charge, the stage's list,
/// the LRU's entry or a listing), the run is retried once, after the
/// evicting cache evicted `A`, and ends as the load refused at its first
/// reservation does: the same answers and the same bytes held, so the
/// refused run left nothing, no key at a lower load number among them
/// (CR1's review, round 1). Where the open absorbs it (a buffer's growth
/// refused, then made at the size it needs), the run goes on, evicts
/// nothing, and ends as the load refused nowhere does.
#[test]
fn a_load_refused_once_ends_as_a_retried_load_or_as_one_refused_nowhere() {
    for load in [Load::Dependency, Load::Resolved] {
        for (what, options, keeps) in options(LoadMode::Lazy) {
            let granted = armed(&options, load, Arm::Grant);
            let reference = armed(&options, load, Arm::Only(1));
            assert!(!reference.failed, "{what}, {load:?}: the retry succeeds");
            let total = granted.asked;
            if keeps {
                assert_eq!(reference.after, granted.after, "{what}, {load:?}");
                assert_eq!(reference.used_after, granted.used_after, "{what}");
            }
            let mut retried = 0;
            for n in 1..=total {
                let o = armed(&options, load, Arm::Only(n));
                assert!(!o.failed, "{what}, {load:?}: refused at {n}, it failed");
                // A failed first run stopped at `n`, and a whole run
                // followed it.
                let retry = o.asked == n + total;
                if !keeps {
                    assert_eq!(o.evicted > 0, retry, "{what}, {load:?}: refused at {n}");
                }
                let like = if retry {
                    retried += 1;
                    &reference
                } else {
                    assert!(o.asked > total, "{what}, {load:?}: {n} not asked for");
                    &granted
                };
                assert_eq!(o.after, like.after, "{what}, {load:?}: refused at {n}");
                assert_eq!(o.used_after, like.used_after, "{what}, {load:?}: at {n}");
            }
            assert!(
                retried > 4 * B_ENTRIES,
                "{what}, {load:?}: {retried} retried"
            );
        }
    }
}

/// Under `Eager`, `unitypy()`'s mode, a failed load keeps the CABs it
/// registered before its failure, as UnityPy's `load_file` keeps those it
/// parsed before a failing entry (plan section 3.13, partial
/// registration): CR1 leaves `unitypy()` as it was. The same sweep finds a
/// refusal after which `CAB-B`, `B`'s first entry, is registered.
#[test]
fn under_eager_a_failed_load_keeps_what_it_registered() {
    for (what, options, _) in options(LoadMode::Eager) {
        let mut kept = 0;
        for n in 1.. {
            let o = armed(&options, Load::Dependency, Arm::From(n));
            if !o.failed {
                break;
            }
            if o.after != o.before {
                assert!(o.after[0].contains("cab-b"), "{what}: {:?}", o.after);
                kept += 1;
            }
        }
        assert_ne!(kept, 0, "{what}: no failed load kept a CAB");
    }
}

/// `D`: a container `INNER`, a bundle holding `CAB-N`, then a raw entry of
/// the same path, which drops it unreached when the load publishes it.
fn d() -> Source {
    src(
        "d.bundle",
        &bundle(&[
            (
                "INNER",
                bundle(&[("CAB-N", serialized(&[], &[Obj::named(1, "n", &[])]))]),
            ),
            ("INNER", res_bytes(4, 50)),
        ]),
    )
}

/// The commit of `D`'s load drops its container `INNER` unreached, and its
/// `settle` reaches it, charging the pool; a refusal there does not fail
/// the load (CR1's review, round 1: the docs said nothing after the
/// commit's start could be refused). With the pool refusing from the
/// settle's first reservation on, the load succeeds with every key
/// published and `INNER`'s entries not yet registered, a lookup while the
/// pool still refuses leaves them so, and the first lookup after it
/// reaches `INNER` and registers them.
#[test]
fn a_refusal_in_the_commits_settle_fails_no_load_and_a_later_call_reaches_the_slot() {
    for (what, options, _) in options(LoadMode::Lazy) {
        let mut reached = false;
        for n in 1.. {
            let pool = Arc::new(Pool::default());
            let mut options = options.clone();
            options.context = ctx_with(pool.clone());
            let env = Environment::new(options);
            pool.arm(Arm::From(n));
            let loaded = env.load_dependency(d());
            if let Err(e) = loaded {
                // Refused before the commit: nothing published.
                assert!(pool_refused(&e), "{what}: {e}");
                assert!(keys(&env).is_empty(), "{what}: refused at {n}");
                continue;
            }
            assert!(
                pool.asked() >= n,
                "{what}: the settle reserved nothing, so nothing was refused"
            );
            assert_eq!(keys(&env), ["inner"], "{what}: INNER not reached");
            pool.disarm();
            assert_eq!(keys(&env), ["inner", "cab-n"], "{what}: INNER reached");
            let got = cab(env.get_cab(&key("CAB-N"), &unlimited()));
            assert_eq!(got, "serialized n", "{what}");
            reached = true;
            break;
        }
        assert!(reached, "{what}");
    }
}

/// `D`'s load refused nowhere: its commit's `settle` reaches `INNER`,
/// charging the pool after the first key is published, and once the
/// Environment and the load's file are dropped the pool holds nothing, so
/// every charge the load made was returned, the settle's among them (CR1's
/// review, round 2). The settle reaches through the load's open, not the
/// LRU, so it never caches the dependency before the load's own touch: the
/// share that touch finds unused, which a share forgotten instead of
/// returned kept, is `env::registry::lru::tests`'s
/// `a_paid_touch_that_finds_its_entry_cached_reads_it_without_the_share`.
#[test]
fn a_load_whose_commit_reaches_a_dropped_container_returns_every_charge() {
    for (what, options, _) in options(LoadMode::Lazy) {
        let pool = Arc::new(Pool::default());
        let mut options = options.clone();
        options.context = ctx_with(pool.clone());
        let env = Environment::new(options);
        let file = env.load_dependency(d()).unwrap();
        assert_eq!(keys(&env), ["inner", "cab-n"], "{what}: INNER reached");
        assert!(pool.used() > 0, "{what}: nothing charged to the pool");
        drop(file);
        drop(env);
        assert_eq!(pool.used(), 0, "{what}: a charge was kept");
    }
}

/// An Environment like [`scene`]'s whose inputs' meters are bounded at
/// `limit` bytes, with `A`'s CABs registered by name on an unlimited meter
/// (so the bound is `B`'s alone) and no `C`.
fn limited(options: &EnvOptions, limit: u64) -> (Environment, Arc<Pool>) {
    let pool = Arc::new(Pool::default());
    let mut ctx = FileContext::unitypy();
    ctx.budget = pool.clone();
    ctx.limits.max_input_memory = limit;
    let ctx = Arc::new(ctx);
    let mut options = options.clone();
    options.context = ctx.clone();
    let env = Environment::new(options);
    let m = unlimited();
    let open = |name: &str, data: Vec<u8>| {
        let data = Bytes::from(data);
        let ty = check_file_type(&data, &ctx.limits).unwrap();
        parse_file(RawStr::from(name), data, ty, None, &m, &ctx).unwrap()
    };
    for (name, object) in [("CAB-S", "a-s"), ("CAB-A", "a-a")] {
        let File::Serialized(sf) = open(name, serialized(&[], &[Obj::named(1, object, &[])]))
        else {
            panic!("{name} is a SerializedFile");
        };
        env.registry()
            .register(&key(name), Cab::Serialized(sf), &m)
            .unwrap();
    }
    let raw = RawFile::new(
        RawStr::from("CAB-S.resS"),
        Bytes::from(res_bytes(64, 1)),
        FileType::ResourceFile,
        &m,
    )
    .unwrap();
    env.registry()
        .register(&key("CAB-S.resS"), Cab::Raw(RawCab::File(raw)), &m)
        .unwrap();
    (env, pool)
}

/// A load its own meter refuses (`LimitKind::InputMemory`, which no retry
/// follows) leaves every lookup by name as it was and returns every
/// charge, refused at each charge whose running total is a new high: the
/// bound is set, in turn, one byte below what `B`'s meter holds at each
/// such reservation of a load refused nowhere, so the first reservation it
/// refuses is that one. A charge whose total is not a new high comes after
/// a greater one, which a bound below it refuses first; the pool's sweeps
/// above refuse those too (CR1's review, round 1: only the pool's refusals
/// were swept; round 2: these docs said each charge was).
#[test]
fn a_load_its_meter_refuses_at_any_charge_leaves_every_lookup_as_it_was() {
    let open = open_reservations();
    for (what, options, _) in options(LoadMode::Lazy) {
        let (env, pool) = limited(&options, u64::MAX);
        pool.record();
        drop(env.load_dependency(b()).unwrap());
        pool.disarm();
        // Each new high, and the reservation it is, from 1.
        let mut highs = Vec::new();
        for (i, total) in pool.totals().into_iter().enumerate() {
            if total > highs.last().map_or(0, |&(_, high)| high) {
                highs.push((i as u64 + 1, total));
            }
        }
        // The reservations targeted after the open's: more than four an
        // entry.
        let staged = highs.iter().filter(|&&(at, _)| at > open).count();
        assert!(staged as u64 > 4 * B_ENTRIES, "{what}: {highs:?}");
        for (_, total) in highs {
            let (env, pool) = limited(&options, total - 1);
            let before = by_name(&env);
            let used = pool.used();
            let e = env.load_dependency(b()).unwrap_err();
            assert!(
                matches!(&e, Error::Limit(l) if l.kind == LimitKind::InputMemory),
                "{what}, bound {}: {e}",
                total - 1
            );
            assert_eq!(pool.used(), used, "{what}, bound {}", total - 1);
            assert_eq!(by_name(&env), before, "{what}, bound {}", total - 1);
        }
    }
}

/// `B`'s names: two new keys and the two `A` holds, whose values `B`
/// replaces.
const B_NAMES: [&str; 4] = ["CAB-B", "CAB-S", "CAB-S.resS", "CAB-B2"];

/// What another thread finds during `B`'s load: the registry's keys, and
/// each of `B`'s names' `get_cab`.
fn probed(env: &Environment) -> Vec<String> {
    let mut out = vec![format!("keys {:?}", keys(env))];
    for name in B_NAMES {
        let found = cab(env.get_cab(&key(name), &unlimited()));
        out.push(format!("get_cab {name}: {found}"));
    }
    out
}

/// A budget that, at each dependency reservation the loading thread makes,
/// has another thread look up what [`probed`] does, and keeps what it saw:
/// lookups made while the load runs, between any two of its charges and
/// after the last one before its commit.
#[derive(Default)]
struct Probe {
    env: Mutex<Option<Environment>>,
    loader: Mutex<Option<ThreadId>>,
    seen: Mutex<Vec<Vec<String>>>,
}

impl std::fmt::Debug for Probe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Probe")
    }
}

impl MemoryBudget for Probe {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        let loading = *self.loader.lock().unwrap() == Some(std::thread::current().id());
        if what == Reservation::Dependency && loading {
            let env = self.env.lock().unwrap().clone();
            if let Some(env) = env {
                let seen =
                    guard::finishes("a lookup on another thread during the load", move || {
                        probed(&env)
                    });
                self.seen.lock().unwrap().push(seen);
            }
        }
        Unbounded.try_reserve(bytes, what)
    }
}

/// While `B`'s load runs, no lookup on another thread finds one of its
/// keys: at each of its dependency reservations, the last of them the
/// LRU's entry just before the commit, another thread's key listing, and
/// its `get_cab` of each of `B`'s names, two of them keys `A` holds whose
/// values `B` replaces, are the ones before the load (plan section 3.16,
/// item 6; CR1's review, round 2: the probe saw the keys, not the values).
#[test]
fn no_lookup_on_another_thread_finds_a_key_of_a_load_before_its_commit() {
    for (what, mut options, _) in options(LoadMode::Lazy) {
        let probe = Arc::new(Probe::default());
        options.context = ctx_with(probe.clone());
        let env = Environment::new(options);
        drop(env.load_dependency(a()).unwrap());
        // `A`'s slots reached, so the commit's `settle` has none to reach
        // (it would reserve after the first key is published).
        for name in NAMES {
            drop(env.get_cab(&key(name), &unlimited()).unwrap());
        }
        let before = probed(&env);
        *probe.env.lock().unwrap() = Some(env.clone());
        *probe.loader.lock().unwrap() = Some(std::thread::current().id());
        drop(env.load_dependency(b()).unwrap());
        *probe.loader.lock().unwrap() = None;
        *probe.env.lock().unwrap() = None;
        let seen = std::mem::take(&mut *probe.seen.lock().unwrap());
        assert!(
            seen.len() as u64 > 4 * B_ENTRIES,
            "{what}: {} probes",
            seen.len()
        );
        for (i, found) in seen.iter().enumerate() {
            assert_eq!(found, &before, "{what}: at reservation {}", i + 1);
        }
        let after = probed(&env);
        assert_ne!(after[0], before[0], "{what}: the load published its keys");
        assert_ne!(after[2], before[2], "{what}: the load replaced CAB-S");
    }
}
