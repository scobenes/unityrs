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
mod collect_support;
mod material_support;
mod support;
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
    refuse: usize,
}
impl MemoryBudget for Budget {
    fn try_reserve(&self, n: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        if self.calls.fetch_add(1, Ordering::Relaxed) >= self.refuse {
            return Err(BudgetExceeded { requested: n, what });
        }
        let held = HELD.fetch_add(n, Ordering::Relaxed) + n;
        PEAK.fetch_max(held, Ordering::Relaxed);
        Ok(Permit::new(n, self.home.clone()))
    }
}
fn run(m: &Arc<Meter>) -> Result<(), GltfError> {
    let mut scene = support::morph_directions(m)?;
    let report = write_glb(&scene, &Options::default(), m, &mut std::io::sink())?;
    drop(report);
    scene.materials.as_mut_slice()[0].base_color_texture = Some(TextureInfo {
        texture: 0,
        scale: [1.; 2],
        offset: [0.; 2],
    });
    if let Data::Float(v) = &mut scene.accessors.as_mut_slice()[4].data {
        v.as_mut_slice()[0] = f64::INFINITY;
    }
    let report = write_glb(&scene, &Options::default(), m, &mut std::io::sink())?;
    drop(report);
    if let Data::Float(v) = &mut scene.accessors.as_mut_slice()[2].data {
        v.as_mut_slice()[0] = f64::NAN;
    }
    let report = write_glb(&scene, &Options::default(), m, &mut std::io::sink())?;
    drop(report);
    if let Data::Float(v) = &mut scene.accessors.as_mut_slice()[0].data {
        v.as_mut_slice()[0] = f64::INFINITY;
    }
    let report = write_glb(&scene, &Options::default(), m, &mut std::io::sink())?;
    drop(report);
    let report = write_glb(
        &scene,
        &Options { normalise: false },
        m,
        &mut std::io::sink(),
    )?;
    drop(scene);
    drop(report);
    let scene = support::animated(m)?;
    let report = write_glb(&scene, &Options::default(), m, &mut std::io::sink())?;
    drop(scene);
    drop(report);
    Ok(())
}
#[test]
#[allow(clippy::too_many_lines)]
fn every_buffer_is_charged_first_and_every_refusal_returns_all_charges() {
    counted::count_this_thread();
    counted::others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let budget = Arc::new(Budget {
        home: Arc::new(Home),
        calls: AtomicUsize::new(0),
        refuse: usize::MAX,
    });
    let m = Meter::new(u64::MAX, budget, Reservation::Input);
    run(&m).unwrap();
    BASE.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    OVER.store(0, Ordering::Relaxed);
    CALLS.store(0, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
    let result = run(&m);
    ARMED.store(false, Ordering::Relaxed);
    result.unwrap();
    let calls = CALLS.load(Ordering::Relaxed);
    assert!(calls > 30, "only {calls} allocations");
    assert_eq!(OVER.load(Ordering::Relaxed), 0, "heap exceeded charges");
    assert_eq!(m.used(), 0);
    assert_eq!(HELD.load(Ordering::Relaxed), 0);
    for at in 0..calls {
        FAIL_AT.store(at, Ordering::Relaxed);
        CALLS.store(0, Ordering::Relaxed);
        REFUSED.store(0, Ordering::Relaxed);
        ARMED.store(true, Ordering::Relaxed);
        let result = run(&m);
        ARMED.store(false, Ordering::Relaxed);
        assert!(
            matches!(
                result,
                Err(GltfError::Charge(
                    ChargeError::Budget(_) | ChargeError::Allocator(_)
                ))
            ),
            "allocation {at}: {result:?}"
        );
        assert!(REFUSED.load(Ordering::Relaxed) > 0);
        assert_eq!(m.used(), 0, "allocation {at}");
        assert_eq!(HELD.load(Ordering::Relaxed), 0);
    }
    let mut succeeded = false;
    for at in 0..512 {
        let budget = Arc::new(Budget {
            home: Arc::new(Home),
            calls: AtomicUsize::new(0),
            refuse: at,
        });
        let m = Meter::new(u64::MAX, budget, Reservation::Dependency);
        match run(&m) {
            Ok(()) => {
                succeeded = true;
            }
            Err(e) => assert!(
                matches!(e,GltfError::Charge(ChargeError::Budget(ref b)) if b.what==Reservation::Dependency),
                "charge {at}: {e:?}"
            ),
        }
        assert_eq!(m.used(), 0);
        assert_eq!(HELD.load(Ordering::Relaxed), 0);
        if succeeded {
            break;
        }
    }
    assert!(succeeded);
    let work = m.decoded();
    let peak = PEAK.load(Ordering::Relaxed);
    for limit in [0, 32, 256, 1024, 4096, peak - 1] {
        let m = Meter::new(
            limit,
            Arc::new(unityrs_core::budget::Unbounded),
            Reservation::Input,
        );
        assert!(
            matches!(run(&m),Err(GltfError::Charge(ChargeError::Limit(e))) if e.kind()==LimitKind::InputMemory)
        );
        assert_eq!(m.used(), 0);
    }
    let full = Meter::unlimited();
    run(&full).unwrap();
    let work = full.decoded().min(work);
    for limit in [0, 32, 256, 1024, 4096, work - 1] {
        let m = Meter::with_decode_work(
            u64::MAX,
            limit,
            Arc::new(unityrs_core::budget::Unbounded),
            Reservation::Input,
        );
        assert!(
            matches!(run(&m),Err(GltfError::Charge(ChargeError::Limit(e))) if e.kind()==LimitKind::DecodeWork)
        );
        assert_eq!(m.used(), 0);
    }
}

struct ImageWriteProbe<'a> {
    meter: &'a Meter,
    held: u64,
    png: &'a [u8],
    written: usize,
    refuse: bool,
}
impl std::io::Write for ImageWriteProbe<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        // A separate meter for layout/output prevents temporary writer
        // charges from hiding an early release of the encoded image.
        assert_eq!(self.meter.used(), self.held);
        let n = bytes.len().min(7);
        if bytes == &self.png[self.written..] && !bytes.is_empty() {
            if self.refuse {
                return Err(std::io::Error::other("image sink refused"));
            }
            self.written += n;
        }
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn encoded_png_charge_survives_layout_partial_writes_and_sink_failure() {
    let image_meter = Meter::unlimited();
    let (env, graph) = collect_support::graph(&material_support::simple());
    let mut collected = collect(&env, &graph, &Profile::new(&image_meter), &image_meter).unwrap();
    assert_eq!(collected.images.len(), 1);
    let image = collected.images.pop().unwrap();
    assert!(image.bytes.as_slice().starts_with(b"\x89PNG\r\n\x1a\n"));
    drop(collected);
    let held = image_meter.used();
    assert!(held >= unityrs_core::limits::heap_cost(image.bytes.capacity() as u64));

    let write_meter = Meter::unlimited();
    let mut scene = Scene::new(&write_meter);
    scene.images.push(image).unwrap();
    for refuse in [false, true] {
        let png = scene.images.as_slice()[0].bytes.as_slice();
        let mut out = ImageWriteProbe {
            meter: &image_meter,
            held,
            png,
            written: 0,
            refuse,
        };
        let result = write_glb(&scene, &Options::default(), &write_meter, &mut out);
        if refuse {
            assert!(matches!(result, Err(GltfError::Io(_))));
            assert_eq!(out.written, 0);
        } else {
            drop(result.unwrap());
            assert_eq!(out.written, png.len());
        }
        // write_glb borrows the scene: its image charge remains owned by
        // the scene after both a completed write and a failed write.
        assert_eq!(image_meter.used(), held);
    }
    drop(scene);
    assert_eq!(image_meter.used(), 0);
    assert_eq!(write_meter.used(), 0);
}
