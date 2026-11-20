use std::process::Command;
use unityrs_testkit::{
    child, env as e,
    ngui::{Object, st},
    scene3d as s,
    scratch::Scratch,
};

fn container(targets: &[i64]) -> Vec<u8> {
    let index = e::serialized(
        &["CAB-roots"],
        &[e::Obj::asset_bundle(
            100,
            e::Bundle {
                rows: targets
                    .iter()
                    .map(|&id| e::row("assets/example.asset", (0, 0), (1, id)))
                    .collect(),
                ..e::Bundle::default()
            },
        )],
    );
    let roots = s::file(
        s::Layout::Unity2022,
        &[],
        &[
            Object::new(1, 1, s::game_object(b"first", &[(0, 2)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            Object::new(3, 1, s::game_object(b"second", &[(0, 4)])),
            Object::new(4, 4, s::transform((0, 3), (0, 0), &[], false)),
            Object::new(5, 1032, st("SceneAsset", vec![])),
            Object::new(6, 1032, st("SceneAsset", vec![])),
            Object::new(7, 43, st("Mesh", vec![])),
            Object::new(8, 21, st("Material", vec![])),
        ],
    )
    .unwrap();
    e::bundle(&[("CAB-index", index), ("CAB-roots", roots)])
}

#[test]
fn prefab_container_path_counts_only_gameobject_rows() {
    for (targets, expected) in [
        (&[7, 1, 8, 5][..], 1),
        (&[7, 8, 5][..], 0),
        (&[7, 1, 3, 8][..], 2),
        (&[1, 1, 1][..], 3),
    ] {
        check("--prefab", "GameObject", targets, expected);
    }
}

#[test]
fn scene_container_path_counts_only_sceneasset_rows() {
    for (targets, expected) in [
        (&[7, 1, 8, 5][..], 1),
        (&[7, 8, 1][..], 0),
        (&[5, 1, 6, 8][..], 2),
        (&[5, 5, 5][..], 3),
    ] {
        check("--scene", "SceneAsset", targets, expected);
    }
}

fn check(flag: &str, class: &str, targets: &[i64], expected: usize) {
    let tmp = Scratch::new("gltf-cli-typed-container");
    let input = tmp.join("bundle");
    let output = tmp.join("output.glb");
    std::fs::write(&input, container(targets)).unwrap();
    let result = child::output(
        Command::new(env!("CARGO_BIN_EXE_unityrs"))
            .arg("gltf")
            .arg(&input)
            .args([flag, "assets/example.asset", "-o"])
            .arg(&output),
    )
    .unwrap();
    let stderr = String::from_utf8_lossy(&result.stderr);
    if expected == 1 {
        assert!(result.status.success(), "{flag} {targets:?}: {stderr}");
        let bytes = std::fs::read(&output).unwrap();
        let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let scene: serde_json::Value = serde_json::from_slice(&bytes[20..20 + len]).unwrap();
        assert_eq!(scene["nodes"][0]["name"], "first");
        assert_eq!(
            scene["nodes"].as_array().unwrap().len(),
            if flag == "--scene" { 2 } else { 1 }
        );
    } else {
        assert_eq!(
            result.status.code(),
            Some(1),
            "{flag} {targets:?}: {stderr}"
        );
        assert!(
            stderr.contains(&format!("found {expected} {class}")),
            "{stderr}"
        );
        assert!(!output.exists());
    }
}

// A bundle's preload table names the built-in meshes its prefabs use, as
// every Editor-built bundle with a primitive does. The container walk that
// selects `--prefab` must not load the supplied resource file: a file loaded
// under the walk ends it, as UnityPy's pass ends when its CABs grow.
#[test]
fn prefab_selection_with_a_builtin_file_resolves_after_the_container_walk() {
    let tmp = Scratch::new("gltf-cli-builtin-preload");
    let index = e::serialized(
        &["CAB-roots", "Library/unity default resources"],
        &[e::Obj::asset_bundle(
            100,
            e::Bundle {
                preload: vec![(2, 10210)],
                rows: vec![e::row("assets/quad.prefab", (0, 1), (1, 1))],
                ..e::Bundle::default()
            },
        )],
    );
    let roots = s::file(
        s::Layout::Unity2022,
        &["Library/unity default resources"],
        &[
            Object::new(1, 1, s::game_object(b"quad", &[(0, 2), (0, 3), (0, 4)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            Object::new(3, 33, s::mesh_filter((0, 1), (1, 10210))),
            Object::new(4, 23, s::mesh_renderer((0, 1), &[], (0, 0))),
        ],
    )
    .unwrap();
    let input = tmp.join("bundle");
    std::fs::write(
        &input,
        e::bundle(&[("CAB-index", index), ("CAB-roots", roots)]),
    )
    .unwrap();
    let resource = tmp.join("supplied.resources");
    let quad = s::mesh::mesh(
        10210,
        b"supplied",
        &[[0., 0., 0.], [2., 0., 0.], [0., 2., 0.]],
        &[0, 1, 2],
        s::mesh::MeshVersion::V2022,
    );
    std::fs::write(
        &resource,
        s::file(s::Layout::Unity2022, &[], &[quad]).unwrap(),
    )
    .unwrap();
    let profile = tmp.join("resolve.json");
    std::fs::write(&profile, br#"{"profile_version":1,"builtin":"resolve"}"#).unwrap();
    for resolve in [true, false] {
        let output = tmp.join("output.glb");
        let mut command = Command::new(env!("CARGO_BIN_EXE_unityrs"));
        command
            .arg("gltf")
            .arg(&input)
            .args(["--prefab", "assets/quad.prefab", "--builtin-file"])
            .arg(&resource)
            .arg("-o")
            .arg(&output);
        if resolve {
            command.arg("--profile").arg(&profile);
        }
        let result = child::output(&mut command).unwrap();
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(result.status.success(), "resolve {resolve}: {stderr}");
        let bytes = std::fs::read(&output).unwrap();
        let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let scene: serde_json::Value = serde_json::from_slice(&bytes[20..20 + len]).unwrap();
        assert_eq!(scene["nodes"][0]["name"], "quad");
        if resolve {
            assert_eq!(scene["accessors"][0]["count"], 3, "{scene}");
        } else {
            assert!(scene.get("meshes").is_none(), "{scene}");
            assert_eq!(scene["nodes"][0]["extras"]["builtin_placeholder"], "1");
        }
    }
}

// A matching row whose asset is in the supplied resource file is classified
// once the walk is over, with the file offered: a GameObject there is the
// one prefab, and no SceneAsset is counted for `--scene`.
#[test]
fn a_selected_row_into_the_builtin_file_is_classified_after_the_walk() {
    let tmp = Scratch::new("gltf-cli-builtin-row");
    let index = e::serialized(
        &["CAB-roots", "Library/unity default resources"],
        &[e::Obj::asset_bundle(
            100,
            e::Bundle {
                preload: vec![(2, 5)],
                rows: vec![e::row("assets/supplied.prefab", (0, 1), (2, 5))],
                ..e::Bundle::default()
            },
        )],
    );
    let roots = s::file(
        s::Layout::Unity2022,
        &[],
        &[
            Object::new(1, 1, s::game_object(b"local", &[(0, 2)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
        ],
    )
    .unwrap();
    let input = tmp.join("bundle");
    std::fs::write(
        &input,
        e::bundle(&[("CAB-index", index), ("CAB-roots", roots)]),
    )
    .unwrap();
    let resource = tmp.join("supplied.resources");
    let supplied = [
        Object::new(5, 1, s::game_object(b"supplied", &[(0, 6)])),
        Object::new(6, 4, s::transform((0, 5), (0, 0), &[], false)),
    ];
    std::fs::write(
        &resource,
        s::file(s::Layout::Unity2022, &[], &supplied).unwrap(),
    )
    .unwrap();
    for selector in ["--prefab", "--scene"] {
        let output = tmp.join("output.glb");
        let result = child::output(
            Command::new(env!("CARGO_BIN_EXE_unityrs"))
                .arg("gltf")
                .arg(&input)
                .args([selector, "assets/supplied.prefab", "--builtin-file"])
                .arg(&resource)
                .arg("-o")
                .arg(&output),
        )
        .unwrap();
        let stderr = String::from_utf8_lossy(&result.stderr);
        if selector == "--prefab" {
            assert!(result.status.success(), "{stderr}");
            let bytes = std::fs::read(&output).unwrap();
            let len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
            let scene: serde_json::Value = serde_json::from_slice(&bytes[20..20 + len]).unwrap();
            assert_eq!(scene["nodes"][0]["name"], "supplied", "{scene}");
        } else {
            assert!(!result.status.success());
            assert!(
                stderr.contains("requires exactly one SceneAsset; found 0 SceneAsset entries"),
                "{stderr}"
            );
        }
    }
}
