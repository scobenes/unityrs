//! Small synthetic inputs for every currently decoded texture format.
use serde_json::Value;
use std::{fs, path::Path};
use unityrs_export::ngui::Format;
use unityrs_testkit::ngui::{self, AtlasScene, CutOptions, SceneBundle};

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

pub fn scene() -> AtlasScene {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../unityrs-texture/tests/fixtures/unitycrunch_native.json");
    let crunch: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let objects: Vec<_> = Format::ALL
        .iter()
        .enumerate()
        .map(|(i, format)| {
            let (data, size) = if format.is_crunched() {
                let row = crunch["cases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|c| c["format"].as_i64() == Some(i64::from(format.texture_format())))
                    .unwrap();
                (
                    unhex(row["data"]["hex"].as_str().unwrap()),
                    (
                        i32::try_from(row["width"].as_i64().unwrap()).unwrap(),
                        i32::try_from(row["height"].as_i64().unwrap()).unwrap(),
                    ),
                )
            } else {
                // Sufficient backing for every 16x8 raw/block representation.
                // HDR ASTC uses legal zero-valued FP16 void blocks; BC6H zero
                // selects valid mode zero. Other zero blocks decode deterministically.
                let mut data = vec![0; 4096];
                if format.is_astc_hdr() {
                    for block in data.as_chunks_mut::<16>().0 {
                        block[..8].fill(255);
                        block[0] = 0xfc;
                    }
                }
                (data, (16, 8))
            };
            ngui::texture(
                i64::try_from(i + 1).unwrap(),
                format.name().as_bytes(),
                format.texture_format(),
                size,
                data,
            )
        })
        .collect();
    let data =
        unityrs_testkit::env::bundle(&[("CAB-formats", ngui::file(&[], &objects, true).unwrap())]);
    AtlasScene {
        bundles: vec![SceneBundle {
            id: "x.bundle".into(),
            data,
            deps: Vec::new(),
        }],
        inputs: vec!["x.bundle".into()],
        options: CutOptions::textures("{name}.png"),
    }
}
