mod controller_support;
mod stage_support;
use controller_support::*;
use std::{fs, os::unix::fs::PermissionsExt, sync::Arc};
use unityrs_batch::{
    controller::{Boundary, Event},
    peek::word::{FailureCode, Outcome},
    reason::LimitKind,
};
use unityrs_testkit::{
    bundle::{self, BundleSpec, EntrySpec},
    guard,
};

fn limited() -> Vec<u8> {
    bundle::build(&BundleSpec {
        block_size: 2 << 20,
        entries: vec![EntrySpec {
            name: b"large".to_vec(),
            flags: 0,
            data: vec![0; 2 << 20],
        }],
        ..BundleSpec::default()
    })
}

#[test]
fn a_bundles_peek_runs_once_and_its_limit_fails_every_input_that_holds_it() {
    let () = guard::finishes("X11 behavioural subset", || {
        for threads in [1, 2, 8] {
            let limited = limited();
            assert!(limited.len() < 1 << 20);
            let mut h = Harness::new(
                &[
                    ("holder", clear(30), vec!["shared"]),
                    ("pressure", clear(40), vec!["shared", "extra"]),
                    ("shared", limited, vec![]),
                    ("zz-own", b"bad".to_vec(), vec!["shared"]),
                    ("least", clear(1), vec!["a-bad", "shared"]),
                    ("incomplete", clear(1), vec!["a-bad", "missing"]),
                    ("marked", marked(), vec![]),
                    ("clear", clear(1), vec![]),
                ],
                &[
                    ("a-bad", b"bad".to_vec(), vec![]),
                    ("missing", clear(1), vec![]),
                    ("unused", marked(), vec![]),
                    ("extra", clear(30_000), vec![]),
                ],
                threads,
                2,
            );
            fs::remove_file(h.source(9)).unwrap();
            h.configure(|s| {
                s.max_decoded_bytes = 1 << 20;
                s.dep_cache_bytes = 1 << 20;
            });
            let events = Arc::new(Observe::default());
            let mut c = h.controller(events.clone());
            assert_eq!(
                c.drive_local(h.execution.as_mut().unwrap()),
                Boundary::ReadyForWinners
            );
            let expected = Outcome::Failed(FailureCode::Limit(LimitKind::InputMemory));
            assert_eq!(c.state().peek_words[2].load(), Ok(expected));
            assert_eq!(c.state().bundles[2].peek.unwrap().outcome, Some(expected));
            for i in 0..2 {
                assert_eq!(
                    h.records(i)[0]["reason"],
                    "dependency_failed(shared, limit(input_memory, 1048576))"
                );
            }
            assert_eq!(h.records(2)[0]["reason"], "limit(input_memory, 1048576)");
            assert_eq!(h.records(3)[0]["reason"], "not_a_bundle");
            assert_eq!(
                h.records(4)[0]["reason"],
                "dependency_failed(a-bad, not_a_bundle)"
            );
            assert_eq!(
                h.records(5)[0]["reason"],
                "dependency_failed(missing, source_missing)"
            );
            assert_eq!(c.state().peek_words[11].load(), Ok(Outcome::Clear(1)));
            let e = events.events.lock().unwrap();
            for i in [0, 1, 2, 3, 4, 6, 7, 8, 11] {
                assert_eq!(
                    e.iter()
                        .filter(|event| **event == Event::PeekDispatched(i))
                        .count(),
                    1,
                    "bundle {i}, threads {threads}"
                );
            }
            for i in [5, 9, 10] {
                assert!(!e.contains(&Event::PeekDispatched(i)));
            }
            let staging_commit = e.iter().position(|e| *e == Event::Renamed(5)).unwrap();
            let first_peek = e
                .iter()
                .position(|e| matches!(e, Event::PeekDispatched(_)))
                .unwrap();
            assert!(staging_commit < first_peek);
            let first_attempt = e
                .iter()
                .position(|e| matches!(e, Event::AttemptDispatched(_)))
                .unwrap();
            for i in 0..6 {
                assert!(e.iter().position(|e| *e == Event::Renamed(i)).unwrap() < first_attempt);
            }
            assert_eq!(
                e[..first_attempt]
                    .iter()
                    .filter(|e| matches!(e, Event::PeekReceived(_)))
                    .count(),
                9
            );
            let bits = h.scratch.join("in/pi6");
            assert_eq!(fs::read(&bits).unwrap(), [0x81, 0x05]);
            assert_eq!(
                fs::metadata(bits).unwrap().permissions().mode() & 0o777,
                0o400
            );
            assert!(!h.scratch.join("in/pi7").exists());
            assert_eq!(c.state().peek_words[10].load(), Ok(Outcome::Pending));
            h.execution.as_mut().unwrap().quiesce().unwrap();
        }
    });
}

#[test]
fn shared_peek_uses_m_while_its_holder_is_limited_by_d() {
    let () = guard::finishes("X11 independent peek and dependency budgets", || {
        let mut h = Harness::new(
            &[
                ("holder", clear(1), vec!["shared"]),
                (
                    "shared",
                    bundle::build(&BundleSpec {
                        entries: (0..16_384)
                            .map(|i| EntrySpec {
                                name: format!("entry{i:05}-{}", "n".repeat(96)).into_bytes(),
                                flags: 0,
                                data: vec![0],
                            })
                            .collect(),
                        ..BundleSpec::default()
                    }),
                    vec![],
                ),
            ],
            &[],
            2,
            2,
        );
        h.configure(|s| {
            s.max_decoded_bytes = 16 << 20;
            s.dep_cache_bytes = 1 << 20;
        });
        let events = Arc::new(Observe::default());
        let mut c = h.controller(events.clone());
        assert_eq!(
            c.drive_local(h.execution.as_mut().unwrap()),
            Boundary::ReadyForWinners
        );
        assert_eq!(c.state().peek_words[1].load(), Ok(Outcome::Clear(16_384)));
        assert_eq!(
            c.state().bundles[1].peek.unwrap().outcome,
            Some(Outcome::Clear(16_384))
        );
        assert_eq!(
            h.records(0).iter().next_back().unwrap()["reason"],
            "dependency_failed(shared, budget(dependency))"
        );
        let e = events.events.lock().unwrap();
        assert_eq!(
            e.iter().filter(|e| **e == Event::PeekDispatched(1)).count(),
            1
        );
        assert!(e.contains(&Event::AttemptDispatched(0)));
        h.execution.as_mut().unwrap().quiesce().unwrap();
    });
}
