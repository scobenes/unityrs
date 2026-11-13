mod controller_support;
mod stage_support;
use controller_support::*;
use std::{
    fs,
    sync::{Arc, Mutex, mpsc},
};
use unityrs_batch::{
    controller::{Boundary, Event, Point},
    phase1::JobKind,
};
use unityrs_testkit::guard;

#[test]
fn spec_stage_order_and_both_largest_first_queues_use_own_length_then_utf8_id() {
    let () = guard::finishes("queue ordering", || {
        let mut h = Harness::new(
            &[
                ("z", clear(1), vec!["large"]),
                ("é", clear(21), vec![]),
                ("b", clear(21), vec![]),
                ("a", clear(21), vec![]),
            ],
            &[("large", clear(90), vec![])],
            1,
            1,
        );
        let observe = Arc::new(Observe::default());
        let mut c = h.controller(observe.clone());
        assert_eq!(
            c.drive_local(h.execution.as_mut().unwrap()),
            Boundary::ReadyForWinners
        );
        let e = observe.events.lock().unwrap();
        let stages: Vec<_> = e
            .iter()
            .filter_map(|e| {
                if let Event::StageDispatched(i) = e {
                    Some(*i)
                } else {
                    None
                }
            })
            .collect();
        let peeks: Vec<_> = e
            .iter()
            .filter_map(|e| {
                if let Event::PeekDispatched(i) = e {
                    Some(*i)
                } else {
                    None
                }
            })
            .collect();
        let attempts: Vec<_> = e
            .iter()
            .filter_map(|e| {
                if let Event::AttemptDispatched(i) = e {
                    Some(*i)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(stages, [0, 1, 2, 3, 4]);
        assert_eq!(peeks, [4, 3, 2, 1, 0]);
        assert_eq!(attempts, [3, 2, 1, 0]);
        h.execution.as_mut().unwrap().quiesce().unwrap();
    });
}

#[test]
fn receipt_order_commits_have_no_input_id_barrier() {
    let () = guard::finishes("reverse completion commits", || {
        let mut h = Harness::new(
            &[("a", clear(20), vec![]), ("z", clear(1), vec![])],
            &[],
            2,
            2,
        );
        let (release, held) = mpsc::channel();
        let held = Mutex::new(held);
        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = events.clone();
        let hooks = Arc::new(Tap {
            point: Box::new(move |p| {
                if p == Point::BeforeSend(JobKind::Attempt, 0) {
                    guard::hold(&held.lock().unwrap(), "second input commits");
                }
            }),
            event: Box::new(move |e| {
                observed.lock().unwrap().push(e);
                if e == Event::Renamed(1) {
                    release.send(()).unwrap();
                }
            }),
        });
        let mut c = h.controller(hooks);
        assert_eq!(
            c.drive_local(h.execution.as_mut().unwrap()),
            Boundary::ReadyForWinners
        );
        let commits: Vec<_> = events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| {
                if let Event::Renamed(i) = e {
                    Some(*i)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(commits, [1, 0]);
        for i in 0..2 {
            assert_eq!(h.records(i)[0]["status"], "complete", "{:?}", h.records(i));
        }
        h.execution.as_mut().unwrap().quiesce().unwrap();
    });
}

#[test]
fn source_rewrite_and_deletion_after_staging_cannot_change_attempts() {
    let () = guard::finishes("pinned staging", || {
        let mut h = Harness::new(
            &[("a", clear(20), vec![]), ("b", clear(1), vec![])],
            &[],
            2,
            2,
        );
        let a = h.source(0);
        let b = h.source(1);
        let count = std::sync::atomic::AtomicUsize::new(0);
        let hooks = Arc::new(Tap {
            event: Box::new(move |event| {
                if matches!(event, Event::StageReceived(_))
                    && count.fetch_add(1, std::sync::atomic::Ordering::AcqRel) == 1
                {
                    fs::write(&a, b"changed").unwrap();
                    fs::remove_file(&b).unwrap();
                }
            }),
            ..Tap::default()
        });
        let mut c = h.controller(hooks);
        assert_eq!(
            c.drive_local(h.execution.as_mut().unwrap()),
            Boundary::ReadyForWinners
        );
        assert_eq!(h.records(0)[0]["status"], "complete", "{:?}", h.records(0));
        assert_eq!(h.records(1)[0]["status"], "complete");
        h.execution.as_mut().unwrap().quiesce().unwrap();
    });
}

#[test]
fn barriers_mark_every_holder_before_the_first_commit_and_keep_failure_precedence() {
    let () = guard::finishes("barrier precedence", || {
        for peeks in [false, true] {
            let mut h = Harness::new(
                &[
                    ("x", b"not a bundle".to_vec(), vec!["a", "z"]),
                    ("y", clear(1), vec!["a", "z"]),
                    ("whole", clear(1), vec!["z"]),
                ],
                &[
                    ("a", b"not a bundle".to_vec(), vec![]),
                    ("z", b"not a bundle".to_vec(), vec![]),
                ],
                2,
                2,
            );
            if !peeks {
                fs::remove_file(h.source(3)).unwrap();
            }
            let mut c = h.controller(Arc::new(unityrs_batch::controller::NoHooks));
            assert_eq!(
                c.drive_local(h.execution.as_mut().unwrap()),
                Boundary::ReadyForWinners
            );
            assert_eq!(
                h.records(0)[0]["reason"],
                if peeks {
                    "not_a_bundle"
                } else {
                    "dependency_failed(a, source_missing)"
                }
            );
            assert_eq!(
                h.records(1)[0]["reason"],
                if peeks {
                    "dependency_failed(a, not_a_bundle)"
                } else {
                    "dependency_failed(a, source_missing)"
                }
            );
            assert_eq!(
                h.records(2)[0]["reason"],
                "dependency_failed(z, not_a_bundle)"
            );
            for i in 0..3 {
                assert!(h.records(i)[0]["ms"].is_null());
            }
            h.execution.as_mut().unwrap().quiesce().unwrap();
        }
    });
}

#[derive(Default)]
struct Registrations(Mutex<Vec<Vec<u8>>>);
impl unityrs_export::ngui::Hooks for Registrations {
    fn environment(&self, environment: &unityrs_core::env::Environment) {
        let mut seen = self.0.lock().unwrap();
        for name in environment.registry().keys() {
            if !seen.iter().any(|old| old == name.as_bytes()) {
                seen.push(name.as_bytes().to_vec());
            }
        }
    }
}
impl unityrs_batch::controller::Hooks for Registrations {}
#[test]
fn members_register_in_ascending_id_order_with_the_input_last() {
    let () = guard::finishes("worker registration order", || {
        let cab = unityrs_testkit::serialized::build(
            &unityrs_testkit::serialized::SerializedSpec::default(),
        );
        let body = |name| unityrs_testkit::env::bundle(&[(name, cab.clone())]);
        let mut h = Harness::new(
            &[("middle", body("CAB-own"), vec!["é", "z", "a"])],
            &[
                ("z", body("CAB-z"), vec![]),
                ("é", body("CAB-third"), vec![]),
                ("a", body("CAB-a"), vec![]),
            ],
            2,
            2,
        );
        let registrations = Arc::new(Registrations::default());
        let mut c = h.controller(registrations.clone());
        assert_eq!(
            c.drive_local(h.execution.as_mut().unwrap()),
            Boundary::ReadyForWinners
        );
        assert_eq!(
            *registrations.0.lock().unwrap(),
            [
                b"cab-a".to_vec(),
                b"cab-z".to_vec(),
                b"cab-third".to_vec(),
                b"cab-own".to_vec()
            ]
        );
        assert_eq!(h.records(0)[0]["status"], "complete");
        h.execution.as_mut().unwrap().quiesce().unwrap();
    });
}
