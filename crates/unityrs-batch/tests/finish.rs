mod common;
#[allow(dead_code)]
mod directory_support;
mod publication_support;
use publication_support::Scene;
use unityrs_batch::{
    finish, manifest, phase::Account, publish::Gate, reason::Code, spool::Host, winners,
};

#[test]
fn normal_head_is_complete_and_self_consistent() {
    let scene = Scene::new(&["key"], false, 4096);
    std::fs::create_dir(scene.scratch.join("sink")).unwrap();
    let dir =
        directory_support::preflight(&scene.scratch.join("sink"), 1, directory_support::fs(true))
            .unwrap();
    let outcomes = scene.outcomes("out", &Host);
    outcomes
        .set(0, unityrs_batch::outcomes::Code::Written)
        .unwrap();
    let gate = Gate::default();
    let frozen = gate.freeze(&outcomes).unwrap();
    let account = Account::new(unityrs_batch::bound::C_FIN);
    let inputs = winners::committed::Committed::received(
        &scene.done,
        &scene.prepared.inputs,
        &Host,
        &|| false,
    )
    .unwrap();
    let cx = manifest::rows::Context {
        prepared: &scene.prepared,
        inputs: &inputs,
        handoff: &scene.handoff,
        workspace: &scene.workspace,
        frozen: &frozen,
        account: &account,
        stopped: &|| false,
        taken_at: std::time::Instant::now(),
    };
    let identity = finish::Identity {
        started_at: "2026-01-01T00:00:00Z",
        memory_bound: 123,
        spool_peak: 456,
        vm_hwm: 789,
    };
    finish::running(&scene.prepared, &dir, &account, &identity).unwrap();
    let result = finish::normal(&cx, &dir, &identity, "2026-01-01T00:00:01Z").unwrap();
    assert_eq!(result.code, Code::Complete);
    assert_eq!(result.status, Some("complete"));
    let bytes = std::fs::read(scene.scratch.join("sink/_unityrs/r/manifest.json")).unwrap();
    let head: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(head["write_seq"], 1);
    assert_eq!(head["exit_code"], 0);
    assert_eq!(head["phase"], "publish");
    assert_eq!(head["stop_reason"], serde_json::Value::Null);
    assert_eq!(head["counts"]["outputs_written"], 1);
    assert_eq!(head["run"]["memory_bound"], 123);
    assert_eq!(head["run"]["spool_peak"], 456);
    assert_eq!(head["run"]["vm_hwm"], 789);
    assert_eq!(head["rows"]["outputs"], 1);
    assert_eq!(account.snapshot().input.used, 0);
}

fn fault(
    operation: Option<&'static str>,
    after_rename: bool,
    row_obstacle: bool,
    code: Code,
) -> (finish::ResultEnd, serde_json::Value, Vec<&'static str>) {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    let mut scene = Scene::new(&["key"], false, 4096);
    scene.prepared.inputs[0].received.as_mut().unwrap().code = code;
    std::fs::create_dir(scene.scratch.join("sink")).unwrap();
    let fs = Arc::new(directory_support::Faults {
        fallback: Some(libc::ENOSYS),
        fail: operation.map(|s| (s, libc::EIO)),
        disarmed: AtomicBool::new(true),
        after_rename,
        crossing: AtomicUsize::new(usize::MAX),
        ..directory_support::Faults::default()
    });
    let directory =
        directory_support::preflight(&scene.scratch.join("sink"), 1, fs.clone()).unwrap();
    let out = scene.outcomes("out", &Host);
    out.set(0, unityrs_batch::outcomes::Code::Written).unwrap();
    let gate = Gate::default();
    let frozen = gate.freeze(&out).unwrap();
    let account = Account::new(unityrs_batch::bound::C_FIN);
    let inputs = winners::committed::Committed::received(
        &scene.done,
        &scene.prepared.inputs,
        &Host,
        &|| false,
    )
    .unwrap();
    let cx = manifest::rows::Context {
        prepared: &scene.prepared,
        inputs: &inputs,
        handoff: &scene.handoff,
        workspace: &scene.workspace,
        frozen: &frozen,
        account: &account,
        stopped: &|| false,
        taken_at: std::time::Instant::now(),
    };
    let identity = finish::Identity {
        started_at: "2026-01-01T00:00:00Z",
        memory_bound: 1,
        spool_peak: 2,
        vm_hwm: 3,
    };
    finish::running(&scene.prepared, &directory, &account, &identity).unwrap();
    if row_obstacle {
        std::fs::create_dir_all(scene.scratch.join("sink/_unityrs/r/1/outputs.0.ndjson")).unwrap();
    }
    fs.calls.lock().unwrap().clear();
    fs.disarmed.store(false, Ordering::SeqCst);
    let result = finish::normal(&cx, &directory, &identity, "2026-01-01T00:00:01Z").unwrap();
    let head = serde_json::from_slice(
        &std::fs::read(scene.scratch.join("sink/_unityrs/r/manifest.json")).unwrap(),
    )
    .unwrap();
    let calls = fs.calls.lock().unwrap().clone();
    (result, head, calls)
}
#[test]
fn rows_syncfs_and_final_head_have_independent_failure_semantics() {
    for operation in ["fsync", "fsync_dir", "write", "rename"] {
        let (result, head, _) = fault(Some(operation), false, false, Code::Complete);
        assert_eq!(result.code, Code::Incomplete, "{operation}");
        assert_eq!(result.status, Some("running"));
        // Write and rename faults have already made row publication
        // incomplete before the head gets its independent try.
        assert_eq!(
            result.reason,
            matches!(operation, "fsync" | "fsync_dir").then_some("publish_failed"),
            "{operation}"
        );
        // Directory fsync can fail after the physical head rename.
        assert_eq!(head["write_seq"], u8::from(operation == "fsync_dir"));
    }
    let (result, head, _) = fault(Some("syncfs"), false, false, Code::Complete);
    assert_eq!(result.code, Code::Host);
    assert_eq!(result.status, Some("incomplete"));
    assert_eq!(head["stop_reason"], "host_io");
    for row in head["rows"].as_object().unwrap().values() {
        assert!(row.is_null());
    }
    for count in head["counts"].as_object().unwrap().values() {
        assert_eq!(count, 0);
    }
    assert!(head["run"]["spooled_parts"].as_u64().unwrap() > 0);
    let (result, head, _) = fault(None, false, true, Code::Complete);
    assert_eq!(result.code, Code::Incomplete);
    assert_eq!(head["rows"]["outputs"], serde_json::Value::Null);
    assert_eq!(head["counts"]["outputs_written"], 0);
    assert_eq!(head["counts"]["bundles"], 2);
}
#[test]
fn final_head_acknowledgment_requires_the_entire_protocol() {
    let (_, _, calls) = fault(None, false, false, Code::Complete);
    let operations: Vec<_> = calls
        .into_iter()
        .filter(|s| ["syncfs", "fsync", "rename", "fsync_dir"].contains(s))
        .collect();
    assert_eq!(
        operations.get(operations.len() - 4..).unwrap(),
        ["syncfs", "fsync", "rename", "fsync_dir"]
    );
    let (result, head, _) = fault(Some("rename"), true, false, Code::Complete);
    assert_eq!(head["write_seq"], 1);
    assert_eq!(result.status, Some("running"));
    assert_eq!(result.code, Code::Incomplete);
}

#[test]
fn failed_head_does_not_invent_a_host_reason_for_an_existing_budget_code() {
    let (result, _, _) = fault(Some("fsync"), false, false, Code::Budget);
    assert_eq!(result.code, Code::Budget);
    assert_eq!(result.reason, None);
}

#[test]
fn failed_head_sets_publish_failed_only_when_it_changes_the_code() {
    for (code, reason) in [
        (Code::Complete, Some("publish_failed")),
        (Code::CompleteWithSkips, Some("publish_failed")),
        (Code::Incomplete, None),
    ] {
        let (result, _, _) = fault(Some("fsync"), false, false, code);
        assert_eq!(result.code, Code::Incomplete);
        assert_eq!(result.reason, reason, "pre-head {code:?}");
    }
}

struct CorruptRecords;
impl winners::Inputs for CorruptRecords {
    fn open(&self, _: u32) -> Result<Box<dyn std::io::Read + Send>, unityrs_batch::phase::Error> {
        Ok(Box::new(std::io::Cursor::new(b"not a record\n")))
    }
    fn failed(&self, _: u32) -> Result<bool, unityrs_batch::phase::Error> {
        Ok(false)
    }
}
#[test]
fn corrupt_internal_artifacts_produce_a_durable_internal_head() {
    corrupt_internal_head(false);
}

#[test]
fn syncfs_failure_preserves_a_more_severe_internal_stop_reason() {
    corrupt_internal_head(true);
}

fn corrupt_internal_head(sync_failed: bool) {
    let scene = Scene::new(&["key"], false, 4096);
    std::fs::create_dir(scene.scratch.join("sink")).unwrap();
    let fs = std::sync::Arc::new(directory_support::Faults {
        fallback: Some(libc::ENOSYS),
        fail: sync_failed.then_some(("syncfs", libc::EIO)),
        crossing: std::sync::atomic::AtomicUsize::new(usize::MAX),
        ..directory_support::Faults::default()
    });
    let directory = directory_support::preflight(&scene.scratch.join("sink"), 1, fs).unwrap();
    let out = scene.outcomes("out", &Host);
    let gate = Gate::default();
    let frozen = gate.freeze(&out).unwrap();
    let account = Account::new(unityrs_batch::bound::C_FIN);
    let identity = finish::Identity {
        started_at: "2026-01-01T00:00:00Z",
        memory_bound: 0,
        spool_peak: 0,
        vm_hwm: 0,
    };
    finish::running(&scene.prepared, &directory, &account, &identity).unwrap();
    let cx = manifest::rows::Context {
        prepared: &scene.prepared,
        inputs: &CorruptRecords,
        handoff: &scene.handoff,
        workspace: &scene.workspace,
        frozen: &frozen,
        account: &account,
        stopped: &|| false,
        taken_at: std::time::Instant::now(),
    };
    let result = finish::normal(&cx, &directory, &identity, "2026-01-01T00:00:01Z").unwrap();
    assert_eq!(result.code, Code::Internal);
    assert_eq!(result.status, Some("incomplete"));
    let head: serde_json::Value = serde_json::from_slice(
        &std::fs::read(scene.scratch.join("sink/_unityrs/r/manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(head["exit_code"], 70);
    assert_eq!(
        head["stop_reason"],
        if sync_failed {
            serde_json::json!("internal")
        } else {
            serde_json::Value::Null
        }
    );
}
