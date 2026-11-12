//! The refusal counts: every kind once per refusal, returned or absorbed;
//! the work a meter past its allowance refuses; one event counted under two
//! kinds; and the allocator's refusals at the core's two marked sites.

use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use flate2::Compression;
use flate2::write::GzEncoder;
use unityrs_core::budget::{Counting, DecodeBuf, MemoryBudget, Reservation, Unbounded};
use unityrs_core::detect::check_file_type;
use unityrs_core::env::{Cab, EnvOptions, Environment};
use unityrs_core::files::BundleFile;
use unityrs_core::limits::{Limits, Meter, MeteredVec};
use unityrs_core::stream::Endian;
use unityrs_core::testing;
use unityrs_core::typetree::{NodeList, NodeRow, Tree, TreeError};
use unityrs_core::walk::{self, ReadCtx, ReadDetail, ReadError, ReadOptions, RefTypes};
use unityrs_core::{Bytes, Error, FileContext, LimitKind, RawStr};
use unityrs_testkit::env::{Obj, bundle, serialized};

use super::{
    ARMED, FALLIBLE, cab_bundle, counted, eager, full_vec, in_child, nesting, probing,
    room_for_1500, src,
};

fn unbounded() -> Arc<dyn MemoryBudget> {
    Arc::new(Unbounded)
}

/// A tree of `(level, type, name)` rows, compiled under the default limits.
fn tree(rows: &[(i32, &str, &str)]) -> Arc<Tree> {
    Tree::compile(list(rows), &Limits::default()).unwrap()
}

fn list(rows: &[(i32, &str, &str)]) -> Arc<NodeList> {
    let rows = rows
        .iter()
        .map(|&(level, ty, name)| NodeRow::new(level, ty, name))
        .collect();
    Arc::new(NodeList::trusted(rows).unwrap())
}

/// Whether the pure-Python reader's read of `data` by `rows` under
/// `limits` is refused at `kind`'s bound.
fn read_refused(rows: &[(i32, &str, &str)], data: &[u8], limits: Limits, kind: LimitKind) -> bool {
    let options = ReadOptions {
        limits,
        ..ReadOptions::default()
    };
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::NoFile,
        options: &options,
        meter: &meter,
    };
    let got = walk::python::read(&Bytes::copy_from_slice(data), 0, &tree(rows), &ctx);
    matches!(&got, Err(ReadError { detail: ReadDetail::Limit(l), .. }) if l.kind() == kind)
}

#[test]
fn refusals_are_counted_once_each_by_kind_returned_or_absorbed() {
    in_child("refusals::refusals_probe");
}

#[test]
#[ignore = "run by refusals_are_counted_once_each_by_kind_returned_or_absorbed"]
fn refusals_probe() {
    if !probing() {
        return;
    }
    // The meter's own bound, returned.
    let r = counted(|| {
        let m = Meter::new(10, unbounded(), Reservation::Input);
        assert!(m.charge(11).is_err());
    });
    assert_eq!((r.limit(LimitKind::InputMemory), r.total()), (1, 1));
    // The same, absorbed: the doubling is refused, the growth to 1,500
    // fits, and the push succeeds.
    let r = counted(|| {
        let m = Meter::new(room_for_1500(), unbounded(), Reservation::Input);
        full_vec(&m).push(1).unwrap();
    });
    assert_eq!((r.limit(LimitKind::InputMemory), r.total()), (1, 1));
    // Decode work.
    let r = counted(|| {
        let m = Meter::with_decode_work(u64::MAX, 5, unbounded(), Reservation::Input);
        assert!(m.count_decode(6).is_err());
    });
    assert_eq!((r.limit(LimitKind::DecodeWork), r.total()), (1, 1));
    // A block's output cap.
    let r = counted(|| {
        let m = Meter::unlimited();
        let mut out = DecodeBuf::new(&m, 0, 10, LimitKind::BlockOutput).unwrap();
        assert!(out.extend_from_slice(&[1; 11]).is_err());
    });
    assert_eq!((r.limit(LimitKind::BlockOutput), r.total()), (1, 1));
    // A budget's refusal, by its reservation, returned.
    for what in [
        Reservation::Input,
        Reservation::Dependency,
        Reservation::Provider,
    ] {
        let r = counted(|| {
            let m = Meter::new(u64::MAX, Arc::new(Counting::new(10)), what);
            assert!(m.charge(11).is_err());
        });
        assert_eq!((r.budget(what), r.total()), (1, 1), "{what:?}");
    }
    // A budget's refusal, absorbed.
    let r = counted(|| {
        let budget = Arc::new(Counting::new(room_for_1500()));
        let m = Meter::new(u64::MAX, budget, Reservation::Dependency);
        full_vec(&m).push(1).unwrap();
    });
    assert_eq!((r.budget(Reservation::Dependency), r.total()), (1, 1));
    // The allocator's refusal, by the meter's reservation: more than
    // `isize::MAX` bytes, refused before any allocator is asked.
    for what in [
        Reservation::Input,
        Reservation::Dependency,
        Reservation::Provider,
    ] {
        let r = counted(|| {
            let m = Meter::new(u64::MAX, unbounded(), what);
            let mut v: MeteredVec<u8> = MeteredVec::new(&m);
            let too_many = usize::try_from(isize::MAX).unwrap() + 1;
            assert!(v.reserve_exact(too_many).is_err());
        });
        assert_eq!((r.allocator(what), r.total()), (1, 1), "{what:?}");
    }
    // An input's byte bound, at a load.
    let r = counted(|| {
        let mut ctx = FileContext::unitypy();
        ctx.limits.max_input_bytes = 10;
        let env = eager(Arc::new(ctx));
        assert!(env.load(src("big", &[0; 100])).is_err());
    });
    assert_eq!((r.limit(LimitKind::InputBytes), r.total()), (1, 1));
    // A container too deep: refused once, and kept, so asked again it
    // answers without a second refusal.
    let r = counted(|| {
        let mut ctx = FileContext::unitypy();
        ctx.limits.max_container_depth = 0;
        let ctx = Arc::new(ctx);
        let data = Bytes::from(nesting(&cab_bundle("a")));
        let b = BundleFile::open(RawStr::from("o"), data, None, &Meter::unlimited(), &ctx).unwrap();
        for _ in 0..2 {
            assert!(
                matches!(b.entry(0), Err(Error::Limit(l)) if l.kind() == LimitKind::ContainerDepth)
            );
        }
    });
    assert_eq!((r.limit(LimitKind::ContainerDepth), r.total()), (1, 1));
}

#[test]
fn the_detection_tree_and_read_kinds_are_counted_once_each() {
    in_child("refusals::kinds_probe");
}

/// The kinds a detection, a type tree's compile and a read refuse.
#[test]
#[ignore = "run by the_detection_tree_and_read_kinds_are_counted_once_each"]
fn kinds_probe() {
    if !probing() {
        return;
    }
    // A gzip stream past the inflate bound, at its detection.
    let r = counted(|| {
        let mut gz = GzEncoder::new(Vec::new(), Compression::best());
        gz.write_all(&vec![0u8; 1 << 20]).unwrap();
        let mut small = Limits::default();
        small.max_inflate = 1 << 16;
        assert!(check_file_type(&gz.finish().unwrap(), &small).is_err());
    });
    assert_eq!((r.limit(LimitKind::Inflate), r.total()), (1, 1));
    // A type tree deeper than its bound.
    let r = counted(|| {
        let mut limits = Limits::default();
        limits.max_tree_depth = 1;
        let got = Tree::compile(list(&[(0, "Base", "Base"), (1, "int", "a")]), &limits);
        assert!(
            matches!(&got, Err(TreeError::Limit(l)) if l.kind() == LimitKind::TreeDepth),
            "{got:?}"
        );
    });
    assert_eq!((r.limit(LimitKind::TreeDepth), r.total()), (1, 1));
    // A value deeper than its bound.
    let r = counted(|| {
        let mut limits = Limits::default();
        limits.max_value_depth = 1;
        let rows = [
            (0, "Base", "Base"),
            (1, "Inner", "s"),
            (2, "Inner", "t"),
            (3, "int", "a"),
        ];
        assert!(read_refused(&rows, &[0; 4], limits, LimitKind::ValueDepth));
    });
    assert_eq!((r.limit(LimitKind::ValueDepth), r.total()), (1, 1));
    // More values than an object may hold.
    let r = counted(|| {
        let mut limits = Limits::default();
        limits.max_values_per_object = 1;
        let rows = [(0, "Base", "Base"), (1, "int", "a"), (1, "int", "b")];
        assert!(read_refused(
            &rows,
            &[0; 8],
            limits,
            LimitKind::ValuesPerObject
        ));
    });
    assert_eq!((r.limit(LimitKind::ValuesPerObject), r.total()), (1, 1));
    // Elements that read nothing past their bound: eleven of a class of two
    // empty fields of one name, a registry in scope (`tests/walk_elements.rs`'s
    // "repeated name").
    let r = counted(|| {
        let mut limits = Limits::default();
        limits.max_zero_size_elements = 10;
        let rows = [
            (0, "Base", "Base"),
            (1, "ManagedReferencesRegistry", "reg"),
            (2, "int", "version"),
            (1, "vector", "v"),
            (2, "Array", "Array"),
            (3, "int", "size"),
            (3, "Foo", "data"),
            (4, "Empty", "x"),
            (4, "Empty", "x"),
        ];
        let mut data = 1_i32.to_le_bytes().to_vec();
        data.extend(11_i32.to_le_bytes());
        assert!(read_refused(
            &rows,
            &data,
            limits,
            LimitKind::ZeroSizeElements
        ));
    });
    assert_eq!((r.limit(LimitKind::ZeroSizeElements), r.total()), (1, 1));
    // `Count` (more than `u32::MAX` lists in one value) is refused where
    // every other kind is (`LimitError::refused`, which `counters.rs`'s scan
    // holds every refusal to); no test can build 2^32 lists.
}

/// A UnityFS 6 bundle whose blocks-info is flagged LZ4 and holds `0xF0`
/// then 64 bytes of `0xFF`, which liblz4 refuses (`tests/bundles/nested.rs`'s,
/// shorter): an entry of it fails to open, whatever reads it.
pub(super) fn broken_bundle() -> Vec<u8> {
    let mut broken = b"UnityFS\0".to_vec();
    broken.extend_from_slice(&6_u32.to_be_bytes());
    broken.extend_from_slice(b"5.x.x\0");
    broken.extend_from_slice(b"2018.4.0f1\0");
    let total = broken.len() + 8 + 12 + 1 + 64;
    broken.extend_from_slice(&i64::try_from(total).unwrap().to_be_bytes());
    broken.extend_from_slice(&65_u32.to_be_bytes());
    broken.extend_from_slice(&16_u32.to_be_bytes());
    broken.extend_from_slice(&0x42_u32.to_be_bytes());
    broken.push(0xF0);
    broken.resize(broken.len() + 64, 0xFF);
    broken
}

#[test]
fn the_work_an_open_past_its_allowance_refuses_is_counted_once_a_call() {
    in_child("refusals::work_probe");
}

/// `Meter::work_refused`, where the core refuses new work of an open past
/// its allowance (here an entry's parse, `crate::once`'s check before it
/// computes): one `DecodeWork` a refusing call, none for the refusal kept
/// and answered again, none for the query `Meter::work_used_up`.
#[test]
#[ignore = "run by the_work_an_open_past_its_allowance_refuses_is_counted_once_a_call"]
fn work_probe() {
    if !probing() {
        return;
    }
    let good = serialized(&[], &[Obj::named(1, "w", &[])]);
    let data = Bytes::from(bundle(&[
        ("broken", broken_bundle()),
        ("CAB-w", good.clone()),
        ("CAB-v", good),
    ]));
    let ctx = Arc::new(FileContext::unitypy());
    let open = |allowance: u64| {
        let m = Meter::with_decode_work(u64::MAX, allowance, unbounded(), Reservation::Input);
        let b = BundleFile::open(RawStr::from("w"), data.clone(), None, &m, &ctx).unwrap();
        (m, b)
    };
    // The work the open and a read of row 0 take: the allowance. Row 0's
    // parse then fails, and what it was granted takes the open past it.
    let (m, b) = open(u64::MAX);
    b.entry_bytes(0).unwrap();
    let (m, b) = open(m.decoded());
    assert!(b.entry(0).is_err());
    assert!(
        m.work_used_up().is_some(),
        "{} of the allowance",
        m.decoded()
    );
    let refused = |b: &BundleFile, row| {
        counted(|| {
            let got = b.entry(row);
            assert!(
                matches!(&got, Err(Error::Limit(l)) if l.kind() == LimitKind::DecodeWork),
                "row {row}: {got:?}"
            );
        })
    };
    for row in [1, 2] {
        let r = refused(&b, row);
        assert_eq!(
            (r.limit(LimitKind::DecodeWork), r.total()),
            (1, 1),
            "row {row}"
        );
    }
    let r = refused(&b, 1);
    assert_eq!(r.total(), 0, "a kept refusal answered again");
    let r = counted(|| assert!(m.work_used_up().is_some()));
    assert_eq!(r.total(), 0, "the query");
}

#[test]
fn one_refused_walk_counts_its_meters_refusal_and_the_work_it_returns() {
    in_child("refusals::two_kinds_probe");
}

/// The container pass's walk of a preload table that the file's own meter
/// refuses, once its grants take the open past its allowance
/// (`env::container_index`'s `walk_counted`, as `tests/env/reach.rs`'s
/// `a_pass_the_meter_refuses_is_counted_until_it_is_kept` measures it):
/// one event, two refusals, the meter's `InputMemory`, absorbed, and the
/// `DecodeWork` returned in its place; the pass kept, so none after.
#[test]
#[ignore = "run by one_refused_walk_counts_its_meters_refusal_and_the_work_it_returns"]
fn two_kinds_probe() {
    if !probing() {
        return;
    }
    let scene = unityrs_testkit::env::preload_paths();
    let data = scene.files[0].1.clone();
    let env_of = |ctx: FileContext| {
        let mut o = EnvOptions::unitypy();
        o.context = Arc::new(ctx);
        Environment::new(o)
    };
    let pass = |env: &Environment| env.container().map(|_| ()).map_err(|e| e.to_string());
    // The memory the reads take, and what the walk keeps.
    let env = env_of(FileContext::unitypy());
    let meter = env.load(src("pp.bundle", &data)).unwrap().meter().clone();
    for cab in ["CAB-PP", "CAB-PQ"] {
        let Some(Cab::Serialized(sf)) = env
            .get_cab(&RawStr::from(cab), &Meter::unlimited())
            .unwrap()
        else {
            panic!("{cab}");
        };
        sf.container().unwrap();
    }
    let read = meter.used();
    pass(&env).unwrap();
    let walked = meter.used();
    // The work a refused walk adds.
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_input_memory = read + (walked - read) / 2;
    let env = env_of(ctx.clone());
    let meter = env.load(src("pp.bundle", &data)).unwrap().meter().clone();
    let first = pass(&env);
    assert!(
        first
            .as_ref()
            .is_err_and(|e| e.starts_with("limit InputMemory")),
        "{first:?}"
    );
    let after_first = meter.decoded();
    assert_eq!(pass(&env), first);
    let per_walk = meter.decoded() - after_first;
    assert!(per_walk > 0);
    // Half a walk's work allowed past the reads: the first refused walk
    // uses the allowance up.
    ctx.limits.max_decode_work = after_first - per_walk / 2;
    let env = env_of(ctx);
    drop(env.load(src("pp.bundle", &data)).unwrap());
    let mut got = Ok(());
    let r = counted(|| got = pass(&env));
    assert!(
        got.as_ref()
            .is_err_and(|e| e.starts_with("limit DecodeWork")),
        "{got:?}"
    );
    assert_eq!(
        (
            r.limit(LimitKind::InputMemory),
            r.limit(LimitKind::DecodeWork),
            r.total()
        ),
        (1, 1, 2),
        "{r:?}"
    );
    // The pass is kept as UnityPy keeps a failed one (the index built):
    // the passes after it refuse nothing.
    let mut kept = Ok(());
    let r = counted(|| kept = pass(&env));
    assert_eq!(r.total(), 0, "the pass after: {kept:?}");
    let r = counted(|| assert_eq!(pass(&env), kept));
    assert_eq!(r.total(), 0, "and the one after it");
}

#[test]
fn an_armed_refusal_lands_only_in_a_marked_site() {
    in_child("refusals::armed_probe");
}

#[test]
#[ignore = "run by an_armed_refusal_lands_only_in_a_marked_site"]
fn armed_probe() {
    if !probing() {
        return;
    }
    // An allocation outside the helper is not marked.
    let seen = FALLIBLE.load(Ordering::Relaxed);
    let plain: Vec<u8> = std::hint::black_box(Vec::with_capacity(4096));
    assert_eq!(FALLIBLE.load(Ordering::Relaxed), seen);
    drop(plain);
    // A charged one is.
    let mut v: MeteredVec<u8> = MeteredVec::new(&Meter::unlimited());
    v.push(1).unwrap();
    assert!(FALLIBLE.load(Ordering::Relaxed) > seen);
    assert!(!testing::in_fallible_reserve());
    // Armed, an allocation outside the helper is still granted, and the
    // helper's next one is refused, as the allocator's refusal.
    ARMED.store(true, Ordering::Release);
    let plain: Vec<u8> = std::hint::black_box(Vec::with_capacity(4096));
    drop(plain);
    assert!(
        ARMED.load(Ordering::Acquire),
        "a plain allocation took the arm"
    );
    let r = counted(|| {
        let mut v: MeteredVec<u8> = MeteredVec::new(&Meter::unlimited());
        assert!(v.reserve(64).is_err());
    });
    assert!(!ARMED.load(Ordering::Acquire));
    assert_eq!((r.allocator(Reservation::Input), r.total()), (1, 1));
    assert!(!testing::in_fallible_reserve());
    // The other marked site: a buffer of room that may be mapped, given
    // back to 10 bytes, moves (`limits::shrink_moves`); armed, that move
    // is refused, once, as the allocator's refusal of its reservation.
    let m = Meter::new(u64::MAX, unbounded(), Reservation::Dependency);
    let mut v = MeteredVec::with_capacity(&m, 256 << 10).unwrap();
    v.extend_from_slice(&[1; 10]).unwrap();
    let seen = FALLIBLE.load(Ordering::Relaxed);
    ARMED.store(true, Ordering::Release);
    let r = counted(|| assert!(v.shrink_to_fit().is_err()));
    assert!(!ARMED.load(Ordering::Acquire), "the move was not marked");
    assert_eq!(FALLIBLE.load(Ordering::Relaxed), seen + 1);
    assert_eq!((r.allocator(Reservation::Dependency), r.total()), (1, 1));
    assert!(!testing::in_fallible_reserve());
}
