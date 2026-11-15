//! PARITY 15.8 WEBP, L5b: `tools/fixtures/image_webp.py`'s Pillow 9.5.0 files.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fmt::Write;
use unityrs_imageio::{Image, Mode, Unmetered, WebpOptions, encode_webp};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        write!(s, "{b:02x}").unwrap();
        s
    })
}

fn pixels(mode: Mode, width: u32, height: u32, kind: &str) -> Vec<u8> {
    let mut state = 0x1234_5678_u32;
    let mut out = Vec::new();
    for i in 0..width * height {
        for c in 0..mode.channels() {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let mut value = (state >> 24) as u8;
            if kind == "flat" {
                value = (73 + c * 37) as u8;
            } else if kind == "gradient" {
                value = ((i % width * 7 + i / width * 13 + c as u32 * 53) & 255) as u8;
            }
            if c == mode.channels() - 1 && matches!(mode, Mode::La | Mode::Rgba) {
                value = match kind {
                    "opaque" => 255,
                    "transparent" => 0,
                    "alpha" => [0, 1, 127, 254, 255][i as usize % 5],
                    _ => value,
                };
            }
            out.push(value);
        }
    }
    out
}

#[test]
fn webps_equal_pillows_still_image_bytes() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/webp.json")).unwrap();
    let rows = fixture["cases"].as_array().unwrap();
    assert_eq!(rows.len(), 576);
    for row in rows {
        let mode = Mode::from_name(row["mode"].as_str().unwrap()).unwrap();
        let width = row["width"].as_u64().unwrap() as u32;
        let height = row["height"].as_u64().unwrap() as u32;
        let raw = pixels(mode, width, height, row["kind"].as_str().unwrap());
        assert_eq!(hex(&Sha256::digest(&raw)), row["pixels_sha256"]);
        let image = Image::new(width, height, mode, &raw).unwrap();
        let options = WebpOptions {
            quality: row["quality"].as_f64().unwrap() as f32,
            exact: row["exact"].as_bool().unwrap(),
            ..WebpOptions::default()
        };
        let file = encode_webp(&image, options, &mut Unmetered).unwrap();
        assert_eq!(file.bytes().len() as u64, row["length"], "{row}");
        assert_eq!(hex(&Sha256::digest(file.bytes())), row["sha256"], "{row}");
        assert_eq!(
            hex(&file.bytes()[..32.min(file.bytes().len())]),
            row["head"]
        );
        if let Some(whole) = row["hex"].as_str() {
            assert_eq!(hex(file.bytes()), whole);
        }
    }
}

#[test]
fn defaults_and_explicit_pillow_defaults_agree() {
    assert_eq!(
        WebpOptions::default(),
        WebpOptions {
            quality: 80.0,
            method: 4,
            exact: false
        }
    );
}

#[test]
fn rgba_subslices_at_every_alignment_match_the_aligned_oracle() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/webp.json")).unwrap();
    let row = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| {
            r["mode"] == "RGBA"
                && r["width"] == 3
                && r["height"] == 5
                && r["kind"] == "alpha"
                && r["quality"] == 80
                && r["exact"] == false
        })
        .unwrap();
    let raw = pixels(Mode::Rgba, 3, 5, "alpha");
    for offset in 0..16 {
        let mut backing = vec![0; offset];
        backing.extend_from_slice(&raw);
        let image = Image::new(3, 5, Mode::Rgba, &backing[offset..]).unwrap();
        let file = encode_webp(&image, WebpOptions::default(), &mut Unmetered).unwrap();
        assert_eq!(hex(&Sha256::digest(file.bytes())), row["sha256"]);
    }
}
