mod controller_support;
mod stage_support;
use controller_support::*;
use serde_json::json;
use std::sync::Arc;
use unityrs_batch::{
    controller::{Boundary, NoHooks},
    phase1,
};
use unityrs_testkit::guard;

#[test]
fn integrity_details_survive_direct_and_dependency_failure_commits() {
    let () = guard::finishes("committed integrity details", || {
        for detail in ["size", "sha256"] {
            let body = clear(1);
            let mut h = Harness::new(
                &[
                    ("holder", clear(1), vec!["shared"]),
                    ("shared", body.clone(), vec![]),
                ],
                &[],
                2,
                2,
            );
            let mut shared = json!({"id":"shared", "path":h.source(1), "sha256":"0".repeat(64)});
            if detail == "size" {
                shared["size"] = (u64::try_from(body.len()).unwrap() + 1).into();
            }
            h.prepared = Some(
                phase1::prepare(stage_support::from_bundles(
                    &[
                        json!({"id":"holder", "path":h.source(0), "deps":["shared"]}),
                        shared,
                    ],
                    &[],
                ))
                .unwrap(),
            );
            let mut c = h.controller(Arc::new(NoHooks));
            assert_eq!(
                c.drive_local(h.execution.as_mut().unwrap()),
                Boundary::ReadyForWinners
            );
            assert_eq!(
                h.records(0)[0]["reason"],
                "dependency_failed(shared, integrity_mismatch)"
            );
            assert_eq!(h.records(1)[0]["reason"], "integrity_mismatch");
            for input in 0..2 {
                assert_eq!(h.records(input)[0]["detail"], detail);
            }
            h.execution.as_mut().unwrap().quiesce().unwrap();
        }
    });
}
