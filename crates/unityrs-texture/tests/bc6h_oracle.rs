//! Khronos Data Format Specification 1.3 section 20.2, independently checked
//! against bcdec v0.985. The fixture preserves UnityPy's actual exceptions.
use serde_json::Value as J;
use sha2::{Digest, Sha256};
use unityrs_core::limits::Meter;
use unityrs_texture::{Format, HdrError, decode, decode_bc6h_block, decode_bc6h_signed};

fn unhex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn hash(data: &[u8]) -> String {
    use std::fmt::Write;
    Sha256::digest(data)
        .iter()
        .fold(String::new(), |mut out, b| {
            write!(out, "{b:02x}").unwrap();
            out
        })
}

#[test]
fn bc6h_specification_worked_values_and_reserved_modes() {
    let bytes = (3u128 | 400 << 5 | 488 << 15 | 500 << 25).to_le_bytes();
    assert_eq!(
        decode_bc6h_block(&bytes, false).unwrap(),
        [[0x307f, 0x3b27, 0x3c9b]; 16]
    );
    for mode in [19u8, 23, 27, 31] {
        let mut data = [0; 16];
        data[0] = mode;
        for signed in [false, true] {
            assert_eq!(
                decode_bc6h_block(&data, signed),
                Err(HdrError::ReservedMode)
            );
        }
    }
}

#[test]
fn bc6h_signed_preserves_negative_zero_and_the_full_precision_negative_infinity() {
    // Mode 15: sixteen-bit bases (-32768, -1, 32767), zero deltas,
    // zero indices. The spec's final 31/32 scaling preserves the sign of -1
    // even when its magnitude becomes zero. bcdec instead returns +0 there.
    let bytes: [u8; 16] = unhex("0f80ffff87007ef00100000000000000")
        .try_into()
        .unwrap();
    assert_eq!(
        decode_bc6h_block(&bytes, true).unwrap(),
        [[0xfc00, 0x8000, 0x7bff]; 16]
    );
}

#[test]
fn bc6h_signed_and_unsigned_raw_half_oracle() {
    let doc: J = serde_json::from_str(include_str!("fixtures/bc6h.json")).unwrap();
    let cases = doc["blocks"].as_array().unwrap();
    assert_eq!(cases.len(), 1309);
    assert_eq!(doc["bcdec_negative_zero_differences"], 99);
    assert_eq!(doc["unitypy"], "1.25.2");
    let mut modes = std::collections::BTreeSet::new();
    let mut partitions = std::collections::BTreeSet::new();
    for c in cases {
        let data: [u8; 16] = unhex(c["data"].as_str().unwrap()).try_into().unwrap();
        modes.insert(c["mode"].as_u64().unwrap());
        if let Some(p) = c["partition"].as_u64() {
            partitions.insert(p);
        }
        for signed in [false, true] {
            let key = if signed { "signed" } else { "unsigned" };
            let got = decode_bc6h_block(&data, signed).unwrap();
            let bytes: Vec<u8> = got.iter().flatten().flat_map(|h| h.to_le_bytes()).collect();
            assert_eq!(hash(&bytes), c[key], "{} {key}", c["id"]);
        }
    }
    assert_eq!(modes.len(), 14);
    assert_eq!(partitions.len(), 32);
}

#[test]
fn bc6h_cropped_flipped_images_match_reference_and_unitypy_still_raises() {
    let doc: J = serde_json::from_str(include_str!("fixtures/bc6h.json")).unwrap();
    assert_eq!(doc["cases"].as_array().unwrap().len(), 13);
    for c in doc["cases"].as_array().unwrap() {
        let data = unhex(c["data"].as_str().unwrap());
        let (w, h) = (
            i32::try_from(c["width"].as_i64().unwrap()).unwrap(),
            i32::try_from(c["height"].as_i64().unwrap()).unwrap(),
        );
        let image = decode(Format::Bc6h, &data, w, h, &Meter::unlimited()).unwrap();
        assert_eq!(image.mode().name(), "RGB");
        assert_eq!(hash(image.pixels()), c["reference_sha256"], "{}", c["id"]);
        let signed = decode_bc6h_signed(&data, w, h, &Meter::unlimited()).unwrap();
        assert_eq!(signed.mode().name(), "RGB");
        assert_eq!(
            hash(signed.pixels()),
            c["signed_reference_sha256"],
            "{} signed",
            c["id"]
        );
        assert_eq!(c["raises"], "ValueError");
        assert_eq!(c["message"], "bad image mode");
    }
}
