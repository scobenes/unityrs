//! X12: coherent heap/charge observations for collect, GLB writing and a unit.
//! Squeeze each operation's meter in a child and compare every success's bytes.
use std::{
    hash::{DefaultHasher, Hasher},
    io::{self, Write},
    process::Command,
    sync::{Arc, Mutex},
};
use unityrs_core::{
    Bytes, FileContext, RawStr,
    budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation},
    env::{EnvOptions, Environment, Source},
    files::{File, SerializedFile},
    limits::{Meter, heap_cost},
};
use unityrs_gltf::{CorePbr, GltfError, Profile, Scene, collect_with_mapper, write_glb};
use unityrs_scene::{
    Graph, GraphOptions, SceneError, Unit, graph_from_scene_file, unit_from_object,
};
use unityrs_testkit::{
    counted::{self, Allocator, Tally},
    ledger::Ledger,
    ngui::Object,
};
mod collect_support;
mod material_support;

static LEDGER: Ledger = Ledger::new(heap_cost);
static SERIAL: Mutex<()> = Mutex::new(());
struct Watching;
impl Tally for Watching {
    fn shrunk(&self, old: usize, new: usize) {
        LEDGER.shrunk(old, new);
    }
    fn growing(&self, n: usize) {
        LEDGER.growing(n);
    }
    fn failed(&self, n: usize) {
        LEDGER.failed(n);
    }
    fn grew(&self, _: usize, _: bool) {}
    fn shrank(&self, n: usize) {
        LEDGER.shrank(n);
    }
}
#[global_allocator]
static ALLOC: Allocator<Watching> = Allocator(Watching);
#[derive(Debug)]
struct Home;
impl PermitHome for Home {
    fn release(&self, n: u64) {
        LEDGER.release(n);
    }
}
#[derive(Debug)]
struct Budget(Arc<Home>);
impl MemoryBudget for Budget {
    fn try_reserve(&self, n: u64, _: Reservation) -> Result<Permit, BudgetExceeded> {
        LEDGER.charge(n);
        Ok(Permit::new(n, self.0.clone()))
    }
}
#[derive(Default)]
struct Digest(DefaultHasher, u64);
impl Write for Digest {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes);
        self.1 += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[derive(Clone, Copy, Debug)]
enum Stage {
    Collect,
    Write,
    Unit,
    Prefab,
}
impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::Collect => "collect",
            Self::Write => "write",
            Self::Unit => "unit",
            Self::Prefab => "prefab",
        }
    }
}
#[derive(Debug)]
enum Failure {
    Gltf(GltfError),
    Graph(SceneError),
}
impl Failure {
    fn memory_refusal(&self) -> bool {
        use unityrs_core::{LimitKind, limits::ChargeError, objmode::ViewError};
        use unityrs_mesh::MeshError;
        use unityrs_meshdecode::MeshError as DecodeError;
        matches!(self,
            Self::Gltf(GltfError::Charge(ChargeError::Limit(e))
                | GltfError::Mesh(MeshError::Charge(ChargeError::Limit(e))
                    | MeshError::Decode(DecodeError::Charge(ChargeError::Limit(e)))
                    | MeshError::View(ViewError::Charge(ChargeError::Limit(e)))))
                | Self::Graph(SceneError::Charge(ChargeError::Limit(e)))
                if e.kind() == LimitKind::InputMemory)
    }
}

impl From<GltfError> for Failure {
    fn from(e: GltfError) -> Self {
        Self::Gltf(e)
    }
}
impl From<SceneError> for Failure {
    fn from(e: SceneError) -> Self {
        Self::Graph(e)
    }
}
struct Input {
    env: Environment,
    file: Arc<SerializedFile>,
    graph: Graph,
    profile: Profile,
    scene: Scene,
    budget: Arc<Budget>,
    verify_meter: Arc<Meter>,
}
impl Input {
    fn new(objects: &[Object]) -> Self {
        let budget = Arc::new(Budget(Arc::new(Home)));
        let mut ctx = FileContext::unitypy();
        ctx.budget = budget.clone();
        let mut options = EnvOptions::unitypy();
        options.context = Arc::new(ctx);
        let env = Environment::new(options);
        let File::Serialized(file) = env
            .load(Source::Bytes {
                name: RawStr::from_static(b"synthetic-memory-scene"),
                data: Bytes::from(
                    unityrs_testkit::scene3d::file(
                        unityrs_testkit::scene3d::Layout::Unity2022,
                        &[],
                        objects,
                    )
                    .unwrap(),
                ),
            })
            .unwrap()
        else {
            panic!("synthetic serialized scene");
        };
        let m = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
        let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &m).unwrap();
        let profile = Profile::new(&m);
        let scene = collect_with_mapper(&env, &graph, &profile, &CorePbr, &m).unwrap();
        Self {
            env,
            file,
            graph,
            profile,
            scene,
            verify_meter: Meter::new(u64::MAX, budget.clone(), Reservation::Input),
            budget,
        }
    }
    fn run(&self, stage: Stage, m: &Arc<Meter>) -> Result<(u64, u64), Failure> {
        let mut digest = Digest::default();
        match stage {
            Stage::Collect => {
                let scene =
                    collect_with_mapper(&self.env, &self.graph, &self.profile, &CorePbr, m)?;
                // The comparison writer has its own unlimited meter, so this
                // squeeze measures collect's need while comparing every output byte.
                write_glb(
                    &scene,
                    &self.profile.options(),
                    &self.verify_meter,
                    &mut digest,
                )?;
            }
            Stage::Write => {
                write_glb(&self.scene, &self.profile.options(), m, &mut digest)?;
            }
            Stage::Unit | Stage::Prefab => {
                let unit = if matches!(stage, Stage::Prefab) {
                    let root = self.file.objects().find(|o| o.class_id() == 1).unwrap();
                    unit_from_object(&root)?
                } else {
                    Unit::Scene(self.file.clone())
                };
                let graph = unit.graph(&self.env, &GraphOptions::default(), m)?;
                let profile =
                    Profile::parse(br#"{"profile_version":1,"inactive":"include_hidden"}"#, m)?;
                let scene = collect_with_mapper(&self.env, &graph, &profile, &CorePbr, m)?;
                write_glb(&scene, &profile.options(), m, &mut digest)?;
            }
        }
        Ok((digest.0.finish(), digest.1))
    }
}
fn cases() -> Vec<Vec<Object>> {
    vec![
        collect_support::objects(true, false),
        collect_support::objects(false, true),
        collect_support::repair_objects(),
        material_support::rich(b"Standard"),
    ]
}
#[test]
fn x12_collect_write_and_unit_heap_never_exceed_their_charges() {
    let _serial = SERIAL.lock().unwrap();
    counted::count_this_thread();
    counted::others_are_not_counted(|| usize::try_from(LEDGER.observe().heap).unwrap_or(0));
    for objects in cases() {
        let input = Input::new(&objects);
        for stage in [Stage::Collect, Stage::Write, Stage::Unit, Stage::Prefab] {
            let m = Meter::new(u64::MAX, input.budget.clone(), Reservation::Input);
            let expected = input.run(stage, &m).unwrap();
            let held = LEDGER.observe().charged;
            LEDGER.arm(0);
            let result = input.run(stage, &m);
            let observation = LEDGER.observe();
            assert_eq!(result.unwrap(), expected);
            assert!(observation.events > 20, "{stage:?}: no measured work");
            assert_eq!(
                observation.first_violation, None,
                "{stage:?}: {observation:?}"
            );
            assert_eq!(
                observation.charged, held,
                "{stage:?}: retained operation charges"
            );
            assert_eq!(m.used(), 0);
        }
    }
    counted::stop_counting_this_thread();
    assert_eq!(LEDGER.observe().charged, 0);
}
#[test]
fn collect_write_and_unit_memory_squeeze_runs_in_children() {
    let exe = std::env::current_exe().unwrap();
    for stage in [Stage::Collect, Stage::Write, Stage::Unit, Stage::Prefab] {
        let out = unityrs_testkit::child::output(
            Command::new(&exe)
                .args([
                    "--exact",
                    "scene_squeeze_child",
                    "--ignored",
                    "--test-threads=1",
                    "--nocapture",
                ])
                .env("UNITYRS_SCENE_SQUEEZE", stage.name()),
        )
        .unwrap();
        assert!(
            out.status.success(),
            "{}: {}\n{}",
            stage.name(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("squeeze need="));
    }
}
#[test]
#[ignore = "run only by the squeeze parent through testkit::child"]
fn scene_squeeze_child() {
    counted::count_this_thread();
    let stage = match std::env::var("UNITYRS_SCENE_SQUEEZE").unwrap().as_str() {
        "collect" => Stage::Collect,
        "write" => Stage::Write,
        "unit" => Stage::Unit,
        "prefab" => Stage::Prefab,
        _ => panic!("unknown squeeze stage"),
    };
    for objects in cases() {
        let input = Input::new(&objects);
        let unlimited = Meter::new(u64::MAX, input.budget.clone(), Reservation::Input);
        let baseline = input.run(stage, &unlimited).unwrap();
        let least = unityrs_testkit::search::least(0, 8 << 20, 3, |limit| {
            let meter = Meter::new(limit, input.budget.clone(), Reservation::Input);
            let held = LEDGER.observe().charged;
            let result = input.run(stage, &meter);
            assert_eq!(meter.used(), 0, "{stage:?} at {limit}");
            assert_eq!(LEDGER.observe().charged, held, "{stage:?} at {limit}");
            match result {
                Ok(bytes) => {
                    assert_eq!(bytes, baseline, "{stage:?} at {limit}");
                    false
                }
                Err(e) => {
                    assert!(e.memory_refusal(), "{stage:?} at {limit}: {e:?}");
                    true
                }
            }
        })
        .unwrap_or_else(|e| panic!("{stage:?}: {e}"));
        assert!(least.need > 0);
        assert_eq!(least.tried.get(&(least.need - 1)), Some(&true));
        assert_eq!(least.tried.get(&least.need), Some(&false));
        println!(
            "{} squeeze need={} bytes, {} limits checked",
            stage.name(),
            least.need,
            least.tried.len()
        );
    }
    counted::stop_counting_this_thread();
}
