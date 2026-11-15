use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
};
use unityrs_testkit::{
    child, env as e,
    ngui::{Object, st},
    scene3d as s,
    scratch::Scratch,
};

fn write_file(tmp: &Scratch, name: &str, externals: &[&str], objects: &[Object]) -> PathBuf {
    let path = tmp.join(name);
    std::fs::write(
        &path,
        s::file(s::Layout::Unity2022, externals, objects).unwrap(),
    )
    .unwrap();
    path
}
fn scene_objects(mesh: (i32, i64)) -> Vec<Object> {
    vec![
        Object::new(1, 1, s::game_object(b"prefab", &[(0, 2), (0, 3), (0, 4)])),
        Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
        Object::new(3, 33, s::mesh_filter((0, 1), mesh)),
        Object::new(4, 23, s::mesh_renderer((0, 1), &[], (0, 0))),
    ]
}
fn mesh(path_id: i64) -> Object {
    s::mesh::mesh(
        path_id,
        b"supplied",
        &[[0., 0., 0.], [2., 0., 0.], [0., 2., 0.]],
        &[0, 1, 2],
        s::mesh::MeshVersion::V2022,
    )
}
fn parse_glb(path: &Path) -> Value {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[..4], b"glTF");
    let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    serde_json::from_slice(&bytes[20..20 + len]).unwrap()
}
fn validator(path: &Path) {
    let Some(executable) = std::env::var_os("UNITYRS_GLTF_VALIDATOR") else {
        assert!(
            std::env::var_os("UNITYRS_GLTF_VALIDATOR_REQUIRED").is_none(),
            "validator required"
        );
        eprintln!("CLI unit validator skipped: set UNITYRS_GLTF_VALIDATOR");
        return;
    };
    let output = child::output(
        Command::new(executable)
            .args(["--stdout", "--all"])
            .arg(path),
    )
    .unwrap();
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["validatorVersion"], "2.0.0-dev.3.10");
    assert_eq!(report["issues"]["numErrors"], 0, "{report}");
    assert_eq!(report["issues"]["numWarnings"], 0, "{report}");
    assert_eq!(report["issues"]["truncated"], false);
    assert!(output.status.success(), "{report}");
}
fn export(tmp: &Scratch, name: &str, inputs: &[&Path], flags: &[&str]) -> (Value, Value) {
    let output = tmp.join(format!("{name}.glb"));
    let report = tmp.join(format!("{name}.json"));
    let result = child::output(
        Command::new(env!("CARGO_BIN_EXE_unityrs"))
            .arg("gltf")
            .args(inputs)
            .args(flags)
            .arg("-o")
            .arg(&output)
            .arg("--report")
            .arg(&report),
    )
    .unwrap();
    assert!(
        result.status.success(),
        "{flags:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    validator(&output);
    let report: Value = serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap();
    let stderr = String::from_utf8_lossy(&result.stderr);
    let final_peak: u64 = stderr.split_whitespace().last().unwrap().parse().unwrap();
    assert_eq!(report["peak_charged_bytes"], final_peak, "{stderr}");
    (parse_glb(&output), report)
}
fn profile(tmp: &Scratch, name: &str, builtin: &str) -> PathBuf {
    let path = tmp.join(name);
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({"profile_version":1,"builtin":builtin})).unwrap(),
    )
    .unwrap();
    path
}
fn has_reason(report: &Value, reason: &str) -> bool {
    report["collection_issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["reason"] == reason)
}

#[test]
fn default_builtin_placeholders_and_missing_resolve_degrade_with_reports() {
    let tmp = Scratch::new("gltf-cli-builtin-missing");
    let input = write_file(
        &tmp,
        "level0",
        &["Library/unity default resources"],
        &scene_objects((1, 10210)),
    );
    let (scene, report) = export(&tmp, "placeholder", &[&input], &["--scene", "level0"]);
    assert!(scene.get("meshes").is_none());
    assert_eq!(
        scene["nodes"][0]["extras"]["unity_builtin"],
        json!([
            {"file": "unity default resources", "id": 10210, "name": "Quad"}
        ])
    );
    assert!(has_reason(&report, "builtin_placeholder"));
    let p = profile(&tmp, "resolve-profile.json", "resolve");
    let (scene, report) = export(
        &tmp,
        "missing-resolve",
        &[&input],
        &["--scene", "level0", "--profile", p.to_str().unwrap()],
    );
    assert!(scene.get("meshes").is_none());
    assert_eq!(
        scene["nodes"][0]["extras"]["unresolved_renderer"],
        "builtin resource unresolved"
    );
    assert!(has_reason(&report, "builtin resource unresolved"));
}

#[test]
fn procedural_cube_quad_plane_and_supplied_builtin_resource_export_through_cli() {
    let tmp = Scratch::new("gltf-cli-builtin-modes");
    let procedural = profile(&tmp, "procedural-profile.json", "procedural");
    for (name, id, vertices, indices) in [
        ("cube", 10202, 8, 36),
        ("quad", 10210, 4, 6),
        ("plane", 10209, 4, 6),
    ] {
        let input = write_file(
            &tmp,
            name,
            &["Library/unity default resources"],
            &scene_objects((1, id)),
        );
        let (scene, _) = export(
            &tmp,
            name,
            &[&input],
            &[
                "--scene",
                name,
                "--profile",
                procedural.to_str().unwrap(),
                "--mapper",
                "pbr",
            ],
        );
        assert_eq!(scene["meshes"][0]["extras"]["approximate"], "true");
        assert_eq!(scene["nodes"][0]["extras"]["approximate"], "true");
        assert_eq!(scene["accessors"][0]["count"], vertices);
        assert_eq!(scene["accessors"][1]["count"], indices);
    }
    let input = write_file(
        &tmp,
        "level0",
        &["Library/unity default resources"],
        &scene_objects((1, 10210)),
    );
    let resource = write_file(&tmp, "supplied.resources", &[], &[mesh(10210)]);
    let resolve = profile(&tmp, "resolve-profile.json", "resolve");
    let (scene, report) = export(
        &tmp,
        "resolved",
        &[&input],
        &[
            "--scene",
            "level0",
            "--profile",
            resolve.to_str().unwrap(),
            "--builtin-file",
            resource.to_str().unwrap(),
            "--mapper",
            "unlit",
        ],
    );
    assert_eq!(scene["meshes"].as_array().unwrap().len(), 1);
    assert_eq!(scene["accessors"][0]["count"], 3);
    assert!(scene["meshes"][0].get("extras").is_none());
    assert!(!has_reason(&report, "builtin resource unresolved"));
    // Merely supplying a resource never changes the default placeholder policy.
    let (scene, report) = export(
        &tmp,
        "supplied-placeholder",
        &[&input],
        &[
            "--scene",
            "level0",
            "--builtin-file",
            resource.to_str().unwrap(),
        ],
    );
    assert!(scene.get("meshes").is_none());
    assert!(has_reason(&report, "builtin_placeholder"));
}

#[test]
fn sceneasset_selects_whole_scene_while_prefab_selects_its_subtree_and_dependencies_stay_separate()
{
    let tmp = Scratch::new("gltf-cli-unit-selection");
    let objects = [
        Object::new(90, 4, s::transform((0, 9), (0, 0), &[(0, 20)], false)),
        Object::new(9, 1, s::game_object(b"first", &[(0, 90)])),
        Object::new(-5, 4, s::transform((0, 8), (0, 0), &[], false)),
        Object::new(8, 1, s::game_object(b"second", &[(0, -5)])),
        Object::new(20, 4, s::transform((0, 2), (0, 90), &[], false)),
        Object::new(2, 1, s::game_object(b"child", &[(0, 20)])),
        Object::new(50, 1032, st("SceneAsset", vec![])),
    ];
    let input = write_file(&tmp, "BuildPlayer-demo", &[], &objects);
    let shared = write_file(
        &tmp,
        "BuildPlayer-demo.sharedAssets",
        &[],
        &[
            Object::new(100, 1, s::game_object(b"dependency-root", &[(0, 101)])),
            Object::new(101, 4, s::transform((0, 100), (0, 0), &[], false)),
        ],
    );
    let (scene, _) = export(&tmp, "sceneasset", &[&input, &shared], &["--path-id", "50"]);
    assert_eq!(scene["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(scene["scenes"][0]["nodes"], json!([0, 2]));
    assert_eq!(scene["nodes"][0]["name"], "first");
    assert_eq!(scene["nodes"][2]["name"], "second");
    let (by_name, _) = export(
        &tmp,
        "scene-file",
        &[&input, &shared],
        &["--scene", "BuildPlayer-demo"],
    );
    assert_eq!(scene, by_name);
    let (prefab, _) = export(&tmp, "prefab-id", &[&input, &shared], &["--path-id", "2"]);
    assert_eq!(prefab["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(prefab["nodes"][0]["name"], "child");
    let filter = tmp.join("filter.json");
    std::fs::write(
        &filter,
        br#"{"profile_version":1,"scene_roots":["first"],"exclusions":["first/child"]}"#,
    )
    .unwrap();
    let (filtered, _) = export(
        &tmp,
        "filtered",
        &[&input, &shared],
        &[
            "--scene",
            "BuildPlayer-demo",
            "--profile",
            filter.to_str().unwrap(),
        ],
    );
    assert_eq!(filtered["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["nodes"][0]["name"], "first");
}

#[test]
fn prefab_container_entry_in_bundle_resolves_external_geometry_input() {
    let tmp = Scratch::new("gltf-cli-container-prefab");
    let index = e::serialized(
        &["CAB-prefab"],
        &[e::Obj::asset_bundle(
            77,
            e::Bundle {
                rows: vec![e::row("assets/example.prefab", (0, 0), (1, 1))],
                ..e::Bundle::default()
            },
        )],
    );
    let prefab = s::file(s::Layout::Unity2022, &["CAB-mesh"], &scene_objects((1, 7))).unwrap();
    let bundle = tmp.join("example.bundle");
    std::fs::write(
        &bundle,
        e::bundle(&[("CAB-index", index), ("CAB-prefab", prefab)]),
    )
    .unwrap();
    let dependency = write_file(&tmp, "CAB-mesh", &[], &[mesh(7)]);
    let (scene, report) = export(
        &tmp,
        "container",
        &[&bundle, &dependency],
        &["--prefab", "assets/example.prefab"],
    );
    assert_eq!(scene["nodes"][0]["name"], "prefab");
    assert_eq!(scene["meshes"].as_array().unwrap().len(), 1);
    assert_eq!(scene["accessors"][0]["count"], 3);
    assert!(has_reason(&report, "submesh_without_material"));
}

#[test]
fn empty_scene_inside_bundle_is_an_exportable_unit() {
    let tmp = Scratch::new("gltf-cli-empty-bundled-scene");
    let bundle = tmp.join("empty-scene.bundle");
    std::fs::write(
        &bundle,
        e::bundle(&[
            (
                "BuildPlayer-empty",
                unityrs_testkit::serialized::build(&unityrs_testkit::serialized::SerializedSpec {
                    user_information: vec![b'x'; 128],
                    ..unityrs_testkit::serialized::SerializedSpec::default()
                }),
            ),
            (
                "BuildPlayer-empty.sharedAssets",
                unityrs_testkit::serialized::build(&unityrs_testkit::serialized::SerializedSpec {
                    user_information: vec![b'x'; 128],
                    ..unityrs_testkit::serialized::SerializedSpec::default()
                }),
            ),
        ]),
    )
    .unwrap();
    let (scene, report) = export(
        &tmp,
        "empty-scene",
        &[&bundle],
        &["--scene", "BuildPlayer-empty"],
    );
    assert!(scene.get("scenes").is_none());
    assert!(scene.get("nodes").is_none());
    assert_eq!(report["collection_issues"], json!([]));
}

#[test]
fn repeated_builtin_references_share_the_supplied_resource_geometry() {
    let tmp = Scratch::new("gltf-cli-builtin-sharing");
    let mut objects = scene_objects((1, 10210));
    objects[0] = Object::new(
        1,
        1,
        s::game_object(b"prefab", &[(0, 2), (0, 3), (0, 4), (0, 5)]),
    );
    objects.push(Object::new(5, 23, s::mesh_renderer((0, 1), &[], (0, 0))));
    let input = write_file(
        &tmp,
        "level0",
        &["Library/unity default resources"],
        &objects,
    );
    let resource = write_file(&tmp, "arbitrary-resource-name", &[], &[mesh(10210)]);
    let resolve = profile(&tmp, "resolve-profile.json", "resolve");
    let (scene, _) = export(
        &tmp,
        "repeated-resolved",
        &[&input],
        &[
            "--scene",
            "level0",
            "--profile",
            resolve.to_str().unwrap(),
            "--builtin-file",
            resource.to_str().unwrap(),
        ],
    );
    assert_eq!(scene["meshes"].as_array().unwrap().len(), 1);
    assert_eq!(scene["accessors"].as_array().unwrap().len(), 2);
    assert_eq!(
        scene["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|node| node.get("mesh").is_some())
            .count(),
        2
    );

    for id in [4, 5] {
        objects[usize::try_from(id - 1).unwrap()] =
            Object::new(id, 23, s::mesh_renderer((0, 1), &[], (0, 1)));
    }
    let input = write_file(
        &tmp,
        "level-combined",
        &["Library/unity default resources"],
        &objects,
    );
    let combined = tmp.join("combined-profile.json");
    std::fs::write(
        &combined,
        br#"{"profile_version":1,"builtin":"resolve","static_batch":"keep_combined"}"#,
    )
    .unwrap();
    let (scene, _) = export(
        &tmp,
        "combined-resolved",
        &[&input],
        &[
            "--scene",
            "level-combined",
            "--profile",
            combined.to_str().unwrap(),
            "--builtin-file",
            resource.to_str().unwrap(),
        ],
    );
    assert_eq!(scene["meshes"].as_array().unwrap().len(), 1);
    assert_eq!(
        scene["meshes"][0]["primitives"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        scene["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|node| node.get("mesh").is_some())
            .count(),
        1
    );
}
