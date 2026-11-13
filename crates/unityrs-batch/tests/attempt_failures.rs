//! E60, site classification, cooperative stops and the sealed lifetime audit.
mod artifact_support;
mod attempt_support;
use attempt_support::*;
use serde_json::json;
use std::{
    cell::Cell,
    fs,
    sync::{Arc, atomic::Ordering},
};
use unityrs_batch::{
    attempt::*,
    reason::{Code, Limit, LimitKind},
    records::{self, Status, Summary, Text},
    spool::Directory,
};
use unityrs_export::ngui::{self, NoHooks};
use unityrs_testkit::ngui::{failure_scenes, texture};
fn pair() -> unityrs_testkit::ngui::AtlasScene {
    simple(&[
        texture(1, b"first", 4, (1, 1), vec![1, 2, 3, 4]),
        texture(2, b"second", 4, (128, 128), vec![0; 65536]),
    ])
}
#[test]
fn input_failure_after_an_output_keeps_only_a_separate_failure_eligible() {
    let h = Harness::new(&pair(), "x");
    let mut cap = limits();
    cap.max_export_work = 8192;
    let end = h.run(cap, &NoHooks);
    clear(&end);
    assert_eq!(end.state, State::Failed);
    assert_eq!(end.code, Code::Budget);
    assert_eq!(
        end.reason,
        Some(ReasonCode::Limit(Limit {
            kind: LimitKind::ExportWork,
            value: 8192
        }))
    );
    assert_eq!(end.outputs, 1);
    assert!(h.directory.path().join("o0").exists());
    assert_eq!(
        fs::metadata(h.directory.path().join("records.ndjson"))
            .unwrap()
            .len(),
        0
    );
    let original_claim = h.directory.claimed();
    assert!(original_claim > 0);
    let mut failure = Directory::create(
        h.scratch.join("fail/0"),
        h.counter.clone(),
        Arc::new(unityrs_batch::spool::Host),
    )
    .unwrap();
    let (_, meter) = artifact_support::meter();
    let reason = format!("limit(export_work, {})", cap.max_export_work);
    let summary = records::write_failure_record(
        &failure,
        &meter,
        Summary {
            input: "x",
            status: Status::Failed,
            reason: Some(records::Reason::Text(Text::Raw(reason.as_bytes()))),
            detail: None,
            figures: Some(end.figures),
            spooled_part: end.spooled_part,
            claimed_bytes: 0,
            cut: false,
        },
    )
    .unwrap();
    let rows = rows(failure.path());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["status"], "failed");
    assert_eq!(rows[0]["skips"], 0);
    assert_eq!(rows[0]["export_work"], json!(end.figures.export_work));
    assert_eq!(rows[0]["spooled_part"], json!(summary.spooled_part));
    let before = h.counter.used();
    failure.remove().unwrap();
    assert!(h.counter.used() < before);
    assert_eq!(h.directory.claimed(), original_claim);
}
#[test]
fn failure_after_a_full_records_flush_never_flushes_pending_bytes() {
    let objects: Vec<_> = (0..140)
        .map(|i| {
            texture(
                i,
                format!("{i:04}/{}", vec!["a".repeat(200); 4].join("/")).as_bytes(),
                4,
                (1, 1),
                vec![0; 4],
            )
        })
        .collect();
    let h = Harness::new(&simple(&objects), "x");
    let mut cap = limits();
    cap.max_export_work = 450_000;
    let end = h.run(cap, &NoHooks);
    clear(&end);
    assert_eq!(end.state, State::Failed, "{end:?}");
    let bytes = fs::read(h.directory.path().join("records.ndjson")).unwrap();
    assert!(bytes.len() >= records::BUFFER);
    assert_eq!(bytes.len() % records::BUFFER, 0);
    assert!(!String::from_utf8(bytes).unwrap().contains("input_summary"));
    assert!(end.outputs > 1);
    assert_eq!(end.figures.skips, 0);
}
#[test]
fn final_record_work_refusal_leaves_outputs_uncommitted() {
    let scene = simple(&[texture(1, b"image", 4, (1, 1), vec![0; 4])]);
    let baseline = Harness::new(&scene, "x");
    let good = baseline.run(limits(), &NoHooks);
    clear(&good);
    let h = Harness::new(&scene, "x");
    let mut cap = limits();
    cap.max_export_work = good.figures.export_work - 1;
    let end = h.run(cap, &NoHooks);
    clear(&end);
    assert_eq!(end.state, State::Failed, "{end:?}");
    assert!(h.directory.path().join("o0").exists());
    assert_eq!(h.rows(), Vec::<serde_json::Value>::new());
}
struct StopAt<'a> {
    at: Point,
    stop: &'a std::sync::atomic::AtomicBool,
}
impl ngui::Hooks for StopAt<'_> {}
impl Hooks for StopAt<'_> {
    fn point(&self, point: Point) {
        if point == self.at {
            self.stop.store(true, Ordering::Release);
        }
    }
}
#[test]
fn stops_at_each_orchestration_boundary_return_with_empty_accounts() {
    for at in [
        Point::Opened,
        Point::Metadata,
        Point::Registered,
        Point::Copied(0),
        Point::BeforeSeal,
    ] {
        let h = Harness::new(&pair(), "x");
        let end = h.run(limits(), &StopAt { at, stop: &h.stop });
        clear(&end);
        assert_eq!(end.state, State::Stopped, "{at:?}: {end:?}");
        assert!(!h.rows().iter().any(|r| r["type"] == "input_summary"));
    }
    let h = Harness::new(&pair(), "x");
    h.stop.store(true, Ordering::Release);
    let end = h.run(limits(), &NoHooks);
    clear(&end);
    assert_eq!(end.state, State::Stopped);
    assert_eq!(h.rows(), Vec::<serde_json::Value>::new());
}
#[test]
fn outer_panic_requests_internal_stop_after_dropping_core_locals() {
    struct Panic {
        requested: Cell<bool>,
    }
    impl ngui::Hooks for Panic {}
    impl Hooks for Panic {
        fn point(&self, p: Point) {
            assert_ne!(p, Point::Registered, "outside decoder");
        }
        fn request_stop(&self, end: &AttemptEnd) {
            assert!(end.accounts.is_clear());
            self.requested.set(true);
        }
    }
    let h = Harness::new(&pair(), "x");
    let hook = Panic {
        requested: Cell::new(false),
    };
    let end = h.run(limits(), &hook);
    clear(&end);
    assert_eq!(end.state, State::RunFailure);
    assert_eq!(end.reason, Some(ReasonCode::Internal));
    assert!(hook.requested.get());
}
#[test]
fn classification_and_lifetime_matrix() {
    for (name, scene) in failure_scenes().unwrap() {
        let h = Harness::new(&scene, &scene.inputs[0]);
        let end = h.run(limits(), &NoHooks);
        clear(&end);
        assert_eq!(end.state, State::Failed, "{name}: {end:?}");
        let reason = match name {
            "member_not_a_bundle" | "input_not_a_bundle" => ReasonCode::NotABundle,
            "member_fails" => ReasonCode::ContainerError,
            _ => ReasonCode::EntryError,
        };
        assert_eq!(end.reason, Some(reason), "{name}: {end:?}");
        if reason == ReasonCode::EntryError {
            assert!(
                end.figures.decode_work_peak > 0,
                "early failed own reads must be sampled"
            );
        }
        assert_eq!(end.dependency.is_some(), name.starts_with("member"));
        assert_eq!(h.rows(), Vec::<serde_json::Value>::new());
    }
    let h = Harness::new(&pair(), "x");
    let mut cap = limits();
    cap.m = 64;
    let end = h.run(cap, &NoHooks);
    clear(&end);
    assert_eq!(end.state, State::Failed);
    assert_eq!(
        end.reason,
        Some(ReasonCode::Limit(Limit {
            kind: LimitKind::InputBytes,
            value: 64
        }))
    );
    let h = Harness::new(&pair(), "x");
    let mut cap = limits();
    cap.d = 1;
    let end = h.run(cap, &NoHooks);
    clear(&end);
    assert_eq!(end.reason, Some(ReasonCode::BudgetDependency));
}
#[test]
fn malformed_peek_shape_and_padding_are_internal() {
    for shape in 0..4 {
        let mut h = Harness::new(&pair(), "x");
        let path = h.scratch.join("in/peek");
        match shape {
            0 => h.peeks[0].0 += 1,
            1 => {
                fs::write(&path, []).unwrap();
                h.peeks[0].1 = Some(path);
            }
            2 => {
                fs::write(&path, [0, 0]).unwrap();
                h.peeks[0].1 = Some(path);
            }
            _ => {
                fs::write(&path, [0x80]).unwrap();
                h.peeks[0].1 = Some(path);
            }
        }
        let end = h.run(limits(), &NoHooks);
        clear(&end);
        assert_eq!(end.state, State::RunFailure, "{shape}: {end:?}");
        assert_eq!(end.reason, Some(ReasonCode::Internal));
    }
}
#[test]
fn final_flush_io_failure_keeps_claim_and_reports_exact_run_detail() {
    struct Set(artifact_support::Faults, i32);
    impl ngui::Hooks for Set {}
    impl Hooks for Set {
        fn point(&self, p: Point) {
            if p == Point::BeforeSeal {
                self.0.set(2, self.1);
            }
        }
    }
    for errno in [libc::ENOSPC, libc::EDQUOT, libc::EIO] {
        let mut h = Harness::new(&pair(), "x");
        let faults = artifact_support::Faults::default();
        h.directory = Directory::create(
            h.scratch.join("attempts/fault"),
            h.counter.clone(),
            Arc::new(faults.clone()),
        )
        .unwrap();
        let end = h.run(limits(), &Set(faults, errno));
        clear(&end);
        assert_eq!(end.state, State::RunFailure);
        if errno == libc::EIO {
            assert_eq!(end.reason, Some(ReasonCode::HostIo));
        } else {
            assert_eq!(end.reason, Some(ReasonCode::SpoolFull));
            assert_eq!(
                end.detail,
                Some(if errno == libc::ENOSPC {
                    DetailCode::Enospc
                } else {
                    DetailCode::Edquot
                })
            );
        }
        assert!(h.directory.claimed() > 0);
        assert_eq!(h.counter.used(), h.directory.claimed());
        assert!(
            fs::metadata(h.directory.path().join("records.ndjson"))
                .unwrap()
                .len()
                < records::BUFFER as u64
        );
    }
}

#[test]
fn dependency_ids_with_delimiters_preserve_member_identity() {
    for id in ["a, b", "a, limit(input_memory, 1)", "a) ("] {
        let (_, mut scene) = failure_scenes()
            .unwrap()
            .into_iter()
            .find(|(n, _)| *n == "member_fails")
            .unwrap();
        let dependency = scene
            .bundles
            .iter_mut()
            .find(|b| b.id == "d.bundle")
            .unwrap();
        dependency.id = id.to_owned();
        scene
            .bundles
            .iter_mut()
            .find(|b| b.id == "x.bundle")
            .unwrap()
            .deps = vec![id.to_owned()];
        let h = Harness::new(&scene, "x.bundle");
        let end = h.run(limits(), &NoHooks);
        clear(&end);
        assert_eq!(end.state, State::Failed, "{id}: {end:?}");
        assert_eq!(end.reason, Some(ReasonCode::ContainerError));
        assert_eq!(end.dependency, Some(0));
    }
}

#[test]
fn stop_just_before_finish_discards_pending_records_and_keeps_claims() {
    let h = Harness::new(&pair(), "x");
    let end = h.run(
        limits(),
        &StopAt {
            at: Point::BeforeFinish,
            stop: &h.stop,
        },
    );
    clear(&end);
    assert_eq!(end.state, State::Stopped, "{end:?}");
    assert_eq!(h.rows(), Vec::<serde_json::Value>::new());
    assert_eq!(end.outputs, 2);
    assert!(h.directory.claimed() > 0);
    assert_eq!(h.counter.used(), h.directory.claimed());
    assert_eq!(end.claimed_bytes, h.directory.claimed());
}

#[test]
fn stop_from_the_final_records_claim_discards_the_summary_and_keeps_claims() {
    use std::{io, path::Path, sync::atomic::AtomicBool};
    use unityrs_batch::spool::FileSystem;
    struct StopOnClaim {
        armed: AtomicBool,
        stop: Arc<AtomicBool>,
    }
    impl FileSystem for StopOnClaim {
        fn available(&self, _: &Path) -> io::Result<u64> {
            if self.armed.load(Ordering::Acquire) {
                self.stop.store(true, Ordering::Release);
            }
            Ok(u64::MAX)
        }
    }
    impl ngui::Hooks for StopOnClaim {
        fn stopped(&self) -> bool {
            self.stop.load(Ordering::Acquire)
        }
    }
    impl Hooks for StopOnClaim {
        fn point(&self, p: Point) {
            if p == Point::BeforeSeal {
                self.armed.store(true, Ordering::Release);
            }
        }
    }
    let mut h = Harness::new(&pair(), "x");
    let hook = Arc::new(StopOnClaim {
        armed: AtomicBool::new(false),
        stop: Arc::new(AtomicBool::new(false)),
    });
    h.directory = Directory::create(
        h.scratch.join("attempts/stop"),
        h.counter.clone(),
        hook.clone(),
    )
    .unwrap();
    let end = h.run(limits(), hook.as_ref());
    clear(&end);
    assert_eq!(end.state, State::Stopped, "{end:?}");
    assert_eq!(h.rows(), Vec::<serde_json::Value>::new());
    assert_eq!(end.outputs, 2);
    assert!(h.directory.claimed() >= 3 * 4096);
    assert_eq!(h.counter.used(), h.directory.claimed());
    assert_eq!(end.claimed_bytes, h.directory.claimed());
}

#[test]
fn expected_skip_has_complete_with_skips_status_on_disk_and_exit_zero() {
    let h = Harness::new(&simple(&[texture(1, b"empty", 4, (0, 1), vec![])]), "x");
    let end = h.run(limits(), &NoHooks);
    clear(&end);
    assert_eq!(end.state, State::Done);
    assert_eq!(end.code, Code::Complete);
    assert_eq!(end.skips_expected, 1);
    let rows = h.rows();
    let summary = rows.iter().find(|r| r["type"] == "input_summary").unwrap();
    assert_eq!(summary["status"], "complete_with_skips");
    assert_eq!(summary["skips"], 1);
}
