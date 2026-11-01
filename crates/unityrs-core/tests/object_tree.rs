//! An object's tree (plan sections 3.12 and 7.5): UnityPy's order
//! (`files/ObjectReader.py:285-307`), `nodes=` over the type record's own
//! tree over the providers', and the `MonoBehaviour` script hook
//! [`Environment::object_tree`] runs for a provider-chain tree only, with a
//! mock script provider standing for UnityPy's `typetree_generator`.

// Test code: a failed lookup is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation};
use unityrs_core::env::{Cab, DerefError, EnvOptions, Environment, LoadMode, Source};
use unityrs_core::files::{File, SerializedFile};
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::provider::{
    ProviderChain, ProviderError, TreeRequest, TreeSource, TypeTreeProvider,
};
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{ParseError, WriteError, WriteRules};
use unityrs_core::{Bytes, FileContext, ObjectRef, RawStr, Value};
use unityrs_testkit::ByteWriter;
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::env::bundle;
use unityrs_testkit::serialized::{ExternalSpec, ObjectSpec, SerializedSpec, TypeSpec, build};

const ALIGN: i64 = 0x4000;

fn string_rows(level: i32, name: &str) -> Vec<NodeRow> {
    vec![
        NodeRow::new(level, "string", name),
        NodeRow::new(level + 1, "Array", "Array").with_meta_flag(ALIGN),
        NodeRow::new(level + 2, "int", "size"),
        NodeRow::new(level + 2, "char", "data"),
    ]
}

fn pptr_rows(level: i32, name: &str) -> Vec<NodeRow> {
    vec![
        NodeRow::new(level, "PPtr<MonoScript>", name),
        NodeRow::new(level + 1, "int", "m_FileID"),
        NodeRow::new(level + 1, "SInt64", "m_PathID"),
    ]
}

fn tree(rows: Vec<NodeRow>) -> Arc<Tree> {
    Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap()
}

/// The class tree of `MonoBehaviour`: `m_Script`, `m_Name`.
fn behaviour_rows() -> Vec<NodeRow> {
    let mut r = vec![NodeRow::new(0, "MonoBehaviour", "Base")];
    r.extend(pptr_rows(1, "m_Script"));
    r.extend(string_rows(1, "m_Name"));
    r
}

/// The script's tree: the class's fields and `m_Value`.
fn script_rows() -> Vec<NodeRow> {
    let mut r = behaviour_rows();
    r.push(NodeRow::new(1, "int", "m_Value"));
    r
}

fn monoscript_rows() -> Vec<NodeRow> {
    let mut r = vec![NodeRow::new(0, "MonoScript", "Base")];
    r.extend(string_rows(1, "m_ClassName"));
    r.extend(string_rows(1, "m_Namespace"));
    r.extend(string_rows(1, "m_AssemblyName"));
    r
}

/// The class trees, as the TPK would answer them: it never sees a script.
#[derive(Debug)]
struct Classes {
    behaviour: Arc<Tree>,
    monoscript: Arc<Tree>,
}

impl TypeTreeProvider for Classes {
    fn name(&self) -> &'static str {
        "tpk"
    }

    fn tree(&self, req: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
        assert!(
            req.script.is_none(),
            "the class provider is never asked for a script"
        );
        Ok(match req.class_id {
            114 => Some(self.behaviour.clone()),
            115 => Some(self.monoscript.clone()),
            _ => None,
        })
    }
}

/// UnityPy's `typetree_generator`: a tree for the script `Game.Foo` in
/// `Game.dll`, and nothing for any other.
#[derive(Debug)]
struct Scripts {
    tree: Arc<Tree>,
    asked: AtomicUsize,
}

impl TypeTreeProvider for Scripts {
    fn name(&self) -> &'static str {
        "script"
    }

    fn tree(&self, req: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
        let Some(s) = req.script else {
            return Ok(None);
        };
        self.asked.fetch_add(1, Ordering::Relaxed);
        let hit = s.assembly.as_bytes() == b"Game.dll"
            && s.namespace.as_bytes() == b"Game"
            && s.class.as_bytes() == b"Foo";
        Ok(hit.then(|| self.tree.clone()))
    }

    fn accepts_scripts(&self) -> bool {
        true
    }
}

fn put_string(w: &mut ByteWriter, s: &str) {
    w.i32(i32::try_from(s.len()).unwrap())
        .bytes(s.as_bytes())
        .align(4);
}

/// A behaviour's bytes: its script pointer, its name, and `m_Value` 42.
fn behaviour(script: (i32, i64)) -> Vec<u8> {
    let mut w = ByteWriter::le();
    w.i32(script.0).i64(script.1);
    put_string(&mut w, "b");
    w.i32(42);
    w.into_inner()
}

fn monoscript(class: &str) -> Vec<u8> {
    let mut w = ByteWriter::le();
    put_string(&mut w, class);
    put_string(&mut w, "Game");
    put_string(&mut w, "Game.dll");
    w.into_inner()
}

/// A file of behaviours (path ids 1..) pointing at `scripts`, with the
/// `MonoScript`s `Foo` (path id 100) and `Bar` (101); type trees stripped
/// unless `embedded`.
fn file(scripts: &[(i32, i64)], embedded: bool) -> Vec<u8> {
    let node_specs = |rows: Vec<NodeRow>| -> Vec<NodeSpec> {
        rows.iter()
            .map(|r| {
                let mut n = NodeSpec::new(
                    u8::try_from(r.level).unwrap(),
                    &String::from_utf8_lossy(r.ty.as_bytes()),
                    &String::from_utf8_lossy(r.name.as_bytes()),
                );
                n.meta_flag = i32::try_from(r.meta_flag.unwrap_or(0)).unwrap();
                n
            })
            .collect()
    };
    let types = vec![
        TypeSpec {
            class_id: 114,
            nodes: if embedded {
                node_specs(behaviour_rows())
            } else {
                Vec::new()
            },
            ..TypeSpec::default()
        },
        TypeSpec {
            class_id: 115,
            nodes: if embedded {
                node_specs(monoscript_rows())
            } else {
                Vec::new()
            },
            ..TypeSpec::default()
        },
    ];
    let mut objects: Vec<ObjectSpec> = scripts
        .iter()
        .enumerate()
        .map(|(i, s)| ObjectSpec {
            path_id: i64::try_from(i).unwrap() + 1,
            type_id: 0,
            class_id: 114,
            data: behaviour(*s),
            ..ObjectSpec::default()
        })
        .collect();
    for (path_id, class) in [(100, "Foo"), (101, "Bar")] {
        objects.push(ObjectSpec {
            path_id,
            type_id: 1,
            class_id: 115,
            data: monoscript(class),
            ..ObjectSpec::default()
        });
    }
    build(&SerializedSpec {
        type_trees: embedded,
        types,
        objects,
        externals: vec![ExternalSpec {
            path: b"archive:/CAB-GONE/CAB-GONE".to_vec(),
            ..ExternalSpec::default()
        }],
        ..SerializedSpec::default()
    })
}

struct Setup {
    env: Environment,
    file: Arc<SerializedFile>,
    scripts: Arc<Scripts>,
}

/// The class provider, and the script provider `scripts` after it when
/// `with_scripts`.
fn chain(scripts: &Arc<Scripts>, with_scripts: bool) -> Arc<ProviderChain> {
    let mut providers: Vec<Arc<dyn TypeTreeProvider>> = vec![Arc::new(Classes {
        behaviour: tree(behaviour_rows()),
        monoscript: tree(monoscript_rows()),
    })];
    if with_scripts {
        providers.push(scripts.clone());
    }
    Arc::new(ProviderChain::new(providers))
}

fn setup(pointers: &[(i32, i64)], embedded: bool, with_scripts: bool) -> Setup {
    let scripts = Arc::new(Scripts {
        tree: tree(script_rows()),
        asked: AtomicUsize::new(0),
    });
    let mut ctx = FileContext::unitypy();
    ctx.providers = chain(&scripts, with_scripts);
    let mut o = EnvOptions::unitypy();
    o.context = Arc::new(ctx);
    let env = Environment::new(o);
    env.load(Source::Bytes {
        name: RawStr::from("CAB-T"),
        data: Bytes::from(file(pointers, embedded)),
    })
    .unwrap();
    let Some(Cab::Serialized(file)) = env
        .get_cab(&RawStr::from("cab-t"), &Meter::unlimited())
        .unwrap()
    else {
        panic!()
    };
    Setup { env, file, scripts }
}

/// A value to hand the writer: object 1 of `s`, read with its tree.
fn stripped_value(s: &Setup) -> unityrs_core::Parsed {
    obj(s, 1).read_typetree(None, false).unwrap()
}

fn obj(s: &Setup, path_id: i64) -> ObjectRef {
    s.file.object(path_id).unwrap()
}

fn value_of(o: &ObjectRef, tree: &Arc<Tree>) -> Option<i64> {
    let p = o.read_typetree(Some(tree), true).ok()?;
    p.get(&[b"m_Value"]).and_then(Value::as_i64)
}

/// `nodes=` wins, then the type record's tree, then the providers'; with
/// neither, the object has no tree and its read is `NoTypeTree`.
#[test]
fn trees_resolve_in_unitypys_order() {
    let embedded = setup(&[(0, 100)], true, false);
    let o = obj(&embedded, 1);
    assert_eq!(o.tree(None).unwrap().unwrap().source, TreeSource::Embedded);
    let explicit = tree(script_rows());
    let t = o.tree(Some(&explicit)).unwrap().unwrap();
    assert_eq!(t.source, TreeSource::Explicit);
    assert!(Arc::ptr_eq(&t.tree, &explicit));

    let stripped = setup(&[(0, 100)], false, false);
    let o = obj(&stripped, 1);
    assert_eq!(
        o.tree(None).unwrap().unwrap().source,
        TreeSource::Provider("tpk")
    );

    let bare = file(&[(0, 100)], false);
    let env = Environment::new(EnvOptions::unitypy());
    env.load(Source::Bytes {
        name: RawStr::from("CAB-B"),
        data: Bytes::from(bare),
    })
    .unwrap();
    let Some(Cab::Serialized(sf)) = env
        .get_cab(&RawStr::from("cab-b"), &Meter::unlimited())
        .unwrap()
    else {
        panic!()
    };
    let o = sf.object(1).unwrap();
    assert!(o.tree(None).unwrap().is_none());
    assert!(matches!(
        o.read_typetree(None, true),
        Err(ParseError::NoTypeTree { .. })
    ));
    // The write raises the same `TypeTreeError` (`files/ObjectReader.py:296-307`).
    let value = stripped_value(&stripped);
    assert_eq!(
        o.write_typetree(&value, None, WriteRules::UnityPy),
        Err(WriteError::NoTypeTree {
            class_id: o.class_id()
        })
    );
}

/// With no provider that accepts scripts, `object_tree` is `ObjectRef::tree`
/// and nothing is dereferenced, even a dangling script pointer.
#[test]
fn without_a_script_provider_object_tree_is_the_objects_tree() {
    let s = setup(&[(0, 100), (1, 5), (0, 0)], false, false);
    for path_id in 1..=3 {
        let o = obj(&s, path_id);
        let a = s.env.object_tree(&o, None).unwrap().unwrap();
        let b = o.tree(None).unwrap().unwrap();
        assert!(Arc::ptr_eq(&a.tree, &b.tree));
        assert_eq!(a.source, b.source);
    }
}

/// A provider-chain `MonoBehaviour` tree is replaced by the script
/// provider's for its resolved script; a null script and a script the
/// provider does not know fall back to the class tree (UnityPy's
/// `ValueError`); a dangling external and a missing object are the
/// object's error (`FileNotFoundError`, `KeyError`: not swallowed).
#[test]
fn the_script_hook_dereferences_m_script_for_a_provider_tree_only() {
    let s = setup(&[(0, 100), (0, 0), (0, 101), (1, 5), (0, 77)], false, true);
    let t = s.env.object_tree(&obj(&s, 1), None).unwrap().unwrap();
    assert_eq!(t.source, TreeSource::Provider("script"));
    assert_eq!(value_of(&obj(&s, 1), &t.tree), Some(42));

    for (path_id, why) in [(2, "null"), (3, "unknown script")] {
        let t = s.env.object_tree(&obj(&s, path_id), None).unwrap().unwrap();
        assert_eq!(t.source, TreeSource::Provider("tpk"), "{why}");
    }
    let deref = |path_id| match s.env.object_tree(&obj(&s, path_id), None) {
        Err(ParseError::Deref(d)) => (*d).clone(),
        other => panic!("{path_id}: {other:?}"),
    };
    assert!(
        matches!(deref(4), DerefError::NotFound { .. }),
        "a dangling external is FileNotFoundError, raised"
    );
    assert!(
        matches!(deref(5), DerefError::MissingObject { path_id: 77, .. }),
        "a missing object is KeyError, raised"
    );
}

/// An embedded `MonoBehaviour` tree, and an explicit one, never dereference
/// the script, so even a dangling one is no error and the script provider
/// is never asked.
#[test]
fn an_embedded_or_explicit_tree_never_dereferences_the_script() {
    let s = setup(&[(1, 5)], true, true);
    let o = obj(&s, 1);
    let t = s.env.object_tree(&o, None).unwrap().unwrap();
    assert_eq!(t.source, TreeSource::Embedded);
    let explicit = tree(behaviour_rows());
    let t = s.env.object_tree(&o, Some(&explicit)).unwrap().unwrap();
    assert_eq!(t.source, TreeSource::Explicit);

    let stripped = setup(&[(1, 5)], false, true);
    let o = obj(&stripped, 1);
    let t = stripped
        .env
        .object_tree(&o, Some(&explicit))
        .unwrap()
        .unwrap();
    assert_eq!(t.source, TreeSource::Explicit);
    assert_eq!(s.scripts.asked.load(Ordering::Relaxed), 0);
    assert_eq!(stripped.scripts.asked.load(Ordering::Relaxed), 0);
}

/// A budget that refuses the `n`-th dependency reservation after it is
/// armed with `n`, once, and counts the dependency reservations it is asked
/// for.
#[derive(Debug, Default)]
struct RefuseOnce {
    countdown: AtomicU64,
    asked: AtomicU64,
    refused: AtomicU64,
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

/// A budget that grants dependencies up to a limit and anything else
/// whole: the dependency pool.
#[derive(Debug)]
struct Pool {
    limit: AtomicU64,
    used: Arc<PoolHome>,
}

#[derive(Debug, Default)]
struct PoolHome(AtomicU64);

impl PermitHome for PoolHome {
    fn release(&self, bytes: u64) {
        self.0.fetch_sub(bytes, Ordering::AcqRel);
    }
}

impl MemoryBudget for Pool {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        if what != Reservation::Dependency {
            return Ok(Permit::detached(bytes));
        }
        let limit = self.limit.load(Ordering::Acquire);
        self.used
            .0
            .try_update(Ordering::AcqRel, Ordering::Acquire, |u| {
                u.checked_add(bytes).filter(|n| *n <= limit)
            })
            .map_err(|_| BudgetExceeded {
                requested: bytes,
                what,
            })?;
        Ok(Permit::new(bytes, self.used.clone()))
    }
}

/// The ways an object's tree is found that a sweep runs: the type record's
/// own tree (`embedded`) or the class provider's, each with and without a
/// script provider in the chain.
const TREES: [(bool, bool); 4] = [(false, true), (false, false), (true, true), (true, false)];

/// The tree UnityPy's order gives the behaviour of [`dependency_objects`]
/// (`files/ObjectReader.py:285-307`): the type record's own, else the
/// class provider's, which the script provider's replaces for its known
/// script `Foo`.
fn behaviour_source(embedded: bool, with_scripts: bool) -> TreeSource {
    match (embedded, with_scripts) {
        (true, _) => TreeSource::Embedded,
        (false, true) => TreeSource::Provider("script"),
        (false, false) => TreeSource::Provider("tpk"),
    }
}

/// An Environment of `batch()`'s options in `mode` on `budget`, its chain
/// the class provider and, when `with_scripts`, `scripts`.
fn batch_env(
    mode: LoadMode,
    budget: Arc<dyn MemoryBudget>,
    scripts: &Arc<Scripts>,
    with_scripts: bool,
) -> Environment {
    let mut ctx = FileContext::unitypy();
    ctx.providers = chain(scripts, with_scripts);
    ctx.budget = budget;
    let mut o = EnvOptions::batch(Arc::new(ctx));
    o.load = mode;
    Environment::new(o)
}

/// `t.bundle`, loaded into `env` as a dependency: `CAB-T`, whose behaviour
/// 1 names the `MonoScript` 100 of its sibling `CAB-GONE`, then `CAB-GONE`,
/// both with their type records' own trees when `embedded`. `CAB-T`'s
/// objects in path id order (the behaviour, then its two `MonoScript`s),
/// which the caller holds alone: only the LRU holds the bundle.
fn dependency_objects(env: &Environment, embedded: bool) -> Vec<ObjectRef> {
    let data = bundle(&[
        ("CAB-T", file(&[(1, 100)], embedded)),
        ("CAB-GONE", file(&[], embedded)),
    ]);
    let held = env
        .load_dependency(Source::Bytes {
            name: RawStr::from("t.bundle"),
            data: Bytes::from(data),
        })
        .unwrap();
    let File::Bundle(b) = &held else {
        panic!("t.bundle is a bundle")
    };
    let Ok(File::Serialized(sf)) = b.entry(0) else {
        panic!("CAB-T is a SerializedFile")
    };
    sf.objects_by_path().collect()
}

/// Every dependency reservation `object_tree` makes on an object of a
/// dependency's file, refused once in turn, evicts the dependencies and is
/// tried once more, under both modes, whichever way the tree is found, so
/// the call answers as it does with nothing refused (plan section 3.13):
/// the compile of a tree the type record stores (review round 9: it ran
/// outside the retry, so every refusal of it failed the call), and the
/// script hook's head read, its dereference and its script read (review
/// round 8). The class provider's tree charges nothing, so with no script
/// provider a stripped file's call makes no reservation. The object's file
/// is `CAB-T` in `t.bundle`, its script in the sibling `CAB-GONE`, and the
/// caller holds the object alone, so that only the LRU holds the bundle: the
/// call holds it while it runs, and an eviction its first step makes does
/// not drop the bundle the dereference after it reads through.
#[test]
fn a_pool_refusal_in_object_tree_evicts_and_retries_once() {
    let scripts = Arc::new(Scripts {
        tree: tree(script_rows()),
        asked: AtomicUsize::new(0),
    });
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        for (embedded, with_scripts) in TREES {
            let what = format!("{mode:?} embedded {embedded} scripts {with_scripts}");
            // The behaviour's tree's source, the dependency reservations
            // the call made, and how many were refused, with the `k`-th of
            // them refused once.
            let run = |k: u64| {
                let budget = Arc::new(RefuseOnce::default());
                let env = batch_env(mode, budget.clone(), &scripts, with_scripts);
                let o = dependency_objects(&env, embedded).swap_remove(0);
                budget.countdown.store(k, Ordering::Release);
                let at = budget.asked.load(Ordering::Acquire);
                let t = env.object_tree(&o, None).map(|t| t.unwrap().source);
                (
                    t.map_err(|e| e.to_string()),
                    budget.asked.load(Ordering::Acquire) - at,
                    budget.refused.load(Ordering::Acquire),
                )
            };
            let (source, n, _) = run(0);
            assert_eq!(
                source,
                Ok(behaviour_source(embedded, with_scripts)),
                "{what}"
            );
            if embedded || with_scripts {
                assert!(n > 0, "{what}: the call read nothing through the pool");
            } else {
                assert_eq!(n, 0, "{what}: the class provider's tree charged the pool");
            }
            for k in 1..=n {
                let (got, _, refused) = run(k);
                assert_eq!(got, source, "{what}: {k} of {n}");
                assert_eq!(refused, 1, "{what}: {k} of {n}");
            }
        }
    }
}

/// A pool full of another dependency's decoded entry, held by nothing but
/// the LRU, with 16 bytes to spare: `object_tree` on each object of a
/// dependency's file evicts that dependency and answers, under both modes,
/// whichever way the tree is found (review round 9: with an embedded tree,
/// every call failed with the pool's refusal and nothing was evicted).
/// Each case starts on a full pool of its own.
#[test]
fn a_full_pool_makes_room_for_object_tree() {
    let scripts = Arc::new(Scripts {
        tree: tree(script_rows()),
        asked: AtomicUsize::new(0),
    });
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        for (embedded, with_scripts) in TREES {
            let what = format!("{mode:?} embedded {embedded} scripts {with_scripts}");
            let pool = Arc::new(Pool {
                limit: AtomicU64::new(u64::MAX),
                used: Arc::new(PoolHome::default()),
            });
            let env = batch_env(mode, pool.clone(), &scripts, with_scripts);
            let objects = dependency_objects(&env, embedded);
            let pad = env
                .load_dependency(Source::Bytes {
                    name: RawStr::from("p.bundle"),
                    data: Bytes::from(bundle(&[("P.resS", vec![3; 128 << 10])])),
                })
                .unwrap();
            let File::Bundle(pb) = &pad else {
                panic!("p.bundle is a bundle")
            };
            assert_eq!(pb.entry_range(0, 0..4).unwrap().len(), 4, "{what}");
            drop(pad);
            let used = pool.used.0.load(Ordering::Acquire);
            pool.limit.store(used + 16, Ordering::Release);
            let got: Vec<_> = objects
                .iter()
                .map(|o| {
                    let t = env.object_tree(o, None);
                    t.map(|t| t.unwrap().source).map_err(|e| e.to_string())
                })
                .collect();
            // The `MonoScript`s are no behaviours: the class tree.
            let script = if embedded {
                TreeSource::Embedded
            } else {
                TreeSource::Provider("tpk")
            };
            let want = vec![
                Ok(behaviour_source(embedded, with_scripts)),
                Ok(script),
                Ok(script),
            ];
            assert_eq!(got, want, "{what}");
            let evicted = env.stats().dependencies_evicted;
            if embedded || with_scripts {
                assert!(evicted > 0, "{what}: nothing was evicted");
            } else {
                assert_eq!(
                    evicted, 0,
                    "{what}: the class provider's tree charged the pool"
                );
            }
        }
    }
}
