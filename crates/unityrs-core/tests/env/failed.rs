//! A dependency load that fails publishes nothing (plan section 3.13,
//! "Dependencies"; `docs/plans/batch.md` section 6, CR1): under
//! `LoadMode::Lazy` every registration of the load is charged, with the
//! LRU's entry, before the first is published, so a load refused at any of
//! them leaves the registry as it was, and every lookup after it answers as
//! before the load. Under `Eager`, `EnvOptions::unitypy()`'s mode, a
//! bundle's CABs are registered as they are parsed, as UnityPy's
//! `load_file` registers them, so a failed load keeps those registered
//! before its failure.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation};
use unityrs_core::env::{Cab, EnvOptions, Environment, LoadMode, PPtr, RawCab, Source};
use unityrs_core::files::{File, SerializedFile};
use unityrs_core::object::ObjectRef;
use unityrs_core::value::Value;
use unityrs_core::{Error, FileContext, RawStr};
use unityrs_testkit::env::{Obj, bundle, res_bytes, serialized};

use crate::{ctx_with, key, keys, src, unlimited};

/// A budget that grants every reservation but, once armed with `n`, the
/// `n`-th dependency reservation after it and every one after that, until
/// it is disarmed: a load refused there is refused again in its retry,
/// which `with_room` makes after it evicts the others, so the load fails.
#[derive(Debug, Default)]
struct RefuseFrom {
    /// 0 when disarmed.
    from: AtomicU64,
    /// The dependency reservations asked for since it was armed.
    asked: AtomicU64,
}

impl RefuseFrom {
    fn arm(&self, n: u64) {
        self.asked.store(0, Ordering::Release);
        self.from.store(n, Ordering::Release);
    }

    fn disarm(&self) {
        self.from.store(0, Ordering::Release);
    }

    fn asked(&self) -> u64 {
        self.asked.load(Ordering::Acquire)
    }
}

impl MemoryBudget for RefuseFrom {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        let from = self.from.load(Ordering::Acquire);
        if what == Reservation::Dependency && from != 0 {
            let asked = self.asked.fetch_add(1, Ordering::AcqRel) + 1;
            if asked >= from {
                return Err(BudgetExceeded {
                    requested: bytes,
                    what,
                });
            }
        }
        Ok(Permit::detached(bytes))
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

/// `B`, the dependency whose load is refused: a new key, the two keys `A`
/// registered, and another new key, in that order, so a refusal at its
/// k-th entry comes after it registered (before CR1, published) a new key,
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

/// What every lookup answers: the keys in order; each name's `get_cab` and
/// `find_file`; each of `c`'s pointers dereferenced; `c`'s texture's
/// resource read; and the files listed.
fn answers(env: &Environment, c: &Arc<SerializedFile>) -> Vec<String> {
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

/// An Environment of `options` on a [`RefuseFrom`] budget, with `A` and
/// `C` loaded: the budget, `C`, and what every lookup answers.
fn scene(mut options: EnvOptions) -> (Environment, Arc<RefuseFrom>, Arc<SerializedFile>) {
    let budget = Arc::new(RefuseFrom::default());
    options.context = ctx_with(budget.clone());
    let env = Environment::new(options);
    drop(env.load_dependency(a()).unwrap());
    let File::Serialized(c) = env.load(c()).unwrap() else {
        panic!("C is a SerializedFile");
    };
    (env, budget, c)
}

/// The options of the sweep: `batch()`'s with a cache that evicts and
/// with one that never does, and `unitypy()`'s, each in `mode` (each
/// scene puts its own budget in the context).
fn options(mode: LoadMode) -> Vec<(&'static str, EnvOptions)> {
    let mut evicting = EnvOptions::batch(Arc::new(FileContext::unitypy()));
    evicting.load = mode;
    let mut keeping = evicting.clone();
    keeping.dep_cache_bytes = u64::MAX;
    keeping.keep_resolver_loads = true;
    let mut unitypy = EnvOptions::unitypy();
    unitypy.load = mode;
    vec![
        ("batch(), evicting", evicting),
        ("batch(), never evicting", keeping),
        ("unitypy()", unitypy),
    ]
}

/// Loads `B` with its `n`-th dependency reservation and every later one
/// refused, on a fresh scene: whether the load failed, with a refusal of
/// the pool, what every lookup answered before it and after it, and the
/// reservations it asked for.
fn refused_at(options: &EnvOptions, n: u64) -> (bool, Vec<String>, Vec<String>, u64) {
    let (env, budget, c) = scene(options.clone());
    let before = answers(&env, &c);
    budget.arm(n);
    let loaded = env.load_dependency(b());
    budget.disarm();
    let asked = budget.asked();
    let failed = match loaded {
        Ok(_) => false,
        Err(e) => {
            assert!(
                pool_refused(&e),
                "refused at {n}: not the pool's refusal: {e}"
            );
            true
        }
    };
    (failed, before, answers(&env, &c), asked)
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

/// CR1's test: `B`'s load is refused at each of its dependency
/// reservations in turn (the open's, then each entry's: its key, its slot,
/// a new key's copy and share, its position's path, the stage's list; then
/// the LRU's entry), after `A` registered two keys of the same names, so
/// the registration is refused at its k-th entry for every k, and at each
/// charge of that entry. Under `Lazy` every lookup after the failed load
/// answers as before it, under `batch()`'s options with a cache that
/// evicts `A` for the retry and one that never does, and under
/// `unitypy()`'s options made lazy. The load that is refused nowhere
/// changes the answers, so the sweep tells the two apart.
#[test]
fn a_failed_load_leaves_every_lookup_as_it_was() {
    for (what, options) in options(LoadMode::Lazy) {
        let mut failures = 0;
        for n in 1.. {
            let (failed, before, after, asked) = refused_at(&options, n);
            if !failed {
                assert!(asked < n, "{what}: the load passed reservation {n}");
                assert_ne!(after, before, "{what}: the load changed nothing");
                break;
            }
            failures += 1;
            assert_eq!(after, before, "{what}: refused at reservation {n}");
        }
        // The open's reservations, and at least four for each entry.
        assert!(failures > 16, "{what}: only {failures} reservations");
    }
}

/// Under `Eager`, `unitypy()`'s mode, a failed load keeps the CABs it
/// registered before its failure, as UnityPy's `load_file` keeps those it
/// parsed before a failing entry (plan section 3.13, partial
/// registration): CR1 leaves `unitypy()` as it was. The same sweep finds a
/// refusal after which `CAB-B`, `B`'s first entry, is registered.
#[test]
fn under_eager_a_failed_load_keeps_what_it_registered() {
    for (what, options) in options(LoadMode::Eager) {
        let mut kept = 0;
        for n in 1.. {
            let (failed, before, after, _) = refused_at(&options, n);
            if !failed {
                break;
            }
            if after != before {
                assert!(after[0].contains("cab-b"), "{what}: {after:?}");
                kept += 1;
            }
        }
        assert_ne!(kept, 0, "{what}: no failed load kept a CAB");
    }
}
