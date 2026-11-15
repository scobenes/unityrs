//! Malformed HDR blocks skip one texture while adjacent outputs continue.

mod common;

use unityrs_core::{FileContext, limits::Meter};
use unityrs_export::ngui::Formats;
use unityrs_testkit::{
    env::bundle,
    ngui::{AtlasScene, CutOptions, SceneBundle, file, texture},
};

#[test]
fn malformed_hdr_texture_is_skipped_and_other_textures_export() {
    for format in [24, 66, 67, 68, 69, 70, 71] {
        let mut bad = vec![0; 16];
        if format == 24 {
            bad[0] = 19; // Reserved BC6H mode; ASTC's all-zero block is illegal.
        }
        let mut good = vec![0; 16];
        if format != 24 {
            // HDR void extent with unbounded extents and zero half-float colour.
            good[..8].fill(255);
            good[0] = 0xfc;
        }
        let objects = [
            texture(1, b"before", 4, (1, 1), vec![255; 4]),
            texture(2, b"bad", format, (1, 1), bad),
            texture(3, b"after", format, (1, 1), good),
        ];
        let scene = AtlasScene {
            bundles: vec![SceneBundle {
                id: "x.bundle".to_owned(),
                data: bundle(&[("CAB-hdr", file(&[], &objects, true).unwrap())]),
                deps: Vec::new(),
            }],
            inputs: vec!["x.bundle".to_owned()],
            options: CutOptions::textures("{name}.png"),
        };
        let engine = common::Engine {
            formats: Some(Formats::DECODED),
            ..common::Engine::default()
        };
        let ran = common::run_in(
            &scene,
            "x.bundle",
            FileContext::default(),
            &Meter::unlimited(),
            &engine,
        )
        .unwrap_or_else(|e| panic!("format {format}: {e:?}"));
        assert_eq!(ran.skips.len(), 1, "format {format}: {ran:?}");
        let skip = &ran.skips[0];
        assert_eq!((skip.path_id, skip.output.name()), (2, "texture"));
        assert_eq!(skip.reason.reason, "decode_error");
        assert_eq!(skip.reason.detail.as_deref(), Some("malformed"));
        assert!(!skip.reason.cut);
        let outputs: Vec<_> = ran
            .outputs
            .iter()
            .map(|o| {
                assert!(o.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
                (o.path_id, o.key.as_str())
            })
            .collect();
        assert_eq!(outputs, [(1, "before.png"), (3, "after.png")]);
    }
}
