//! The `sprites` scenes (`unityrs_testkit::ngui::sprite_scenes`) are the
//! ones the `sprites` kind's reference was recorded on:
//! `tests/fixtures/sprites.json`, written by `tools/fixtures/sprites.py`
//! from `tools/parity/sprites_kind.py` on UnityPy 1.25.2 and Pillow 9.5.0,
//! names each scene's bundles by sha256, with their `deps`, the inputs and
//! the key, beside the reference's outputs. A builder change that moves a
//! byte fails here until the fixture is recorded again. With
//! `UNITYRS_WRITE_SPRITE_SCENES=<path>` the test writes the scenes there
//! instead, one JSON line each, which `tools/parity/sprites_scenes.py`
//! reads.

use std::fmt::Write as _;
use std::path::Path;

use serde_json::{Value as J, json};
use sha2::{Digest, Sha256};
use unityrs_testkit::ngui::sprite_scenes;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        write!(s, "{b:02x}").unwrap();
        s
    })
}

#[test]
fn sprite_scenes_are_the_recorded_scenes() {
    let all = sprite_scenes().unwrap();
    if let Ok(out) = std::env::var("UNITYRS_WRITE_SPRITE_SCENES") {
        let mut lines = String::new();
        for (name, scene) in &all {
            let bundles: Vec<J> = scene
                .bundles
                .iter()
                .map(|b| json!({"id": b.id, "hex": hex(&b.data), "deps": b.deps}))
                .collect();
            let row = json!({"name": name, "bundles": bundles, "inputs": scene.inputs,
                "key": scene.options.sprites_key});
            lines.push_str(&row.to_string());
            lines.push('\n');
        }
        std::fs::write(out, lines).unwrap();
        return;
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sprites.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let doc: J = serde_json::from_str(&text).unwrap();
    let recorded = doc["scenes"].as_array().unwrap();
    let names: Vec<&str> = recorded
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    let built: Vec<&str> = all.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, built, "the scenes, by name and order");
    for (r, (name, scene)) in recorded.iter().zip(&all) {
        let bundles: Vec<J> = scene
            .bundles
            .iter()
            .map(|b| json!({"id": b.id, "sha256": hex(&Sha256::digest(&b.data)), "deps": b.deps}))
            .collect();
        assert_eq!(r["bundles"], json!(bundles), "{name}: the builder changed");
        assert_eq!(r["inputs"], json!(scene.inputs), "{name}: the inputs");
        assert_eq!(
            r["key"],
            json!(scene.options.sprites_key),
            "{name}: the key"
        );
    }
}
