//! The Environment's own rules (plan sections 3.13 and 7.5): the load modes'
//! differences, dereferences outside the oracle's reach, resources located
//! without parsing, the dependency LRU and its pool, and the lock rules.
//! What UnityPy decides is replayed from its record in `env_fixtures.rs`.

// Test code: a failed lookup is a failed test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use unityrs_core::budget::{BudgetExceeded, Counting, MemoryBudget, Permit, Reservation};
use unityrs_core::env::{
    Cab, CabResolver, DerefError, EnvOptions, Environment, Fetch, LoadMode, PPtr, RawCab, Source,
};
use unityrs_core::error::{Error, LimitKind};
use unityrs_core::files::{BundleFile, File, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::value::Value;
use unityrs_core::{Bytes, FileContext, ObjectRef, RawStr};
use unityrs_testkit::Rng;
use unityrs_testkit::env::{
    Bundle, Obj, bad_cab, bundle, last_block_fails, nested_after_cab, res_bytes, row, scenes,
    serialized,
};

fn src(name: &str, data: &[u8]) -> Source {
    Source::Bytes {
        name: RawStr::from(name),
        data: Bytes::copy_from_slice(data),
    }
}

fn key(s: &str) -> RawStr {
    RawStr::from(s)
}

fn unlimited() -> Arc<Meter> {
    Meter::unlimited()
}

fn env(mode: LoadMode) -> Environment {
    let mut o = EnvOptions::unitypy();
    o.load = mode;
    Environment::new(o)
}

fn ctx_with(budget: Arc<dyn MemoryBudget>) -> Arc<FileContext> {
    let mut c = FileContext::unitypy();
    c.budget = budget;
    Arc::new(c)
}

fn batch(ctx: Arc<FileContext>, dep_cache_bytes: u64) -> Environment {
    let mut o = EnvOptions::batch(ctx);
    o.dep_cache_bytes = dep_cache_bytes;
    Environment::new(o)
}

fn keys(env: &Environment) -> Vec<String> {
    env.registry()
        .keys()
        .map(|k| String::from_utf8_lossy(k.as_bytes()).into_owned())
        .collect()
}

fn serialized_cab(env: &Environment, name: &str) -> Arc<SerializedFile> {
    match env.get_cab(&key(name), &unlimited()).unwrap() {
        Some(Cab::Serialized(sf)) => sf,
        other => panic!("{name}: {other:?}"),
    }
}

fn object(sf: &Arc<SerializedFile>, path_id: i64) -> ObjectRef {
    sf.object(path_id).unwrap()
}

fn name_of(o: &ObjectRef) -> String {
    let p = o.parse_as_dict(true).unwrap();
    match p.get(&[b"m_Name"]) {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("{other:?}"),
    }
}

fn stream(o: &ObjectRef) -> (RawStr, u64, u64) {
    let p = o.parse_as_dict(true).unwrap();
    let Some(Value::Str(path)) = p.get(&[b"m_StreamData", b"path"]) else {
        panic!("no path")
    };
    let offset = p
        .get(&[b"m_StreamData", b"offset"])
        .unwrap()
        .as_u64()
        .unwrap();
    let size = p
        .get(&[b"m_StreamData", b"size"])
        .unwrap()
        .as_u64()
        .unwrap();
    (path.clone(), offset, size)
}

fn file_of(scene: &unityrs_testkit::env::Scene, i: usize) -> Source {
    let (n, d) = &scene.files[i];
    Source::Bytes {
        name: RawStr::new(n.clone()),
        data: Bytes::from(d.clone()),
    }
}

// --------------------------------------------------------- load modes

/// (a): under `Eager` a CAB that fails to parse fails the bundle's load,
/// with the CABs before it registered and nothing in `files`; under `Lazy`
/// the load succeeds, the sibling reads, only the failing CAB's `get`
/// fails, and the bundle's `load_all` returns exactly `Eager`'s error.
#[test]
fn difference_a_a_failing_entry_fails_only_its_reads_under_lazy() {
    let scene = bad_cab();
    let eager = env(LoadMode::Eager);
    let eager_error = eager.load(file_of(&scene, 0)).unwrap_err();
    assert!(eager.files().is_empty());
    assert_eq!(keys(&eager), ["cab-g", "cab-g.ress"]);

    let lazy = env(LoadMode::Lazy);
    let File::Bundle(b) = lazy.load(file_of(&scene, 0)).unwrap() else {
        panic!()
    };
    assert_eq!(keys(&lazy), ["cab-g", "cab-g.ress", "cab-bad"]);
    assert_eq!(name_of(&object(&serialized_cab(&lazy, "cab-g"), 1)), "g1");
    assert!(lazy.get_cab(&key("cab-bad"), &unlimited()).is_err());
    assert_eq!(
        b.load_all().unwrap_err().to_string(),
        eager_error.to_string()
    );
}

/// (a): an entry shadowed by a later one of the same path is never parsed
/// under `Lazy`; `Eager` parses it first and fails.
#[test]
fn difference_a_a_shadowed_entry_is_never_parsed_under_lazy() {
    let bad = serialized(
        &[],
        &[Obj::asset_bundle(
            1,
            Bundle {
                no_container: true,
                ..Bundle::default()
            },
        )],
    );
    let good = serialized(&[], &[Obj::named(1, "good", &[])]);
    let data = bundle(&[("CAB-X", bad), ("CAB-X", good)]);
    assert!(env(LoadMode::Eager).load(src("x.bundle", &data)).is_err());
    let lazy = env(LoadMode::Lazy);
    lazy.load(src("x.bundle", &data)).unwrap();
    assert_eq!(name_of(&object(&serialized_cab(&lazy, "cab-x"), 1)), "good");
    assert_eq!(lazy.stats().entries_parsed, 1);
}

/// (b): a nested bundle under a CAB's path: `Eager` keeps the CAB under the
/// key and registers the nested entries in place; `Lazy` registers the key
/// for the nested entry, whose first `get` answers `None` and registers its
/// entries then, at the end of the order.
#[test]
fn difference_b_a_nested_entry_is_a_key_until_its_first_get() {
    let scene = nested_after_cab();
    let eager = env(LoadMode::Eager);
    eager.load(file_of(&scene, 0)).unwrap();
    assert_eq!(keys(&eager), ["cab-n", "cab-inner"]);
    assert_eq!(name_of(&object(&serialized_cab(&eager, "cab-n"), 1)), "n1");

    let lazy = env(LoadMode::Lazy);
    lazy.load(file_of(&scene, 0)).unwrap();
    assert_eq!(keys(&lazy), ["cab-n"]);
    assert!(lazy.get_cab(&key("cab-n"), &unlimited()).unwrap().is_none());
    assert_eq!(keys(&lazy), ["cab-n", "cab-inner"]);
    assert_eq!(
        name_of(&object(&serialized_cab(&lazy, "cab-inner"), 2)),
        "inner"
    );
}

/// (c): a block past every entry's bytes that fails to decode fails the
/// `Eager` load and nothing under `Lazy`.
#[test]
fn difference_c_a_block_after_every_entry_fails_only_eager() {
    let scene = last_block_fails();
    assert!(env(LoadMode::Eager).load(file_of(&scene, 0)).is_err());
    let lazy = env(LoadMode::Lazy);
    lazy.load(file_of(&scene, 0)).unwrap();
    let sf = serialized_cab(&lazy, "cab-l");
    let target = lazy
        .deref(
            &sf,
            PPtr {
                file_id: 0,
                path_id: 1,
            },
        )
        .unwrap();
    assert_eq!(name_of(&target), "l1");
}

/// (d): entries whose spans are nested suffixes of the data: `Eager` decodes
/// one run and fits; `Lazy` decodes each entry's span again and keeps it,
/// so listing the objects stops at the meter (`InputMemory`).
#[test]
fn difference_d_overlapping_spans_stop_lazy_at_the_meter() {
    const N: usize = 24;
    const B: usize = 1 << 16;
    let entries: Vec<(String, Vec<u8>)> = (0..N)
        .map(|k| {
            (
                format!("suffix-{k:02}.resS"),
                (0..B)
                    .map(|i| ((i * 31 + k) % 251).to_le_bytes()[0])
                    .collect(),
            )
        })
        .collect();
    let mut spec = unityrs_testkit::bundle::BundleSpec {
        info_codec: unityrs_testkit::bundle::TestCodec::None,
        block_size: B,
        entries: entries
            .iter()
            .map(|(n, d)| unityrs_testkit::bundle::EntrySpec {
                name: n.clone().into_bytes(),
                data: d.clone(),
                flags: 0,
            })
            .collect(),
        ..unityrs_testkit::bundle::BundleSpec::default()
    };
    spec.data_codec = unityrs_testkit::bundle::TestCodec::Lz4;
    let mut data = unityrs_testkit::bundle::build(&spec);
    for k in 0..N {
        let name = format!("suffix-{k:02}.resS\0").into_bytes();
        let at = data.windows(name.len()).position(|w| w == name).unwrap();
        let offset = i64::try_from((N - 1 - k) * B).unwrap();
        let size = i64::try_from((k + 1) * B).unwrap();
        data[at - 20..at - 12].copy_from_slice(&offset.to_be_bytes());
        data[at - 12..at - 4].copy_from_slice(&size.to_be_bytes());
    }
    let ctx = {
        let mut c = FileContext::unitypy();
        c.limits.max_input_memory = 6 << 20;
        Arc::new(c)
    };
    let mut o = EnvOptions::unitypy();
    o.context = ctx.clone();
    let eager = Environment::new(o.clone());
    eager.load(src("d.bundle", &data)).unwrap();
    assert!(eager.objects().all(|o| o.is_ok()));

    o.load = LoadMode::Lazy;
    let lazy = Environment::new(o);
    lazy.load(src("d.bundle", &data)).unwrap();
    let stopped = lazy.objects().find_map(Result::err).expect("a read stops");
    assert!(
        matches!(&stopped, Error::Limit(l) if l.kind == LimitKind::InputMemory),
        "{stopped:?}"
    );
}

// --------------------------------------------------------- dereference

/// A `from` the Environment never loaded (a second handle on the same
/// bytes) finds its siblings through its own bundle and every other CAB
/// through the Environment; once its bundle is dropped, a pointer that
/// needs the container is `ContainerDropped`, and one into its own file
/// still resolves.
#[test]
fn a_second_handle_derefs_through_its_bundle_and_the_environment() {
    let scene = unityrs_testkit::env::cross_bundle_pointer();
    let env = env(LoadMode::Eager);
    env.load(file_of(&scene, 1)).unwrap();
    let ctx = Arc::new(FileContext::unitypy());
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let a = BundleFile::open(
        RawStr::from("a.bundle"),
        Bytes::from(scene.files[0].1.clone()),
        None,
        &meter,
        &ctx,
    )
    .unwrap();
    let File::Serialized(cab_a) = a.entry(0).unwrap() else {
        panic!()
    };
    let to_b = PPtr {
        file_id: 1,
        path_id: 7,
    };
    assert_eq!(name_of(&env.deref(&cab_a, to_b).unwrap()), "b7");
    drop(a);
    assert!(matches!(
        env.deref(&cab_a, to_b),
        Err(DerefError::ContainerDropped { .. })
    ));
    let own = PPtr {
        file_id: 0,
        path_id: 2,
    };
    assert_eq!(name_of(&env.deref(&cab_a, own).unwrap()), "tex");
}

/// Without B, A's pointer finds nothing (`find_file` has no resolver).
#[test]
fn a_loose_pointer_without_its_target_is_not_found() {
    let scene = unityrs_testkit::env::loose_external_pointer();
    let env = env(LoadMode::Eager);
    env.load(file_of(&scene, 0)).unwrap();
    let sf = serialized_cab(&env, "cab-a");
    assert!(matches!(
        env.deref(
            &sf,
            PPtr {
                file_id: 1,
                path_id: 5
            }
        ),
        Err(DerefError::NotFound { .. })
    ));
}

/// `external_index`'s edges through `deref`: `i32::MIN` and `-len - 1`
/// are below the start, `-len` is the first external.
#[test]
fn negative_file_ids_index_from_the_end() {
    let data = serialized(
        &["archive:/X/CAB-T1", "archive:/X/CAB-T2"],
        &[Obj::named(1, "o", &[])],
    );
    let env = env(LoadMode::Eager);
    env.load(src("CAB-O", &data)).unwrap();
    env.load(src("CAB-T1", &serialized(&[], &[Obj::named(4, "t1", &[])])))
        .unwrap();
    let sf = serialized_cab(&env, "cab-o");
    let deref = |file_id| {
        env.deref(
            &sf,
            PPtr {
                file_id,
                path_id: 4,
            },
        )
    };
    assert!(matches!(
        deref(i32::MIN),
        Err(DerefError::FileIdBelowStart { .. })
    ));
    assert!(matches!(
        deref(-2),
        Err(DerefError::FileIdBelowStart { .. })
    ));
    assert_eq!(name_of(&deref(-1).unwrap()), "t1");
    assert!(matches!(deref(3), Err(DerefError::FileIdPastEnd { .. })));
}

// --------------------------------------------------------- resources

fn res_bundle() -> Vec<u8> {
    bundle(&[
        (
            "CAB-R",
            serialized(
                &[],
                &[Obj::streamed(2, "tex", "archive:/CAB-R/CAB-R.resS", 3, 5)],
            ),
        ),
        ("CAB-R.resS", res_bytes(32, 0)),
    ])
}

/// A resource read on a second handle over a `Source::Bytes` dependency's
/// own bytes reads through that handle: the shared slot is never parsed,
/// and the dependency's meter (and so its pool) is charged nothing.
#[test]
fn a_resource_on_a_second_handle_over_a_dependencys_bytes_charges_the_slot_nothing() {
    let data = Bytes::from(res_bundle());
    let env = batch(Arc::new(FileContext::unitypy()), u64::MAX);
    let dep = env
        .load_dependency(Source::Bytes {
            name: RawStr::from("r.bundle"),
            data: data.clone(),
        })
        .unwrap();
    let dep_used = dep.meter().used();
    let parsed = env.stats().entries_parsed;

    let ctx = Arc::new(FileContext::unitypy());
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let second = BundleFile::open(RawStr::from("r.bundle"), data, None, &meter, &ctx).unwrap();
    let File::Serialized(cab) = second.entry(0).unwrap() else {
        panic!()
    };
    let (path, offset, size) = stream(&object(&cab, 2));
    let got = env.resource(&cab, &path, offset, size).unwrap();
    assert_eq!(got.as_ref(), &res_bytes(32, 0)[3..8]);
    assert_eq!(dep.meter().used(), dep_used, "the slot's meter");
    assert_eq!(
        env.stats().entries_parsed,
        parsed,
        "the slot was not parsed"
    );
}

#[derive(Debug)]
struct BytesFetch(Bytes, AtomicUsize);

impl Fetch for BytesFetch {
    fn fetch(&self) -> Result<Bytes, Error> {
        self.1.fetch_add(1, Ordering::Relaxed);
        Ok(self.0.clone())
    }
}

/// A file of the scratch directory this test owns, removed on drop.
struct TempFile(PathBuf);

impl TempFile {
    fn new(tag: &str, data: &[u8]) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "unityrs-env-{}-{tag}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, data).unwrap();
        Self(path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Through the handle `load_dependency` returned, for every `Source` kind,
/// a resource read never parses the shared slot.
#[test]
fn a_resource_through_the_dependency_handle_parses_no_slot_for_every_source() {
    let data = Bytes::from(res_bundle());
    let file = TempFile::new("res", &data);
    let sources = [
        Source::Bytes {
            name: RawStr::from("r.bundle"),
            data: data.clone(),
        },
        Source::Path(file.0.clone()),
        Source::Fetch {
            name: RawStr::from("r.bundle"),
            fetch: Arc::new(BytesFetch(data.clone(), AtomicUsize::new(0))),
        },
    ];
    for source in sources {
        let env = batch(Arc::new(FileContext::unitypy()), u64::MAX);
        let File::Bundle(b) = env.load_dependency(source.clone()).unwrap() else {
            panic!()
        };
        let File::Serialized(cab) = b.entry(0).unwrap() else {
            panic!()
        };
        let (path, offset, size) = stream(&object(&cab, 2));
        let parsed = env.stats().entries_parsed;
        let got = env.resource(&cab, &path, offset, size).unwrap();
        assert_eq!(got.as_ref(), &res_bytes(32, 0)[3..8], "{source:?}");
        assert_eq!(env.stats().entries_parsed, parsed, "{source:?}");
        assert!(env.files().is_empty());
    }
}

/// An evicted `Source::Path` dependency's resource resolves through its
/// slot, which opens the path again: the same bytes.
#[test]
fn an_evicted_path_dependency_resolves_through_its_slot() {
    let file = TempFile::new("evicted", &res_bundle());
    let other = TempFile::new("other", &bundle(&[("CAB-Z", serialized(&[], &[]))]));
    let env = batch(Arc::new(FileContext::unitypy()), 1);
    let File::Bundle(b) = env.load_dependency(Source::Path(file.0.clone())).unwrap() else {
        panic!()
    };
    let File::Serialized(cab) = b.entry(0).unwrap() else {
        panic!()
    };
    let (path, offset, size) = stream(&object(&cab, 2));
    let cab = Arc::new(cab);
    drop(b);
    env.load_dependency(Source::Path(other.0.clone())).unwrap();
    assert_eq!(env.stats().dependencies_evicted, 1);
    let got = env.resource(&cab, &path, offset, size).unwrap();
    assert_eq!(got.as_ref(), &res_bytes(32, 0)[3..8]);
}

/// A resource no candidate names is loaded through the resolver (UnityPy's
/// `load_dependencies`) and read; with none, `NotFound` names the basename.
#[test]
fn a_missing_resource_is_loaded_through_the_resolver_then_read() {
    #[derive(Debug)]
    struct One(Bytes);
    impl CabResolver for One {
        fn resolve(&self, name: &[u8]) -> Option<Source> {
            (name == b"CAB-W.resS").then(|| Source::Bytes {
                name: RawStr::from("CAB-W.resS"),
                data: self.0.clone(),
            })
        }
    }
    let data = serialized(
        &[],
        &[Obj::streamed(2, "tex", "archive:/CAB-W/CAB-W.resS", 1, 2)],
    );
    for keep in [true, false] {
        let mut o = EnvOptions::unitypy();
        o.keep_resolver_loads = keep;
        o.resolver = Some(Arc::new(One(Bytes::from(res_bytes(8, 4)))));
        let env = Environment::new(o);
        env.load(src("CAB-O", &data)).unwrap();
        let sf = serialized_cab(&env, "cab-o");
        let (path, offset, size) = stream(&object(&sf, 2));
        let got = env.resource(&sf, &path, offset, size).unwrap();
        assert_eq!(got.as_ref(), &res_bytes(8, 4)[1..3]);
        assert_eq!(env.files().len(), if keep { 2 } else { 1 });
    }
    let env = env(LoadMode::Eager);
    env.load(src("CAB-O", &data)).unwrap();
    let sf = serialized_cab(&env, "cab-o");
    let (path, offset, size) = stream(&object(&sf, 2));
    assert!(matches!(
        env.resource(&sf, &path, offset, size),
        Err(Error::NotFound(unityrs_core::NotFound::Resource(n))) if n.as_bytes() == b"CAB-W.resS"
    ));
}

/// Under `EnvOptions::batch()` (no `keep_resolver_loads`) every file the
/// resolver loads is an LRU dependency: none is in `files`, `objects` or
/// the container, and the pointer and the resource still resolve.
#[test]
fn batch_options_keep_no_resolver_load_in_files_objects_or_the_container() {
    #[derive(Debug)]
    struct SceneResolver(Vec<(Vec<u8>, Vec<u8>)>);
    impl CabResolver for SceneResolver {
        fn resolve(&self, name: &[u8]) -> Option<Source> {
            let (n, d) = self.0.iter().find(|(n, _)| n == name)?;
            Some(Source::Bytes {
                name: RawStr::new(n.clone()),
                data: Bytes::from(d.clone()),
            })
        }
    }
    let scene = unityrs_testkit::env::resolver_loads();
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let mut o = EnvOptions::batch(Arc::new(FileContext::unitypy()));
        o.load = mode;
        o.resolver = Some(Arc::new(SceneResolver(scene.resolver.clone())));
        let env = Environment::new(o);
        env.load(file_of(&scene, 0)).unwrap();
        let sf = serialized_cab(&env, "cab-h");
        let loose = PPtr {
            file_id: 2,
            path_id: 5,
        };
        assert_eq!(name_of(&env.deref(&sf, loose).unwrap()), "loose5");
        assert_eq!(env.container().unwrap().count(), 1, "{mode:?}");
        // `cab-dep` is a bundle: loaded, it is the pointer's container.
        let to_dep = PPtr {
            file_id: 1,
            path_id: 3,
        };
        assert!(matches!(
            env.deref(&sf, to_dep),
            Err(DerefError::RawTarget { .. })
        ));
        let (path, offset, size) = stream(&object(&sf, 2));
        let got = env.resource(&sf, &path, offset, size).unwrap();
        assert_eq!(got.as_ref(), &res_bytes(24, 9)[2..8]);
        assert_eq!(env.files().len(), 1, "{mode:?}");
        let objects: Vec<_> = env.objects().map(Result::unwrap).collect();
        assert_eq!(objects.len(), 3, "{mode:?}");
        assert!(objects.iter().all(|o| Arc::ptr_eq(o.file(), &sf)));
        assert!(env.stats().dependencies_loaded >= 2, "{mode:?}");
        // `cab-dep`'s own `CAB-H` took the key: no caller CAB is left with rows.
        assert_eq!(env.container().unwrap().count(), 0, "{mode:?}");
    }
}

// --------------------------------------------------------- the LRU

fn named_bundle(cab: &str, pad: usize) -> Vec<u8> {
    bundle(&[
        (cab, serialized(&[], &[Obj::named(1, cab, &[])])),
        ("pad.resS", res_bytes(pad, 1)),
    ])
}

/// `load_dependency` registers an evictable dependency whose meter names the
/// dependency pool and lists nothing in `files`; an evicted one opens again
/// on `Meter::reopen` (the same account, its work from zero, the old
/// count unchanged); a caller's file is never evicted; and every charge
/// returns.
#[test]
fn dependencies_are_evicted_and_reopened_and_every_charge_returns() {
    let budget = Arc::new(Counting::new(u64::MAX));
    let ctx = ctx_with(budget.clone());
    {
        let env = batch(ctx.clone(), 1);
        let caller = env
            .load(src("c.bundle", &named_bundle("CAB-C", 64)))
            .unwrap();
        let a = env
            .load_dependency(src("a.bundle", &named_bundle("CAB-A", 64)))
            .unwrap();
        assert_eq!(a.meter().reservation(), Reservation::Dependency);
        assert_eq!(env.files().len(), 1, "only the caller's file");
        let first = serialized_cab(&env, "cab-a");
        let old = first.meter().clone();
        let old_work = old.decoded();
        drop((a, first));
        env.load_dependency(src("b.bundle", &named_bundle("CAB-B", 64)))
            .unwrap();
        assert_eq!(env.stats().dependencies_evicted, 1);
        let again = serialized_cab(&env, "cab-a");
        assert!(!Arc::ptr_eq(again.meter(), &old));
        assert!(again.meter().same_input(&old));
        assert_eq!(old.decoded(), old_work, "the old count is unchanged");
        assert!(again.meter().decoded() <= old_work, "work from zero");
        assert_eq!(env.stats().dependencies_evicted, 2);
        let File::Bundle(cb) = &caller else { panic!() };
        let c1 = serialized_cab(&env, "cab-c");
        assert!(Arc::ptr_eq(
            &c1,
            &match cb.entry(0).unwrap() {
                File::Serialized(sf) => sf,
                _ => panic!(),
            }
        ));
    }
    assert_eq!(budget.used(), 0, "every charge returned");
}

/// `Source::Path` dependencies totalling twice the LRU's bound, read round
/// robin, are never refused by a pool sized for the bound and two more.
#[test]
fn path_dependencies_twice_the_bound_round_robin_are_never_refused() {
    let files: Vec<TempFile> = (0..6)
        .map(|i| TempFile::new("rr", &named_bundle(&format!("CAB-{i}"), 4096)))
        .collect();
    let one = {
        let env = batch(Arc::new(FileContext::unitypy()), u64::MAX);
        let f = env
            .load_dependency(Source::Path(files[0].0.clone()))
            .unwrap();
        let _ = serialized_cab(&env, "cab-0");
        f.meter().used()
    };
    let bound = 3 * one;
    let budget = Arc::new(Counting::new(bound + 3 * one));
    let env = batch(ctx_with(budget.clone()), bound);
    for f in &files {
        env.load_dependency(Source::Path(f.0.clone())).unwrap();
    }
    for _ in 0..4 {
        for i in 0..files.len() {
            let sf = serialized_cab(&env, &format!("cab-{i}"));
            assert_eq!(name_of(&object(&sf, 1)), format!("CAB-{i}"));
        }
    }
    assert!(env.stats().dependencies_evicted > 0);
}

/// A budget that grants inputs anything and dependencies up to a limit,
/// recording the most the dependencies held at once.
#[derive(Debug)]
struct DepPool {
    limit: AtomicU64,
    used: Arc<PoolHome>,
}

#[derive(Debug, Default)]
struct PoolHome {
    used: AtomicU64,
    peak: AtomicU64,
}

impl unityrs_core::budget::PermitHome for PoolHome {
    fn release(&self, bytes: u64) {
        self.used.fetch_sub(bytes, Ordering::AcqRel);
    }
}

impl DepPool {
    fn new(limit: u64) -> Self {
        Self {
            limit: AtomicU64::new(limit),
            used: Arc::new(PoolHome::default()),
        }
    }
}

impl MemoryBudget for DepPool {
    fn try_reserve(&self, bytes: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        if what == Reservation::Input {
            return Ok(Permit::detached(bytes));
        }
        let limit = self.limit.load(Ordering::Acquire);
        let used = &self.used.used;
        let now =
            used.try_update(Ordering::AcqRel, Ordering::Acquire, |u| {
                u.checked_add(bytes).filter(|n| *n <= limit)
            })
            .map_err(|_| BudgetExceeded {
                requested: bytes,
                what,
            })? + bytes;
        self.used.peak.fetch_max(now, Ordering::AcqRel);
        Ok(Permit::new(bytes, self.used.clone()))
    }
}

/// A pool refusal evicts the cached dependencies and retries once; a
/// second refusal fails naming the dependency pool, and nothing is kept.
#[test]
fn a_pool_refusal_evicts_and_retries_once_then_fails_naming_the_pool() {
    let a = TempFile::new("pa", &named_bundle("CAB-A", 8192));
    let b = TempFile::new("pb", &named_bundle("CAB-B", 8192));
    let big = TempFile::new("pbig", &named_bundle("CAB-BIG", 64 << 10));
    // Measure one open: what it holds at most, and once loaded.
    let (peak, held) = {
        let pool = Arc::new(DepPool::new(u64::MAX));
        let env = batch(ctx_with(pool.clone()), u64::MAX);
        drop(env.load_dependency(Source::Path(a.0.clone())).unwrap());
        (
            pool.used.peak.load(Ordering::Acquire),
            pool.used.used.load(Ordering::Acquire),
        )
    };
    assert!(held > 0 && peak >= held);
    // Room for one open and less than the second's on top of the first.
    let pool = Arc::new(DepPool::new(peak + held / 2));
    let env = batch(ctx_with(pool.clone()), u64::MAX);
    drop(env.load_dependency(Source::Path(a.0.clone())).unwrap());
    drop(env.load_dependency(Source::Path(b.0.clone())).unwrap());
    assert_eq!(env.stats().dependencies_evicted, 1, "A made room for B");
    let refused = env
        .load_dependency(Source::Path(big.0.clone()))
        .unwrap_err();
    assert!(
        matches!(&refused, Error::Budget(e) if e.what == Reservation::Dependency),
        "{refused:?}"
    );
    assert!(!keys(&env).contains(&"cab-big".to_owned()));
    assert!(env.files().is_empty());
    assert_eq!(
        env.stats().dependency_bytes,
        0,
        "B was evicted for the retry"
    );
    drop(env);
    assert_eq!(pool.used.used.load(Ordering::Acquire), 0);
}

// --------------------------------------------------------- locks

/// Runs `f` on its own thread and fails the test if it has not finished in
/// ten seconds (a deadlock), without waiting for it.
fn within_ten_seconds(f: impl FnOnce() + Send + 'static) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        f();
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("finished within 10 s");
}

/// A resolver that calls back into the Environment (`get_cab` and `deref`)
/// from inside its call, and one that takes a lock another thread holds
/// while it uses the Environment: neither deadlocks, in either mode.
#[test]
fn a_resolver_and_a_fetch_may_call_the_environment_and_take_caller_locks() {
    #[derive(Debug)]
    struct Reentrant {
        env: Mutex<Option<Environment>>,
        lock: Arc<Mutex<()>>,
        target: Bytes,
    }
    impl CabResolver for Reentrant {
        fn resolve(&self, name: &[u8]) -> Option<Source> {
            let _held = self.lock.lock().unwrap();
            let env = self.env.lock().unwrap().clone()?;
            let _ = env.get_cab(&RawStr::from("cab-a"), &Meter::unlimited());
            if let Ok(Some(Cab::Serialized(sf))) =
                env.get_cab(&RawStr::from("cab-a"), &Meter::unlimited())
            {
                let _ = env.deref(
                    &sf,
                    PPtr {
                        file_id: 0,
                        path_id: 1,
                    },
                );
            }
            (name == b"cab-b").then(|| Source::Fetch {
                name: RawStr::from("cab-b"),
                fetch: Arc::new(ReentrantFetch {
                    env: env.clone(),
                    lock: self.lock.clone(),
                    data: self.target.clone(),
                }),
            })
        }
    }
    #[derive(Debug)]
    struct ReentrantFetch {
        env: Environment,
        lock: Arc<Mutex<()>>,
        data: Bytes,
    }
    impl Fetch for ReentrantFetch {
        fn fetch(&self) -> Result<Bytes, Error> {
            let _held = self.lock.lock().unwrap();
            let _ = self
                .env
                .get_cab(&RawStr::from("cab-a"), &Meter::unlimited());
            Ok(self.data.clone())
        }
    }
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        within_ten_seconds(move || {
            let scene = unityrs_testkit::env::cross_bundle_pointer();
            let lock = Arc::new(Mutex::new(()));
            let resolver = Arc::new(Reentrant {
                env: Mutex::new(None),
                lock: lock.clone(),
                // A `SerializedFile`, so the pointer's target is in it (a
                // bundle would be the pointer's container, with no objects).
                target: Bytes::from(serialized(&[], &[Obj::named(7, "b7", &[])])),
            });
            let mut o = EnvOptions::unitypy();
            o.load = mode;
            o.resolver = Some(resolver.clone());
            let env = Environment::new(o);
            *resolver.env.lock().unwrap() = Some(env.clone());
            env.load(file_of(&scene, 0)).unwrap();
            // Another thread uses the Environment while holding the lock
            // the resolver and the fetch take.
            let other = {
                let env = env.clone();
                let lock = lock.clone();
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        let _held = lock.lock().unwrap();
                        let _ = env.get_cab(&RawStr::from("cab-a"), &Meter::unlimited());
                        let _ = env.files();
                    }
                })
            };
            let sf = serialized_cab(&env, "cab-a");
            let t = env
                .deref(
                    &sf,
                    PPtr {
                        file_id: 1,
                        path_id: 7,
                    },
                )
                .unwrap();
            assert_eq!(name_of(&t), "b7");
            other.join().unwrap();
            *resolver.env.lock().unwrap() = None;
        });
    }
}

// --------------------------------------------------------- the container

/// The container's rows with their preload rows dereferenced: a preload row
/// that loads nothing new lists every row once, and listing again does not
/// preload again.
#[test]
fn the_container_lists_rows_after_its_preload_once() {
    let scene = unityrs_testkit::env::cross_bundle_pointer();
    let env = env(LoadMode::Eager);
    env.load(file_of(&scene, 0)).unwrap();
    env.load(file_of(&scene, 1)).unwrap();
    for _ in 0..2 {
        let rows: Vec<_> = env.container().unwrap().map(Result::unwrap).collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].row.path.as_ref().map(RawStr::as_bytes),
            Some(&b"assets/a1.prefab"[..])
        );
    }
    let data = serialized(
        &[],
        &[Obj::asset_bundle(
            1,
            Bundle {
                rows: vec![row("x", (0, 0), (0, 1))],
                ..Bundle::default()
            },
        )],
    );
    env.load(src("CAB-NEW", &data)).unwrap();
    assert_eq!(env.container().unwrap().count(), 2);
}

/// A container as a damaged tree holds it (`Bundle::hostile`): the file
/// loads, as UnityPy's does (its load reads only each asset's `path_id`,
/// `helpers/ContainerHelper.py:21-29`); each row keeps `None` where the
/// object holds no string or `int`; and the preload raises at the first row,
/// as UnityPy's `info.preloadSize` does (`AttributeError`: the member is
/// named `preloadSizX`, `:35-37`).
#[test]
fn a_hostile_container_loads_and_its_preload_raises() {
    let data = serialized(
        &[],
        &[
            Obj::named(1, "n", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(0, 1)],
                    rows: vec![row("a", (0, 1), (0, 1)), row("b", (0, 1), (0, -7))],
                    hostile: true,
                    ..Bundle::default()
                },
            ),
        ],
    );
    let env = env(LoadMode::Eager);
    env.load(src("CAB-H", &data)).unwrap();
    let sf = serialized_cab(&env, "cab-h");
    let rows: Vec<_> = sf
        .container()
        .unwrap()
        .rows()
        .iter()
        .map(|r| {
            (
                r.path.clone(),
                r.preload_index,
                r.preload_size,
                r.file_id,
                r.path_id,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (None, None, None, None, Some(1)),
            (None, None, None, None, Some(-7))
        ]
    );
    assert!(sf.container().unwrap().rows()[0].asset().is_none());
    assert!(
        matches!(
            env.container().err(),
            Some(Error::Malformed {
                what: "container",
                ..
            })
        ),
        "the preload raises"
    );
}

/// Every Environment step on random scenes, both modes: no panic.
#[test]
fn seeded_scenes_never_panic() {
    let all = scenes();
    let mut rng = Rng::new(0x0005_eedd);
    for round in 0..200 {
        let (_, scene) = &all[rng.below(all.len())];
        for mode in [LoadMode::Eager, LoadMode::Lazy] {
            let env = env(mode);
            for (n, d) in &scene.files {
                let mut d = d.clone();
                for _ in 0..=rng.below(4) {
                    let i = rng.below(d.len());
                    d[i] = u8::try_from(rng.below(256)).unwrap();
                }
                if round % 3 == 0 {
                    d.truncate(rng.below(d.len() + 1));
                }
                let _ = env.load(Source::Bytes {
                    name: RawStr::new(n.clone()),
                    data: Bytes::from(d),
                });
            }
            let objects: Vec<_> = env.objects().take(64).filter_map(Result::ok).collect();
            for o in &objects {
                let _ = o.parse_as_dict(true);
                for file_id in [-2, 0, 1, 2] {
                    let _ = env.deref(
                        o.file(),
                        PPtr {
                            file_id,
                            path_id: 1,
                        },
                    );
                }
                let _ = env.resource(o.file(), &RawStr::from("archive:/x/CAB-A.resS"), 1, 4);
            }
            if let Ok(items) = env.container() {
                for _ in items.take(64) {}
            }
            let _ = env.stats();
            for k in env.registry().keys().collect::<Vec<_>>() {
                let _ = env.get_cab(&k, &unlimited());
            }
        }
    }
    // Keep a raw CAB in the vocabulary of `Cab` used above.
    let _ = |c: Cab| matches!(c, Cab::Raw(RawCab::File(_)));
}
