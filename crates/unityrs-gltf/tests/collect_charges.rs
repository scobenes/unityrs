//! Sweep authored material/collector allocations and all budget reservations.
//! Streamed lookup names are covered by the same sticky refusal sweep.
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
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
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
mod material_support;
use unityrs_core::env::{EnvOptions, Environment};
use unityrs_scene::Graph;
fn run(
    env: &Environment,
    g: &Graph,
    m: &Arc<Meter>,
    mapper: Option<&dyn MaterialMapper>,
) -> Result<(), GltfError> {
    let p=Profile::parse(br#"{"profile_version":1,"inactive":"include_hidden","layer_skips":[31],"scene_roots":["parent"],"material_property_allowlist":["_Metallic","_Color","_MainTex"],"shader_rules":{"drawable":["*"]},"export_metadata":{"source":"synthetic","generator":"test"}}"#,m)?;
    let s = match mapper {
        Some(mapper) => collect_with_mapper(env, g, &p, mapper, m)?,
        None => collect(env, g, &p, m)?,
    };
    write_glb(&s, &p.options(), m, &mut std::io::sink())?;
    Ok(())
}
#[test]
fn material_buffers_and_profile_allocations_are_fallible_and_charged() {
    let _serial = SERIAL.lock().unwrap();
    counted::count_this_thread();
    counted::others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    for objects in [
        collect_support::objects(true, false),
        collect_support::repair_objects(),
        repeated_renderers(),
    ] {
        sweep(&objects, None);
    }
    let materials = material_support::rich(b"Standard");
    sweep(&materials, None);
    sweep(&materials, Some(&CorePbr));
    for shader in [b"Standard".as_slice(), b"Standard (Specular setup)"] {
        use unityrs_testkit::{ngui::V, scene3d as s};
        let mut packed = material_support::objects(
            shader,
            &[("_GlossMapScale", 0.5), ("_SmoothnessTextureChannel", 1.)],
            &[
                ("_SpecColor", [0.25, 0.5, 0.75, 1.]),
                ("_EmissionColor", [2., 1., 0.5, 1.]),
            ],
            &[
                ("_MainTex", s::tex_env((0, 12), [1.; 2], [0.; 2])),
                ("_MetallicGlossMap", s::tex_env((0, 12), [1.; 2], [0.; 2])),
                ("_SpecGlossMap", s::tex_env((0, 12), [1.; 2], [0.; 2])),
            ],
        );
        s::set(
            &mut packed[7].value,
            "m_ShaderKeywords",
            V::Str(b"_EMISSION".to_vec()),
        );
        sweep(&packed, Some(&CorePbr));
    }
    let mut streamed = materials;
    streamed[9] = unityrs_testkit::ngui::texture_streamed(
        12,
        b"asymmetric",
        4,
        (2, 2),
        "archive:/PIXELS.resS",
        0,
        16,
    );
    sweep_with(&streamed, Some(&CorePbr), false);
    counted::stop_counting_this_thread();
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
fn sweep(objects: &[unityrs_testkit::ngui::Object], mapper: Option<&dyn MaterialMapper>) {
    sweep_with(objects, mapper, true);
}
fn sweep_with(
    objects: &[unityrs_testkit::ngui::Object],
    mapper: Option<&dyn MaterialMapper>,
    allocator: bool,
) {
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
    env.load(unityrs_core::env::Source::Bytes {
        name: unityrs_core::RawStr::from_static(b"pixels.resS"),
        data: unityrs_core::Bytes::from_static(&material_support::PIXELS),
    })
    .unwrap();
    let m = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
    run(&env, &g, &m, mapper).unwrap();
    let held = HELD.load(Ordering::Relaxed);
    BASE.store(
        LIVE.load(Ordering::Relaxed)
            .saturating_sub(usize::try_from(held).unwrap()),
        Ordering::Relaxed,
    );
    OVER.store(0, Ordering::Relaxed);
    CALLS.store(0, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
    let result = run(&env, &g, &m, mapper);
    ARMED.store(false, Ordering::Relaxed);
    result.unwrap();
    let calls = CALLS.load(Ordering::Relaxed);
    assert!(calls > 100, "{calls}");
    assert_eq!(OVER.load(Ordering::Relaxed), 0, "heap exceeded charges");
    assert_eq!(m.used(), 0);
    for at in 0..if allocator { calls } else { 0 } {
        FAIL_AT.store(at, Ordering::Relaxed);
        CALLS.store(0, Ordering::Relaxed);
        REFUSED.store(0, Ordering::Relaxed);
        ARMED.store(true, Ordering::Relaxed);
        let result = run(&env, &g, &m, mapper);
        ARMED.store(false, Ordering::Relaxed);
        assert!(result.is_err(), "allocation {at} succeeded");
        assert!(REFUSED.load(Ordering::Relaxed) > 0);
        assert_eq!(m.used(), 0, "allocation {at}");
        assert_eq!(HELD.load(Ordering::Relaxed), held, "allocation {at}");
    }
    budget.calls.store(0, Ordering::Relaxed);
    run(&env, &g, &m, mapper).unwrap();
    let charges = budget.calls.load(Ordering::Relaxed);
    eprintln!("collect/profile sweep: {calls} allocator calls, {charges} budget reservations");
    for at in 0..charges {
        budget.calls.store(0, Ordering::Relaxed);
        budget.refuse.store(at, Ordering::Relaxed);
        let result = run(&env, &g, &m, mapper);
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
            matches!(run(&env,&g,&limited,mapper),Err(GltfError::Charge(ChargeError::Limit(e))) if e.kind()==LimitKind::InputMemory)
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
        let result = run(&env, &g, &limited, mapper);
        assert!(
            matches!(&result,Err(GltfError::Charge(ChargeError::Limit(e)) | GltfError::Mesh(unityrs_mesh::MeshError::Charge(ChargeError::Limit(e)) | unityrs_mesh::MeshError::Decode(unityrs_meshdecode::MeshError::Charge(ChargeError::Limit(e))))) if e.kind()==LimitKind::DecodeWork),
            "limit {limit}: {result:?}"
        );
        assert_eq!(limited.used(), 0);
    }
    drop(g);
    drop(env);
    assert_eq!(HELD.load(Ordering::Relaxed), 0);
}

/// Every streamed lookup allocation must return a refusal, never abort.
#[test]
fn streamed_texture_host_allocation_refusals() {
    let _serial = SERIAL.lock().unwrap();
    counted::count_this_thread();
    counted::others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let mut streamed = material_support::rich(b"Standard");
    streamed[9] = unityrs_testkit::ngui::texture_streamed(
        12,
        b"asymmetric",
        4,
        (2, 2),
        "archive:/PIXELS.resS",
        0,
        16,
    );
    sweep(&streamed, Some(&CorePbr));
    // Exercise all four concatenated candidates and normalization on misses.
    streamed[9] = unityrs_testkit::ngui::texture_streamed(
        12,
        b"asymmetric",
        4,
        (2, 2),
        "archive:/PIXELS.bin",
        0,
        16,
    );
    sweep(&streamed, Some(&CorePbr));
    counted::stop_counting_this_thread();
}

#[test]
fn resolved_candidate_name_owners_return_host_refusals() {
    use unityrs_core::{RawStr, env::resource::Candidate};
    let _serial = SERIAL.lock().unwrap();
    counted::count_this_thread();
    let meter = Meter::unlimited();
    let candidate = Candidate {
        stem: RawStr::from_static(b"PIXELS"),
        suffix: b".resS",
    };
    FAIL_AT.store(usize::MAX, Ordering::Relaxed);
    CALLS.store(0, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
    let name = candidate.joined(&meter);
    ARMED.store(false, Ordering::Relaxed);
    assert_eq!(name.unwrap().as_bytes(), b"PIXELS.resS");
    let calls = CALLS.load(Ordering::Relaxed);
    for at in 0..calls {
        FAIL_AT.store(at, Ordering::Relaxed);
        CALLS.store(0, Ordering::Relaxed);
        ARMED.store(true, Ordering::Relaxed);
        let result = candidate.joined(&meter);
        ARMED.store(false, Ordering::Relaxed);
        assert!(
            matches!(
                result,
                Err(ChargeError::Allocator(_) | ChargeError::Budget(_))
            ),
            "name allocation {at}: {result:?}"
        );
        assert_eq!(meter.used(), 0);
    }
    FAIL_AT.store(usize::MAX, Ordering::Relaxed);
    let computed = RawStr::charged(b"dir/pixels.resource", &meter).unwrap();
    FAIL_AT.store(0, Ordering::Relaxed);
    CALLS.store(0, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
    let slice = unityrs_core::pystr::simplify_name(&computed, &meter);
    let clone = computed.clone();
    ARMED.store(false, Ordering::Relaxed);
    FAIL_AT.store(usize::MAX, Ordering::Relaxed);
    assert_eq!(
        CALLS.load(Ordering::Relaxed),
        0,
        "name slices and clones allocate nothing"
    );
    let slice = slice.unwrap();
    assert_eq!(slice.as_bytes(), b"pixels.resource");
    drop(computed);
    drop(clone);
    assert!(meter.used() > 0, "slice retains the owner charge");
    drop(slice);
    assert_eq!(meter.used(), 0);
    drop(meter);
    counted::stop_counting_this_thread();
}

/// Rebuild the environment for each attempt: a warm registry hit would skip
/// the successful resolver load and miss its persistent candidate-name owner.
#[test]
fn successful_suffixed_resolver_load_refusals() {
    use unityrs_core::{
        Bytes, RawStr,
        env::{CabResolver, Source},
    };
    #[derive(Debug)]
    struct Resolver {
        arm_owner: bool,
    }
    impl CabResolver for Resolver {
        fn resolve(&self, name: &[u8]) -> Option<Source> {
            if name != b"PIXELS.resS" {
                return None;
            }
            if self.arm_owner {
                CALLS.store(0, Ordering::Relaxed);
                FAIL_AT.store(0, Ordering::Relaxed);
                ARMED.store(true, Ordering::Relaxed);
            }
            Some(Source::Bytes {
                name: RawStr::from_static(b"resolver-source"),
                data: Bytes::from_static(&material_support::PIXELS),
            })
        }
    }
    let _serial = SERIAL.lock().unwrap();
    counted::count_this_thread();
    let mut objects = material_support::rich(b"Standard");
    objects[9] = unityrs_testkit::ngui::texture_streamed(
        12,
        b"resolved",
        4,
        (2, 2),
        "archive:/PIXELS.bin",
        0,
        16,
    );
    let budget = Arc::new(Budget {
        home: Arc::new(Home),
        calls: AtomicUsize::new(0),
        refuse: AtomicUsize::new(usize::MAX),
    });
    let attempt = |refuse: usize, arm_owner: bool| {
        let mut context = unityrs_core::FileContext::unitypy();
        context.budget = budget.clone();
        let mut options = EnvOptions::unitypy();
        options.context = Arc::new(context);
        options.resolver = Some(Arc::new(Resolver { arm_owner }));
        let (env, graph) = collect_support::graph_in(&objects, options);
        let meter = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
        budget.calls.store(0, Ordering::Relaxed);
        budget.refuse.store(refuse, Ordering::Relaxed);
        let result = run(&env, &graph, &meter, Some(&CorePbr));
        ARMED.store(false, Ordering::Relaxed);
        FAIL_AT.store(usize::MAX, Ordering::Relaxed);
        budget.refuse.store(usize::MAX, Ordering::Relaxed);
        let calls = budget.calls.load(Ordering::Relaxed);
        assert_eq!(meter.used(), 0);
        // A successful resolve registers under the requested candidate alias.
        if result.is_ok() {
            let file = env
                .files()
                .find(|entry| entry.name.as_bytes() == b"PIXELS.resS")
                .unwrap();
            assert_eq!(file.name.as_bytes(), b"PIXELS.resS");
        }
        drop(graph);
        drop(env);
        assert_eq!(HELD.load(Ordering::Relaxed), 0);
        (result, calls)
    };
    let (result, charges) = attempt(usize::MAX, false);
    result.unwrap();
    let (result, _) = attempt(usize::MAX, true);
    assert!(result.is_err(), "resolved owner refusal: {result:?}");
    assert_eq!(CALLS.load(Ordering::Relaxed), 1);
    for at in 0..charges {
        let (result, _) = attempt(at, false);
        assert!(result.is_err(), "resolver charge {at}");
    }
    eprintln!(
        "successful resolver sweep: 1 persistent owner refusal, {charges} budget reservations"
    );
    counted::stop_counting_this_thread();
}

#[test]
fn builtin_cache_and_procedural_buffers_refuse_each_allocation_and_reservation() {
    let _serial = SERIAL.lock().unwrap();
    counted::count_this_thread();
    counted::others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let budget = Arc::new(Budget {
        home: Arc::new(Home),
        calls: AtomicUsize::new(0),
        refuse: AtomicUsize::new(usize::MAX),
    });
    let mut options = EnvOptions::unitypy();
    let mut context = (*options.context).clone();
    context.budget = budget.clone();
    options.context = Arc::new(context);
    let (env, graph) = builtin_refusal_fixture(options);
    let m = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
    for mode in [Builtin::Resolve, Builtin::Procedural] {
        let mut profile = Profile::new(&m);
        profile.builtin = mode;
        let attempt = || -> Result<(), GltfError> {
            let scene = collect(&env, &graph, &profile, &m)?;
            write_glb(&scene, &profile.options(), &m, &mut std::io::sink())?;
            Ok(())
        };
        FAIL_AT.store(usize::MAX, Ordering::Relaxed);
        attempt().unwrap();
        let held = HELD.load(Ordering::Relaxed);
        BASE.store(
            LIVE.load(Ordering::Relaxed)
                .saturating_sub(usize::try_from(held).unwrap()),
            Ordering::Relaxed,
        );
        OVER.store(0, Ordering::Relaxed);
        CALLS.store(0, Ordering::Relaxed);
        ARMED.store(true, Ordering::Relaxed);
        let result = attempt();
        ARMED.store(false, Ordering::Relaxed);
        result.unwrap();
        let calls = CALLS.load(Ordering::Relaxed);
        assert!(calls > 30);
        assert_eq!(OVER.load(Ordering::Relaxed), 0);
        for at in 0..calls {
            FAIL_AT.store(at, Ordering::Relaxed);
            CALLS.store(0, Ordering::Relaxed);
            ARMED.store(true, Ordering::Relaxed);
            let result = attempt();
            ARMED.store(false, Ordering::Relaxed);
            assert!(result.is_err(), "{mode:?} allocation {at}");
            assert_eq!(m.used(), 0);
            assert_eq!(HELD.load(Ordering::Relaxed), held);
        }
        FAIL_AT.store(usize::MAX, Ordering::Relaxed);
        budget.calls.store(0, Ordering::Relaxed);
        attempt().unwrap();
        let charges = budget.calls.load(Ordering::Relaxed);
        for at in 0..charges {
            budget.calls.store(0, Ordering::Relaxed);
            budget.refuse.store(at, Ordering::Relaxed);
            let result = attempt();
            budget.refuse.store(usize::MAX, Ordering::Relaxed);
            assert!(result.is_err(), "{mode:?} reservation {at}");
            assert_eq!(m.used(), 0);
            assert_eq!(HELD.load(Ordering::Relaxed), held);
        }
    }
    drop(graph);
    drop(env);
    assert_eq!(HELD.load(Ordering::Relaxed), 0);
    counted::stop_counting_this_thread();
}

fn builtin_refusal_fixture(options: EnvOptions) -> (Environment, Graph) {
    use unityrs_core::{Bytes, RawStr, env::Source, files::File};
    use unityrs_scene::{GraphOptions, Unit};
    use unityrs_testkit::{ngui::Object, scene3d as s};
    let env = Environment::new(options);
    let mut objects = collect_support::objects(false, false);
    objects[2] = Object::new(
        3,
        1,
        s::game_object(b"child", &[(0, 4), (0, 5), (0, 6), (0, 10)]),
    );
    objects[4] = Object::new(5, 33, s::mesh_filter((0, 3), (1, 10210)));
    objects.push(Object::new(10, 23, s::mesh_renderer((0, 3), &[], (0, 0))));
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"level0"),
            data: Bytes::from(
                s::file(
                    s::Layout::Unity2022,
                    &["Library/unity default resources"],
                    &objects,
                )
                .unwrap(),
            ),
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    let mesh = s::mesh::mesh(
        10210,
        b"quad",
        &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
        &[0, 1, 2],
        s::mesh::MeshVersion::V2022,
    );
    env.load(Source::Bytes {
        name: RawStr::from_static(b"unity default resources"),
        data: Bytes::from(s::file(s::Layout::Unity2022, &[], &[mesh]).unwrap()),
    })
    .unwrap();
    let graph = Unit::Scene(file)
        .graph(&env, &GraphOptions::default(), &Meter::unlimited())
        .unwrap();
    (env, graph)
}
