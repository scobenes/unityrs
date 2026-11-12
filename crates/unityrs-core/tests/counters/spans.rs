//! The span reads: each read counted once by load and row; the bare key;
//! a nested bundle's and a reopen's load id; a load id kept across a retry,
//! a resolver's among them; paths as deep as the reserved width; overflow;
//! a reader that calls the tables again; and nothing allocated with no
//! table reserved.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use unityrs_core::env::{Cab, CabResolver, EnvOptions, Environment, Fetch, LoadMode, Source};
use unityrs_core::files::BundleFile;
use unityrs_core::limits::Meter;
use unityrs_core::testing;
use unityrs_core::{Bytes, Error, FileContext, RawStr};
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build};
use unityrs_testkit::guard::finishes;

use super::refusals::broken_bundle;
use super::{
    MADE, Pool, cab_bundle, counted, eager, id_of, in_child, lazy, nested, nesting, probing, row,
    src, tables, the_bundle, whole,
};

#[test]
fn span_reads_are_counted_once_by_load_and_row() {
    in_child("spans::span_reads_probe");
}

#[test]
#[ignore = "run by span_reads_are_counted_once_by_load_and_row"]
fn span_reads_probe() {
    if !probing() {
        return;
    }
    let env = lazy();
    // With no table reserved nothing is recorded: a load of a bundle whose
    // row 0 is a bundle, and that nested bundle.
    let early = env
        .load(src("early", &nesting(&nesting(&cab_bundle("e")))))
        .unwrap();
    let mid = nested(the_bundle(&early), 0);
    let (slots, names, overflowed) = tables();
    assert!(slots.is_empty() && names.is_empty() && !overflowed);
    // A method path is not general enough over the reader's lifetime.
    #[allow(clippy::redundant_closure_for_method_calls)]
    let reserved = testing::span_reads(|r| r.reserved());
    assert!(!reserved);

    testing::reserve_span_reads(16, 64, 17);
    // Bundles opened before the reserve stay untracked after it, and so
    // does a bundle nested in one, opened after it (round 3): their reads,
    // uncached, count nothing and overflow nothing.
    the_bundle(&early).entry_bytes(0).unwrap();
    mid.entry_bytes(0).unwrap();
    let deep = nested(&mid, 0);
    deep.entry(0).unwrap();
    deep.entry_bytes(1).unwrap();
    let (slots, names, overflowed) = tables();
    assert!(slots.is_empty() && names.is_empty() && !overflowed);
    let file = env.load(src("b", &cab_bundle("a"))).unwrap();
    let b_id = id_of("b");
    let id = Some(b_id);
    // The open decoded its blocks-info, and the lazy load registered the
    // rows unread: no span read.
    assert_eq!(tables().0, []);
    let b = the_bundle(&file);
    // A parse reads its row once; a cached entry reads nothing.
    b.entry(0).unwrap();
    b.entry(0).unwrap();
    // A range and the whole entry are a read each.
    b.entry_range(1, 0..4).unwrap();
    b.entry_bytes(1).unwrap();
    // The eager load: the whole file, then the one row not parsed yet.
    b.load_all().unwrap();
    let (slots, names, overflowed) = tables();
    assert_eq!(
        slots,
        [row(id, &[0], 1), row(id, &[1], 3), whole(id, &[], 1)]
    );
    assert_eq!(names, [(b_id, b"b".to_vec())]);
    assert!(!overflowed);

    // A bundle opened outside a load is read under the bare key.
    testing::reserve_span_reads(16, 64, 17);
    let ctx = Arc::new(FileContext::unitypy());
    let data = Bytes::from(cab_bundle("c"));
    let bare = BundleFile::open(RawStr::from("c"), data, None, &Meter::unlimited(), &ctx).unwrap();
    bare.entry(0).unwrap();
    assert_eq!(tables().0, [row(None, &[0], 1)]);
}

#[test]
fn a_twins_kept_failure_reads_nothing() {
    in_child("spans::twins_probe");
}

#[test]
#[ignore = "run by a_twins_kept_failure_reads_nothing"]
fn twins_probe() {
    if !probing() {
        return;
    }
    // Two rows over one span that fails to open as a bundle: the second
    // answers the first's kept failure.
    let broken = broken_bundle();
    let entry = |name: &str, data: &[u8]| EntrySpec {
        name: name.as_bytes().to_vec(),
        data: data.to_vec(),
        flags: 4,
    };
    // A stored blocks-info, so the second row can be moved in place.
    let mut data = build(&BundleSpec {
        format: 6,
        version_engine: b"2018.4.0f1".to_vec(),
        info_codec: TestCodec::None,
        data_codec: TestCodec::None,
        block_size: broken.len(),
        entries: vec![entry("CAB-00000000", &broken), entry("CAB-00000001", &[])],
        ..BundleSpec::default()
    });
    let second = data
        .windows(13)
        .position(|w| w == b"CAB-00000001\0")
        .unwrap();
    data[second - 20..second - 12].copy_from_slice(&0_i64.to_be_bytes());
    let len = i64::try_from(broken.len()).unwrap();
    data[second - 12..second - 4].copy_from_slice(&len.to_be_bytes());
    testing::reserve_span_reads(16, 64, 17);
    let env = lazy();
    let file = env.load(src("t", &data)).unwrap();
    let id = Some(id_of("t"));
    let b = the_bundle(&file);
    assert!(b.entry(0).is_err());
    assert!(b.entry(1).is_err());
    assert_eq!(tables().0, [row(id, &[0], 1)]);
}

/// A fetcher that counts its runs.
#[derive(Debug)]
struct Fetcher {
    data: Bytes,
    runs: AtomicU64,
}

impl Fetch for Fetcher {
    fn fetch(&self) -> Result<Bytes, Error> {
        self.runs.fetch_add(1, Ordering::AcqRel);
        Ok(self.data.clone())
    }
}

#[test]
fn a_nested_bundle_and_a_reopen_keep_their_loads_id() {
    in_child("spans::inherit_probe");
}

#[test]
#[ignore = "run by a_nested_bundle_and_a_reopen_keep_their_loads_id"]
fn inherit_probe() {
    if !probing() {
        return;
    }
    testing::reserve_span_reads(16, 64, 17);
    // Nested: the inner bundle's rows carry the outer load's id and the
    // path through the row that holds it.
    let env = lazy();
    let file = env.load(src("outer", &nesting(&cab_bundle("n")))).unwrap();
    let id = Some(id_of("outer"));
    let inner = nested(the_bundle(&file), 0);
    inner.entry(1).unwrap();
    assert_eq!(tables().0, [row(id, &[0], 1), row(id, &[0, 1], 1)]);

    // A dependency evicted and opened again reads under its load's id: its
    // fetcher's second run is the reopen.
    testing::reserve_span_reads(16, 64, 17);
    let mut o = EnvOptions::batch(Arc::new(FileContext::unitypy()));
    o.dep_cache_bytes = 1;
    let env = Environment::new(o);
    let fetcher = Arc::new(Fetcher {
        data: Bytes::from(cab_bundle("a")),
        runs: AtomicU64::new(0),
    });
    let a_src = Source::Fetch {
        name: RawStr::from("a.bundle"),
        fetch: fetcher.clone(),
    };
    drop(env.load_dependency(a_src).unwrap());
    drop(
        env.load_dependency(src("b.bundle", &cab_bundle("b")))
            .unwrap(),
    );
    assert_eq!(fetcher.runs.load(Ordering::Acquire), 1);
    let a = Some(id_of("a.bundle"));
    let before: u64 = tables().0.iter().filter(|s| s.0 == a).map(|s| s.3).sum();
    let Some(Cab::Serialized(_)) = env
        .get_cab(&RawStr::from("cab-a"), &Meter::unlimited())
        .unwrap()
    else {
        panic!("cab-a");
    };
    assert_eq!(
        fetcher.runs.load(Ordering::Acquire),
        2,
        "a was not reopened"
    );
    let (slots, names, overflowed) = tables();
    let after: u64 = slots.iter().filter(|s| s.0 == a).map(|s| s.3).sum();
    assert!(after > before, "{slots:?}");
    assert!(slots.iter().all(|s| s.0.is_some()), "{slots:?}");
    assert_eq!(names.len(), 2, "one id a load: {names:?}");
    assert!(!overflowed);
}

#[test]
fn a_reopen_after_a_reserve_counts_under_its_unnamed_load_id() {
    in_child("spans::late_reopen_probe");
}

/// A dependency loaded before a reserve, evicted, and opened again after
/// it: the reopen keys its bundle by the id its load drew, which a reserve
/// made since counts, though the draw recorded no name (the table had none
/// to take it). So its reads are counted under an id the names never list.
#[test]
#[ignore = "run by a_reopen_after_a_reserve_counts_under_its_unnamed_load_id"]
fn late_reopen_probe() {
    if !probing() {
        return;
    }
    let mut o = EnvOptions::batch(Arc::new(FileContext::unitypy()));
    o.dep_cache_bytes = 1;
    let env = Environment::new(o);
    let fetcher = Arc::new(Fetcher {
        data: Bytes::from(cab_bundle("a")),
        runs: AtomicU64::new(0),
    });
    let a_src = Source::Fetch {
        name: RawStr::from("a.bundle"),
        fetch: fetcher.clone(),
    };
    drop(env.load_dependency(a_src).unwrap());
    drop(
        env.load_dependency(src("b.bundle", &cab_bundle("b")))
            .unwrap(),
    );
    testing::reserve_span_reads(16, 64, 17);
    let Some(Cab::Serialized(_)) = env
        .get_cab(&RawStr::from("cab-a"), &Meter::unlimited())
        .unwrap()
    else {
        panic!("cab-a");
    };
    assert_eq!(
        fetcher.runs.load(Ordering::Acquire),
        2,
        "a was not reopened"
    );
    let (slots, names, overflowed) = tables();
    assert!(!slots.is_empty() && !overflowed, "{slots:?}");
    let load = slots[0].0;
    assert!(load.is_some(), "{slots:?}");
    assert!(slots.iter().all(|s| s.0 == load), "{slots:?}");
    assert!(names.is_empty(), "{names:?}");
}

#[test]
fn a_load_id_is_kept_across_a_with_room_retry() {
    in_child("spans::retry_probe");
}

/// A resolver of one name, `r.bundle`.
#[derive(Debug)]
struct One(Bytes);

impl CabResolver for One {
    fn resolve(&self, name: &[u8]) -> Option<Source> {
        (name == b"r.bundle").then(|| Source::Bytes {
            name: RawStr::from("r.bundle"),
            data: self.0.clone(),
        })
    }
}

#[test]
#[ignore = "run by a_load_id_is_kept_across_a_with_room_retry"]
fn retry_probe() {
    if !probing() {
        return;
    }
    let data = cab_bundle("r");
    // Each load, its dependency reservations counted on a dry run, then
    // run again with its last refused, so `with_room` retries it.
    let retried = |load: &dyn Fn(&Environment), env: &dyn Fn(Arc<FileContext>) -> Environment| {
        let pool = Arc::new(Pool::default());
        let mut ctx = FileContext::unitypy();
        ctx.budget = pool.clone();
        let ctx = Arc::new(ctx);
        load(&env(ctx.clone()));
        let n = pool.asked.swap(0, Ordering::AcqRel);
        pool.n.store(n, Ordering::Release);
        testing::reserve_span_reads(16, 64, 17);
        let env = env(ctx);
        let r = counted(|| load(&env));
        assert_eq!(
            r.budget(unityrs_core::budget::Reservation::Dependency),
            1,
            "refused once"
        );
        assert!(pool.asked.load(Ordering::Acquire) > n, "and retried");
    };
    // A dependency's load (`load_dep`).
    retried(
        &|env: &Environment| drop(env.load_dependency(src("dep", &data)).unwrap()),
        &eager,
    );
    let id = Some(id_of("dep"));
    let (slots, names, overflowed) = tables();
    // Both attempts read the whole file and each row, under the one id.
    assert_eq!(
        slots,
        [whole(id, &[], 2), row(id, &[0], 2), row(id, &[1], 2)]
    );
    assert_eq!(names.len(), 1);
    assert!(!overflowed);
    // A resolver's load (`load_resolved`), through `find_file`.
    let resolver = Arc::new(One(Bytes::from(data.clone())));
    let with_resolver = |ctx: Arc<FileContext>| {
        let mut o = EnvOptions::unitypy();
        o.context = ctx;
        o.load = LoadMode::Eager;
        o.resolver = Some(resolver.clone() as Arc<dyn CabResolver>);
        Environment::new(o)
    };
    retried(
        &|env: &Environment| {
            let found = env
                .find_file(&RawStr::from("r.bundle"), true, &Meter::unlimited())
                .unwrap();
            assert!(found.is_some());
        },
        &with_resolver,
    );
    let id = Some(id_of("r.bundle"));
    let (slots, names, overflowed) = tables();
    assert_eq!(
        slots,
        [whole(id, &[], 2), row(id, &[0], 2), row(id, &[1], 2)]
    );
    assert_eq!(names.len(), 1);
    assert!(!overflowed);
}

#[test]
fn a_path_as_deep_as_the_reserved_width_is_counted() {
    in_child("spans::deep_probe");
}

/// Round 1: a bundle kept at most 32 indexes of its path, so a read deeper
/// than that set the overflow flag whatever width the table had.
#[test]
#[ignore = "run by a_path_as_deep_as_the_reserved_width_is_counted"]
fn deep_probe() {
    if !probing() {
        return;
    }
    let mut data = cab_bundle("d");
    for _ in 0..33 {
        data = nesting(&data);
    }
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_container_depth = 40;
    let env = Environment::new(EnvOptions::batch(Arc::new(ctx)));
    testing::reserve_span_reads(4, 64, 34);
    let file = env.load(src("deep", &data)).unwrap();
    let id = Some(id_of("deep"));
    let mut b = the_bundle(&file).clone();
    for _ in 0..33 {
        b = nested(&b, 0);
    }
    b.entry_bytes(1).unwrap();
    let (slots, _, overflowed) = tables();
    assert!(!overflowed);
    assert_eq!(slots.len(), 34);
    let mut path = vec![0; 33];
    path.push(1);
    assert_eq!(slots.last(), Some(&row(id, &path, 1)));
    // One index narrower, the same read does not fit.
    testing::reserve_span_reads(4, 64, 33);
    b.entry_bytes(1).unwrap();
    assert!(tables().2);
}

#[test]
fn a_table_that_does_not_fit_sets_the_overflow_flag() {
    in_child("spans::overflow_probe");
}

#[test]
#[ignore = "run by a_table_that_does_not_fit_sets_the_overflow_flag"]
fn overflow_probe() {
    if !probing() {
        return;
    }
    let env = lazy();
    let outer = nesting(&cab_bundle("n"));
    // A path deeper than the width: the row above it is still counted.
    testing::reserve_span_reads(4, 4, 1);
    let file = env.load(src("deep", &outer)).unwrap();
    nested(the_bundle(&file), 0).entry(0).unwrap();
    let (slots, _, overflowed) = tables();
    assert!(overflowed);
    assert_eq!(slots, [row(Some(id_of("deep")), &[0], 1)]);
    // The flag is sticky.
    the_bundle(&file).entry(0).unwrap();
    assert!(tables().2);
    // A longer name.
    testing::reserve_span_reads(4, 4, 17);
    assert!(!tables().2, "a new reserve starts over");
    let long = "n".repeat(testing::NAME_BYTES + 1);
    env.load(src(&long, &cab_bundle("l"))).unwrap();
    assert!(tables().2);
    // A full span-read table.
    testing::reserve_span_reads(4, 1, 17);
    let file = env.load(src("full", &cab_bundle("f"))).unwrap();
    let b = the_bundle(&file);
    b.entry(0).unwrap();
    assert!(!tables().2);
    b.entry(1).unwrap();
    let (slots, _, overflowed) = tables();
    assert!(overflowed);
    assert_eq!(slots, [row(Some(id_of("full")), &[0], 1)]);
    // A full load-name table.
    testing::reserve_span_reads(1, 4, 17);
    env.load(src("one", &cab_bundle("o"))).unwrap();
    assert!(!tables().2);
    env.load(src("two", &cab_bundle("t"))).unwrap();
    assert!(tables().2);
    // A read inside the reader is not counted and does not wait.
    testing::reserve_span_reads(4, 4, 17);
    let file = env.load(src("inside", &cab_bundle("i"))).unwrap();
    testing::span_reads(|_| the_bundle(&file).entry(0).unwrap());
    let (slots, _, overflowed) = tables();
    assert!(overflowed && slots.is_empty());
}

#[test]
fn a_reader_that_calls_the_tables_again_panics_and_does_not_hang() {
    in_child("spans::reentry_probe");
}

/// Round 1: a nested call waited on the lock its own reader held.
#[test]
#[ignore = "run by a_reader_that_calls_the_tables_again_panics_and_does_not_hang"]
fn reentry_probe() {
    if !probing() {
        return;
    }
    testing::reserve_span_reads(4, 4, 17);
    let nested = finishes("span_reads inside its reader", || {
        std::panic::catch_unwind(|| testing::span_reads(|_| testing::span_reads(|_| ()))).is_err()
    });
    assert!(nested, "span_reads inside its reader");
    let reserve = finishes("reserve_span_reads inside the reader", || {
        std::panic::catch_unwind(|| testing::span_reads(|_| testing::reserve_span_reads(1, 1, 1)))
            .is_err()
    });
    assert!(reserve, "reserve_span_reads inside the reader");
    // The tables, still reserved, read as before.
    let (slots, _, overflowed) = tables();
    assert!(slots.is_empty() && !overflowed);
}

/// A fetcher that opens a bundle of its own and reads a row of it before
/// it returns the source's bytes.
#[derive(Debug)]
struct Opener {
    own: Bytes,
    data: Bytes,
}

impl Fetch for Opener {
    fn fetch(&self) -> Result<Bytes, Error> {
        let ctx = Arc::new(FileContext::unitypy());
        let own = BundleFile::open(
            RawStr::from("own"),
            self.own.clone(),
            None,
            &Meter::unlimited(),
            &ctx,
        )?;
        own.entry(0)?;
        Ok(self.data.clone())
    }
}

#[test]
fn a_bundle_a_fetcher_opens_itself_is_bare() {
    in_child("spans::fetcher_probe");
}

/// Round 1: the load's key was set around its fetch too, so a fetcher's
/// own bundle was counted as the load's.
#[test]
#[ignore = "run by a_bundle_a_fetcher_opens_itself_is_bare"]
fn fetcher_probe() {
    if !probing() {
        return;
    }
    testing::reserve_span_reads(4, 8, 17);
    let env = lazy();
    let source = Source::Fetch {
        name: RawStr::from("f"),
        fetch: Arc::new(Opener {
            own: Bytes::from(cab_bundle("o")),
            data: Bytes::from(cab_bundle("f")),
        }),
    };
    let file = env.load(source).unwrap();
    the_bundle(&file).entry(0).unwrap();
    let id = Some(id_of("f"));
    assert_eq!(tables().0, [row(None, &[0], 1), row(id, &[0], 1)]);
}

#[test]
fn with_no_table_reserved_the_counters_allocate_nothing() {
    in_child("spans::quiet_probe");
}

/// A load and its reads make as many blocks with no table reserved as
/// with one recording them: recording a top-level bundle's reads allocates
/// nothing (a nested bundle's key's node is the counters' one allocation,
/// made while a table is reserved; the core's side with no table is
/// counted alone in `keys::hooks_probe`).
#[test]
#[ignore = "run by with_no_table_reserved_the_counters_allocate_nothing"]
fn quiet_probe() {
    if !probing() {
        return;
    }
    unityrs_testkit::counted::count_this_thread();
    let data = cab_bundle("q");
    let run = || {
        let env = lazy();
        let file = env.load(src("q", &data)).unwrap();
        let b = the_bundle(&file);
        b.entry(0).unwrap();
        b.entry_bytes(1).unwrap();
        b.load_all().unwrap();
    };
    let made = || {
        let before = MADE.load(Ordering::Relaxed);
        run();
        MADE.load(Ordering::Relaxed) - before
    };
    // The first run's one-time blocks.
    run();
    let without = made();
    assert!(without > 0, "the thread's blocks are counted");
    testing::reserve_span_reads(16, 64, 17);
    let with = made();
    let (slots, names, overflowed) = tables();
    assert!(!slots.is_empty() && !names.is_empty() && !overflowed);
    assert_eq!(without, with, "blocks made with no table, and recording");
}
