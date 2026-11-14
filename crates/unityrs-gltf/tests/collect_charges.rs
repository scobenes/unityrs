//! Sweep every actual allocation in model building, layout, JSON and streaming.
//! Refusal is sticky so an infallible retry or fallback aborts this test.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use unityrs_core::LimitKind;
use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation};
use unityrs_core::limits::{ChargeError, Meter};
use unityrs_gltf::*;
use unityrs_testkit::counted::{self, Allocator, Tally};
static LIVE: AtomicUsize = AtomicUsize::new(0);
static BASE: AtomicUsize = AtomicUsize::new(0);
static HELD: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);
static OVER: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static FAIL_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
static REFUSED: AtomicUsize = AtomicUsize::new(0);
struct Watching;
impl Tally for Watching {
    fn grew(&self, n: usize, _: bool) {
        let now = LIVE.fetch_add(n, Ordering::Relaxed) + n;
        let above = now.saturating_sub(BASE.load(Ordering::Relaxed)) as u64;
        OVER.fetch_max(
            above.saturating_sub(HELD.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
    }
    fn shrank(&self, n: usize) {
        LIVE.fetch_sub(n, Ordering::Relaxed);
    }
    fn refuses(&self, _: usize) -> bool {
        if !counted::counted() || !ARMED.load(Ordering::Relaxed) {
            return false;
        }
        let n = CALLS.fetch_add(1, Ordering::Relaxed);
        if n >= FAIL_AT.load(Ordering::Relaxed) {
            REFUSED.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}
#[global_allocator]
static ALLOC: Allocator<Watching> = Allocator(Watching);
#[derive(Debug)]
struct Home;
impl PermitHome for Home {
    fn release(&self, n: u64) {
        HELD.fetch_sub(n, Ordering::Relaxed);
    }
}
#[derive(Debug)]
struct Budget {
    home: Arc<Home>,
    calls: AtomicUsize,
    refuse: AtomicUsize,
}
impl MemoryBudget for Budget {
    fn try_reserve(&self, n: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        if self.calls.fetch_add(1, Ordering::Relaxed) >= self.refuse.load(Ordering::Relaxed) {
            return Err(BudgetExceeded { requested: n, what });
        }
        let held = HELD.fetch_add(n, Ordering::Relaxed) + n;
        PEAK.fetch_max(held, Ordering::Relaxed);
        Ok(Permit::new(n, self.home.clone()))
    }
}
mod collect_support;
use unityrs_core::env::{EnvOptions, Environment};
use unityrs_scene::Graph;
fn run(env: &Environment, g: &Graph, m: &Arc<Meter>) -> Result<(), GltfError> {
    let p=Profile::parse(br#"{"profile_version":1,"inactive":"include_hidden","layer_skips":[31],"scene_roots":["parent"],"shader_rules":{"drawable":["*"]},"export_metadata":{"source":"synthetic","generator":"test"}}"#,m)?;
    let s = collect(env, g, &p, m)?;
    write_glb(&s, &p.options(), m, &mut std::io::sink())?;
    Ok(())
}
#[test]
fn every_collect_and_profile_allocation_is_fallible_and_charged() {
    counted::count_this_thread();
    counted::others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    for objects in [
        collect_support::objects(true, false),
        collect_support::repair_objects(),
        repeated_renderers(),
    ] {
        sweep(&objects);
    }
}
fn repeated_renderers() -> Vec<unityrs_testkit::ngui::Object> {
    use unityrs_testkit::{ngui::Object, scene3d as s};
    let mut objects = collect_support::objects(true, false);
    objects[2] = Object::new(
        3,
        1,
        s::game_object(
            b"child",
            &[(0, 4), (0, 6), (0, 10), (0, 11), (0, 12), (0, 13)],
        ),
    );
    objects[5] = Object::new(
        6,
        137,
        s::skinned_mesh_renderer((0, 3), (0, 7), &[(0, 999)], &[25.]),
    );
    for (id, weight) in [(10, f32::NAN), (11, f32::NAN), (12, 25.), (13, 75.)] {
        objects.push(Object::new(
            id,
            137,
            s::skinned_mesh_renderer((0, 3), (0, 7), &[], &[weight]),
        ));
    }
    objects
}
fn sweep(objects: &[unityrs_testkit::ngui::Object]) {
    FAIL_AT.store(usize::MAX, Ordering::Relaxed);
    let budget = Arc::new(Budget {
        home: Arc::new(Home),
        calls: AtomicUsize::new(0),
        refuse: AtomicUsize::new(usize::MAX),
    });
    let mut options = EnvOptions::unitypy();
    let mut context = unityrs_core::FileContext::unitypy();
    context.budget = budget.clone();
    options.context = Arc::new(context);
    let (env, g) = collect_support::graph_in(objects, options);
    let m = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
    run(&env, &g, &m).unwrap();
    let held = HELD.load(Ordering::Relaxed);
    BASE.store(
        LIVE.load(Ordering::Relaxed)
            .saturating_sub(usize::try_from(held).unwrap()),
        Ordering::Relaxed,
    );
    OVER.store(0, Ordering::Relaxed);
    CALLS.store(0, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
    let result = run(&env, &g, &m);
    ARMED.store(false, Ordering::Relaxed);
    result.unwrap();
    let calls = CALLS.load(Ordering::Relaxed);
    assert!(calls > 100, "{calls}");
    assert_eq!(OVER.load(Ordering::Relaxed), 0, "heap exceeded charges");
    assert_eq!(m.used(), 0);
    for at in 0..calls {
        FAIL_AT.store(at, Ordering::Relaxed);
        CALLS.store(0, Ordering::Relaxed);
        REFUSED.store(0, Ordering::Relaxed);
        ARMED.store(true, Ordering::Relaxed);
        let result = run(&env, &g, &m);
        ARMED.store(false, Ordering::Relaxed);
        assert!(result.is_err(), "allocation {at} succeeded");
        assert!(REFUSED.load(Ordering::Relaxed) > 0);
        assert_eq!(m.used(), 0, "allocation {at}");
        assert_eq!(HELD.load(Ordering::Relaxed), held, "allocation {at}");
    }
    budget.calls.store(0, Ordering::Relaxed);
    run(&env, &g, &m).unwrap();
    let charges = budget.calls.load(Ordering::Relaxed);
    eprintln!("collect/profile sweep: {calls} allocator calls, {charges} budget reservations");
    for at in 0..charges {
        budget.calls.store(0, Ordering::Relaxed);
        budget.refuse.store(at, Ordering::Relaxed);
        let result = run(&env, &g, &m);
        budget.refuse.store(usize::MAX, Ordering::Relaxed);
        assert!(result.is_err(), "charge {at}: {result:?}");
        assert_eq!(m.used(), 0);
        assert_eq!(HELD.load(Ordering::Relaxed), held);
    }
    let work = m.decoded();
    for limit in [0, 32, 256, 1024, 4096] {
        let limited = Meter::new(
            limit,
            Arc::new(unityrs_core::budget::Unbounded),
            Reservation::Input,
        );
        assert!(
            matches!(run(&env,&g,&limited),Err(GltfError::Charge(ChargeError::Limit(e))) if e.kind()==LimitKind::InputMemory)
        );
        assert_eq!(limited.used(), 0);
    }
    for limit in [0, 32, 256, 1024, 4096, work.min(10000)] {
        let limited = Meter::with_decode_work(
            u64::MAX,
            limit,
            Arc::new(unityrs_core::budget::Unbounded),
            Reservation::Input,
        );
        let result = run(&env, &g, &limited);
        assert!(
            matches!(&result,Err(GltfError::Charge(ChargeError::Limit(e)) | GltfError::Mesh(unityrs_mesh::MeshError::Charge(ChargeError::Limit(e)))) if e.kind()==LimitKind::DecodeWork),
            "limit {limit}: {result:?}"
        );
        assert_eq!(limited.used(), 0);
    }
    drop(g);
    drop(env);
    assert_eq!(HELD.load(Ordering::Relaxed), 0);
}
