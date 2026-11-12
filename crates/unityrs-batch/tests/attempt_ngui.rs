//! X5 through the sealed staged-path API, reading candidates and bytes on disk.
mod attempt_support;
use attempt_support::*;
use serde_json::{Value as J, json};
use std::cell::RefCell;
use unityrs_batch::attempt::{Hooks, State};
use unityrs_export::ngui::{self, SheetBy};
use unityrs_testkit::{ngui::scenes, scratch::Scratch};
#[derive(Default)]
struct Observe(RefCell<Vec<J>>);
impl ngui::Hooks for Observe {}
impl Hooks for Observe {
    fn sheets(&self, rows: &[ngui::SheetRow]) {
        *self.0.borrow_mut() = rows
            .iter()
            .map(|r| {
                json!([
                    r.atlas_file.to_str().unwrap(),
                    r.atlas_path_id,
                    match r.by {
                        SheetBy::MaterialName => "material_name",
                        SheetBy::AtlasName => "atlas_name",
                    },
                    r.texture_file.to_str().unwrap(),
                    r.texture_path_id
                ])
            })
            .collect();
    }
}
#[test]
#[allow(clippy::too_many_lines)]
fn every_ngui_scene_matches_the_recorded_oracle_from_disk() {
    let fixture = fixture("../unityrs-testkit/tests/fixtures/ngui.json");
    let scenes = scenes().unwrap();
    let built: std::collections::BTreeSet<_> = scenes
        .iter()
        .flat_map(|(name, scene)| {
            scene
                .inputs
                .iter()
                .map(|input| ((*name).to_owned(), input.clone()))
        })
        .collect();
    let recorded: std::collections::BTreeSet<_> = fixture["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["scene"].as_str().unwrap().to_owned(),
                r["input"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(built, recorded);
    let mut held = Vec::new();
    let mut jobs = Vec::new();
    let mut outputs = 0;
    let mut skips = 0;
    let mut inputs = 0;
    for want in fixture["results"].as_array().unwrap() {
        let name = want["scene"].as_str().unwrap();
        let input = want["input"].as_str().unwrap();
        let scene = &scenes.iter().find(|(n, _)| *n == name).unwrap().1;
        let harness = Harness::new(scene, input);
        let observer = Observe::default();
        let end = harness.run(limits(), &observer);
        clear(&end);
        assert_eq!(end.state, State::Done, "{name}/{input}: {end:?}");
        let rows = harness.rows();
        let candidates: Vec<_> = rows.iter().filter(|r| r["type"] == "candidate").collect();
        let actual_skips: Vec<_> = rows.iter().filter(|r| r["type"] == "skip").collect();
        assert_eq!(
            candidates.len(),
            want["outputs"].as_array().unwrap().len(),
            "{name}/{input}"
        );
        for (got, want) in candidates.iter().zip(want["outputs"].as_array().unwrap()) {
            for field in ["key", "kind", "path_id", "serialized_file", "unit", "mode"] {
                assert_eq!(got[field], want[field], "{name}/{input}: {field}");
            }
            let bytes = harness.output(got);
            assert_eq!(got["size"], json!(bytes.len()));
            let mut expected = want.clone();
            if want["kind"] == "atlas_json" {
                assert_eq!(got["md5"], want["md5"]);
            } else {
                assert_eq!(got["md5"], want["png_md5"]);
                expected["md5"] = want["png_md5"].clone();
                expected["sha256"] = want["png_sha256"].clone();
            }
            expected["visible_hash"] = got["visible_hash"].clone();
            expected["crc32"] = got["crc32"].clone();
            // NGUI's `sha256` is the file hash; pixel hash has its own field.
            if let Some(hash) = want.get("pixels_sha256") {
                expected["pixels_sha256"] = hash.clone();
            }
            jobs.push(json!({"file": harness.directory.path().join(got["file"].as_str().unwrap()), "want": expected}));
        }
        assert_eq!(
            actual_skips.len(),
            want["skips"].as_array().unwrap().len(),
            "{name}/{input}"
        );
        for (got, want) in actual_skips.iter().zip(want["skips"].as_array().unwrap()) {
            for field in [
                "path_id",
                "serialized_file",
                "output",
                "sprite",
                "reason",
                "detail",
            ] {
                assert_eq!(got[field], want[field], "{name}/{input}: {field}");
            }
            assert_eq!(got["class"], want["cls"]);
        }
        assert_eq!(
            json!(*observer.0.borrow()),
            want["sheets"],
            "{name}/{input}"
        );
        assert_eq!(
            json!(end.figures.unresolved_scripts),
            want["unresolved_scripts"]
        );
        let summary = rows.last().unwrap();
        assert_eq!(summary["type"], "input_summary");
        for (key, value) in [
            ("input_peak", end.figures.input_peak),
            ("dependency_peak", end.figures.dependency_peak),
            ("export_work", end.figures.export_work),
            ("spooled_part", end.spooled_part),
            ("claimed_bytes", end.claimed_bytes),
            ("decode_work_peak", end.figures.decode_work_peak),
            ("ms", end.figures.ms),
        ] {
            assert_eq!(summary[key], json!(value), "{name}/{input}: {key}");
        }
        let fixture_scene = fixture["scenes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == name)
            .unwrap();
        for (i, b) in harness.bundles.iter().enumerate() {
            let expected = fixture_scene["bundles"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["id"] == b.id)
                .unwrap();
            jobs.push(json!({"file": harness.paths[i], "want": {"sha256": expected["sha256"]}}));
        }
        inputs += 1;
        outputs += candidates.len();
        skips += actual_skips.len();
        held.push(harness);
    }
    assert_eq!(inputs, 41);
    assert!(outputs > 100 && skips > 50);
    check_files(&Scratch::new("ngui-check"), &jobs);
}

#[test]
fn grouped_exports_use_plan_ordinals_with_gaps() {
    let all = scenes().unwrap();
    let scene = &all
        .iter()
        .find(|(name, _)| *name == "two_sheet_groups")
        .unwrap()
        .1;
    let mut h = Harness::new(scene, &scene.inputs[0]);
    h.options.texture_key =
        Some(ngui::Template::parse("{name}.png", ngui::TemplateKind::Texture).unwrap());
    let end = h.run(limits(), &ngui::NoHooks);
    clear(&end);
    assert_eq!(end.state, State::Done);
    let rows = h.rows();
    let candidates: Vec<_> = rows.iter().filter(|r| r["type"] == "candidate").collect();
    let files: Vec<_> = candidates
        .iter()
        .map(|r| r["file"].as_str().unwrap())
        .collect();
    assert_eq!(files, ["o1", "o3", "o6", "o7", "o0", "o5"]);
    for row in candidates {
        assert_eq!(row["size"], json!(h.output(row).len()));
    }
    assert!(!h.directory.path().join("o2").exists());
    assert!(!h.directory.path().join("o4").exists());
}
