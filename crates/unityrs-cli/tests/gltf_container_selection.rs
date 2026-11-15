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
