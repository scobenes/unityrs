mod common;
mod controller_support;
#[allow(dead_code)]
mod directory_support;
mod stage_support;
use std::fs;
use unityrs_batch::{
    phase1,
    progress::{Snapshot, live::Service},
    reason::Code,
    run,
};
use unityrs_testkit::{guard::DEADLOCK_GUARD, scratch::Scratch};
struct Space;
impl unityrs_batch::start::spool::FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> std::io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
}
#[test]
fn complete_pipeline_handles_zero_candidates_and_failed_unused_dependency() {
    for failed in [false, true] {
        let scratch = Scratch::new("normal-run");
        fs::create_dir(scratch.join("spool")).unwrap();
        fs::create_dir(scratch.join("sink")).unwrap();
        fs::write(scratch.join("input"), controller_support::clear(1)).unwrap();
        let mut spec = common::base();
        spec["run_id"] = "r".into();
        spec["spool"]["dir"] = scratch.join("spool").to_str().unwrap().into();
        spec["sink"]["root"] = scratch.join("sink").to_str().unwrap().into();
        spec["inputs"] = serde_json::json!([{"id":"a","path":scratch.join("input")}]);
        spec["dependencies"] = if failed {
            serde_json::json!([{"id":"unused","path":scratch.join("missing")}])
        } else {
            serde_json::json!([])
        };
        let prepared = phase1::prepare(
            unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap(),
        )
        .unwrap();
        let service = Service::start(
            fs::File::create(scratch.join("progress")).unwrap(),
            Snapshot::validate(0),
        )
        .unwrap();
        let result = run::drive(
            prepared,
            123,
            &service.sender(),
            &Space,
            directory_support::fs(true),
        )
        .unwrap();
        assert_eq!(
            result.code,
            if failed {
                Code::Incomplete
            } else {
                Code::Complete
            }
        );
        let snapshot = service.sender().snapshot().unwrap();
        assert!(service.exit(
            snapshot,
            unityrs_batch::progress::Exit {
                status: result.status,
                code: result.code,
                reason: result.reason,
                detail: None
            },
            DEADLOCK_GUARD.min(std::time::Duration::from_secs(1))
        ));
        let head: serde_json::Value = serde_json::from_slice(
            &fs::read(scratch.join("sink/_unityrs/r/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(head["exit_code"], result.code.value());
        assert_eq!(head["rows"]["outputs"], 0);
    }
}
