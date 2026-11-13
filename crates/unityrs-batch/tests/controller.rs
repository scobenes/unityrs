mod controller_support;
mod stage_support;
use controller_support::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use unityrs_batch::{
    attempt::ReasonCode,
    control::{Control, Culprit},
    controller::{Admission, Boundary, Event},
    job::RunFailure,
};

#[test]
fn admission_reserves_whole_slots_and_refuses_the_impossible_floor() {
    let () = unityrs_testkit::guard::finishes("controller test", || {
        for threads in [1, 2, 8] {
            let a = Admission::new(threads, 13, 17, 95).unwrap();
            assert_eq!(a.slots(), threads.min(3) as usize);
            assert_eq!(a.reservation(), 30);
        }
        assert!(Admission::new(8, 13, 17, 29).is_err());
        assert!(Admission::new(0, 13, 17, 95).is_err());
        assert!(Admission::new(8, u64::MAX, 17, u64::MAX).is_err());
    });
}

#[test]
fn control_is_canonical_and_requests_are_synchronous() {
    let control = AtomicBool::new(false);
    assert!(!control.stopped());
    control.request_stop(RunFailure::new(ReasonCode::Internal), Culprit::None);
    assert!(control.load(Ordering::Acquire));
}

#[test]
fn stages_peeks_and_attempts_read_pinned_files_and_commit_disk_artifacts() {
    let () = unityrs_testkit::guard::finishes("controller test", || {
        for threads in [1, 2, 8] {
            let mut h = Harness::new(
                &[("z", clear(30), vec![]), ("a", clear(10), vec![])],
                &[],
                threads,
                2,
            );
            let events = Arc::new(Observe::default());
            let mut c = h.controller(events.clone());
            assert_eq!(
                c.drive_local(h.execution.as_mut().unwrap()),
                Boundary::ReadyForWinners
            );
            assert_eq!(c.state().uncommitted.get(0), Some(false));
            for i in 0..2 {
                let records = h.records(i);
                assert_eq!(records.len(), 1);
                assert_eq!(records[0]["status"], "complete");
            }
            let events = events.events.lock().unwrap();
            assert_eq!(
                events
                    .iter()
                    .filter(|e| matches!(e, Event::StageDispatched(_)))
                    .count(),
                2
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|e| matches!(e, Event::PeekDispatched(_)))
                    .count(),
                2
            );
            let first_attempt = events
                .iter()
                .position(|e| matches!(e, Event::AttemptDispatched(_)))
                .unwrap();
            assert_eq!(
                events[..first_attempt]
                    .iter()
                    .filter(|e| matches!(e, Event::PeekReceived(_)))
                    .count(),
                2
            );
            h.execution.as_mut().unwrap().quiesce().unwrap();
        }
    });
}

#[test]
fn staging_failure_is_committed_before_any_peek_and_unused_dependencies_are_staged() {
    let () = unityrs_testkit::guard::finishes("staging barrier failure", || {
        let mut h = Harness::new(
            &[("z", clear(4), vec!["missing"]), ("a", clear(5), vec![])],
            &[("missing", vec![], vec![]), ("unused", clear(9), vec![])],
            2,
            2,
        );
        std::fs::remove_file(h.source(2)).unwrap();
        let events = Arc::new(Observe::default());
        let mut c = h.controller(events.clone());
        assert_eq!(
            c.drive_local(h.execution.as_mut().unwrap()),
            Boundary::ReadyForWinners
        );
        assert_eq!(
            h.records(0)[0]["reason"],
            "dependency_failed(missing, source_missing)"
        );
        let e = events.events.lock().unwrap();
        let failure = e.iter().position(|e| *e == Event::Renamed(0)).unwrap();
        let peek = e
            .iter()
            .position(|e| matches!(e, Event::PeekDispatched(_)))
            .unwrap();
        assert!(failure < peek);
        assert!(!e.contains(&Event::PeekDispatched(3)));
        assert!(e.contains(&Event::StageDispatched(3)));
        h.execution.as_mut().unwrap().quiesce().unwrap();
    });
}
