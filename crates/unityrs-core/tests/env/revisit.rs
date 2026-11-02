//! The container preload dereferences a table index again at a later row
//! whose span reaches it once a load of the walk's own has changed what its
//! external resolves to, as UnityPy's walk of every row's whole span finds
//! the new file there (`helpers/ContainerHelper.py:40-47`; review round
//! 13), and counts each step past an index's first dereference and an
//! external's first lookup as work on the walking file's meter. Counted in
//! steps (`EnvStats`, the meter's work counter), never by a clock.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_core::container::ContainerPath;
use unityrs_core::env::{CabResolver, EnvOptions, Environment, LoadMode, Source};
use unityrs_core::error::{Error, LimitKind};
use unityrs_core::walk::STEP_WORK;
use unityrs_core::{Bytes, RawStr};
use unityrs_testkit::env::{Bundle, Obj, bundle, preload_revisits_after_load, row, serialized};

use super::{file_of, object, serialized_cab, src};

/// `env.path` holding one file, `cab-q`, which the walk finds for every
/// name it ends with (UnityPy's suffix match, `environment.py:345-354`),
/// counting what it is asked.
#[derive(Debug)]
struct Suffix(AtomicUsize, Bytes);

impl CabResolver for Suffix {
    fn resolve(&self, name: &[u8]) -> Option<Source> {
        self.0.fetch_add(1, Ordering::SeqCst);
        b"cab-q".ends_with(name).then(|| Source::Bytes {
            name: RawStr::from("@/cab-q"),
            data: self.1.clone(),
        })
    }
}

/// An Environment under `EnvOptions::unitypy()` in `mode`, with `found` as
/// `env.path`'s `cab-q` and the limit on each open's work `work`.
fn environment(mode: LoadMode, found: &[u8], work: u64) -> (Environment, Arc<Suffix>) {
    let resolver = Arc::new(Suffix(AtomicUsize::new(0), Bytes::copy_from_slice(found)));
    let mut o = EnvOptions::unitypy();
    let mut context = (*o.context).clone();
    context.limits.max_decode_work = work;
    o.context = Arc::new(context);
    o.load = mode;
    o.resolver = Some(resolver.clone());
    (Environment::new(o), resolver)
}

/// What putting one index back costs a walk over a table of at most 64
/// elements, whose pending set has one level: a step for the dereference
/// and one for the level (`container_index::reopen_work`).
const REOPEN_ONE_LEVEL: u64 = 2 * STEP_WORK;

/// The cost of a lookup of an external the walk looked up before: a step
/// and its path's bytes.
fn relookup(path: &str) -> u64 {
    STEP_WORK + u64::try_from(path.len()).unwrap()
}

/// The review's case (`tests/fixtures/d/env.json` scene
/// `preload_revisits_after_load`, which `tools/fixtures/env.py` records
/// from UnityPy): row `b` dereferences `CAB-Q`'s 6 again after row `a`'s
/// load of `q` put `env.path`'s copy under `cab-q`, so that copy's 6 gets
/// row `b`'s path. Three dereferences, three lookups (`cab-q`, `q`, and
/// `cab-q` again), one load; the walk counts one index put back and one
/// lookup of an external it looked up before.
#[test]
fn a_later_row_dereferences_again_an_index_whose_external_a_load_changed() {
    let scene = preload_revisits_after_load();
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let (env, resolver) = environment(mode, &scene.resolver[0].1, 32 << 30);
        env.load(file_of(&scene, 0)).unwrap();
        env.load(file_of(&scene, 1)).unwrap();
        let b = serialized_cab(&env, "cab-b");
        // Read first, so what the meter counts next is the walk's.
        b.container().unwrap();
        let before = b.meter().decoded();
        assert_eq!(env.container().unwrap().count(), 2, "{mode:?}");
        let stats = env.stats();
        assert_eq!(stats.preload_dereferences, 3, "{mode:?}");
        assert_eq!(stats.preload_lookups, 3, "{mode:?}");
        assert_eq!(resolver.0.load(Ordering::SeqCst), 1, "{mode:?}: one load");
        assert_eq!(
            b.meter().decoded() - before,
            REOPEN_ONE_LEVEL + relookup("archive:/CAB-Q/CAB-Q"),
            "{mode:?}: the walk's counted work"
        );
        let found = serialized_cab(&env, "cab-q");
        let path = |id| env.container_path(&object(&found, id)).unwrap();
        assert_eq!(
            path(5),
            Some(ContainerPath::Str(RawStr::from("assets/a.prefab")))
        );
        assert_eq!(
            path(6),
            Some(ContainerPath::Str(RawStr::from("assets/b.prefab")))
        );
    }
}

/// `CAB-B`'s table: `M` pointers through `archive:/CAB-Q/CAB-Q`, which the
/// caller's `CAB-Q` answers, then `M` through `q`, which the walk loads by
/// suffix; row `k` (from 1) spans the first `M + k`.
fn growing(m: usize) -> Vec<u8> {
    let mut preload = vec![(1, 5); m];
    preload.extend(vec![(2, 5); m]);
    let size = |k: usize| i32::try_from(m + k).unwrap();
    let rows = (1..=m)
        .map(|k| row(&format!("assets/{k}.prefab"), (0, size(k)), (0, 1)))
        .collect();
    let b = serialized(
        &["archive:/CAB-Q/CAB-Q", "q"],
        &[
            Obj::named(1, "b1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload,
                    rows,
                    ..Bundle::default()
                },
            ),
        ],
    );
    bundle(&[("CAB-B", b)])
}

/// The bound on a hostile table (plan decision 11; the decision on bounded
/// work, plan section 9): each of the `M` loads of `q` changes the key
/// `cab-q`, so the next row dereferences again all `M` indexes before it
/// (UnityPy's walk dereferences every row's span, and loads `q` again at
/// each of its indexes, which the core does not: `PARITY.md` section 19.3).
/// `M(M + 1)` dereferences and `2M` lookups, every index put back and every
/// lookup past an external's first counted on `CAB-B`'s meter, so a work
/// allowance below that stops the walk at `DecodeWork`, which the input
/// decides. The table has `2M` = 64 elements: one level.
#[test]
fn a_walk_counts_what_it_does_again_and_its_allowance_stops_it() {
    const M: usize = 32;
    let m = u64::try_from(M).unwrap();
    let found = serialized(&[], &[Obj::named(5, "found5", &[])]);
    let caller = bundle(&[("CAB-Q", serialized(&[], &[Obj::named(5, "c5", &[])]))]);
    let b = growing(M);
    let want = m * m * REOPEN_ONE_LEVEL
        + (m - 1) * relookup("archive:/CAB-Q/CAB-Q")
        + (m - 1) * relookup("q");
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let (env, resolver) = environment(mode, &found, 32 << 30);
        env.load(src("b.bundle", &b)).unwrap();
        env.load(src("c.bundle", &caller)).unwrap();
        let cab_b = serialized_cab(&env, "cab-b");
        cab_b.container().unwrap();
        let before = cab_b.meter().decoded();
        assert_eq!(env.container().unwrap().count(), M, "{mode:?}");
        let stats = env.stats();
        assert_eq!(stats.preload_dereferences, m * (m + 1), "{mode:?}");
        assert_eq!(stats.preload_lookups, 2 * m, "{mode:?}");
        assert_eq!(
            resolver.0.load(Ordering::SeqCst),
            M,
            "{mode:?}: a load per index"
        );
        assert_eq!(cab_b.meter().decoded() - before, want, "{mode:?}");
        // Each row's path went to the copy its own load made, the last row
        // to the last copy.
        let last = serialized_cab(&env, "cab-q");
        let path = env.container_path(&object(&last, 5)).unwrap();
        assert_eq!(
            path,
            Some(ContainerPath::Str(RawStr::new(
                format!("assets/{M}.prefab").into_bytes()
            )))
        );

        // The same walk with half of that work left after the read.
        let (env, _) = environment(mode, &found, before + want / 2);
        env.load(src("b.bundle", &b)).unwrap();
        env.load(src("c.bundle", &caller)).unwrap();
        let cab_b = serialized_cab(&env, "cab-b");
        cab_b.container().unwrap();
        assert_eq!(cab_b.meter().decoded(), before, "{mode:?}: the read's work");
        match env.container().err() {
            Some(Error::Limit(l)) => assert_eq!(l.kind, LimitKind::DecodeWork, "{mode:?}"),
            other => panic!("{mode:?}: {other:?}"),
        }
        assert!(env.stats().preload_dereferences < m * (m + 1), "{mode:?}");
    }
}

/// An index whose external the resolver loaded under another name (UnityPy's
/// suffix match: `ab.assets` finds `xab.assets`, `environment.py:345-354`)
/// is dereferenced again once a later load of the walk's own registers the
/// name it asked: `zz`, which the resolver answers with a file named
/// `ab.assets`. UnityPy's row 1 asks `get_cab("ab.assets")` first
/// (`environment.py:326-329`) and finds that file, whose 5 gets row 1's
/// path; the walk asks the resolver twice and the registry once more. A
/// caller's `xab.assets` holds that key, so only `ab.assets` is a new key,
/// which fails the pass after the walk, as UnityPy's loop over `env.cabs`
/// does, and the next pass ends.
#[test]
fn a_loads_index_is_dereferenced_again_once_its_name_is_registered() {
    #[derive(Debug)]
    struct Renames(AtomicUsize, Bytes);
    impl CabResolver for Renames {
        fn resolve(&self, name: &[u8]) -> Option<Source> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let found = match name {
                b"ab.assets" => "@/xab.assets",
                b"zz" => "@/ab.assets",
                _ => return None,
            };
            Some(Source::Bytes {
                name: RawStr::from(found),
                data: self.1.clone(),
            })
        }
    }
    let found = serialized(&[], &[Obj::named(5, "f5", &[]), Obj::named(6, "f6", &[])]);
    let b = serialized(
        &["archive:/CAB-B/ab.assets", "zz"],
        &[
            Obj::named(1, "b1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(1, 5), (2, 6)],
                    rows: vec![
                        row("assets/0.prefab", (0, 2), (0, 1)),
                        row("assets/1.prefab", (0, 1), (0, 1)),
                    ],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let b = bundle(&[("CAB-B", b)]);
    let caller = serialized(&[], &[Obj::named(5, "c5", &[])]);
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let resolver = Arc::new(Renames(AtomicUsize::new(0), Bytes::from(found.clone())));
        let mut o = EnvOptions::unitypy();
        o.load = mode;
        o.resolver = Some(resolver.clone());
        let env = Environment::new(o);
        env.load(src("caller/xab.assets", &caller)).unwrap();
        env.load(src("b.bundle", &b)).unwrap();
        let e = env.container().expect_err("the walk made a key");
        assert!(e.to_string().contains("changed size"), "{mode:?}: {e}");
        assert_eq!(env.container().unwrap().count(), 2, "{mode:?}");
        let stats = env.stats();
        assert_eq!(stats.preload_dereferences, 3, "{mode:?}");
        assert_eq!(stats.preload_lookups, 3, "{mode:?}");
        assert_eq!(resolver.0.load(Ordering::SeqCst), 2, "{mode:?}");
        let path = |cab: &str, id| {
            let sf = serialized_cab(&env, cab);
            env.container_path(&object(&sf, id)).unwrap()
        };
        let row = |r: &str| Some(ContainerPath::Str(RawStr::from(r)));
        assert_eq!(path("xab.assets", 5), row("assets/0.prefab"), "{mode:?}");
        assert_eq!(path("ab.assets", 5), row("assets/1.prefab"), "{mode:?}");
        assert_eq!(path("ab.assets", 6), row("assets/0.prefab"), "{mode:?}");
    }
}
