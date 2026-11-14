//! Reuse pinned `MeshHandler` answers as input arrays, round-trip through a
//! synthetic Unity scene and collect, then let the independent checker compare
//! the emitted accessor with the oracle's original f64 values.
use serde_json::Value;
use std::process::Command;
use unityrs_gltf::{Profile, collect};
use unityrs_testkit::scene3d::mesh::{self, MeshVersion};
mod collect_support;
use collect_support as h;
fn number(v: &Value) -> f64 {
    if let Some(s) = v.get("f").and_then(Value::as_str) {
        f64::from_bits(u64::from_str_radix(s, 16).unwrap())
    } else {
        v.as_f64().unwrap()
    }
}
#[test]
#[allow(clippy::cast_possible_truncation)]
fn l6_position_arrays_survive_collect_and_glb_conversion() {
    let oracle: Value = serde_json::from_str(include_str!(
        "../../unityrs-meshdecode/tests/fixtures/process.json"
    ))
    .unwrap();
    let scratch = unityrs_testkit::scratch::Scratch::new("gltf-collect-oracle");
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/parity/gltf_check.py");
    let mut checked = 0;
    for case in oracle["cases"].as_array().unwrap() {
        if case["version"][0].as_u64().unwrap() < 2019 {
            continue;
        }
        let Some(rows) = case["process"]["ok"]["attrs"]["m_Vertices"].as_array() else {
            continue;
        };
        if rows.len() < 3 || rows.iter().any(|r| r.as_array().unwrap().len() != 3) {
            continue;
        }
        let raw: Vec<Vec<f64>> = rows
            .iter()
            .map(|r| r.as_array().unwrap().iter().map(number).collect())
            .collect();
        if raw
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || v.abs() > 1e10)
        {
            continue;
        }
        let vertices: Vec<[f32; 3]> = raw
            .iter()
            .map(|r| [r[0] as f32, r[1] as f32, r[2] as f32])
            .collect();
        let mut objects = h::objects(false, false);
        objects[6] = mesh::mesh(7, b"oracle", &vertices, &[0, 1, 2], MeshVersion::V2022);
        let (env, g) = h::graph(&objects);
        let m = h::meter();
        let scene = collect(&env, &g, &Profile::new(&m), &m).unwrap();
        let path = scratch.path().join("oracle.glb");
        std::fs::write(&path, h::write(&scene)).unwrap();
        let expected = serde_json::json!([{"mesh":0,"primitive":0,"attributes":{"POSITION":raw},"indices":[[0,1,2]]}]);
        let expect_path = scratch.path().join("expected.json");
        std::fs::write(&expect_path, serde_json::to_vec(&expected).unwrap()).unwrap();
        let out = unityrs_testkit::child::output(
            Command::new("python3")
                .arg(&script)
                .arg(&path)
                .arg("--expected")
                .arg(&expect_path),
        )
        .unwrap();
        assert!(
            out.status.success(),
            "{}: {}",
            case["name"],
            String::from_utf8_lossy(&out.stderr)
        );
        checked += 1;
        if checked == 16 {
            break;
        }
    }
    assert_eq!(checked, 16);
}
