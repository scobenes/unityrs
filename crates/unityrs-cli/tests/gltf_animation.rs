//! Rig-scoped selectors through the standalone glTF command.
use serde_json::{Value, json};
use std::process::Command;
use unityrs_testkit::{
    child,
    ngui::Object,
    scene3d::{
        self as s,
        anim::{
            self,
            playback::{self as p, Key},
        },
    },
    scratch::Scratch,
};

fn address(path_id: i64) -> Value {
    json!({"file_index":0,"path_id":path_id})
}

fn fixture() -> Vec<u8> {
    s::file(
        s::Layout::Unity2022,
        &["CAB-missing"],
        &[
            Object::new(1, 1, s::game_object(b"first", &[(0, 2), (0, 3)])),
            Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            anim::legacy(3, (0, 9), &[(1, 9)]),
            Object::new(4, 1, s::game_object(b"second", &[(0, 5), (0, 6)])),
            Object::new(5, 4, s::transform((0, 4), (0, 0), &[], false)),
            anim::legacy(6, (0, 9), &[]),
            p::explicit(
                9,
                1.,
                &[(
                    "m_PositionCurves",
                    vec![p::curve(
                        "",
                        &[Key::new(2., [3., 4., 5.])],
                        [2, 2],
                        None,
                        true,
                        None,
                    )],
                )],
            ),
        ],
    )
    .unwrap()
}

fn run(scratch: &Scratch, profile: &Value, unit: [&str; 2]) -> std::process::Output {
    let path = scratch.join("profile.json");
    std::fs::write(&path, serde_json::to_vec(profile).unwrap()).unwrap();
    child::output(
        Command::new(env!("CARGO_BIN_EXE_unityrs"))
            .arg("gltf")
            .arg(scratch.join("level0"))
            .args(unit)
            .arg("--profile")
            .arg(path)
            .arg("-o")
            .arg(scratch.join("result.glb")),
    )
    .unwrap()
}

fn doc(scratch: &Scratch) -> Value {
    let bytes = std::fs::read(scratch.join("result.glb")).unwrap();
    let length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    serde_json::from_slice(&bytes[20..20 + length]).unwrap()
}

#[test]
fn scene_selects_same_named_clips_by_rig_and_deduplicates_aliases() {
    let scratch = Scratch::new("gltf-animation-scene");
    std::fs::write(scratch.join("level0"), fixture()).unwrap();
    let second = json!({"root":address(5),"component":address(6),
        "source":{"owner":address(6),"field":"m_Animation","ordinal":0}});
    let result = run(
        &scratch,
        &json!({"profile_version":1,"clips":[second,second,
        {"root":address(2),"component":address(3),"clip":address(9)}]}),
        ["--scene", "level0"],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let doc = doc(&scratch);
    let animations = doc["animations"].as_array().unwrap();
    assert_eq!(animations.len(), 2);
    assert_eq!(animations[0]["name"], animations[1]["name"]);
    for (animation, node) in animations.iter().zip([1, 0]) {
        assert_eq!(
            animation["channels"][0]["target"],
            json!({"node":node,"path":"translation"})
        );
        assert_eq!(animation["samplers"][0]["interpolation"], "STEP");
        let input = usize::try_from(animation["samplers"][0]["input"].as_u64().unwrap()).unwrap();
        assert_eq!(doc["accessors"][input]["count"], 1);
        assert_eq!(doc["accessors"][input]["min"], json!([0]));
    }
}

#[test]
fn prefab_diagnoses_discovery_and_refuses_an_explicit_unresolved_slot() {
    let scratch = Scratch::new("gltf-animation-prefab");
    std::fs::write(scratch.join("level0"), fixture()).unwrap();
    let result = run(
        &scratch,
        &json!({"profile_version":1,"clips":[]}),
        ["--path-id", "1"],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(doc(&scratch).get("animations").is_none());
    let result = run(
        &scratch,
        &json!({"profile_version":1,"clips":"all_discovered"}),
        ["--path-id", "1"],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(doc(&scratch)["animations"].as_array().unwrap().len(), 1);
    assert!(String::from_utf8_lossy(&result.stderr).contains("outside_view"));
    let before = std::fs::read(scratch.join("result.glb")).unwrap();
    let result = run(
        &scratch,
        &json!({"profile_version":1,"clips":[{
            "root":address(2),"component":address(3),
            "source":{"owner":address(3),"field":"m_Animations","ordinal":0}
        }]}),
        ["--path-id", "1"],
    );
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("outside_view"));
    assert_eq!(std::fs::read(scratch.join("result.glb")).unwrap(), before);
}
