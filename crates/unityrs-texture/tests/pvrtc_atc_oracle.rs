//! Replay the coordinator-generated pinned oracle, with the same data recipe
//! and image/error checks as the ETC/ASTC fixtures. No expected image is made
//! by this port. Generate `pvrtc_atc.json` before running this test.
use serde_json::Value as J;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};
use unityrs_core::limits::Meter;
use unityrs_testkit::Rng;
use unityrs_texture::{DecodeError, Format, Mode, decode, plan};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn hex(data: &[u8]) -> String {
    use std::fmt::Write;
    data.iter().fold(String::new(), |mut s, b| {
        write!(s, "{b:02x}").unwrap();
        s
    })
}

fn build(spec: &J) -> Vec<u8> {
    if let Some(h) = spec["hex"].as_str() {
        return unhex(h);
    }
    let n = usize::try_from(spec["len"].as_u64().unwrap()).unwrap();
    let mut rng = Rng::new(spec["seed"].as_u64().unwrap());
    let mut data = Vec::new();
    while data.len() < n {
        data.extend_from_slice(&rng.next_u64().to_le_bytes());
    }
    data.truncate(n);
    data
}

#[test]
fn every_pvrtc_atc_decode_is_the_oracle_image_or_error() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pvrtc_atc.json");
    let doc: J = serde_json::from_str(
        &std::fs::read_to_string(path).expect("generate texture_pvrtc_atc.py first"),
    )
    .unwrap();
    assert_eq!(doc["unitypy"], "1.25.2");
    assert_eq!(doc["pillow"], "9.5.0");
    assert_eq!(doc["texture2ddecoder"], "1.0.5");
    let mut seen = BTreeSet::new();
    let mut errors = BTreeSet::new();
    let rows = doc["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 789);
    for c in rows {
        let id = c["id"].as_str().unwrap();
        let f = Format::from_texture_format(i32::try_from(c["format"].as_i64().unwrap()).unwrap())
            .unwrap();
        seen.insert(f.texture_format());
        let (w, h) = (
            i32::try_from(c["width"].as_i64().unwrap()).unwrap(),
            i32::try_from(c["height"].as_i64().unwrap()).unwrap(),
        );
        let data = build(&c["data"]);
        assert_eq!(hex(&Sha256::digest(&data)), c["data_sha256"], "{id}");
        let p = plan(f, data.len(), w, h);
        let meter = Meter::unlimited();
        let got = decode(f, &data, w, h, &meter);
        if let Some(mode) = c["mode"].as_str() {
            let p = p.unwrap_or_else(|e| panic!("{id}: {e}"));
            let image = got.unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(mode, "RGBA", "{id}");
            assert_eq!(image.mode(), Mode::Rgba, "{id}");
            assert_eq!(
                J::from(vec![image.width(), image.height()]),
                c["size"],
                "{id}"
            );
            assert_eq!(hex(&Sha256::digest(image.pixels())), c["sha256"], "{id}");
            if let Some(px) = c["pixels"].as_str() {
                assert_eq!(hex(image.pixels()), px, "{id}");
            }
            assert_eq!(p.scratch, 0, "{id}");
            assert_eq!(
                p.output_bytes,
                u64::try_from(image.pixels().len()).unwrap(),
                "{id}"
            );
            assert_eq!(p.charge(), image.charge().bytes(), "{id}");
            assert_eq!(meter.used(), p.peak_charge(), "{id}");
        } else {
            let raises = c["raises"].as_str().unwrap();
            errors.insert(raises.to_owned());
            let expected = match raises {
                "SystemError" => {
                    assert!((30..=33).contains(&f.texture_format()), "{id}");
                    assert!(
                        c["message"]
                            .as_str()
                            .unwrap()
                            .contains("without setting an exception"),
                        "{id}"
                    );
                    DecodeError::PvrtcBlockCount
                }
                "ValueError" => {
                    assert_eq!(c["message"], "Texture2D has no image data", "{id}");
                    DecodeError::NoImageData
                }
                _ => panic!("{id}: unexpected oracle error: {c}"),
            };
            assert_eq!(got.unwrap_err(), expected, "{id}");
            assert_eq!(p.unwrap_err(), expected, "{id}");
        }
        assert_eq!(meter.used(), 0, "{id}");
    }
    assert_eq!(seen, [30, 31, 32, 33, 35, 36].into());
    assert_eq!(
        errors,
        ["SystemError".to_owned(), "ValueError".to_owned()].into()
    );
}
