//! The container preload's answers for a file's externals (plan decision
//! 11): what a walk keeps, and how many lookups it makes, counted in steps
//! (`EnvStats::preload_lookups`), never by a clock.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use unityrs_core::container::ContainerPath;
use unityrs_core::env::{Cab, CabResolver, EnvOptions, Environment, LoadMode, Source};
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::env::{Bundle, Obj, bundle, row, serialized};
use unityrs_testkit::guard;

use super::{key, object, src, unlimited};

/// Each mode under `EnvOptions::unitypy()` (false) and `batch()` (true).
fn modes() -> [(LoadMode, bool); 4] {
    [
        (LoadMode::Eager, false),
        (LoadMode::Eager, true),
        (LoadMode::Lazy, false),
        (LoadMode::Lazy, true),
    ]
}

fn options(mode: LoadMode, batch: bool) -> EnvOptions {
    let mut o = if batch {
        EnvOptions::batch(Arc::new(FileContext::unitypy()))
    } else {
        EnvOptions::unitypy()
    };
    o.load = mode;
    o
}

/// `CAB-F` in `f.bundle`, whose one row's preload span covers `table`, an
/// element per `(file id, path id)`, over `externals`.
fn walked(externals: &[String], table: Vec<(i32, i64)>) -> Vec<u8> {
    let externals: Vec<&str> = externals.iter().map(String::as_str).collect();
    let span = i32::try_from(table.len()).unwrap();
    let f = serialized(
        &externals,
        &[
            Obj::named(1, "f1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: table,
                    rows: vec![row("assets/f.prefab", (0, span), (0, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    bundle(&[("CAB-F", f)])
}

/// A walk keeps an external's miss until its own lookups change the key the
/// miss read: the second element's resolver load registers `cab-a` (the
/// resolver answers `cab-l` with a file named so), and the third element
/// finds there what the first missed, so `CAB-A`'s object 5 gets the row's
/// path, as UnityPy's next lookup finds it. Three lookups: the first, the
/// load (whose answer is never kept) and the one the change expired.
#[test]
fn a_kept_miss_is_asked_again_after_the_walks_own_change_to_its_key() {
    #[derive(Debug)]
    struct Renames(Bytes);
    impl CabResolver for Renames {
        fn resolve(&self, name: &[u8]) -> Option<Source> {
            (name == b"cab-l").then(|| Source::Bytes {
                name: RawStr::from("cab-a"),
                data: self.0.clone(),
            })
        }
    }
    let a = serialized(&[], &[Obj::named(3, "a3", &[]), Obj::named(5, "a5", &[])]);
    let externals = [
        "archive:/CAB-A/CAB-A".to_owned(),
        "archive:/CAB-L/CAB-L".to_owned(),
    ];
    let f = walked(&externals, vec![(1, 5), (2, 3), (1, 5)]);
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let mut o = options(mode, false);
        o.resolver = Some(Arc::new(Renames(Bytes::from(a.clone()))));
        let env = Environment::new(o);
        env.load(src("f.bundle", &f)).unwrap();
        // The walk's load made a new key: the pass fails as UnityPy's does
        // (`RuntimeError`), and the next one ends.
        assert!(env.container().is_err(), "{mode:?}");
        assert!(env.container().is_ok(), "{mode:?}");
        assert_eq!(env.stats().preload_lookups, 3, "{mode:?}");
        let Some(Cab::Serialized(sf)) = env.get_cab(&key("cab-a"), &unlimited()).unwrap() else {
            panic!("{mode:?}: cab-a is no SerializedFile");
        };
        for path_id in [3, 5] {
            let path = env.container_path(&object(&sf, path_id)).unwrap();
            assert!(
                matches!(&path, Some(ContainerPath::Str(p)) if p.as_bytes() == b"assets/f.prefab"),
                "{mode:?} object {path_id}: {path:?}"
            );
        }
    }
}

/// Asks another thread for a load and waits for it to end, whenever the
/// walk asks the resolver for a name, then answers none: a load another
/// thread makes between two of the walk's lookups.
#[derive(Debug)]
struct Racing {
    go: Mutex<Sender<Option<usize>>>,
    done: Mutex<Receiver<()>>,
    asked: AtomicUsize,
}

impl CabResolver for Racing {
    fn resolve(&self, _name: &[u8]) -> Option<Source> {
        let i = self.asked.fetch_add(1, Ordering::SeqCst);
        self.go.lock().unwrap().send(Some(i)).unwrap();
        guard::wait(&self.done.lock().unwrap(), "the other thread's load");
        None
    }
}

/// Runs a container pass on `walked` (loaded after `before`) under `o`,
/// another thread making the load `load(i)` gives for the walk's `i`-th
/// resolver call; returns the Environment and the rows.
fn race(
    mut o: EnvOptions,
    before: &[(&str, &[u8])],
    walked: &[u8],
    load: impl Fn(usize) -> (String, Vec<u8>) + Send + 'static,
) -> (Environment, usize) {
    let (go_tx, go_rx) = channel::<Option<usize>>();
    let (done_tx, done_rx) = channel::<()>();
    o.resolver = Some(Arc::new(Racing {
        go: Mutex::new(go_tx.clone()),
        done: Mutex::new(done_rx),
        asked: AtomicUsize::new(0),
    }));
    let env = Environment::new(o);
    for (name, data) in before {
        env.load(src(name, data)).unwrap();
    }
    env.load(src("f.bundle", walked)).unwrap();
    let other = env.clone();
    let loader = thread::spawn(move || {
        while let Some(i) = guard::recv(&go_rx, "a load request") {
            let (name, data) = load(i);
            other.load(src(&name, &data)).unwrap();
            done_tx.send(()).unwrap();
        }
    });
    let walk = env.clone();
    let rows = guard::finishes("the container pass", move || {
        walk.container().map(Iterator::count).unwrap()
    });
    go_tx.send(None).unwrap();
    loader.join().unwrap();
    (env, rows)
}

/// A miss the walk kept is asked again after another thread registers its
/// key, as UnityPy's next lookup finds it: a resolver call waits for
/// another thread's load of `CAB-A`, so the last element finds what the
/// first missed and `CAB-A`'s object 5 gets the row's path. The call is the
/// second element's, or the first's own (`CAB-A` registered while its
/// lookup runs, so the key is watched before the lookup reads it). One
/// lookup per external, and one more because the key changed (review
/// round 4: the walk kept its answers past every change another thread
/// made).
#[test]
fn a_kept_miss_is_asked_again_after_another_threads_load_of_its_key() {
    let a = serialized(&[], &[Obj::named(5, "a5", &[])]);
    let externals = ["archive:/CAB-A/CAB-A".to_owned(), "archive:/T/t".to_owned()];
    let tables = [(vec![(1, 5), (2, 1), (1, 5)], 3), (vec![(1, 5), (1, 5)], 2)];
    for (table, lookups) in tables {
        let f = walked(&externals, table);
        for (mode, batch) in modes() {
            let a = a.clone();
            let load = move |_| ("CAB-A".to_owned(), a.clone());
            let (env, rows) = race(options(mode, batch), &[], &f, load);
            let what = format!("{mode:?} batch {batch} {lookups}");
            assert_eq!(rows, 1, "{what}");
            assert_eq!(env.stats().preload_lookups, lookups, "{what}");
            let Some(Cab::Serialized(sf)) = env.get_cab(&key("cab-a"), &unlimited()).unwrap()
            else {
                panic!("{what}: cab-a is no SerializedFile");
            };
            let path = env.container_path(&object(&sf, 5)).unwrap();
            assert!(
                matches!(&path, Some(ContainerPath::Str(p)) if p.as_bytes() == b"assets/f.prefab"),
                "{what}: {path:?}"
            );
        }
    }
}

/// A load another thread makes is seen by the walk's next lookup of the
/// external it changed, and puts back no table index the walk has
/// dereferenced, so what a walk does again, and counts, depends on its
/// input and its own loads alone (review round 13; a load of the walk's own
/// puts such an index back: `tests/env/revisit.rs`). Row 0 spans `CAB-A`'s
/// 5 and `t`'s 1, row 1 the first alone; the resolver call `CAB-A`'s miss
/// makes waits for another thread's load of `CAB-A`. Two dereferences and
/// two lookups, and row 1 gives `CAB-A`'s 5 no path.
#[test]
fn a_load_on_another_thread_puts_back_no_index_the_walk_dereferenced() {
    let a = serialized(&[], &[Obj::named(5, "a5", &[])]);
    let f = serialized(
        &["archive:/CAB-A/CAB-A", "archive:/T/t"],
        &[
            Obj::named(1, "f1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(1, 5), (2, 1)],
                    rows: vec![
                        row("assets/0.prefab", (0, 2), (0, 1)),
                        row("assets/1.prefab", (0, 1), (0, 1)),
                    ],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let f = bundle(&[("CAB-F", f)]);
    for (mode, batch) in modes() {
        let a = a.clone();
        let load = move |i| {
            if i == 0 {
                ("CAB-A".to_owned(), a.clone())
            } else {
                (
                    "CAB-Z".to_owned(),
                    serialized(&[], &[Obj::named(1, "z", &[])]),
                )
            }
        };
        let (env, rows) = race(options(mode, batch), &[], &f, load);
        let what = format!("{mode:?} batch {batch}");
        assert_eq!(rows, 2, "{what}");
        let stats = env.stats();
        assert_eq!(stats.preload_dereferences, 2, "{what}");
        assert_eq!(stats.preload_lookups, 2, "{what}");
        let Some(Cab::Serialized(sf)) = env.get_cab(&key("cab-a"), &unlimited()).unwrap() else {
            panic!("{what}: cab-a is no SerializedFile");
        };
        assert_eq!(env.container_path(&object(&sf, 5)).unwrap(), None, "{what}");
    }
}

/// A walk looks each external up once while another thread loads between
/// its lookups, under every mode and option set: `R` rounds of a pointer
/// to a missing file (which the resolver answers after a load on another
/// thread) and `K` pointers to registered CABs cost `K + R` lookups, not
/// one per element (review round 4: every change, another thread's
/// included, expired every answer the walk kept, so each round looked
/// every external up again: a table of pointers to long names cost the
/// table times the names).
#[test]
fn a_walk_looks_each_external_up_once_while_another_thread_loads() {
    const K: usize = 8;
    const R: usize = 8;
    let xs: Vec<(String, Vec<u8>)> = (0..K)
        .map(|i| {
            let name = format!("CAB-X{i}");
            let cab = serialized(&[], &[Obj::named(1, &format!("x{i}"), &[])]);
            (name, cab)
        })
        .collect();
    let entries: Vec<(&str, Vec<u8>)> = xs.iter().map(|(n, c)| (n.as_str(), c.clone())).collect();
    let x = bundle(&entries);
    let mut externals: Vec<String> = (0..K)
        .map(|i| format!("archive:/CAB-X{i}/CAB-X{i}"))
        .collect();
    externals.extend((0..R).map(|r| format!("archive:/T{r}/t{r}")));
    let mut table = Vec::new();
    for r in 0..R {
        table.push((i32::try_from(K + 1 + r).unwrap(), 1));
        table.extend((0..K).map(|i| (i32::try_from(i + 1).unwrap(), 1)));
    }
    let f = walked(&externals, table);
    for (mode, batch) in modes() {
        let load = |i| {
            let y = serialized(&[], &[Obj::named(1, "y", &[])]);
            (format!("CAB-Y{i}"), y)
        };
        let (env, count) = race(options(mode, batch), &[("x.bundle", &x)], &f, load);
        let what = format!("{mode:?} batch {batch}");
        assert_eq!(count, 1, "{what}: rows");
        let stats = env.stats();
        assert_eq!(
            stats.preload_dereferences,
            u64::try_from(R * (K + 1)).unwrap(),
            "{what}"
        );
        assert_eq!(
            stats.preload_lookups,
            u64::try_from(K + R).unwrap(),
            "{what}: lookups"
        );
    }
}
