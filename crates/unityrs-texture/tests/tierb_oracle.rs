//! UnityPy's actual outcomes beside the independent references recorded by
//! `texture_tierb.py`. No oracle exception is replaced by a corrected run.
use std::collections::BTreeSet;

use serde_json::Value as J;
use sha2::{Digest, Sha256};
use unityrs_core::limits::Meter;
use unityrs_testkit::Rng;
use unityrs_texture::{
    DecodeError, Format, Formats, Selection, Skip, TextureFields, decode, plan, select_in,
};

fn load() -> J {
    serde_json::from_str(include_str!("fixtures/tierb.json")).unwrap()
}

fn unhex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").unwrap();
        out
    })
}

fn build(spec: &J) -> Vec<u8> {
    if let Some(s) = spec["hex"].as_str() {
        return unhex(s);
    }
    if let Some(n) = spec["zeros"].as_u64() {
        return vec![0; usize::try_from(n).unwrap()];
    }
    let n = usize::try_from(spec["len"].as_u64().unwrap()).unwrap();
    let mut out = Vec::new();
    if let Some(size) = spec["ramp"].as_u64() {
        let size = usize::try_from(size).unwrap();
        for i in 0..n.div_ceil(size) {
            out.extend_from_slice(&u64::try_from(i).unwrap().to_le_bytes()[..size]);
        }
    } else {
        let mut rng = Rng::new(spec["seed"].as_u64().unwrap());
        while out.len() < n {
            out.extend_from_slice(&rng.next_u64().to_le_bytes());
        }
    }
    out.truncate(n);
    out
}

fn number(c: &J, key: &str) -> i32 {
    i32::try_from(c[key].as_i64().unwrap()).unwrap()
}

fn format(c: &J) -> Format {
    if c["format"] == 6 {
        Format::ArgbFloat
    } else {
        Format::from_texture_format(number(c, "format")).unwrap()
    }
}

fn check_reference(c: &J) {
    let data = build(&c["data"]);
    let id = c["id"].as_str().unwrap();
    assert_eq!(hex(&Sha256::digest(&data)), c["data_sha256"], "{id}");
    let format = format(c);
    let (width, height) = (number(c, "width"), number(c, "height"));
    let meter = Meter::unlimited();
    let planned = plan(format, data.len(), width, height);
    let got = decode(format, &data, width, height, &meter);
    let reference = &c["reference"];
    if let Some(error) = reference["raises"].as_str() {
        let expected = match error {
            "Unsupported" => DecodeError::Unsupported { format, width },
            "NoImageData" => DecodeError::NoImageData,
            "NegativeSize" => DecodeError::NegativeSize { width, height },
            "NotEnoughData" => DecodeError::NotEnoughData {
                needed: reference["needed"].as_u64().unwrap(),
                got: u64::try_from(data.len()).unwrap(),
            },
            other => panic!("{id}: unknown reference error {other}"),
        };
        assert_eq!(planned.err(), Some(expected), "{id}");
        assert_eq!(
            got.err(),
            plan(format, data.len(), width, height).err(),
            "{id}"
        );
    } else {
        let image = got.unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(image.mode().name(), reference["mode"], "{id}");
        assert_eq!(
            serde_json::json!([image.width(), image.height()]),
            reference["size"],
            "{id}"
        );
        assert_eq!(
            hex(&Sha256::digest(image.pixels())),
            reference["sha256"],
            "{id}"
        );
        if let Some(pixels) = reference["pixels"].as_str() {
            assert_eq!(hex(image.pixels()), pixels, "{id}");
        }
        let p = planned.unwrap();
        assert_eq!(u64::try_from(image.pixels().len()).unwrap(), p.output_bytes);
        assert_eq!(meter.used(), p.charge());
        assert_eq!(p.scratch, 0);
        assert_eq!(image.charge().bytes(), p.charge());
    }
    assert_eq!(meter.used(), 0, "{id}");
}

#[test]
fn every_tier_b_and_c_decode_is_the_closed_form_beside_unitypys_outcome() {
    let doc = load();
    assert_eq!(doc["unitypy"], "1.25.2");
    assert_eq!(doc["pillow"], "9.5.0");
    let cases = doc["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 607);
    assert_eq!(
        cases
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect::<BTreeSet<_>>()
            .len(),
        cases.len()
    );
    assert_eq!(cases.iter().filter(|c| c["mode"].is_string()).count(), 113);
    assert_eq!(
        cases
            .iter()
            .filter(|c| c["reference"]["mode"].is_string())
            .count(),
        470
    );
    let mut formats = BTreeSet::new();
    for c in cases {
        formats.insert(number(c, "format"));
        check_reference(c);
    }
    assert_eq!(formats.len(), 23); // twenty-two decoders and ARGBFloat's refusal
}

/// The exact oracle exception class and message on the matrix's 0x30 input.
/// C rows instead pin a finite channel case's complete, wrong image below.
fn divergence(name: &str) {
    let doc = load();
    let suffix = if matches!(name, "RGBAHalf" | "RGB48") {
        "channels"
    } else {
        "matrix"
    };
    let id = format!("{name}-{suffix}");
    let c = doc["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap();
    check_reference(c);
    let (kind, message) = match name {
        "RHalf" => ("error", "unpack requires a buffer of 2 bytes"),
        "RGHalf" | "RGFloat" | "RG16" | "RG32" | "RG16_SIGNED" | "RG32_SIGNED" => {
            ("KeyError", "'raw'")
        }
        "R16_SIGNED" | "R8_SIGNED" => ("ValueError", "unrecognized image mode"),
        "RGB9e5Float" => (
            "TypeError",
            "'float' object cannot be interpreted as an integer",
        ),
        "YUY2" => (
            "NotImplementedError",
            "Not implemented texture format: YUY2",
        ),
        "RGBFloat" => (
            "NotImplementedError",
            "Not implemented texture format: RGBFloat",
        ),
        "RGBAHalf" => {
            assert_eq!(c["mode"], "RGB");
            assert_eq!(c["pixels"], "20c0804020c0804020c08040");
            assert_eq!(c["reference"]["mode"], "RGBA");
            assert_eq!(c["reference"]["pixels"], "804020bf804020bf804020bf804020bf");
            return;
        }
        "RGB48" => {
            assert_eq!(c["mode"], "RGB");
            assert_eq!(c["pixels"], "000000ffffffffffff000000");
            assert_eq!(c["reference"]["pixels"], "ff0000ff0000ff0000ff0000");
            return;
        }
        _ => ("ValueError", "unknown raw mode for given image mode"),
    };
    assert_eq!(c["raises"], kind, "{name}");
    assert_eq!(c["message"], message, "{name}");
    assert!(
        c["reference"]["mode"].is_string(),
        "{name}: reference must decode"
    );
}

#[test]
fn rhalf() {
    divergence("RHalf");
}

#[test]
fn rghalf() {
    divergence("RGHalf");
}

#[test]
fn rfloat() {
    divergence("RFloat");
}

#[test]
fn rgfloat() {
    divergence("RGFloat");
}

#[test]
fn rgbfloat() {
    divergence("RGBFloat");
}

#[test]
fn rgbafloat() {
    divergence("RGBAFloat");
}

#[test]
fn rg16() {
    divergence("RG16");
}

#[test]
fn r16() {
    divergence("R16");
}

#[test]
fn rg32() {
    divergence("RG32");
}

#[test]
fn rgba64() {
    divergence("RGBA64");
}

#[test]
fn r8_signed() {
    divergence("R8_SIGNED");
}

#[test]
fn rg16_signed() {
    divergence("RG16_SIGNED");
}

#[test]
fn rgb24_signed() {
    divergence("RGB24_SIGNED");
}

#[test]
fn rgba32_signed() {
    divergence("RGBA32_SIGNED");
}

#[test]
fn r16_signed() {
    divergence("R16_SIGNED");
}

#[test]
fn rg32_signed() {
    divergence("RG32_SIGNED");
}

#[test]
fn rgb48_signed() {
    divergence("RGB48_SIGNED");
}

#[test]
fn rgba64_signed() {
    divergence("RGBA64_SIGNED");
}

#[test]
fn rgb9e5float() {
    divergence("RGB9e5Float");
}

#[test]
fn yuy2() {
    divergence("YUY2");
}

#[test]
fn rgb48() {
    divergence("RGB48");
}

#[test]
fn rgbahalf() {
    divergence("RGBAHalf");
}

#[test]
fn tier_u_is_refused_before_data_but_zero_sides_keep_the_empty_image() {
    assert_eq!(Formats::decoded("ARGBFloat"), Err(true));
    assert!(!Formats::DECODED.contains(Format::ArgbFloat));
    for (format, width) in [
        (Format::ArgbFloat, 2),
        (Format::Yuy2, 1),
        (Format::Yuy2, -3),
    ] {
        for height in [1, -2] {
            let fields = TextureFields {
                texture_format: format.texture_format(),
                width,
                height,
                platform: 19,
                platform_blob: None,
            };
            assert_eq!(
                select_in(&fields, Formats::DECODED, (2021, 3)),
                Selection::Skip(Skip::UnsupportedFormat(format.texture_format()))
            );
            for len in [0, 1, 64] {
                assert_eq!(
                    plan(format, len, width, height),
                    Err(DecodeError::Unsupported { format, width })
                );
            }
        }
    }
    for (format, width) in [
        (Format::ArgbFloat, 0),
        (Format::ArgbFloat, 1),
        (Format::Yuy2, 1),
    ] {
        assert_eq!(plan(format, 0, width, 0).unwrap().output_bytes, 0);
        let fields = TextureFields {
            texture_format: format.texture_format(),
            width,
            height: 0,
            platform: 19,
            platform_blob: None,
        };
        assert_eq!(
            select_in(&fields, Formats::DECODED, (2021, 3)),
            Selection::Skip(Skip::EmptyImage)
        );
        assert_eq!(
            unityrs_texture::select(&fields),
            Selection::Skip(Skip::EmptyImage)
        );
    }
    let fields = TextureFields {
        texture_format: 21,
        width: 2,
        height: 1,
        platform: 19,
        platform_blob: None,
    };
    assert_eq!(
        select_in(&fields, Formats::DECODED, (2021, 3)),
        Selection::Export(Format::Yuy2)
    );
}
