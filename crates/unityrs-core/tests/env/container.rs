//! The container and its preload.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_core::container::ContainerPath;
use unityrs_core::env::{
    Cab, CabResolver, DerefError, EnvOptions, Environment, LoadMode, PPtr, RawCab, Source,
};
use unityrs_core::error::{Error, Unsupported};
use unityrs_core::files::File;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::Rng;
use unityrs_testkit::env::{Bundle, Obj, bundle, res_bytes, row, scenes, serialized};

use crate::{TempDir, env, file_of, key, name_of, object, serialized_cab, src, unlimited};

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
                let _ = env.container_path(o);
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

/// A resolver that finds nothing and counts what it is asked.
#[derive(Debug, Default)]
struct Asked(AtomicUsize);

impl CabResolver for Asked {
    fn resolve(&self, _: &[u8]) -> Option<Source> {
        self.0.fetch_add(1, Ordering::SeqCst);
        None
    }
}

/// A file whose `n` container rows each span its whole `n`-pointer preload
/// table, each pointer into its own external (`distinct`) or all into one.
fn spanning(n: usize, distinct: bool) -> Vec<u8> {
    let externals: Vec<String> = (0..if distinct { n } else { 1 })
        .map(|i| format!("archive:/CAB-X{i}/CAB-X{i}"))
        .collect();
    let externals: Vec<&str> = externals.iter().map(String::as_str).collect();
    let preload = (0..n)
        .map(|i| {
            (
                if distinct {
                    i32::try_from(i + 1).unwrap()
                } else {
                    1
                },
                1,
            )
        })
        .collect();
    let size = i32::try_from(n).unwrap();
    let rows = (0..n)
        .map(|i| row(&format!("assets/{i}.prefab"), (0, size), (0, 1)))
        .collect();
    serialized(
        &externals,
        &[
            Obj::named(1, "o1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload,
                    rows,
                    ..Bundle::default()
                },
            ),
        ],
    )
}

/// The container preload dereferences each table index once and resolves
/// each external once, however many rows' spans cover them (review round
/// 1: every row walked its whole span, so `n` rows over one `n`-pointer
/// table made `n`² dereferences), and counts no work on the walking file's
/// meter while no load changes an answer (what a walk does again is
/// counted: `tests/env/revisit.rs`). Counted in steps, never by a clock.
#[test]
fn the_preload_dereferences_each_table_index_once() {
    const N: usize = 64;
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        for distinct in [true, false] {
            let asked = Arc::new(Asked::default());
            let mut o = EnvOptions::unitypy();
            o.load = mode;
            o.resolver = Some(asked.clone());
            let env = Environment::new(o);
            env.load(src("CAB-S", &spanning(N, distinct))).unwrap();
            let walked = serialized_cab(&env, "cab-s");
            walked.container().unwrap();
            let before = walked.meter().decoded();
            assert_eq!(env.container().unwrap().count(), N);
            assert_eq!(
                walked.meter().decoded(),
                before,
                "{mode:?} distinct {distinct}: work"
            );
            let want = if distinct { N } else { 1 };
            assert_eq!(
                env.stats().preload_dereferences,
                u64::try_from(N).unwrap(),
                "{mode:?} distinct {distinct}: dereferences"
            );
            assert_eq!(
                asked.0.load(Ordering::SeqCst),
                want,
                "{mode:?} distinct {distinct}: resolver calls"
            );
        }
    }
}

/// A preload external the resolver answers with a file it names otherwise,
/// as UnityPy's `find_file` does when its walk matches a longer file name
/// by suffix (`ab.assets` finds `xab.assets`, `environment.py:345-354`,
/// registered under the name it found): the next lookup of the name misses
/// again, so each dereference loads the file again, as UnityPy's does, and
/// the last copy keeps the key and only the path its own dereference set
/// (review round 2: the walk kept the first load's answer, so both objects
/// of one copy got the row's path; UnityPy's registered copy has none for
/// object 5, `tests/fixtures/d/env.json` scene `preload_suffix_external`).
/// Where rows' spans overlap, an index is still dereferenced once
/// (`PARITY.md` section 19.3), where UnityPy loads once per row and index.
/// The walk's loads register a new key, so the pass fails as UnityPy's does.
#[test]
fn a_preload_external_found_by_suffix_loads_at_each_dereference() {
    #[derive(Debug)]
    struct Suffix(AtomicUsize, Bytes);
    impl CabResolver for Suffix {
        fn resolve(&self, name: &[u8]) -> Option<Source> {
            self.0.fetch_add(1, Ordering::SeqCst);
            b"xab.assets".ends_with(name).then(|| Source::Bytes {
                name: RawStr::from("@/xab.assets"),
                data: self.1.clone(),
            })
        }
    }
    let other = serialized(&[], &[Obj::named(5, "x5", &[]), Obj::named(6, "x6", &[])]);
    let file = |rows: usize| {
        serialized(
            &["archive:/CAB-B/ab.assets"],
            &[
                Obj::named(1, "r1", &[]),
                Obj::asset_bundle(
                    2,
                    Bundle {
                        preload: vec![(1, 5), (1, 6)],
                        rows: (0..rows)
                            .map(|i| row(&format!("assets/{i}.prefab"), (0, 2), (0, 1)))
                            .collect(),
                        ..Bundle::default()
                    },
                ),
            ],
        )
    };
    for rows in [1, 3] {
        for mode in [LoadMode::Eager, LoadMode::Lazy] {
            let resolver = Arc::new(Suffix(AtomicUsize::new(0), Bytes::from(other.clone())));
            let mut o = EnvOptions::unitypy();
            o.load = mode;
            o.resolver = Some(resolver.clone());
            let env = Environment::new(o);
            env.load(src("CAB-R", &file(rows))).unwrap();
            let e = env.container().expect_err("the dict grew");
            assert!(e.to_string().contains("changed size"), "{e}");
            assert_eq!(
                resolver.0.load(Ordering::SeqCst),
                2,
                "{mode:?} {rows} rows: a load per index"
            );
            assert_eq!(env.container().unwrap().count(), rows, "{mode:?}");
            let registered = serialized_cab(&env, "xab.assets");
            let path = |id: i64| env.container_path(&object(&registered, id)).unwrap();
            assert_eq!(path(5), None, "{mode:?} {rows} rows: an earlier copy's");
            assert_eq!(
                path(6),
                Some(ContainerPath::Str(RawStr::from("assets/0.prefab"))),
                "{mode:?} {rows} rows"
            );
        }
    }
}

/// The container pass holds the dependency whose table it walks, so a load
/// the walk makes that evicts it leaves its bundle, and the pointers into
/// its siblings, reachable (review round 1: `container()` failed with
/// `ContainerDropped`). `get_cab_held` gives a caller the same handle;
/// `get_cab` does not.
#[test]
fn the_container_pass_holds_the_dependency_it_walks() {
    let d = bundle(&[
        (
            "CAB-D",
            serialized(
                &["archive:/CAB-X/CAB-X", "archive:/CAB-D2/CAB-D2"],
                &[
                    Obj::named(1, "d1", &[]),
                    Obj::asset_bundle(
                        2,
                        Bundle {
                            preload: vec![(1, 1), (2, 3)],
                            rows: vec![row("assets/d1.prefab", (0, 2), (0, 1))],
                            ..Bundle::default()
                        },
                    ),
                ],
            ),
        ),
        ("CAB-D2", serialized(&[], &[Obj::named(3, "d3", &[])])),
    ]);
    let x = serialized(&[], &[Obj::named(1, "x1", &[])]);
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let mut o = EnvOptions::batch(Arc::new(FileContext::unitypy()));
        o.load = mode;
        o.dep_cache_bytes = 1;
        let env = Environment::new(o);
        drop(env.load_dependency(src("CAB-X", &x)).unwrap());
        drop(env.load_dependency(src("d.bundle", &d)).unwrap());
        // Each use of one dependency evicts the other.
        let evicted = env.stats().dependencies_evicted;
        assert_eq!(env.container().unwrap().count(), 0, "{mode:?}");
        assert!(env.stats().dependencies_evicted > evicted);

        let sibling = PPtr {
            file_id: 2,
            path_id: 3,
        };
        let held = env
            .get_cab_held(&key("cab-d"), &unlimited())
            .unwrap()
            .unwrap();
        assert!(held.hold.is_some());
        let Cab::Serialized(sf) = &held.cab else {
            panic!()
        };
        let _ = serialized_cab(&env, "cab-x");
        assert_eq!(name_of(&env.deref(sf, sibling).unwrap()), "d3");
        drop(held);
        let sf = serialized_cab(&env, "cab-d");
        let _ = serialized_cab(&env, "cab-x");
        assert!(
            matches!(
                env.deref(&sf, sibling),
                Err(DerefError::ContainerDropped { .. })
            ),
            "{mode:?}"
        );
    }
}

/// `obj.container` for objects a dependency's preload reached, where nothing
/// is evicted: a path the row's span gave a sibling's object, none for an
/// object no row reaches. Where a dependency may be evicted, an object of
/// one is refused (`PARITY.md` section 19.3), in both modes.
#[test]
fn container_paths_of_a_dependencys_objects() {
    let d = bundle(&[
        (
            "CAB-D",
            serialized(
                &["archive:/CAB-D2/CAB-D2"],
                &[
                    Obj::named(1, "d1", &[]),
                    Obj::asset_bundle(
                        2,
                        Bundle {
                            preload: vec![(1, 3)],
                            rows: vec![row("assets/d1.prefab", (0, 1), (0, 1))],
                            ..Bundle::default()
                        },
                    ),
                ],
            ),
        ),
        (
            "CAB-D2",
            serialized(&[], &[Obj::named(3, "d3", &[]), Obj::named(4, "d4", &[])]),
        ),
    ]);
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let env = env(mode);
        drop(env.load_dependency(src("d.bundle", &d)).unwrap());
        let path = |cab: &str, id: i64| {
            env.container_path(&object(&serialized_cab(&env, cab), id))
                .unwrap()
        };
        let d1 = ContainerPath::Str(RawStr::from("assets/d1.prefab"));
        assert_eq!(path("cab-d", 1), Some(d1.clone()), "{mode:?}");
        assert_eq!(path("cab-d2", 3), Some(d1), "{mode:?}");
        assert_eq!(path("cab-d2", 4), None, "{mode:?}");
        assert_eq!(path("cab-d", 2), None, "{mode:?}");

        let mut o = EnvOptions::batch(Arc::new(FileContext::unitypy()));
        o.load = mode;
        let batch = Environment::new(o);
        let _hold = batch.load_dependency(src("d.bundle", &d)).unwrap();
        for (cab, id) in [("cab-d", 1), ("cab-d2", 3)] {
            let r = batch.container_path(&object(&serialized_cab(&batch, cab), id));
            assert!(
                matches!(
                    r,
                    Err(Error::Unsupported(Unsupported::EvictableContainerPath))
                ),
                "{mode:?} {cab}: {r:?}"
            );
        }
        // A file the caller loaded is never evicted: its paths are kept.
        batch.load(src("c.bundle", &d)).unwrap();
        let sf = match batch.files().next().unwrap().file {
            File::Bundle(b) => match b.entry(0).unwrap() {
                File::Serialized(sf) => sf,
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        };
        assert_eq!(
            batch.container_path(&object(&sf, 1)).unwrap(),
            Some(ContainerPath::Str(RawStr::from("assets/d1.prefab"))),
            "{mode:?}"
        );
    }
}

/// The container preload walks each registered file's table once, as
/// UnityPy, which never evicts, does: a dependency evicted and opened again
/// is not walked again (review round 1's fix: the mark lived in the parsed
/// file, so a reopen walked again, and a pointer the first walk skipped
/// could raise once a later load registered a raw file under its name).
#[test]
fn a_reopened_dependency_is_not_walked_again() {
    let d = serialized(
        &["archive:/CAB-R/CAB-R.resS"],
        &[
            Obj::named(1, "d1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(1, 1)],
                    rows: vec![row("assets/d1.prefab", (0, 1), (0, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let dir = TempDir::new("walk");
    let path = dir.0.join("CAB-D");
    std::fs::write(&path, &d).unwrap();
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let mut o = EnvOptions::batch(Arc::new(FileContext::unitypy()));
        o.load = mode;
        o.dep_cache_bytes = 1;
        let env = Environment::new(o);
        drop(env.load_dependency(Source::Path(path.clone())).unwrap());
        // CAB-R.resS is not found: the walk skips the pointer.
        assert_eq!(env.container().unwrap().count(), 0);
        let walked = env.stats().preload_dereferences;
        // Evicts CAB-D, and registers a raw file under CAB-R's key.
        drop(
            env.load_dependency(src("CAB-R.resS", &res_bytes(64, 3)))
                .unwrap(),
        );
        assert!(env.stats().dependencies_evicted > 0, "{mode:?}");
        // The pass after the load walks nothing again: UnityPy's would
        // find CAB-D's table walked.
        assert_eq!(env.container().unwrap().count(), 0, "{mode:?}");
        assert_eq!(env.stats().preload_dereferences, walked, "{mode:?}");
    }
}
