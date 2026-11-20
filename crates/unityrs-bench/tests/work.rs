//! The Rust workloads (`unityrs_bench::work`, plan section 4.2): one scope
//! for each row of `tools/bench/workloads.json`, named for it, and each
//! runs on its gate-scale fixture to what the fixture's inventory says it
//! holds: its objects, its textures and pixels, and for `many-blocks` the
//! entry whose sha256 the inventory records (the digest `py_workloads.py`
//! computes too).

use std::path::Path;

use serde_json::Value;
use unityrs_bench::work::{self, Op, SCOPES};
use unityrs_bench::{Scale, build};

fn workloads() -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/bench/workloads.json");
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    doc["workloads"].as_array().unwrap().clone()
}

#[test]
fn every_workload_has_its_scope_and_operation_and_no_other() {
    let table = workloads();
    let names: Vec<&str> = table.iter().map(|w| w["name"].as_str().unwrap()).collect();
    let scopes: Vec<&str> = SCOPES.iter().map(|s| s.0).collect();
    assert_eq!(scopes, names);
    for w in &table {
        let (op, _) = work::find(w["name"].as_str().unwrap()).unwrap();
        let want = match w["op"].as_str().unwrap() {
            "load" => Op::Load,
            "load_entries" => Op::LoadEntries,
            "typetree" => Op::Typetree,
            "texture" => Op::Texture,
            other => panic!("op {other}"),
        };
        assert_eq!(op, want, "{}", w["name"]);
    }
    for (name, func, ..) in SCOPES {
        assert_eq!(work::scope_name(name), *func);
    }
}

#[test]
fn every_scope_processes_its_gate_fixture() {
    for w in workloads() {
        let name = w["name"].as_str().unwrap();
        let fixture = build(w["fixture"].as_str().unwrap(), Scale::Gate)
            .unwrap()
            .unwrap();
        let (op, scope) = work::find(name).unwrap();
        let input = work::setup(op, fixture.bytes.into()).unwrap();
        let out = scope(&input).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (digest, counts) = work::digest(&out).unwrap();
        let inv = &fixture.inventory;
        match op {
            Op::Load | Op::Typetree => assert_eq!(counts["objects"], inv["objects"], "{name}"),
            Op::LoadEntries => {
                assert_eq!(counts["entry_bytes"], inv["entry_bytes"]);
                assert_eq!(digest, inv["entry_sha256"].as_str().unwrap());
            }
            Op::Texture => {
                assert_eq!(counts["textures"], inv["textures"], "{name}");
                assert_eq!(counts["pixels"], inv["pixels"], "{name}");
            }
        }
        assert_eq!(digest.len(), 64);
    }
}

#[test]
fn a_scope_refuses_an_input_of_another_operation() {
    let fixture = build("textures-DXT1.bundle", Scale::Gate).unwrap().unwrap();
    let input = work::setup(Op::Load, fixture.bytes.into()).unwrap();
    assert!(work::run_texture_dxt1(&input).is_err());
}
