//! Gate row X5 (`docs/plans/extract.md`): the cutter on every NGUI scene
//! (`unityrs_testkit::ngui::scenes`) gives what the reference recorded in
//! `unityrs-testkit`'s `tests/fixtures/ngui.json` (`tools/fixtures/ngui.py`
//! over `tools/parity/ngui_cutter.py`, UnityPy 1.25.2, Pillow 9.5.0, zlib
//! 1.2.13): each input's outputs in export order (key, kind, unit, object,
//! sprite index, mode, size, pixels and their SHA-256, the PNG's or JSON's
//! bytes by MD5 and SHA-256, and the JSON's text), its skips in record order
//! (class, output, sprite, reason, detail), its `unresolved_scripts` and its
//! sheets; and 4.5's winners over the winners scenes, decided every way the
//! fixture records.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use serde_json::{Value as J, json};
use sha2::{Digest, Sha256};
use unityrs_core::detect::{FileType, check_file_type};
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::{Bytes, File, FileContext, RawStr};
use unityrs_export::ngui::{self, Candidate, OutputKind, Prefer, SheetBy};
use unityrs_testkit::ngui::{AtlasScene, scenes};

mod common;
use common::pixels::{hex, png_pixels, unhex, visible_hash};
use common::{Ran, run};

fn fixture() -> J {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../unityrs-testkit/tests/fixtures/ngui.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

fn text(s: &unityrs_core::RawStr) -> String {
    String::from_utf8_lossy(s.as_bytes()).into_owned()
}

/// The top-level rows of `input` that are bundles, in directory order,
/// shadowed rows too, found apart from the cutter: each row's bytes peeked
/// with the core's `check_file_type`.
fn nested_rows(scene: &AtlasScene, input: &str) -> Vec<String> {
    let b = scene.bundles.iter().find(|b| b.id == input).unwrap();
    let env = Environment::new(EnvOptions::batch(Arc::new(FileContext::default())));
    let Ok(File::Bundle(bundle)) = env.load_dependency(Source::Bytes {
        name: RawStr::from(input),
        data: Bytes::from(b.data.clone()),
    }) else {
        return Vec::new();
    };
    let limits = unityrs_core::limits::Limits::default();
    (0..bundle.directory().len())
        .filter(|&i| {
            let bytes = bundle.entry_bytes(i).unwrap();
            matches!(check_file_type(&bytes, &limits), Ok(FileType::BundleFile))
        })
        .map(|i| text(&bundle.directory()[i].path))
        .collect()
}

/// A cut as the fixture records a result.
fn record(cut: &Ran) -> J {
    let outputs: Vec<J> = cut
        .outputs
        .iter()
        .map(|o| {
            let mut row = json!({
                "key": o.key,
                "kind": o.kind.name(),
                "path_id": o.path_id,
                "serialized_file": text(&o.serialized_file),
                "sprite": o.sprite,
                "unit": o.unit,
            });
            let m = row.as_object_mut().unwrap();
            if o.kind == OutputKind::AtlasJson {
                m.insert("md5".into(), json!(hex(&o.md5)));
                m.insert("sha256".into(), json!(hex(&Sha256::digest(&o.bytes))));
                m.insert(
                    "text".into(),
                    json!(String::from_utf8(o.bytes.clone()).unwrap()),
                );
            } else {
                let p = o.picture.as_ref().unwrap();
                let channels = if p.mode == "RGB" { 3 } else { 4 };
                let pixels = png_pixels(&o.bytes, p.width as usize, p.height as usize, channels);
                m.insert("mode".into(), json!(p.mode));
                m.insert("size".into(), json!([p.width, p.height]));
                if p.width * p.height <= 64 {
                    m.insert("pixels".into(), json!(hex(&pixels)));
                }
                m.insert("pixels_sha256".into(), json!(hex(&Sha256::digest(&pixels))));
                assert_eq!(
                    p.visible_hash,
                    visible_hash(p.width, p.height, channels, &pixels),
                    "{}: visible_hash",
                    o.key
                );
                m.insert("png_md5".into(), json!(hex(&o.md5)));
                m.insert("png_sha256".into(), json!(hex(&Sha256::digest(&o.bytes))));
            }
            row
        })
        .collect();
    let skips: Vec<J> = cut
        .skips
        .iter()
        .map(|s| {
            json!({
                "cls": s.class,
                "detail": s.reason.detail,
                "output": s.output.name(),
                "path_id": s.path_id,
                "reason": s.reason.reason,
                "serialized_file": text(&s.serialized_file),
                "sprite": s.sprite,
            })
        })
        .collect();
    let sheets: Vec<J> = cut
        .sheets
        .iter()
        .map(|s| {
            let by = match s.by {
                SheetBy::MaterialName => "material_name",
                SheetBy::AtlasName => "atlas_name",
            };
            json!([
                text(&s.atlas_file),
                s.atlas_path_id,
                by,
                text(&s.texture_file),
                s.texture_path_id
            ])
        })
        .collect();
    json!({
        "outputs": outputs,
        "skips": skips,
        "unresolved_scripts": cut.unresolved_scripts,
        "sheets": sheets,
    })
}

/// The fixture's scene rows are the built scenes': names, bundles (ids,
/// bytes by SHA-256, deps), inputs and options.
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
        let o = &scene.options;
        let options = json!({
            "json_key": o.json_key,
            "script_classes": o.script_classes,
            "skip_name_suffixes": o.skip_name_suffixes,
            "sprite_key": o.sprite_key,
            "texture_key": o.texture_key,
        });
        assert_eq!(row["options"], options, "{name}: options");
    }
}

/// The rows the fixture test compares: outputs, skips, sheets, and nested
/// containers (these computed apart from the reference, which records none).
const ROWS_OUTPUTS: usize = 211;
const ROWS_SKIPS: usize = 97;
const ROWS_SHEETS: usize = 99;
const ROWS_NESTED: usize = 5;

#[test]
fn every_scene_cuts_as_the_reference_recorded() {
    let doc = fixture();
    let all = scenes().unwrap();
    scene_rows_match(&doc, &all);
    let results = doc["results"].as_array().unwrap();
    // One recorded result for each built (scene, input), and no other.
    let built: BTreeSet<(String, String)> = all
        .iter()
        .flat_map(|(n, s)| s.inputs.iter().map(|i| ((*n).to_owned(), i.clone())))
        .collect();
    let mut recorded = BTreeSet::new();
    for r in results {
        let key = (
            r["scene"].as_str().unwrap().to_owned(),
            r["input"].as_str().unwrap().to_owned(),
        );
        assert!(recorded.insert(key.clone()), "{key:?} recorded twice");
    }
    assert_eq!(
        recorded, built,
        "the recorded results are the built inputs'"
    );
    let mut rows = [0usize; 4];
    let mut failures = Vec::new();
    for want in results {
        let name = want["scene"].as_str().unwrap();
        let input = want["input"].as_str().unwrap();
        let (_, scene) = all.iter().find(|(n, _)| *n == name).unwrap();
        let cut = run(scene, input);
        let nested: Vec<String> = cut.nested_containers.iter().map(text).collect();
        assert_eq!(
            nested,
            nested_rows(scene, input),
            "{name}/{input}: nested_containers"
        );
        rows[3] += nested.len();
        let got = record(&cut);
        let mut expected = want.clone();
        let m = expected.as_object_mut().unwrap();
        m.remove("scene");
        m.remove("input");
        for field in ["outputs", "skips", "unresolved_scripts", "sheets"] {
            if got[field] != expected[field] {
                failures.push(format!(
                    "{name}/{input} {field}:\n got  {}\n want {}",
                    serde_json::to_string_pretty(&got[field]).unwrap(),
                    serde_json::to_string_pretty(&expected[field]).unwrap()
                ));
            }
        }
        for (i, field) in ["outputs", "skips", "sheets"].into_iter().enumerate() {
            rows[i] += got[field].as_array().unwrap().len();
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    assert_eq!(recorded.len(), 41, "every recorded result");
    assert_eq!(
        rows,
        [ROWS_OUTPUTS, ROWS_SKIPS, ROWS_SHEETS, ROWS_NESTED],
        "the outputs, skips, sheets and nested containers compared"
    );
    assert_eq!(doc["zlib"], "1.2.13");
    assert_eq!(unityrs_imageio::zlib_version(), "1.2.13");
}

/// Every input's candidates of the named scenes, by input.
fn candidates(names: &[&str]) -> BTreeMap<String, Vec<Candidate>> {
    let all = scenes().unwrap();
    let mut out = BTreeMap::new();
    for name in names {
        let (_, scene) = all.iter().find(|(n, _)| n == name).unwrap();
        for input in &scene.inputs {
            let cut = run(scene, input);
            let cands = cut
                .outputs
                .iter()
                .map(|o| Candidate {
                    input: input.clone(),
                    key: o.key.clone(),
                    unit: o.unit.clone(),
                    md5: o.md5,
                })
                .collect();
            out.insert(input.clone(), cands);
        }
    }
    out
}

#[test]
fn winners_are_decided_as_the_reference_decided_them() {
    let doc = fixture();
    let by_input = candidates(&["winners", "textured_winners"]);
    let cases = doc["winners"].as_array().unwrap();
    let mut failures = Vec::new();
    let mut decisions = 0;
    for case in cases {
        let name = case["case"].as_str().unwrap();
        let inputs: Vec<&str> = case["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i.as_str().unwrap())
            .collect();
        let cands: Vec<Candidate> = inputs
            .iter()
            .flat_map(|i| by_input[*i].iter().cloned())
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
        let failed: BTreeSet<String> = case["failed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f.as_str().unwrap().to_owned())
            .collect();
        let incomplete = case["publish_incomplete"].as_bool().unwrap();
        let got: Vec<J> = ngui::decide(&cands, &prefer, &failed, incomplete)
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
        if J::from(got.clone()) != case["decisions"] {
            failures.push(format!(
                "{name}:\n got  {}\n want {}",
                serde_json::to_string(&got).unwrap(),
                serde_json::to_string(&case["decisions"]).unwrap()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    assert_eq!(cases.len(), 17);
    assert_eq!(decisions, 80, "the decisions compared");
}
