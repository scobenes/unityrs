mod artifact_support;
mod controller_support;
mod stage_support;
use controller_support::*;
use std::sync::Arc;
use unityrs_batch::{
    control::Culprit,
    controller::{Boundary, Controller, NoHooks},
    execution::Execution,
    lifetime::{Process, Runtimes},
    reason::StopReason,
    spool::Counter,
};

#[test]
fn local_staging_claim_write_and_pool_panics_have_no_culprit() {
    use unityrs_batch::{controller::Point, stage::local::Point as StagePoint};
    for mode in 0..3 {
        let mut h = Harness::new(&[("a", clear(1), vec![])], &[], 1, 1);
        if mode == 0 {
            h.spool.counter = Counter::new(1, 0, 4096).unwrap();
        }
        if mode == 1 {
            let faults = artifact_support::Faults::default();
            faults.set(2, libc::ENOSPC);
            h.spool.fs = Arc::new(faults);
        }
        let hooks: Arc<dyn unityrs_batch::controller::Hooks> = if mode == 2 {
            Arc::new(Tap {
                point: Box::new(|p| {
                    if matches!(p, Point::Stage(_, StagePoint::Buffer(_))) {
                        panic!("pool copy panic")
                    }
                }),
                ..Tap::default()
            })
        } else {
            Arc::new(NoHooks)
        };
        let process = Arc::new(Process::prepare().unwrap());
        let mut c = Controller::new(
            h.prepared.take().unwrap(),
            h.spool.clone(),
            process.clone(),
            hooks,
        )
        .unwrap();
        let mut execution = Execution::new(&c).unwrap();
        assert_eq!(c.drive_local(&mut execution), Boundary::HandOver);
        let stop = process.state.take().unwrap();
        assert_eq!(stop.culprit, Culprit::None);
        assert_eq!(
            stop.reason,
            if mode == 2 {
                StopReason::Internal
            } else {
                StopReason::SpoolFull
            }
        );
        execution.quiesce().unwrap();
    }
}
#[test]
fn two_pools_are_prepared_and_warmed_without_a_spool() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static STARTED: AtomicUsize = AtomicUsize::new(0);
    let runtimes = Runtimes::prepare_with(2, || {
        STARTED.fetch_add(1, Ordering::Relaxed);
    })
    .unwrap();
    runtimes.prewarm().unwrap();
    assert_eq!(STARTED.load(Ordering::Relaxed), 6);
    let wake = Arc::new(unityrs_batch::sys::Wake::new().unwrap());
    let registration =
        unityrs_batch::sys::RegisteredWake::new(&wake, runtimes.controller.handle()).unwrap();
    wake.notify().unwrap();
    runtimes.controller.block_on(registration.ready()).unwrap();
    assert_eq!(STARTED.load(Ordering::Relaxed), 6);
    assert!(Runtimes::prepare(0).is_err());
    assert!(Runtimes::prepare(257).is_err());
}
