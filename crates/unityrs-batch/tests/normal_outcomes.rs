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
use unityrs_testkit::{ngui, scratch::Scratch};
struct Space;
impl unityrs_batch::start::spool::FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> std::io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
}
fn execute(
    scratch: &Scratch,
    spec: &Value,
) -> Result<unityrs_batch::finish::ResultEnd, run::Error> {
    let prepared =
        phase1::prepare(unityrs_batch::spec::parse(serde_json::to_vec(spec).unwrap()).unwrap())
            .unwrap();
    let service = Service::start(
        fs::File::create(scratch.join("progress")).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    run::drive(
        prepared,
        0,
        &service.sender(),
        &Space,
        Arc::new(publication_support::Portable),
    )
}
fn base(scratch: &Scratch, objects: &[ngui::Object]) -> Value {
    for name in ["spool", "sink"] {
        fs::create_dir(scratch.join(name)).unwrap();
    }
    let file = ngui::file(&[], objects, true).unwrap();
    fs::write(
        scratch.join("source"),
        unityrs_testkit::env::bundle(&[("CAB-test", file)]),
    )
    .unwrap();
    let mut spec = common::base();
    spec["spool"]["dir"] = json!(scratch.join("spool"));
    spec["sink"]["root"] = json!(scratch.join("sink"));
    spec["inputs"] = json!([{"id":"x","path":scratch.join("source")}]);
    spec["dependencies"] = json!([]);
    spec["outputs"] = json!({"textures":{"key":"{name}"}});
    spec
}
fn rows(scratch: &Scratch, family: &str) -> Vec<Value> {
    let head: Value =
        serde_json::from_slice(&fs::read(scratch.join("sink/_unityrs/r1/manifest.json")).unwrap())
            .unwrap();
    let path = scratch.join(format!(
        "sink/_unityrs/r1/{}/{family}.0.ndjson",
        head["attempt"].as_u64().unwrap()
    ));
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}
#[test]
fn budget_failure_is_five_and_failed_inputs_withhold_only_when_requested() {
    let scratch = Scratch::new("export-budget");
    let large = ngui::texture(1, b"large", 4, (512, 512), vec![0; 512 * 512 * 4]);
    let mut spec = base(&scratch, &[large]);
    spec["max_export_work"] = (1 << 20).into();
    assert_eq!(execute(&scratch, &spec).unwrap().code, Code::Budget);
    assert_eq!(
        rows(&scratch, "inputs")[0]["reason"],
        "limit(export_work, 1048576)"
    );
    for incomplete in [false, true] {
        let scratch = Scratch::new("withhold-normal");
        let mut spec = base(
            &scratch,
            &[ngui::texture(1, b"key", 4, (1, 1), vec![1, 2, 3, 255])],
        );
        spec["inputs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"missing","path":scratch.join("missing")}));
        spec["publish_incomplete"] = incomplete.into();
        assert_eq!(execute(&scratch, &spec).unwrap().code, Code::Incomplete);
        let output = rows(&scratch, "outputs");
        assert_eq!(
            output[0]["status"],
            if incomplete { "written" } else { "withheld" }
        );
        assert_eq!(scratch.join("sink/key").exists(), incomplete);
    }
}
#[test]
fn prefix_conflicts_and_preferences_survive_the_complete_pipeline() {
    let scratch = Scratch::new("prefix-normal");
    let spec = base(
        &scratch,
        &[
            ngui::texture(1, b"a", 4, (1, 1), vec![1, 2, 3, 255]),
            ngui::texture(2, b"a/b", 4, (1, 1), vec![4, 5, 6, 255]),
        ],
    );
    assert_eq!(
        execute(&scratch, &spec).unwrap().code,
        Code::CompleteWithSkips
    );
    assert!(
        rows(&scratch, "outputs")
            .iter()
            .any(|r| r["reason"] == "unwritable_key(prefix_conflict)")
    );
    let scratch = Scratch::new("prefer-normal");
    let mut spec = base(
        &scratch,
        &[ngui::texture(1, b"key", 4, (1, 1), vec![1, 2, 3, 255])],
    );
    let second = ngui::file(
        &[],
        &[ngui::texture(1, b"key", 4, (1, 1), vec![4, 5, 6, 255])],
        true,
    )
    .unwrap();
    fs::write(
        scratch.join("second"),
        unityrs_testkit::env::bundle(&[("CAB-second", second)]),
    )
    .unwrap();
    spec["inputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"z","path":scratch.join("second")}));
    assert_eq!(execute(&scratch, &spec).unwrap().code, Code::Complete);
    assert!(
        rows(&scratch, "collisions")
            .iter()
            .all(|r| r["flip"] == true)
    );
    spec["attempt"] = 2.into();
    spec["prefer"] = json!({"key":{"input":"z"}});
    assert_eq!(execute(&scratch, &spec).unwrap().code, Code::Complete);
    assert_eq!(rows(&scratch, "outputs")[0]["input"], "z");
    let collisions = rows(&scratch, "collisions");
    assert_eq!(collisions.len(), 2);
    assert!(
        collisions
            .iter()
            .all(|r| r["rule"] == "prefer_input" && r["flip"] == false)
    );
}
#[test]
fn a_completed_attempt_is_fenced_and_bad_spool_does_not_write_a_head() {
    let scratch = Scratch::new("fenced-normal");
    let spec = base(&scratch, &[]);
    assert_eq!(execute(&scratch, &spec).unwrap().code, Code::Complete);
    let head = fs::read(scratch.join("sink/_unityrs/r1/manifest.json")).unwrap();
    assert_eq!(execute(&scratch, &spec).unwrap_err().code, Code::Superseded);
    assert_eq!(
        fs::read(scratch.join("sink/_unityrs/r1/manifest.json")).unwrap(),
        head
    );
    let scratch = Scratch::new("bad-spool-normal");
    let spec = base(&scratch, &[]);
    fs::write(scratch.join("spool/foreign"), b"keep").unwrap();
    assert_eq!(
        execute(&scratch, &spec).unwrap_err().code,
        Code::CouldNotStart
    );
    assert!(!scratch.join("sink/_unityrs/r1/manifest.json").exists());
}
