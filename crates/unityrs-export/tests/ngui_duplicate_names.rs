//! First-name ownership of valid textures, from `docs/BATCH_SPEC.md`
//! section 4.4: names precede selection and keys, span an input's selected
//! `SerializedFiles`, and compare exact bytes.

use unityrs_export::ngui::{Output, OutputKind};
use unityrs_testkit::env::bundle;
use unityrs_testkit::ngui::{AtlasScene, CutOptions, SceneBundle, file, format, texture};

mod common;
use common::pixels::png_pixels;
use common::{Ran, run};

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn scene(split_files: bool, names: [&[u8]; 2], key: &str) -> AtlasScene {
    // Descending path ids distinguish object order from path-id order.
    let first = texture(20, names[0], format::RGBA32, (1, 1), RED.to_vec());
    let second = texture(10, names[1], format::RGBA32, (1, 1), BLUE.to_vec());
    let files = if split_files {
        vec![
            ("CAB-a", file(&[], &[first], true).unwrap()),
            ("CAB-b", file(&[], &[second], true).unwrap()),
        ]
    } else {
        vec![("CAB-a", file(&[], &[first, second], true).unwrap())]
    };
    AtlasScene {
        bundles: vec![SceneBundle {
            id: "x.bundle".to_owned(),
            data: bundle(&files),
            deps: Vec::new(),
        }],
        inputs: vec!["x.bundle".to_owned()],
        options: CutOptions::textures(key),
    }
}

fn assert_texture(output: &Output, file: &str, path_id: i64, key: &str, pixels: [u8; 4]) {
    assert_eq!(output.kind, OutputKind::Texture);
    assert_eq!(output.serialized_file.as_bytes(), file.as_bytes());
    assert_eq!(output.path_id, path_id);
    assert_eq!(output.key, key);
    let picture = output.picture.as_ref().unwrap();
    assert_eq!(
        (picture.width, picture.height, picture.mode),
        (1, 1, "RGBA")
    );
    assert_eq!(png_pixels(&output.bytes, 1, 1, 4), pixels);
}

fn assert_first_owns(ran: &Ran, second_file: &str, key: &str) {
    assert_eq!(ran.outputs.len(), 1);
    assert_eq!(ran.handed.len(), 1);
    assert_texture(&ran.outputs[0], "CAB-a", 20, key, RED);
    assert_eq!(ran.skips.len(), 1);
    assert_eq!(ran.plan_skips, 1, "the duplicate is skipped while planning");
    let skip = &ran.skips[0];
    assert_eq!(skip.serialized_file.as_bytes(), second_file.as_bytes());
    assert_eq!(skip.path_id, 10);
    assert_eq!(skip.class, "Texture2D");
    assert_eq!(skip.output.name(), "texture");
    assert_eq!(skip.sprite, None);
    assert_eq!(skip.reason.reason, "duplicate_name");
    assert_eq!(skip.reason.detail, None);
}

#[test]
fn first_valid_same_name_texture_owns_the_name_in_one_file() {
    let s = scene(false, [b"tex", b"tex"], "{name}.png");
    assert_first_owns(&run(&s, "x.bundle"), "CAB-a", "tex.png");
}

#[test]
fn first_valid_same_name_texture_owns_the_name_across_files_of_one_input() {
    let s = scene(true, [b"tex", b"tex"], "{name}.png");
    assert_first_owns(&run(&s, "x.bundle"), "CAB-b", "tex.png");
}

#[test]
fn path_id_keys_cannot_rescue_a_later_same_name_texture() {
    for split_files in [false, true] {
        // The keys would be distinct if the later texture reached expansion.
        let s = scene(split_files, [b"tex", b"tex"], "{name}-{path_id}.png");
        let second_file = if split_files { "CAB-b" } else { "CAB-a" };
        assert_first_owns(&run(&s, "x.bundle"), second_file, "tex-20.png");
    }
}

#[test]
fn case_differing_names_both_export_without_folding() {
    for split_files in [false, true] {
        // Reverse bytewise name order: outputs still follow object order.
        // These expansions are distinct keys (BATCH_SPEC sections 2.3, 4.4).
        let s = scene(split_files, [b"tex", b"Tex"], "{name}.png");
        let ran = run(&s, "x.bundle");
        assert!(ran.skips.is_empty(), "{:?}", ran.skips);
        assert_eq!(ran.outputs.len(), 2);
        assert_eq!(ran.handed.len(), 2);
        assert_texture(&ran.outputs[0], "CAB-a", 20, "tex.png", RED);
        let second_file = if split_files { "CAB-b" } else { "CAB-a" };
        assert_texture(&ran.outputs[1], second_file, 10, "Tex.png", BLUE);
    }
}
