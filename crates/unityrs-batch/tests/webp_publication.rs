//! All publication paths select WebP; keys and archive paths remain templates.
mod attempt_support;
use attempt_support::*;
use std::collections::BTreeSet;
use unityrs_export::ngui::ImageEncoding;
use unityrs_testkit::ngui::{scenes, sprite_scenes};

#[test]
fn every_image_publication_path_writes_webp_and_keeps_source_hashes() {
    let scenes: Vec<_> = scenes()
        .unwrap()
        .into_iter()
        .filter(|(name, _)| ["own_sheet_by_material_name", "two_atlases_one_sheet"].contains(name))
        .chain(
            sprite_scenes()
                .unwrap()
                .into_iter()
                .filter(|(name, _)| *name == "own_sheets"),
        )
        .collect();
    let mut kinds = BTreeSet::new();
    for (_, scene) in scenes {
        for input in &scene.inputs {
            let png = Harness::new(&scene, input);
            let end = png.run(limits(), &unityrs_export::ngui::NoHooks);
            clear(&end);
            let expected: Vec<_> = png
                .rows()
                .into_iter()
                .filter(|r| r["type"] == "candidate")
                .collect();
            for quality in [80, 95] {
                let mut webp = Harness::new(&scene, input);
                webp.options.image = ImageEncoding::Webp { quality };
                let end = webp.run(limits(), &unityrs_export::ngui::NoHooks);
                clear(&end);
                let rows: Vec<_> = webp
                    .rows()
                    .into_iter()
                    .filter(|r| r["type"] == "candidate")
                    .collect();
                assert_eq!(rows.len(), expected.len(), "{end:?}");
                for (row, png_row) in rows.iter().zip(&expected) {
                    assert_eq!(row["key"], png_row["key"]);
                    let bytes = webp.output(row);
                    if row["kind"] == "atlas_json" {
                        assert_eq!(bytes, png.output(png_row));
                    } else {
                        kinds.insert(row["kind"].as_str().unwrap().to_owned());
                        assert_eq!(&bytes[..4], b"RIFF");
                        assert_eq!(&bytes[8..12], b"WEBP");
                        assert_ne!(row["md5"], png_row["md5"]);
                        assert_eq!(row["visible_hash"], png_row["visible_hash"]);
                        assert_eq!(row["mode"], png_row["mode"]);
                    }
                }
            }
        }
    }
    assert_eq!(
        kinds,
        ["texture", "atlas_sprite", "sprite"]
            .map(str::to_owned)
            .into()
    );
}
