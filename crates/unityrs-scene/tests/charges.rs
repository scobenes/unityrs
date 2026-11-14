//! Observe heap growth against live charges at every allocation, then (with
//! test-counters) refuse the fallible allocations. No game data is used.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
};
use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation};
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::files::File;
use unityrs_core::limits::{Meter, heap_cost};
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_scene::{GraphOptions, graph_from_game_object};
use unityrs_testkit::counted::{Allocator, Tally, count_this_thread};
use unityrs_testkit::ngui::{Object, st};
use unityrs_testkit::scene3d::{self as s, Layout};

static LIVE: AtomicI64 = AtomicI64::new(0);
static CHARGED: AtomicI64 = AtomicI64::new(0);
static HEAP_BASE: AtomicI64 = AtomicI64::new(0);
static CHARGE_BASE: AtomicI64 = AtomicI64::new(0);
static OVER: AtomicI64 = AtomicI64::new(0);
static WATCH: AtomicBool = AtomicBool::new(false);
static REFUSE_AFTER: AtomicUsize = AtomicUsize::new(usize::MAX);
static REQUESTS: AtomicUsize = AtomicUsize::new(0);
struct Watching;
impl Tally for Watching {
    fn grew(&self, size: usize, _: bool) {
        let live = LIVE.fetch_add(
            i64::try_from(heap_cost(size as u64)).unwrap_or(i64::MAX),
            Ordering::Relaxed,
        ) + i64::try_from(heap_cost(size as u64)).unwrap_or(i64::MAX);
        if WATCH.load(Ordering::Relaxed) {
            OVER.fetch_max(
                live - HEAP_BASE.load(Ordering::Relaxed)
                    - (CHARGED.load(Ordering::Relaxed) - CHARGE_BASE.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
        }
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(
            i64::try_from(heap_cost(size as u64)).unwrap_or(i64::MAX),
            Ordering::Relaxed,
        );
    }
    fn refuses(&self, _: usize) -> bool {
        #[cfg(feature = "test-counters")]
        if unityrs_core::testing::in_fallible_reserve() {
            return REQUESTS.fetch_add(1, Ordering::Relaxed)
                >= REFUSE_AFTER.load(Ordering::Relaxed);
        }
        false
    }
}
#[global_allocator]
static ALLOCATOR: Allocator<Watching> = Allocator(Watching);
#[derive(Debug)]
struct Budget(Arc<Home>);
#[derive(Debug)]
struct Home;
impl PermitHome for Home {
    fn release(&self, bytes: u64) {
        CHARGED.fetch_sub(i64::try_from(bytes).unwrap(), Ordering::Relaxed);
    }
}
impl MemoryBudget for Budget {
    fn try_reserve(&self, bytes: u64, _: Reservation) -> Result<Permit, BudgetExceeded> {
        CHARGED.fetch_add(i64::try_from(bytes).unwrap(), Ordering::Relaxed);
        Ok(Permit::new(bytes, self.0.clone()))
    }
}

fn objects() -> Vec<Object> {
    let mut objects = vec![];
    for id in 1..=40 {
        let children = if id == 1 {
            (2..=40).map(|i| (0, 100 + i)).collect()
        } else {
            vec![]
        };
        objects.extend([
            Object::new(
                id,
                1,
                s::game_object(
                    b"heap",
                    &[(0, 100 + id), (0, 201), (0, 202), (0, 203), (0, 204)],
                ),
            ),
            Object::new(
                100 + id,
                4,
                s::transform(
                    (0, id),
                    (0, if id == 1 { 0 } else { 101 }),
                    &children,
                    false,
                ),
            ),
        ]);
    }
    objects.extend([
        Object::new(201, 33, s::mesh_filter((0, 1), (0, 300))),
        Object::new(202, 23, s::mesh_renderer((0, 1), &[(0, 301); 8], (1, 2))),
        Object::new(
            203,
            137,
            s::skinned_mesh_renderer((0, 1), (0, 300), &[(0, 101); 12], &[1.; 16]),
        ),
        Object::new(204, 205, s::lod_group((0, 1), &[(0.5, vec![(0, 202); 8])])),
        Object::new(300, 43, st("Mesh", vec![])),
        Object::new(301, 21, st("Material", vec![])),
    ]);
    objects
}

#[test]
fn arrays_are_charged_before_allocation_and_refusals_unwind_their_charges() {
    count_this_thread();
    let budget = Arc::new(Budget(Arc::new(Home)));
    let mut ctx = FileContext::unitypy();
    ctx.budget = budget.clone();
    let mut opts = EnvOptions::unitypy();
    opts.context = Arc::new(ctx);
    let env = Environment::new(opts);
    let objects = objects();
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"heap"),
            data: Bytes::from(s::file(Layout::Unity2022, &[], &objects).unwrap()),
        })
        .unwrap()
    else {
        panic!("file")
    };
    let root = file.object(1).unwrap();
    let meter = Meter::new(u64::MAX, budget, Reservation::Input);
    let opts = GraphOptions::default();
    // Compile/cache every tree and initialise the hasher before measuring.
    drop(graph_from_game_object(&env, &root, &opts, &meter).unwrap());
    HEAP_BASE.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    CHARGE_BASE.store(CHARGED.load(Ordering::Relaxed), Ordering::Relaxed);
    OVER.store(0, Ordering::Relaxed);
    REQUESTS.store(0, Ordering::Relaxed);
    WATCH.store(true, Ordering::Relaxed);
    let graph = graph_from_game_object(&env, &root, &opts, &meter).unwrap();
    WATCH.store(false, Ordering::Relaxed);
    assert_eq!(graph.nodes().len(), 40);
    assert_eq!(
        OVER.load(Ordering::Relaxed),
        0,
        "an allocation preceded its charge"
    );
    drop(graph);
    assert_eq!(meter.used(), 0);
    assert_eq!(
        LIVE.load(Ordering::Relaxed),
        HEAP_BASE.load(Ordering::Relaxed)
    );
    assert_eq!(
        CHARGED.load(Ordering::Relaxed),
        CHARGE_BASE.load(Ordering::Relaxed)
    );
    #[cfg(feature = "test-counters")]
    {
        let allocations = REQUESTS.load(Ordering::Relaxed);
        assert!(allocations > 100);
        for at in 0..allocations {
            REQUESTS.store(0, Ordering::Relaxed);
            REFUSE_AFTER.store(at, Ordering::Relaxed);
            let result = graph_from_game_object(&env, &root, &opts, &meter);
            REFUSE_AFTER.store(usize::MAX, Ordering::Relaxed);
            assert!(result.is_err(), "allocator refusal {at}");
            drop(result);
            assert_eq!(meter.used(), 0, "allocator refusal {at}");
            assert_eq!(
                CHARGED.load(Ordering::Relaxed),
                CHARGE_BASE.load(Ordering::Relaxed)
            );
        }
    }
    // Keep atomics referenced in the default feature build too.
    let _ = REFUSE_AFTER.load(Ordering::Relaxed);
}
