mod controller_support;
mod stage_support;
use controller_support::*;
use std::{process::Command, sync::Arc};
use unityrs_batch::controller::{Boundary, NoHooks};
use unityrs_core::limits::Limits;
use unityrs_testkit::{
    blob::NodeSpec,
    child,
    depth::{Depth, Row, Z_ROWS},
    env, guard,
    serialized::{self, TypeSpec},
};

fn nodes(rows: &[Row]) -> Vec<NodeSpec> {
    rows.iter()
        .map(|&(level, ty, name, meta)| {
            let mut node = NodeSpec::new(level, ty, name);
            node.meta_flag = meta;
            node
        })
        .collect()
}
fn deepest_value() -> Vec<u8> {
    let depth = Depth::new(&[]);
    let levels = Limits::default().max_value_depth;
    assert_eq!((levels - 2) % 5, 0);
    let n = usize::try_from((levels - 2) / 5).unwrap();
    let mut file = serialized::one_object(28, nodes(&depth.rows), depth.data(n));
    file.ref_types = [("A", depth.rows.as_slice()), ("Z", Z_ROWS.as_slice())]
        .into_iter()
        .map(|(class, rows)| TypeSpec {
            class_id: 114,
            script_type_index: -1,
            nodes: nodes(rows),
            ref_names: Some((class.as_bytes().to_vec(), b"n".to_vec(), b"a".to_vec())),
            ..TypeSpec::default()
        })
        .collect();
    env::bundle(&[("CAB-depth", serialized::build(&file))])
}
#[test]
fn depth_child() {
    let Ok(mode) = std::env::var("UNITYRS_CONTROLLER_DEPTH") else {
        return;
    };
    let () = guard::finishes("full worker depth", move || {
        let bytes = if mode == "value" {
            deepest_value()
        } else {
            let mut bytes = clear(32);
            for _ in 0..Limits::default().max_container_depth {
                bytes = env::bundle(&[("nested", bytes)]);
            }
            bytes
        };
        let mut h = Harness::new(&[("deep", bytes, vec![])], &[], 1, 1);
        let mut c = h.controller(Arc::new(NoHooks));
        assert_eq!(
            c.drive_local(h.execution.as_mut().unwrap()),
            Boundary::ReadyForWinners
        );
        let records = h.records(0);
        assert!(
            records
                .iter()
                .all(|r| r["reason"].as_str().is_none_or(|r| !r.contains("limit("))),
            "{records:?}"
        );
        assert!(
            c.state().inputs[0]
                .received
                .unwrap()
                .figures
                .decode_work_peak
                > 0
        );
        if mode == "value" {
            assert!(
                records.iter().any(|r| r["type"] == "skip"),
                "must read the deep value: {records:?}"
            );
        }
        h.execution.as_mut().unwrap().quiesce().unwrap();
    });
}
#[test]
fn the_deepest_value_and_nest_fit_a_worker_stack() {
    for mode in ["value", "nest"] {
        let output = child::output(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "depth_child", "--nocapture", "--test-threads=1"])
                .env("UNITYRS_CONTROLLER_DEPTH", mode),
        )
        .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
