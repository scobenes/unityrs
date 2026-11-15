//! Full HDR profile recorded by `tools/fixtures/texture_hdr.py`, alongside
//! UnityPy's unchanged LDR-profile hashes. No Python is needed to replay.
use std::collections::BTreeSet;

use serde_json::Value as J;
use sha2::{Digest, Sha256};
use unityrs_core::limits::Meter;
use unityrs_texture::{DecodeError, Format, HdrError, decode, plan};

fn fixture() -> J {
    let doc: J = serde_json::from_str(include_str!("fixtures/astc_hdr.json")).unwrap();
    assert_eq!(doc["unitypy"], "1.25.2");
    assert_eq!(doc["pillow"], "9.5.0");
    assert_eq!(doc["astc_encoder"], "0.1.12");
    assert_eq!(
        doc["astcenc_commit"],
        "631fb04e1c4bca002a8b653f34e044dbb35b32b4"
    );
    doc
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn hash(data: &[u8]) -> String {
    use std::fmt::Write;
    Sha256::digest(data)
        .iter()
        .fold(String::new(), |mut out, byte| {
            write!(out, "{byte:02x}").unwrap();
            out
        })
}

#[test]
fn every_hdr_endpoint_and_footprint_decodes_as_the_full_profile() {
    let doc = fixture();
    let mut diverging = BTreeSet::new();
    for c in doc["blocks"].as_array().unwrap() {
        let id = c["id"].as_str().unwrap();
        let side = i32::try_from(c["side"].as_u64().unwrap()).unwrap();
        let format =
            Format::from_texture_format(i32::try_from(c["format"].as_u64().unwrap()).unwrap())
                .unwrap();
        let data = unhex(c["data"].as_str().unwrap());
        let p = plan(format, data.len(), side, side).unwrap();
        let meter = Meter::unlimited();
        let got = decode(format, &data, side, side, &meter);
        if c["raises"] == "InvalidBlock" {
            assert_eq!(
                got.err(),
                Some(DecodeError::Hdr(HdrError::InvalidBlock)),
                "{id}"
            );
        } else {
            let image = got.unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(image.mode().name(), "RGBA", "{id}");
            assert_eq!(image.width(), u32::try_from(side).unwrap());
            assert_eq!(image.height(), u32::try_from(side).unwrap());
            assert_eq!(hash(image.pixels()), c["sha256"].as_str().unwrap(), "{id}");
            assert_eq!(image.charge().bytes(), p.charge(), "{id}");
            if c["sha256"] != c["unitypy_sha256"] {
                for tag in c["tags"].as_array().unwrap() {
                    if let Some(tag) = tag.as_str().filter(|s| s.starts_with("cem:")) {
                        diverging.insert((side, tag.to_owned()));
                    }
                }
            }
            drop(image);
        }
        assert_eq!(meter.used(), 0, "{id}");
    }
    for side in [4, 5, 6, 8, 10, 12] {
        for cem in [2, 3, 7, 11, 14, 15] {
            assert!(
                diverging.contains(&(side, format!("cem:{cem}"))),
                "side {side}, CEM {cem} never differs from UnityPy"
            );
        }
    }
}

#[test]
fn hdr_image_crops_lengths_and_content_errors_follow_the_reference() {
    let doc = fixture();
    let cases = doc["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 66);
    for c in cases {
        let id = c["id"].as_str().unwrap();
        let format =
            Format::from_texture_format(i32::try_from(c["format"].as_i64().unwrap()).unwrap())
                .unwrap();
        let width = i32::try_from(c["width"].as_i64().unwrap()).unwrap();
        let height = i32::try_from(c["height"].as_i64().unwrap()).unwrap();
        let data = unhex(c["data"]["hex"].as_str().unwrap());
        assert_eq!(hash(&data), c["data_sha256"].as_str().unwrap(), "{id}");
        let meter = Meter::unlimited();
        let planned = plan(format, data.len(), width, height);
        let got = decode(format, &data, width, height, &meter);
        let reference = &c["reference"];
        if let Some(raises) = reference["raises"].as_str() {
            let error = match raises {
                "InvalidBlock" => DecodeError::Hdr(HdrError::InvalidBlock),
                "NoImageData" => DecodeError::NoImageData,
                "NegativeSize" => DecodeError::NegativeSize { width, height },
                "NotEnoughData" => DecodeError::NotEnoughData {
                    needed: reference["needed"].as_u64().unwrap(),
                    got: u64::try_from(data.len()).unwrap(),
                },
                other => panic!("{id}: unknown reference refusal {other}"),
            };
            assert_eq!(got.err().as_ref(), Some(&error), "{id}");
            if raises == "InvalidBlock" {
                assert!(planned.is_ok(), "{id}");
            } else {
                assert_eq!(planned.err(), Some(error), "{id}");
            }
        } else {
            let image = got.unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(image.mode().name(), "RGBA", "{id}");
            assert_eq!(
                u64::from(image.width()),
                reference["size"][0].as_u64().unwrap(),
                "{id}"
            );
            assert_eq!(
                u64::from(image.height()),
                reference["size"][1].as_u64().unwrap(),
                "{id}"
            );
            assert_eq!(
                hash(image.pixels()),
                reference["sha256"].as_str().unwrap(),
                "{id}"
            );
            assert_eq!(image.charge().bytes(), planned.unwrap().charge(), "{id}");
            drop(image);
        }
        assert_eq!(meter.used(), 0, "{id}");
    }
}
