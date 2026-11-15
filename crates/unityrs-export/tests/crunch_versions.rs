//! Packed version failures at texture and Sprite sheet selection.
mod common;

use unityrs_core::{FileContext, RawStr, limits::Meter, version::UnityVersion};
use unityrs_export::ngui::{Formats, SkipOutput};
use unityrs_testkit::ngui::{self, AtlasScene, CutOptions, SceneBundle, SpriteSpec};

#[test]
fn malformed_versions_fail_crunched_textures_and_color_and_alpha_sheets() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../unityrs-texture/tests/fixtures/unitycrunch_native.json"
    ))
    .unwrap();
    for format in [28, 29, 64, 65] {
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["format"] == format && c["width"] == 1 && c["height"] == 1)
            .unwrap();
        let hex = case["data"]["hex"].as_str().unwrap();
        let data = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let objects = [
            ngui::texture(1, b"crunch", format, (1, 1), data),
            ngui::texture(2, b"plain", 4, (1, 1), vec![255; 4]),
            ngui::texture(3, b"empty", format, (0, 1), vec![]),
            SpriteSpec::rect("color", [0.0, 0.0, 1.0, 1.0], (0, 1)).object(4),
            SpriteSpec::rect("alpha", [0.0, 0.0, 1.0, 1.0], (0, 2))
                .alpha((0, 1))
                .object(5),
        ];
        let scene = AtlasScene {
            bundles: vec![SceneBundle {
                id: "x.bundle".to_owned(),
                data: unityrs_testkit::env::bundle(&[(
                    "CAB-versions",
                    ngui::file(&[], &objects, true).unwrap(),
                )]),
                deps: vec![],
            }],
            inputs: vec!["x.bundle".to_owned()],
            options: CutOptions {
                texture_key: Some("textures/{name}.png".to_owned()),
                sprites_key: Some("sprites/{name}.png".to_owned()),
                ..CutOptions::default()
            },
        };
        let mut context = FileContext::unitypy();
        context.version.force_serialized =
            Some(UnityVersion::parse(&RawStr::from("5.0.0a1536")).unwrap());
        let engine = common::Engine {
            formats: Some(Formats::DECODED),
            ..common::Engine::default()
        };
        let ran =
            common::run_in(&scene, "x.bundle", context, &Meter::unlimited(), &engine).unwrap();
        assert_eq!(ran.handed.len(), 1, "format {format}");
        assert_eq!(ran.handed[0].key, "textures/plain.png");
        for (id, kind) in [
            (1, SkipOutput::Texture),
            (4, SkipOutput::Sprite),
            (5, SkipOutput::Sprite),
        ] {
            let skip = ran
                .skips
                .iter()
                .find(|s| s.path_id == id && s.output == kind)
                .unwrap();
            assert_eq!(skip.reason.reason, "decode_error");
            assert_eq!(skip.reason.detail.as_deref(), Some("version"));
        }
        let empty = ran.skips.iter().find(|s| s.path_id == 3).unwrap();
        assert_eq!(empty.reason.reason, "empty_image");
    }
}

#[test]
fn missing_data_precedes_malformed_versions_for_textures_and_both_sprite_sheets() {
    for format in [28, 29, 64, 65, 4] {
        for source in ["absent", "empty", "missing"] {
            let texture = match source {
                "absent" => ngui::texture(1, b"missing", format, (1, 1), vec![]),
                path => ngui::texture_streamed(1, b"missing", format, (1, 1), path, 0, 4),
            };
            let objects = [
                texture,
                ngui::texture(2, b"plain", 4, (1, 1), vec![255; 4]),
                SpriteSpec::rect("color", [0.0, 0.0, 1.0, 1.0], (0, 1)).object(3),
                SpriteSpec::rect("alpha", [0.0, 0.0, 1.0, 1.0], (0, 2))
                    .alpha((0, 1))
                    .object(4),
                SpriteSpec::rect("plain", [0.0, 0.0, 1.0, 1.0], (0, 2)).object(5),
            ];
            let scene = AtlasScene {
                bundles: vec![SceneBundle {
                    id: "x.bundle".to_owned(),
                    data: unityrs_testkit::env::bundle(&[
                        ("CAB-versions", ngui::file(&[], &objects, true).unwrap()),
                        ("empty", vec![]),
                    ]),
                    deps: vec![],
                }],
                inputs: vec!["x.bundle".to_owned()],
                options: CutOptions {
                    texture_key: Some("textures/{name}.png".to_owned()),
                    sprites_key: Some("sprites/{name}.png".to_owned()),
                    ..CutOptions::default()
                },
            };
            let mut context = FileContext::unitypy();
            context.version.force_serialized =
                Some(UnityVersion::parse(&RawStr::from("5.0.0a1536")).unwrap());
            let engine = common::Engine {
                formats: Some(Formats::DECODED),
                ..common::Engine::default()
            };
            let ran =
                common::run_in(&scene, "x.bundle", context, &Meter::unlimited(), &engine).unwrap();
            assert_eq!(ran.handed.len(), 2, "format {format}, {source}");
            assert_eq!(ran.skips.len(), 3, "format {format}, {source}");
            for skip in &ran.skips {
                if source == "missing" {
                    assert_eq!(skip.reason.reason, "outside_view(missing)");
                    assert_eq!(skip.reason.detail, None);
                } else {
                    assert_eq!(skip.reason.reason, "decode_error");
                    assert_eq!(skip.reason.detail.as_deref(), Some("no_image_data"));
                }
            }
        }
    }
}
