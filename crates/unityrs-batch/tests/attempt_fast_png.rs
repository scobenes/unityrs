//! W1b: every PNG publication site uses the opt-in encoder, while the
//! decoded pixels replay the existing UnityPy/Pillow oracle recordings.
mod attempt_support;
use attempt_support::*;
use serde_json::json;
use unityrs_batch::attempt::State;
use unityrs_export::ngui::NoHooks;
use unityrs_testkit::{ngui, scratch::Scratch};

#[test]
fn fast_png_replays_texture_atlas_and_sprite_pixels_from_disk() {
    let scratch = Scratch::new("fast-png");
    let mut jobs = Vec::new();
    let mut kinds = std::collections::BTreeSet::new();
    let mut inputs = 0;
    for (name, scenes) in [
        ("ngui", ngui::scenes().unwrap()),
        ("sprites", ngui::sprite_scenes().unwrap()),
    ] {
        let recorded = fixture(&format!("../unityrs-testkit/tests/fixtures/{name}.json"));
        for want in recorded["results"].as_array().unwrap() {
            let scene = &scenes
                .iter()
                .find(|(n, _)| Some(*n) == want["scene"].as_str())
                .unwrap()
                .1;
            let mut harness = Harness::new(scene, want["input"].as_str().unwrap());
            harness.options.png_fast = true;
            let end = harness.run(limits(), &NoHooks);
            clear(&end);
            assert_eq!(end.state, State::Done, "{want}");
            inputs += 1;
            let rows = harness.rows();
            let got: Vec<_> = rows.iter().filter(|r| r["type"] == "candidate").collect();
            let expected = want["outputs"].as_array().unwrap();
            assert_eq!(got.len(), expected.len(), "{want}");
            for (got, want) in got.into_iter().zip(expected) {
                assert_eq!(got["key"], want["key"]);
                kinds.insert(got["kind"].as_str().unwrap().to_owned());
                let mut pixels = want.clone();
                if got["kind"] != "atlas_json" {
                    // Only pixel facts apply to fast PNG, never Pillow's file hashes.
                    for field in ["md5", "sha256", "png_md5", "png_sha256"] {
                        pixels.as_object_mut().unwrap().remove(field);
                    }
                    assert!(pixels["pixels_sha256"].is_string());
                    pixels["filter"] = json!(1);
                    pixels["visible_hash"] = got["visible_hash"].clone();
                }
                let path = scratch.join(format!("out{}", jobs.len()));
                std::fs::write(&path, harness.output(got)).unwrap();
                jobs.push(json!({"file": path, "want": pixels}));
            }
        }
    }
    assert_eq!(inputs, 61);
    assert_eq!(
        kinds.into_iter().collect::<Vec<_>>(),
        ["atlas_json", "atlas_sprite", "sprite", "texture"]
    );
    check_files(&scratch, &jobs);
}
