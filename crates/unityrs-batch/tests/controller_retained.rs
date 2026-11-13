mod controller_support;
mod stage_support;
use controller_support::{clear, marked};
use serde_json::json;
use std::{
    fs,
    sync::{Arc, atomic::AtomicBool},
};
use unityrs_batch::{
    alloc::{self, Counting},
    controller::{Boundary, Controller, NoHooks, PreparedSpool},
    execution::Execution,
    phase1,
    spool::{Counter, Host},
};
use unityrs_testkit::{guard, scratch::Scratch};

#[global_allocator]
static ALLOC: Counting = Counting::new();

#[test]
fn long_root_controller_retains_no_more_than_phase1_after_last_commit() {
    let () = guard::finishes("retained controller allocation bound", || {
        let scratch = Scratch::new("controller-retained");
        let mut root = scratch.path().to_path_buf();
        for _ in 0..10 {
            root.push("r".repeat(64));
        }
        fs::create_dir_all(&root).unwrap();
        for area in ["in", "attempts", "fail", "done"] {
            fs::create_dir(root.join(area)).unwrap();
        }
        let source = scratch.join("source");
        let bits_source = scratch.join("bits-source");
        let bad_source = scratch.join("bad-source");
        let missing_source = scratch.join("missing-source");
        fs::write(&bad_source, b"bad").unwrap();
        fs::write(&source, clear(1)).unwrap();
        fs::write(&bits_source, marked()).unwrap();
        let inputs: Vec<_> = (0..16).map(|i| json!({
            "id":format!("input{i}"), "path":match i % 4 { 0 => &source, 1 => &bits_source, 2 => &bad_source, _ => &missing_source }
        })).collect();
        let spool = PreparedSpool::new(
            root,
            Counter::new(1 << 30, 0, 4_096).unwrap(),
            Arc::new(Host),
        )
        .unwrap();
        let control = Arc::new(AtomicBool::new(false));
        let hooks = Arc::new(NoHooks);
        let (prepared, measured) =
            alloc::measured(|| phase1::prepare(stage_support::from_bundles(&inputs, &[])).unwrap());
        let mut controller =
            Controller::new(prepared, spool.clone(), control.clone(), hooks.clone()).unwrap();
        let mut execution = Execution::new(&controller).unwrap();
        assert_eq!(
            controller.drive_local(&mut execution),
            Boundary::ReadyForWinners
        );
        execution.quiesce().unwrap();
        drop(execution);
        assert!(
            controller
                .state()
                .inputs
                .iter()
                .all(|s| s.phase == phase1::InputPhase::Committed)
        );
        assert_eq!(fs::read_dir(spool.root.join("done")).unwrap().count(), 16);
        let files = controller
            .state()
            .files
            .iter()
            .map(|f| {
                f.staged.as_ref().map_or(0, |c| c.claimed_bytes)
                    + f.peek.as_ref().map_or(0, |c| c.claimed_bytes)
            })
            .sum::<u64>();
        let committed = controller
            .state()
            .inputs
            .iter()
            .map(|s| s.committed.as_ref().unwrap().claimed_bytes)
            .sum::<u64>();
        assert_eq!(spool.counter.used(), files + committed);
        assert!(spool.counter.used() > 0);
        // Every retained block is freed here, even if a worker allocated it.
        // Count deallocations on this thread so runtime TLS teardown on other
        // threads cannot contaminate the retained-controller measurement.
        let before_drop = alloc::live();
        drop(controller);
        let retained = u64::try_from(before_drop - alloc::live()).unwrap();
        assert!(
            retained <= measured.kept,
            "retained {retained}, phase 1 {}",
            measured.kept
        );
    });
}
