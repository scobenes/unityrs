use std::process::Command;
mod collect_support;
use collect_support as h;
#[test]
fn collected_scenes_pass_independent_accessors_and_orientation() {
    let scratch = unityrs_testkit::scratch::Scratch::new("gltf-collect-check");
    let scene = h::collect(false, false);
    let path = scratch.path().join("collect.glb");
    std::fs::write(&path, h::write(&scene)).unwrap();
    let expected = serde_json::json!([{"mesh":0,"primitive":0,"attributes":{
        "POSITION":[[1.,0.,0.],[0.,1.,0.],[0.,0.,0.]],
        "NORMAL":[[0.,0.,1.],[0.,0.,1.],[0.,0.,1.]],
        "TANGENT":[[1.,0.,0.,1.],[1.,0.,0.,1.],[1.,0.,0.,1.]],
        "TEXCOORD_0":[[f64::from(0.2f32),0.25],[f64::from(0.3f32),0.25],[f64::from(0.4f32),0.25]]},
        "indices":[[0,1,2]],"targets":[{"POSITION":[[0.,0.,0.],[0.,0.,0.],[2.,0.,0.]]}]}]);
    let expected_path = scratch.path().join("expected.json");
    std::fs::write(&expected_path, serde_json::to_vec(&expected).unwrap()).unwrap();
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/parity/gltf_check.py");
    let out = unityrs_testkit::child::output(
        Command::new("python3")
            .arg(script)
            .arg(path)
            .arg("--expected")
            .arg(expected_path),
    )
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["compared"], 1);
    assert_eq!(report["orientation"][0]["status"], "pass");
}
#[test]
fn collected_scenes_are_validator_clean() {
    let Some(exe) = std::env::var_os("UNITYRS_GLTF_VALIDATOR") else {
        assert!(std::env::var_os("UNITYRS_GLTF_VALIDATOR_REQUIRED").is_none());
        eprintln!("collect validator skipped: set UNITYRS_GLTF_VALIDATOR");
        return;
    };
    let scratch = unityrs_testkit::scratch::Scratch::new("gltf-collect-validator");
    let mut scenes = vec![
        h::collect(false, false),
        h::collect(true, false),
        h::collect(false, true),
    ];
    for cross_root in [false, true] {
        let (env, graph) = h::graph(&h::hierarchy_objects(cross_root));
        let meter = h::meter();
        scenes.push(
            unityrs_gltf::collect(&env, &graph, &unityrs_gltf::Profile::new(&meter), &meter)
                .unwrap(),
        );
    }
    let mut objects = h::objects(false, false);
    unityrs_testkit::scene3d::set(
        &mut objects[2].value,
        "m_IsActive",
        unityrs_testkit::ngui::V::Bool(false),
    );
    let (env, g) = h::graph(&objects);
    let m = h::meter();
    let mut p = unityrs_gltf::Profile::new(&m);
    scenes.push(unityrs_gltf::collect(&env, &g, &p, &m).unwrap());
    p.inactive = unityrs_gltf::Inactive::IncludeHidden;
    scenes.push(unityrs_gltf::collect(&env, &g, &p, &m).unwrap());
    for (i, scene) in scenes.into_iter().enumerate() {
        let path = scratch.path().join(format!("collect-{i}.glb"));
        std::fs::write(&path, h::write(&scene)).unwrap();
        let out = unityrs_testkit::child::output(
            Command::new(&exe).args(["--stdout", "--all"]).arg(&path),
        )
        .unwrap();
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["validatorVersion"], "2.0.0-dev.3.10");
        assert_eq!(report["issues"]["numErrors"], 0, "{report}");
        assert_eq!(report["issues"]["numWarnings"], 0, "{report}");
        assert_eq!(report["issues"]["truncated"], false);
        for message in report["issues"]["messages"].as_array().unwrap() {
            assert!(message["severity"].as_u64().unwrap() > 1, "{message}");
        }
        assert!(out.status.success(), "{report}");
    }
}
