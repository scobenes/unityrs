//! Component probes of `docs/plans/batch.md` 5.3's provisional terms
//! (`docs/plans/extract.md` X12's coefficients), each measured alone on
//! the engine's counting allocator through the testkit's ledger
//! (`unityrs_testkit::ledger`), which every event observes coherently with
//! the charges, and held against [`PROVISIONAL`] as an uncharged
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
mod stage_support;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde_json::json;
use unityrs_batch::alloc::{self, Counting, Hook};
use unityrs_batch::bound::{END_SLOT_OVERHEAD, PROVISIONAL};
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

/// The engine's allocator, bridged to the ledger for registered threads:
/// the stand-in for B9's tally hook.
struct Bridge;

impl Hook for Bridge {
    fn growing(&self, size: usize) {
        if counted() {
            LEDGER.growing(size);
        }
    }
    fn failed(&self, size: usize) {
        if counted() {
            LEDGER.failed(size);
        }
    }
    fn grew(&self, size: usize, zeroed: bool) {
        if counted() {
            LEDGER.grew(size, zeroed);
        }
    }
    fn shrank(&self, size: usize) {
        if counted() {
            LEDGER.shrank(size);
        }
    }
}

#[global_allocator]
static ALLOC: Counting<Bridge> = Counting(Bridge);

/// Runs `f` on a registered thread of its own, the only one counted, so
/// the probes, which libtest runs side by side, never see each other.
fn alone<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = std::thread::spawn(move || {
        static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one = ONE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        count_this_thread();
        let _ = tx.send(f());
    });
    guard::recv(&rx, what)
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
        LEDGER.arm(PROVISIONAL.m_end * cap as u64);
        let (tx, rx) = std::sync::mpsc::sync_channel::<End>(cap);
        let more = tx.clone();
        for _ in 0..cap {
            tx.try_send(end()).unwrap();
        }
        assert!(more.try_send(end()).is_err(), "the channel holds {cap}");
        let o = LEDGER.observe();
        drop((tx, more, rx));
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
        let (k, fits) = alone("the Environment probe", move || {
            LEDGER.arm(PROVISIONAL.c_env);
            let mut options = EnvOptions::batch(Arc::new(FileContext::unitypy()));
            options.dep_cache_bytes = d;
            let env = Environment::new(options);
            let o = LEDGER.observe();
            drop(env);
            (
                o.heap - (LEDGER.observe().heap),
                o.first_violation.is_none(),
            )
        });
        assert!(fits, "d {d}: an Environment past c_env");
        kept.push(k);
    }
    println!("c_env: one Environment keeps {} B", kept[0]);
    assert_eq!(kept[0], kept[1], "an Environment's fixed parts grew with D");
    assert!(kept[0] > 0);
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
