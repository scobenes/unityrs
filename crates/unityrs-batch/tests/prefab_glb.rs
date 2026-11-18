#[path = "prefab_glb/animation.rs"]
mod animation;
mod common;
mod publication_support;

use serde_json::{Value, json};
use std::{fs, sync::Arc};
use unityrs_batch::{
    phase1,
    progress::{Snapshot, live::Service},
    reason::Code,
    run,
};
use unityrs_testkit::{
    env,
    ngui::Object,
    scene3d::{self, Layout},
    scratch::Scratch,
};

struct Space;
impl unityrs_batch::start::spool::FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> std::io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
}

fn fixture(paths: &[(&str, i64)]) -> Vec<u8> {
    let graph = scene3d::file(
        Layout::Unity2022,
        &[],
        &[
            Object::new(1, 1, scene3d::game_object(b"root", &[(0, 2)])),
            Object::new(2, 4, scene3d::transform((0, 1), (0, 0), &[], false)),
        ],
    )
    .unwrap();
    let rows = paths
        .iter()
        .map(|&(path, id)| env::row(path, (0, 0), (1, id)))
        .collect();
    let catalog = env::serialized(
        &["CAB-rig"],
        &[env::Obj::asset_bundle(
            1,
            env::Bundle {
                rows,
                ..Default::default()
            },
        )],
    );
    env::bundle(&[("CAB-index", catalog), ("CAB-rig", graph)])
}

fn job(paths: &[(&str, i64)]) -> (Scratch, Value) {
    let scratch = Scratch::new("prefab-glb");
    fs::create_dir(scratch.join("spool")).unwrap();
    fs::create_dir(scratch.join("sink")).unwrap();
    fs::write(scratch.join("one.bundle"), fixture(paths)).unwrap();
    let mut spec = common::base();
    spec["spec_version"] = json!(3);
    spec["spool"]["dir"] = json!(scratch.join("spool"));
    spec["sink"]["root"] = json!(scratch.join("sink"));
    spec["inputs"] = json!([{"id":"a","path":scratch.join("one.bundle")}]);
    spec["dependencies"] = json!([]);
    spec["outputs"] = json!({"glb":{"prefab_key_template":"{container_path}.glb"}});
    (scratch, spec)
}
fn drive(scratch: &Scratch, spec: &Value) -> unityrs_batch::finish::ResultEnd {
    drive_with(scratch, spec, &run::Options::default())
}

fn drive_with(
    scratch: &Scratch,
    spec: &Value,
    options: &run::Options,
) -> unityrs_batch::finish::ResultEnd {
    let prepared =
        phase1::prepare(unityrs_batch::spec::parse(serde_json::to_vec(spec).unwrap()).unwrap())
            .unwrap();
    let service = Service::start(
        fs::File::create(scratch.join("progress")).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    run::drive_with(
        prepared,
        123,
        &service.sender(),
        (&Space, Arc::new(publication_support::Portable)),
        options,
    )
    .unwrap()
}
fn head(scratch: &Scratch) -> Value {
    serde_json::from_slice(&fs::read(scratch.join("sink/_unityrs/r1/manifest.json")).unwrap())
        .unwrap()
}
fn rows(scratch: &Scratch, attempt: u32, name: &str) -> Vec<Value> {
    fs::read_to_string(scratch.join(format!("sink/_unityrs/r1/{attempt}/{name}.0.ndjson")))
        .unwrap()
        .lines()
        .map(|row| serde_json::from_str(row).unwrap())
        .collect()
}

#[test]
fn two_container_rows_to_the_same_root_publish_two_units() {
    let (scratch, spec) = job(&[("first", 1), ("second", 1)]);
    assert_eq!(drive(&scratch, &spec).code, Code::Complete);
    let rows = rows(&scratch, 1, "outputs");
    assert_eq!(rows.len(), 2);
    for (ordinal, name) in ["first", "second"].into_iter().enumerate() {
        let row = &rows[ordinal];
        assert_eq!(row["kind"], "glb");
        assert_eq!(row["key"], format!("{name}.glb"));
        assert_eq!(row["unit"], row["key"]);
        assert_eq!(row["container_row"], ordinal);
        assert_eq!(row["container_path"], name);
        assert_eq!(row["visible_hash"], Value::Null);
        assert_eq!(row["mode"], Value::Null);
    }
    let first = fs::read(scratch.join("sink/first.glb")).unwrap();
    assert_eq!(&first[..4], b"glTF");
    assert_eq!(first, fs::read(scratch.join("sink/second.glb")).unwrap());
    assert_eq!(head(&scratch)["manifest_version"], 3);
}

#[test]
fn repeated_cab_names_and_records_are_stable_across_worker_counts() {
    let run = |workers| {
        let (scratch, mut spec) = job(&[("shared", 1), ("broken", 99)]);
        fs::write(
            scratch.join("two.bundle"),
            fixture(&[("shared", 1), ("other", 1)]),
        )
        .unwrap();
        fs::write(scratch.join("dep.bundle"), fixture(&[("dep-only", 1)])).unwrap();
        spec["dependencies"] = json!([{"id":"dep", "path":scratch.join("dep.bundle")}]);
        spec["inputs"][0]["deps"] = json!(["dep"]);
        spec["inputs"].as_array_mut().unwrap().push(json!({
            "id":"b", "path":scratch.join("two.bundle"), "deps":["dep"]
        }));
        spec["threads"] = json!(workers);
        assert_eq!(drive(&scratch, &spec).code, Code::CompleteWithSkips);
        let outputs = rows(&scratch, 1, "outputs");
        let skips = rows(&scratch, 1, "skips");
        let collisions = rows(&scratch, 1, "collisions");
        let diagnostics = rows(&scratch, 1, "diagnostics");
        assert_eq!(outputs.len(), 2);
        assert_eq!(skips.len(), 1);
        assert_eq!(collisions.len(), 2);
        assert!(
            diagnostics
                .iter()
                .any(|row| row["kind"] == "same_named_cab")
        );
        assert!(!scratch.join("sink/dep-only.glb").exists());
        (outputs, skips, collisions, diagnostics)
    };
    assert_eq!(run(1), run(2));
}

#[test]
fn broken_first_owner_reserves_the_key() {
    let (scratch, spec) = job(&[("same", 99), ("same", 1)]);
    assert_eq!(drive(&scratch, &spec).code, Code::CompleteWithSkips);
    let skips = rows(&scratch, 1, "skips");
    assert_eq!(skips[0]["reason"], "bad_pointer");
    assert_eq!(skips[1]["reason"], "duplicate_key");
    assert_eq!(
        rows(&scratch, 1, "inputs")[0]["status"],
        "complete_with_skips"
    );
    assert!(!scratch.join("sink/same.glb").exists());
}

#[test]
fn unknown_prefab_key_fences_final_head_and_can_be_corrected() {
    let (scratch, mut spec) = job(&[("first", 1)]);
    spec["outputs"]["glb"]["profile"] = json!({
        "profile_version":1, "clips_by_prefab":{"some/missing.glb":{"a":[]}}
    });
    let end = drive(&scratch, &spec);
    assert_eq!(end.code, Code::Incomplete);
    assert_eq!(end.reason, Some("unknown_prefab_key"));
    let final_head = head(&scratch);
    assert_eq!(final_head["status"], "incomplete");
    assert_eq!(final_head["stop_reason"], "unknown_prefab_key");
    assert_eq!(
        final_head["stop_detail"],
        "/outputs/glb/profile/clips_by_prefab/some~1missing.glb/a"
    );
    assert_eq!(final_head["write_seq"], 1);
    assert!(!scratch.join("sink/first.glb").exists());
    spec["attempt"] = json!(2);
    spec["resume"] = json!(true);
    assert_eq!(drive(&scratch, &spec).reason, Some("unknown_prefab_key"));
    spec["attempt"] = json!(3);
    spec["outputs"]["glb"]["profile"]["clips_by_prefab"] = json!({"first.glb":{"a":[]}});
    assert_eq!(drive(&scratch, &spec).code, Code::Complete);
    assert!(scratch.join("sink/first.glb").exists());
}

#[test]
fn a_broken_row_requiring_the_root_name_cannot_claim_the_key() {
    let (scratch, mut spec) = job(&[("broken", 99), ("valid", 1)]);
    spec["outputs"]["glb"]["prefab_key_template"] = json!("{name}.glb");
    assert_eq!(drive(&scratch, &spec).code, Code::CompleteWithSkips);
    let skips = rows(&scratch, 1, "skips");
    assert_eq!(skips.len(), 1);
    assert_eq!(skips[0]["reason"], "bad_pointer");
    assert_eq!(skips[0]["container_row"], 0);
    assert!(skips[0]["root_serialized_file"].is_null());
    assert!(scratch.join("sink/root.glb").exists());
}

#[test]
fn invalid_and_prefix_conflicting_keys_leave_other_units_eligible() {
    let (scratch, mut spec) = job(&[("../invalid", 1), ("same", 1), ("same/child", 1)]);
    spec["outputs"]["glb"]["prefab_key_template"] = json!("{container_path}");
    assert_eq!(drive(&scratch, &spec).code, Code::CompleteWithSkips);
    assert_eq!(
        rows(&scratch, 1, "skips")[0]["reason"],
        "unwritable_key(dot_segment)"
    );
    let output = rows(&scratch, 1, "outputs");
    assert_eq!(output[0]["reason"], "unwritable_key(prefix_conflict)");
    assert_eq!(output[1]["status"], "written");
    assert!(scratch.join("sink/same/child").exists());
}

#[test]
fn a_zero_artifact_v3_job_has_v3_head_and_v2_progress() {
    let (scratch, mut spec) = job(&[]);
    spec["outputs"] = json!({"textures":{"key":"{input}/{name}.png"}});
    assert_eq!(drive(&scratch, &spec).code, Code::Complete);
    let head = head(&scratch);
    assert_eq!(head["manifest_version"], 3);
    assert_eq!(head["spec_version"], 3);
    assert!(head["glb"].is_null());
    assert_eq!(head["counts"]["outputs_written"], 0);
    assert_eq!(head["run"]["glb_degraded"], 0);
    let log = fs::read_to_string(scratch.join("progress")).unwrap();
    assert_ne!(log, "");
    for line in log.lines() {
        let line: Value = serde_json::from_str(line).unwrap();
        assert_eq!(line["p"], 2);
    }
}

#[test]
fn interrupted_publication_of_a_v3_head_resumes_the_same_glbs() {
    use unityrs_batch::{lifetime::Stop, publish, reason::StopReason};
    struct AfterFirst(Arc<run::Lifetime>);
    impl publish::Hooks for AfterFirst {
        fn point(&self, point: publish::Point) {
            if point == publish::Point::Outcome {
                self.0.request(Stop::external(StopReason::Sigterm));
            }
        }
    }

    let (scratch, mut spec) = job(&[("first", 1), ("second", 1)]);
    spec["threads"] = json!(1);
    spec["download_concurrency"] = json!(1);
    let prepared =
        phase1::prepare(unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap())
            .unwrap();
    let service = Service::start(
        fs::File::create(scratch.join("progress")).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    let lifetime = Arc::new(run::Lifetime::default());
    let opts = run::Options {
        lifetime: lifetime.clone(),
        publish_hooks: Arc::new(AfterFirst(lifetime)),
        ..Default::default()
    };
    let interrupted = run::drive_with(
        prepared,
        123,
        &service.sender(),
        (&Space, Arc::new(publication_support::Portable)),
        &opts,
    )
    .unwrap();
    assert_eq!(interrupted.code, Code::Stopped);
    let interim = head(&scratch);
    assert_eq!(interim["manifest_version"], 3);
    assert_eq!(interim["status"], "incomplete");
    assert_eq!(interim["stop_reason"], "sigterm");
    drop(service);
    spec["attempt"] = json!(2);
    spec["resume"] = json!(true);
    assert_eq!(drive(&scratch, &spec).code, Code::Complete);
    let output = rows(&scratch, 2, "outputs");
    assert_eq!(output[0]["status"], "present");
    assert_eq!(output[1]["status"], "written");
    assert_eq!(output[0]["md5"], output[1]["md5"]);
}

#[test]
fn running_and_final_heads_match_every_spec_version() {
    struct RunningHead(std::path::PathBuf, u64);
    impl run::Observer for RunningHead {
        fn seam(&self, seam: run::Seam) {
            if seam != run::Seam::Selection {
                return;
            }
            let row: Value = serde_json::from_slice(&fs::read(&self.0).unwrap()).unwrap();
            assert_eq!(row["status"], "running");
            assert_eq!(row["write_seq"], 0);
            assert_eq!(row["manifest_version"], self.1);
            assert_eq!(row["spec_version"], self.1);
            assert_eq!(row.get("glb").is_some(), self.1 == 3);
        }
    }
    for version in [1, 2, 3] {
        let (scratch, mut spec) = job(&[]);
        spec["spec_version"] = json!(version);
        spec["outputs"] = json!({"textures":{"key":"{name}.png"}});
        let options = run::Options {
            observer: Arc::new(RunningHead(
                scratch.join("sink/_unityrs/r1/manifest.json"),
                version,
            )),
            ..Default::default()
        };
        assert_eq!(drive_with(&scratch, &spec, &options).code, Code::Complete);
        let row = head(&scratch);
        assert_eq!(row["write_seq"], 1);
        assert_eq!(row["manifest_version"], version);
        assert_eq!(row.get("glb").is_some(), version == 3);
    }
}

#[test]
fn a_stop_captured_in_phase_six_takes_precedence_over_unknown_key() {
    use unityrs_batch::{
        finish::{FinishCause, HandOver},
        lifetime::Stop,
        reason::StopReason,
    };
    struct LateStop(Arc<run::Lifetime>);
    impl run::Observer for LateStop {
        fn handed_over(&self, handover: &HandOver) {
            assert!(matches!(handover.cause, FinishCause::UnknownPrefab(_)));
            self.0.request(Stop::external(StopReason::Sigterm));
        }
    }
    let (scratch, mut spec) = job(&[("first", 1)]);
    spec["outputs"]["glb"]["profile"] = json!({
        "profile_version":1, "clips_by_prefab":{"missing.glb":{"a":[]}}
    });
    let lifetime = Arc::new(run::Lifetime::default());
    let options = run::Options {
        lifetime: lifetime.clone(),
        observer: Arc::new(LateStop(lifetime)),
        ..Default::default()
    };
    assert_eq!(drive_with(&scratch, &spec, &options).code, Code::Stopped);
    assert_eq!(head(&scratch)["stop_reason"], "sigterm");
    assert!(!scratch.join("sink/first.glb").exists());
}
