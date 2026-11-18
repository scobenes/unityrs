//! Component probes of `docs/plans/batch.md` 5.3's compiled constants
//! (`docs/plans/extract.md` X12's coefficients), each measured alone on
//! the engine's counting allocator through the testkit's ledger
//! (`unityrs_testkit::ledger`), which every event observes coherently with
//! the charges, and held against [`SELECTED`] as an uncharged
//! allowance: an event past it is a violation. These are component
//! measurements, not B12's process-level calibration, which needs every
//! engine thread registered.
//!
//! - **`m_end`**: the end channel, `sync_channel::<End>(threads)`, full,
//!   at every capacity from one: `threads x m_end` covers it whole, its
//!   fixed part included, and its per-slot slope is the union and
//!   [`END_SLOT_OVERHEAD`] at most.
//! - **`c_env`**: one private Environment's fixed parts, as an attempt
//!   builds them, whatever `D`.
//! - **The per-bundle audit**: what the controller keeps once every input
//!   is committed, at three bundle counts, is no more than what phase 1's
//!   measure kept (`C_spec`'s measured part): no bundle-proportional heap
//!   outside it, so 5.3 adds no per-bundle allowance.

mod controller_support;
#[cfg(feature = "test-hooks")]
#[path = "memory_support/pools.rs"]
mod pools;
mod stage_support;

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::SeqCst};

use serde_json::json;
use unityrs_batch::alloc::{self, Counting, Hook};
use unityrs_batch::bound::{END_SLOT_OVERHEAD, SELECTED};
use unityrs_batch::controller::{Boundary, Controller, NoHooks, PreparedSpool};
use unityrs_batch::execution::Execution;
use unityrs_batch::job::End;
use unityrs_batch::peek::{PeekEnd, State};
use unityrs_batch::phase1;
use unityrs_batch::spool::{Counter, Host};
use unityrs_core::FileContext;
use unityrs_core::env::{EnvOptions, Environment};
use unityrs_core::limits::heap_cost;
use unityrs_testkit::counted::{Tally, count_this_thread, counted};
use unityrs_testkit::guard;
use unityrs_testkit::ledger::Ledger;
use unityrs_testkit::scratch::Scratch;

static LEDGER: Ledger = Ledger::new(heap_cost);
static GENERATION: AtomicU64 = AtomicU64::new(0);
// Admission and its ledger event must finish before a generation can change.
// Checking the generation before taking this lock would leave a check/use race.
static DISPATCH: AtomicBool = AtomicBool::new(false);
struct Dispatch;
impl Dispatch {
    fn enter() -> Self {
        while DISPATCH
            .compare_exchange_weak(false, true, SeqCst, SeqCst)
            .is_err()
        {
            std::hint::spin_loop();
        }
        Self
    }
}
impl Drop for Dispatch {
    fn drop(&mut self) {
        DISPATCH.store(false, SeqCst);
    }
}
std::thread_local! {
    static PROBE: Cell<u64> = const { Cell::new(0) };
}
fn register() {
    PROBE.with(|p| p.set(GENERATION.load(SeqCst)));
    count_this_thread();
}
fn observing() -> bool {
    counted()
        && PROBE
            .try_with(|p| p.get() == GENERATION.load(SeqCst))
            .unwrap_or(false)
}
fn observed(event: impl FnOnce(&Ledger)) {
    if !counted() {
        return;
    }
    let _dispatch = Dispatch::enter();
    if observing() {
        event(&LEDGER);
    }
}

/// The engine's allocator, bridged to the ledger for registered threads:
/// the stand-in for B9's tally hook.
struct Bridge;

impl Hook for Bridge {
    fn shrunk(&self, old: usize, new: usize) {
        observed(|ledger| ledger.shrunk(old, new));
    }
    fn growing(&self, size: usize) {
        observed(|ledger| ledger.growing(size));
    }
    fn failed(&self, size: usize) {
        observed(|ledger| ledger.failed(size));
    }
    fn grew(&self, size: usize, zeroed: bool) {
        observed(|ledger| ledger.grew(size, zeroed));
    }
    fn shrank(&self, size: usize) {
        observed(|ledger| ledger.shrank(size));
    }
}

#[global_allocator]
static ALLOC: Counting<Bridge> = Counting(Bridge);

/// Serialize complete driver lifetimes. Pool teardown can finish after its
/// runtime's drop; its registration belongs only to the generation that
/// created it, so a later probe cannot observe those trailing frees.
fn alone<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = ONE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    {
        let _dispatch = Dispatch::enter();
        GENERATION.fetch_add(1, SeqCst);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let driver = std::thread::spawn(move || {
        unityrs_batch::sys::allocator(1).unwrap();
        register();
        let result = f();
        unityrs_testkit::counted::stop_counting_this_thread();
        let _ = tx.send(result);
    });
    let result = guard::recv(&rx, what);
    driver.join().unwrap();
    result
}

#[test]
fn late_cleanup_from_a_completed_probe_cannot_change_the_next_measurement() {
    static READY: AtomicBool = AtomicBool::new(false);
    static RELEASE: AtomicBool = AtomicBool::new(false);
    static DONE: AtomicBool = AtomicBool::new(false);
    fn wait(flag: &AtomicBool) {
        let until = std::time::Instant::now() + guard::DEADLOCK_GUARD;
        while !flag.load(SeqCst) {
            assert!(
                std::time::Instant::now() < until,
                "probe cleanup acknowledgment"
            );
            std::thread::yield_now();
        }
    }
    let old = alone("old probe", || {
        let old = std::thread::spawn(|| {
            register();
            let buffer = vec![0_u8; 64 << 10];
            READY.store(true, SeqCst);
            wait(&RELEASE);
            drop(std::hint::black_box(buffer));
            DONE.store(true, SeqCst);
        });
        wait(&READY);
        old
    });
    alone("next probe", move || {
        LEDGER.arm(0);
        let before = LEDGER.observe().heap;
        RELEASE.store(true, SeqCst);
        wait(&DONE);
        assert_eq!(
            LEDGER.observe().heap,
            before,
            "old probe entered this window"
        );
        old.join().unwrap();
    });
}

fn end() -> End {
    End::Peek(PeekEnd {
        bundle: u32::MAX,
        state: State::Done,
        outcome: None,
        failure: None,
        input_peak: u64::MAX,
        decode_work: u64::MAX,
        claimed_bytes: u64::MAX,
        accounts: unityrs_batch::budget::Snapshot::default(),
        slot_releasable: true,
    })
}

/// The heap a full end channel of `cap` slots holds at its peak, its
/// senders and receiver among it, and whether no event saw it pass
/// `cap x m_end`.
fn full_channel(cap: usize) -> (i64, bool) {
    alone("the end channel probe", move || {
        LEDGER.arm(SELECTED.m_end * cap as u64);
        let (tx, rx) = std::sync::mpsc::sync_channel::<End>(cap);
        let more = tx.clone();
        for _ in 0..cap {
            tx.try_send(end()).unwrap();
        }
        assert!(more.try_send(end()).is_err(), "the channel holds {cap}");
        let o = LEDGER.observe();
        drop((tx, more, rx));
        println!(
            "\nB12_COEFFICIENT m_end {}",
            u64::try_from(o.peak_above_arm)
                .unwrap()
                .div_ceil(cap as u64)
        );
        println!("\nB12_FLOOR m_end {}", size_of::<End>() + END_SLOT_OVERHEAD);
        (o.peak_above_arm, o.first_violation.is_none())
    })
}

#[test]
fn a_full_end_channel_fits_threads_times_m_end_at_every_capacity() {
    let mut held = Vec::new();
    for cap in [1, 2, 3, 4, 8, 16, 64, 256] {
        let (heap, fits) = full_channel(cap);
        assert!(fits, "cap {cap}: {heap} past {cap} x m_end");
        held.push((cap, heap));
    }
    // The per-slot slope, past the small capacities' fixed part.
    let (c1, h1) = held[4];
    let (c2, h2) = held[7];
    let slope = u64::try_from((h2 - h1) / i64::try_from(c2 - c1).unwrap()).unwrap();
    let union = size_of::<End>() as u64;
    println!(
        "m_end: End {union} B, slope {slope} B a slot, fixed {} B, cap 1 {} B",
        held[0].1 - i64::try_from(slope).unwrap(),
        held[0].1
    );
    assert!(
        slope >= union,
        "the slot holds the union: {slope} < {union}"
    );
    assert!(
        slope <= union + END_SLOT_OVERHEAD as u64,
        "slot overhead past the allowance: {slope} > {union} + {END_SLOT_OVERHEAD}"
    );
}

/// One private Environment's fixed parts, as `run_attempt`'s context
/// builds them, with `D` small and large: within `c_env` at its peak, and
/// independent of `D`.
#[test]
fn a_private_environments_fixed_parts_fit_c_env_whatever_d() {
    let mut kept = Vec::new();
    for d in [0, 1 << 30] {
        let (k, peak, fits) = alone("the Environment probe", move || {
            LEDGER.arm(SELECTED.c_env);
            let mut options = EnvOptions::batch(Arc::new(FileContext::unitypy()));
            options.dep_cache_bytes = d;
            let env = Environment::new(options);
            let o = LEDGER.observe();
            drop(env);
            (
                o.heap - (LEDGER.observe().heap),
                o.peak_above_arm,
                o.first_violation.is_none(),
            )
        });
        assert!(fits, "d {d}: an Environment past c_env");
        println!("\nB12_COEFFICIENT c_env {peak}");
        kept.push(k);
    }
    println!("c_env: one Environment keeps {} B", kept[0]);
    assert_eq!(kept[0], kept[1], "an Environment's fixed parts grew with D");
    assert!(kept[0] > 0);
}

/// Both process request-slot tables are built after phase 1. Their bounded
/// per-pool storage must follow the pool slope, not a fixed `c_base` residual.
/// This isolates their contribution; runtime/task-cell heap still needs its
/// own incremental probe before `m_pool` can be selected.
#[test]
fn both_request_slot_tables_fit_the_pool_terms_at_maximum_concurrency() {
    use unityrs_batch::http::permit::{FINISHER_BASE, PermitPool};
    for dc in [1_usize, 8, 256] {
        let (peak, fits, retained) = alone("the request-slot tables probe", move || {
            let baseline = LEDGER.observe().heap;
            LEDGER.arm(SELECTED.m_pool * 2 * (dc as u64 + 1));
            let controller = PermitPool::with_base(0, dc).unwrap();
            let finisher = PermitPool::with_base(FINISHER_BASE, dc).unwrap();
            let observation = LEDGER.observe();
            drop((controller, finisher));
            (
                observation.peak_above_arm,
                observation.first_violation.is_none(),
                LEDGER.observe().heap - baseline,
            )
        });
        println!("two request-slot tables: dc {dc}, peak {peak} B");
        assert!(fits, "dc {dc}: request tables passed the pool allowance");
        assert_eq!(retained, 0, "request-slot storage remained after drop");
    }
}

/// Initialize the real blocking pools, counting their parent-side startup
/// allocations and their worker threads. Runtime construction is measured
/// separately; this probe adds pool state to the actual table contribution.
#[test]
fn initialized_pools_and_request_tables_fit_the_incremental_pool_coefficient() {
    for dc in [1, 4, 16] {
        let (peak, fits) = alone("both initialized pools", move || {
            let runtimes = unityrs_batch::lifetime::Runtimes::prepare_with(dc, register).unwrap();
            LEDGER.arm(2 * u64::from(dc + 1) * SELECTED.m_pool);
            let controller =
                unityrs_batch::http::permit::PermitPool::with_base(0, dc as usize).unwrap();
            let finisher =
                unityrs_batch::http::permit::PermitPool::with_base(257, dc as usize).unwrap();
            runtimes.prewarm().unwrap();
            let o = LEDGER.observe();
            drop((controller, finisher, runtimes));
            (o.peak_above_arm, o.first_violation.is_none())
        });
        println!("initialized pool/table increment: dc {dc}, peak {peak} B");
        println!(
            "\nB12_COEFFICIENT m_pool {}",
            u64::try_from(peak).unwrap().div_ceil(2 * u64::from(dc + 1))
        );
        assert!(fits, "dc {dc}: initialized pools exceed m_pool");
    }
}

#[test]
fn a_nonmoving_shrink_does_not_create_an_uncharged_instant() {
    let (peak, fits) = alone("coherent in-place shrink", || {
        let mut buffer = Vec::with_capacity(64 * 1024);
        buffer.resize(1024, 1_u8);
        LEDGER.arm(0);
        buffer.shrink_to_fit();
        let o = LEDGER.observe();
        drop(std::hint::black_box(buffer));
        (o.peak_above_arm, o.first_violation.is_none())
    });
    assert_eq!(peak, 0, "shrinking in place needs no second block");
    assert!(fits);
}

/// Actual engine view containers at the schema's maximum, independently
/// of a scene's payloads. The core checks its private dependency owners'
/// remaining 896 B share in its own module, where their types are visible.
#[test]
fn full_size_view_vectors_fit_their_structural_share() {
    alone("full-size view vector capacities", || {
        use unityrs_batch::{attempt::StagedBundle, view::BoundedWalker};
        const N: usize = 65536;
        LEDGER.arm(128 * N as u64);
        let walk = BoundedWalker::try_new(N).unwrap();
        let paths = Vec::<std::path::PathBuf>::with_capacity(N);
        let members = Vec::<StagedBundle<'static>>::with_capacity(N);
        let o = LEDGER.observe();
        drop((walk, paths, members));
        assert!(o.first_violation.is_none(), "full view containers: {o:?}");
    });
}

/// Drives `n` inputs, each with a dependency of its own, to the last
/// commit; returns phase 1's kept heap and what the controller held then.
fn controller_at(n: usize) -> (u64, u64) {
    alone("the controller probe", move || {
        let scratch = Scratch::new("per-bundle-audit");
        let root = scratch.join("spool");
        for area in ["in", "attempts", "fail", "done"] {
            std::fs::create_dir_all(root.join(area)).unwrap();
        }
        let source = scratch.join("source");
        std::fs::write(&source, controller_support::clear(1)).unwrap();
        let deps: Vec<_> = (0..n)
            .map(|i| json!({"id": format!("d{i:04}"), "path": &source}))
            .collect();
        let inputs: Vec<_> = (0..n)
            .map(|i| json!({"id": format!("i{i:04}"), "path": &source, "deps": [format!("d{i:04}")]}))
            .collect();
        let spool = PreparedSpool::new(
            root,
            Counter::new(1 << 30, 0, 4_096).unwrap(),
            Arc::new(Host),
        )
        .unwrap();
        let (prepared, measured) = alloc::measured(|| {
            phase1::prepare(stage_support::from_bundles(&inputs, &deps)).unwrap()
        });
        // The spool, the stop flag and the hooks are the caller's, made
        // before phase 1: clones kept here, so the drop below frees only
        // what the controller holds of its own.
        let control = Arc::new(AtomicBool::new(false));
        let hooks = Arc::new(NoHooks);
        let mut controller =
            Controller::new(prepared, spool.clone(), control.clone(), hooks.clone()).unwrap();
        let mut execution = Execution::new(&controller).unwrap();
        assert_eq!(
            controller.drive_local(&mut execution),
            Boundary::ReadyForWinners
        );
        execution.quiesce().unwrap();
        drop(execution);
        // Every block the controller holds is freed by this thread's drop,
        // whichever thread made it.
        let before = LEDGER.observe().heap;
        drop(controller);
        let held = u64::try_from(before - LEDGER.observe().heap).unwrap();
        drop((spool, control, hooks));
        (measured.kept, held)
    })
}

/// The 256 B per-bundle audit (5.3's `C_spec`): every structure the
/// controller keeps per bundle (its state, owners, peek word, bits, queues
/// and walk scratch) is built by `phase1::prepare` inside phase 1's
/// measure, and staging, peeks, attempts and commits add none that
/// outlives them: at 8, 32 and 128 bundles the controller holds no more
/// than phase 1 kept.
#[test]
fn the_controller_keeps_no_bundle_proportional_heap_past_phase_1() {
    for n in [4, 16, 64] {
        let (kept, held) = controller_at(n);
        println!(
            "per-bundle audit: {} bundles, phase 1 kept {kept} B, held {held} B",
            2 * n
        );
        assert!(
            held <= kept,
            "{} bundles: held {held} B past phase 1's {kept} B",
            2 * n
        );
    }
}

#[test]
#[cfg(feature = "test-hooks")]
fn full_dispatch_channels_include_fixed_overhead_at_small_capacities() {
    for cap in [1, 2, 4, 16, 256, 1024] {
        alone("full dispatch channel", move || {
            LEDGER.arm(SELECTED.m_dispatch * cap as u64);
            let mut peak = 0;
            let size = unityrs_batch::run::hooks::memory::probe_dispatch(cap, || {
                let o = LEDGER.observe();
                peak = o.peak_above_arm;
                assert!(o.first_violation.is_none(), "dispatch channel: {o:?}");
            })
            .unwrap();
            println!(
                "\nB12_COEFFICIENT m_dispatch {}",
                u64::try_from(peak).unwrap().div_ceil(cap as u64)
            );
            println!("\nB12_FLOOR m_dispatch {}", size.max(64));
        });
    }
}

/// Conservative isolated worker increment: includes both channels and startup
/// bookkeeping again, rather than deducting unrelated peaks. Each worker runs
/// initialized peeks/attempts before quiescence. Fixture/spec/runtime predate arm.
#[test]
#[cfg(feature = "test-hooks")]
fn initialized_workers_fit_the_worker_increment() {
    for n in [1_u32, 4, 16] {
        alone("initialized worker increment", move || {
            unityrs_batch::sys::allocator(n).unwrap();
            let scratch = Scratch::new("worker-increment");
            let root = scratch.join("spool");
            for area in ["in", "attempts", "fail", "done"] {
                std::fs::create_dir_all(root.join(area)).unwrap();
            }
            let source = scratch.join("source");
            std::fs::write(&source, controller_support::clear(1)).unwrap();
            let inputs: Vec<_> = (0..n)
                .map(|i| json!({"id":format!("i{i:04}"), "path":source}))
                .collect();
            let mut spec = stage_support::from_bundles(&inputs, &[]);
            spec.spec.threads = n;
            let prepared = phase1::prepare(spec).unwrap();
            let spool = PreparedSpool::new(
                root,
                Counter::new(1 << 30, 0, 4096).unwrap(),
                Arc::new(Host),
            )
            .unwrap();
            let control = Arc::new(AtomicBool::new(false));

            let runtimes = unityrs_batch::lifetime::Runtimes::prepare_with(1, register).unwrap();
            runtimes.prewarm().unwrap();
            let wake = Arc::new(unityrs_batch::sys::Wake::new().unwrap());
            LEDGER.arm(u64::from(n) * SELECTED.m_thread);
            let mut execution = Execution::prepare_hosted(
                &prepared,
                runtimes.controller.handle().clone(),
                wake,
                register,
            )
            .unwrap();
            let mut controller =
                Controller::new(prepared, spool, control, Arc::new(NoHooks)).unwrap();
            execution.bind(&controller).unwrap();
            assert_eq!(
                controller.drive_local(&mut execution),
                Boundary::ReadyForWinners
            );
            execution.quiesce().unwrap();
            let o = LEDGER.observe();
            assert!(o.first_violation.is_none(), "worker increment: {o:?}");
            println!(
                "\nB12_COEFFICIENT m_thread {}",
                u64::try_from(o.peak_above_arm)
                    .unwrap()
                    .div_ceil(u64::from(n))
            );
        });
    }
}

#[test]
fn six_capped_credential_holders_share_one_bounded_provider() {
    alone("credential holder ownership", || {
        use unityrs_batch::store::credentials;
        let cap = 6 * (heap_cost(128) * 2 + heap_cost(16384));
        LEDGER.arm(cap);
        let credentials = credentials::read(
            |name| match name {
                "AWS_ACCESS_KEY_ID" => Some("K".repeat(128).into()),
                "AWS_SECRET_ACCESS_KEY" => Some("S".repeat(128).into()),
                "AWS_SESSION_TOKEN" => Some("T".repeat(16384).into()),
                _ => None,
            },
            None,
        )
        .unwrap();
        let provider = credentials.into_shared_provider();
        let holders = std::array::from_fn::<_, 6, _>(|_| provider.clone());
        for holder in &holders {
            assert!(Arc::ptr_eq(holder, &provider));
            assert_eq!(holder.values().map(str::len), [128, 128, 16384]);
        }
        let o = LEDGER.observe();
        assert!(o.first_violation.is_none(), "credentials: {o:?}");
        println!("\nB12_ENVELOPE credential_holders 6");
        drop((holders, provider));
    });
}
