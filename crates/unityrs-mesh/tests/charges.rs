//! Every allocation is observed after a warm object tree, including the core
//! parse and all nested metadata. Refuse each allocation, then all later ones,
//! to catch an infallible fallback as well as an infallible primary path.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation};
use unityrs_core::limits::{ChargeError, Meter};
use unityrs_mesh::{MeshError, read_mesh};
use unityrs_testkit::counted::{self, Allocator, Tally};
use unityrs_testkit::scene3d::mesh::MeshVersion;
mod support;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static BASE: AtomicUsize = AtomicUsize::new(0);
static HELD: AtomicU64 = AtomicU64::new(0);
static BASE_HELD: AtomicU64 = AtomicU64::new(0);
static OVER: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static FAIL_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
static REFUSED: AtomicUsize = AtomicUsize::new(0);
struct Watching;
impl Tally for Watching {
    fn grew(&self, size: usize, _: bool) {
        let now = LIVE.fetch_add(size, Ordering::Relaxed) + size;
        let above = now.saturating_sub(BASE.load(Ordering::Relaxed)) as u64;
        let charge = HELD
            .load(Ordering::Relaxed)
            .saturating_sub(BASE_HELD.load(Ordering::Relaxed));
        OVER.fetch_max(above.saturating_sub(charge), Ordering::Relaxed);
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(size, Ordering::Relaxed);
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
struct Budget(Arc<Home>);
impl MemoryBudget for Budget {
    fn try_reserve(&self, n: u64, _: Reservation) -> Result<Permit, BudgetExceeded> {
        HELD.fetch_add(n, Ordering::Relaxed);
        Ok(Permit::new(n, self.0.clone()))
    }
}
fn budget_refusal(e: &MeshError) -> bool {
    match e {
        MeshError::Charge(ChargeError::Budget(_))
        | MeshError::Decode(unityrs_meshdecode::MeshError::Charge(ChargeError::Budget(_))) => true,
        MeshError::Parse(unityrs_core::walk::ParseError::Read(e)) => matches!(
            e.detail,
            unityrs_core::walk::ReadDetail::Charge(ChargeError::Budget(_))
        ),
        _ => false,
    }
}
#[test]
#[allow(clippy::too_many_lines)]
fn allocations_are_charged_first_and_each_allocator_refusal_returns() {
    counted::count_this_thread();
    counted::others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let budget = Arc::new(Budget(Arc::new(Home)));
    let (env, file) = support::load_with_budget(
        MeshVersion::V6000,
        &[support::rich_object(96)],
        budget.clone(),
    );
    let object = file.object(1).unwrap();
    let meter = Meter::new(u64::MAX, budget, Reservation::Input);
    drop(read_mesh(&object, &env, &meter).unwrap());
    let baseline = HELD.load(Ordering::Relaxed);
    BASE_HELD.store(baseline, Ordering::Relaxed);
    BASE.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    OVER.store(0, Ordering::Relaxed);
    CALLS.store(0, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
    let model = read_mesh(&object, &env, &meter).unwrap();
    ARMED.store(false, Ordering::Relaxed);
    let calls = CALLS.load(Ordering::Relaxed);
    let over = OVER.load(Ordering::Relaxed);
    assert_eq!(over, 0, "heap stood {over} bytes past charges");
    assert!(calls > 100, "{calls} allocations");
    // Moving an individual buffer keeps its charge after the rest of the mesh.
    let names = model.bone_name_hashes;
    drop(model.name);
    drop(model.processed);
    assert_eq!(names.len(), 96);
    drop(names);
    // The remaining fields drop here, before measuring refusals.
    drop(model.submeshes);
    drop(model.triangles);
    drop(model.bind_poses);
    drop(model.blend_shapes);
    drop(model.lod);
    assert_eq!(meter.used(), 0);
    assert_eq!(HELD.load(Ordering::Relaxed), baseline);
    for at in 0..calls {
        CALLS.store(0, Ordering::Relaxed);
        REFUSED.store(0, Ordering::Relaxed);
        FAIL_AT.store(at, Ordering::Relaxed);
        ARMED.store(true, Ordering::Relaxed);
        let answer = read_mesh(&object, &env, &meter);
        ARMED.store(false, Ordering::Relaxed);
        let e = answer.unwrap_err();
        assert!(budget_refusal(&e), "allocation {at}: {e:?}");
        assert!(REFUSED.load(Ordering::Relaxed) > 0);
        assert_eq!(meter.used(), 0, "allocation {at}");
        assert_eq!(HELD.load(Ordering::Relaxed), baseline, "allocation {at}");
    }
    // Error diagnostics are fallible too: the packed-width refusal and an
    // object-mode type refusal each allocate a bounded message.
    for malformed in [false, true] {
        use unityrs_testkit::ngui::{V, set_field, st, vector};
        use unityrs_testkit::scene3d::mesh as m;
        let mut value = m::compressed();
        if let V::Struct(ty, fields) = &mut value {
            if malformed {
                *ty = "NotCompressedMesh".to_owned();
            } else {
                fields
                    .iter_mut()
                    .find(|(n, _)| n == "m_Triangles")
                    .unwrap()
                    .1 = st(
                    "PackedBitVector",
                    vec![
                        ("m_NumItems", V::UInt(1)),
                        ("m_Data", vector(V::UInt8(0), vec![V::UInt8(0)])),
                        ("m_BitSize", V::UInt8(33)),
                    ],
                );
            }
        }
        let obj = set_field(
            m::mesh(1, b"bad", &[[0.0; 3]; 3], &[0, 1, 2], MeshVersion::V2022),
            "m_CompressedMesh",
            value,
            None,
        );
        let (env, file) = support::load(MeshVersion::V2022, &[obj]);
        let obj = file.object(1).unwrap();
        let input = file.meter().used();
        FAIL_AT.store(usize::MAX, Ordering::Relaxed);
        let original = read_mesh(&obj, &env, &meter).unwrap_err();
        assert!(matches!(
            original,
            MeshError::Decode(unityrs_meshdecode::MeshError::Malformed(_))
        ));
        // The object tree is now warm, including any permanent source charges.
        let input = input.max(file.meter().used());
        CALLS.store(0, Ordering::Relaxed);
        ARMED.store(true, Ordering::Relaxed);
        let original = read_mesh(&obj, &env, &meter);
        ARMED.store(false, Ordering::Relaxed);
        drop(original);
        let calls = CALLS.load(Ordering::Relaxed);
        for at in 0..calls {
            CALLS.store(0, Ordering::Relaxed);
            FAIL_AT.store(at, Ordering::Relaxed);
            ARMED.store(true, Ordering::Relaxed);
            let result = read_mesh(&obj, &env, &meter);
            ARMED.store(false, Ordering::Relaxed);
            assert!(
                budget_refusal(&result.unwrap_err()),
                "malformed {malformed}, allocation {at}"
            );
            assert_eq!(meter.used(), 0);
            assert_eq!(file.meter().used(), input);
        }
    }
    FAIL_AT.store(usize::MAX, Ordering::Relaxed);
    drop(object);
    drop(file);
    drop(env);
    drop(meter);
    assert_eq!(HELD.load(Ordering::Relaxed), 0);
}
