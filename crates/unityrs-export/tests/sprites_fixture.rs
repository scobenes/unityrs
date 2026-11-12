//! Gate rows X2 and X6 (`docs/plans/extract.md`) for the `sprites` kind:
//! the engine on every `sprites` scene (`unityrs_testkit::ngui::sprite_scenes`)
//! gives what the reference recorded in `unityrs-testkit`'s
//! `tests/fixtures/sprites.json` (`tools/fixtures/sprites.py` over
//! `tools/parity/sprites_kind.py`, UnityPy 1.25.2, Pillow 9.5.0, zlib
//! 1.2.13): each input's outputs in export order (key, object, mode, size,
//! pixels and their SHA-256, the PNG's bytes by MD5 and SHA-256, and the
//! visible hash, E20, computed here from the PNG's own pixels), its skips in
//! record order (class, output, reason, detail); and 4.5's winners over the
//! winners scene, a Sprite's output a plain candidate, decided every way the
//! fixture records.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{Value as J, json};
use sha2::{Digest, Sha256};
use unityrs_export::ngui::{self, Candidate, OutputKind, Prefer};
use unityrs_testkit::ngui::{AtlasScene, sprite_scenes};

mod common;
use common::pixels::{hex, png_pixels, unhex, visible_hash};
use common::{Ran, run};

fn fixture() -> J {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../unityrs-testkit/tests/fixtures/sprites.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

fn text(s: &unityrs_core::RawStr) -> String {
    String::from_utf8_lossy(s.as_bytes()).into_owned()
}

/// A cut as the fixture records a result.
fn record(cut: &Ran) -> J {
    let outputs: Vec<J> = cut
        .outputs
        .iter()
        .map(|o| {
            assert_eq!(o.kind, OutputKind::Sprite, "{}: kind", o.key);
            assert_eq!(o.unit, None, "{}: a plain candidate", o.key);
            assert_eq!(o.sprite, None, "{}: no atlas sprite", o.key);
            let p = o.picture.as_ref().unwrap();
            let channels = if p.mode == "RGB" { 3 } else { 4 };
            let pixels = png_pixels(&o.bytes, p.width as usize, p.height as usize, channels);
            assert_eq!(
                p.visible_hash,
                visible_hash(p.width, p.height, channels, &pixels),
                "{}: visible_hash",
                o.key
            );
            let mut row = json!({
                "key": o.key,
                "mode": p.mode,
                "path_id": o.path_id,
                "pixels_sha256": hex(&Sha256::digest(&pixels)),
                "png_md5": hex(&o.md5),
                "png_sha256": hex(&Sha256::digest(&o.bytes)),
                "serialized_file": text(&o.serialized_file),
                "size": [p.width, p.height],
            });
            if p.width * p.height <= 64 {
                row["pixels"] = json!(hex(&pixels));
            }
            row
        })
        .collect();
    let skips: Vec<J> = cut
        .skips
        .iter()
        .map(|s| {
            assert_eq!(s.sprite, None);
            json!({
                "cls": s.class,
                "detail": s.reason.detail,
                "output": s.output.name(),
                "path_id": s.path_id,
                "reason": s.reason.reason,
                "serialized_file": text(&s.serialized_file),
            })
        })
        .collect();
    json!({"outputs": outputs, "skips": skips})
}

/// The fixture's scene rows are the built scenes': names, bundles (ids,
/// bytes by SHA-256, deps), inputs and keys.
fn scene_rows_match(doc: &J, all: &[(&str, AtlasScene)]) {
    let rows = doc["scenes"].as_array().unwrap();
    let names: Vec<&str> = rows.iter().map(|r| r["name"].as_str().unwrap()).collect();
    let built: Vec<&str> = all.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, built, "the scenes, in order");
    for (row, (name, scene)) in rows.iter().zip(all) {
        let bundles: Vec<J> = scene
            .bundles
            .iter()
            .map(|b| json!({"deps": b.deps, "id": b.id, "sha256": hex(&Sha256::digest(&b.data))}))
            .collect();
        assert_eq!(row["bundles"], J::Array(bundles), "{name}: bundles");
        assert_eq!(row["inputs"], json!(scene.inputs), "{name}: inputs");
        assert_eq!(row["key"], json!(scene.options.sprites_key), "{name}: key");
    }
}

#[test]
fn every_sprite_scene_exports_as_the_reference_recorded() {
    let doc = fixture();
    let all = sprite_scenes().unwrap();
    scene_rows_match(&doc, &all);
    let results = doc["results"].as_array().unwrap();
    let built: BTreeSet<(String, String)> = all
        .iter()
        .flat_map(|(n, s)| s.inputs.iter().map(|i| ((*n).to_owned(), i.clone())))
        .collect();
    let recorded: BTreeSet<(String, String)> = results
        .iter()
        .map(|r| {
            let s = r["scene"].as_str().unwrap().to_owned();
            (s, r["input"].as_str().unwrap().to_owned())
        })
        .collect();
    assert_eq!(recorded.len(), results.len(), "a result recorded twice");
    assert_eq!(
        recorded, built,
        "the recorded results are the built inputs'"
    );
    let mut rows = [0usize; 2];
    let mut failures = Vec::new();
    for want in results {
        let name = want["scene"].as_str().unwrap();
        let input = want["input"].as_str().unwrap();
        let (_, scene) = all.iter().find(|(n, _)| *n == name).unwrap();
        let got = record(&run(scene, input));
        for (i, field) in ["outputs", "skips"].into_iter().enumerate() {
            if got[field] != want[field] {
                failures.push(format!(
                    "{name}/{input} {field}:\n got  {}\n want {}",
                    serde_json::to_string_pretty(&got[field]).unwrap(),
                    serde_json::to_string_pretty(&want[field]).unwrap()
                ));
            }
            rows[i] += got[field].as_array().unwrap().len();
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    assert_eq!(recorded.len(), 20, "every recorded result");
    assert_eq!(rows, [55, 62], "the outputs and skips compared");
    assert_eq!(doc["zlib"], "1.2.13");
    assert_eq!(unityrs_imageio::zlib_version(), "1.2.13");
}

#[test]
fn sprite_winners_are_decided_as_the_reference_decided_them() {
    let doc = fixture();
    let all = sprite_scenes().unwrap();
    let (_, scene) = all.iter().find(|(n, _)| *n == "sprite_winners").unwrap();
    let mut by_input = BTreeMap::new();
    for input in &scene.inputs {
        let cands: Vec<Candidate> = run(scene, input)
            .outputs
            .iter()
            .map(|o| Candidate {
                input: input.clone(),
                key: o.key.clone(),
                unit: o.unit.clone(),
                md5: o.md5,
            })
            .collect();
        by_input.insert(input.clone(), cands);
    }
    let cases = doc["winners"].as_array().unwrap();
    let mut decisions = 0;
    for case in cases {
        let name = case["case"].as_str().unwrap();
        let cands: Vec<Candidate> = case["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|i| by_input[i.as_str().unwrap()].iter().cloned())
            .collect();
        let prefer: BTreeMap<String, Prefer> = case["prefer"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| {
                let p = Prefer {
                    input: v["input"].as_str().map(str::to_owned),
                    md5: v["md5"].as_str().map(unhex),
                };
                (k.clone(), p)
            })
            .collect();
        let got: Vec<J> = ngui::decide(&cands, &prefer, &BTreeSet::new(), true)
            .iter()
            .map(|d| {
                let cs: Vec<J> = d
                    .candidates
                    .iter()
                    .map(|c| json!([c.input, c.unit, c.rule.name(), c.winner]))
                    .collect();
                json!({
                    "candidates": cs,
                    "flip": d.flip,
                    "key": d.key,
                    "reason": d.reason,
                    "status": if d.publish { "publish" } else { "withheld" },
                    "winner": d.winner,
                })
            })
            .collect();
        decisions += got.len();
        assert_eq!(J::from(got), case["decisions"], "{name}");
    }
    assert_eq!(cases.len(), 3);
    assert_eq!(decisions, 12, "the decisions compared");
}
