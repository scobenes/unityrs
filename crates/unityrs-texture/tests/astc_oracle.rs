//! Replays `tests/fixtures/astc.json`'s `format_cases` (written by
//! `tools/fixtures/texture_astc.py` with UnityPy 1.25.2, astc-encoder-py
//! 0.1.12 and Pillow 9.5.0) without Python: every case of the twelve ASTC
//! LDR formats, its image bit for bit, or its exception, by type and
//! message, with the plan giving the decode's answer and its charge. The
//! fixture's `footprint_cases`, astcenc's fourteen footprints through
//! `Texture2DConverter.astc` alone, are the crate's unit test
//! (`src/astc/oracle_tests.rs`).

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value as J;
use sha2::{Digest, Sha256};
use unityrs_core::limits::Meter;
use unityrs_testkit::Rng;
use unityrs_texture::{
    DecodeError, Format, MAX_CROP_PIXELS, Selection, Skip, TextureFields, decode, plan, select,
};

/// The cases `texture_astc.py` lists, and how many are images.
const CASES: usize = 461;
const IMAGES: usize = 180;

/// The twelve formats.
const FORMATS: [Format; 12] = [
    Format::AstcRgb4x4,
    Format::AstcRgb5x5,
    Format::AstcRgb6x6,
    Format::AstcRgb8x8,
    Format::AstcRgb10x10,
    Format::AstcRgb12x12,
    Format::AstcRgba4x4,
    Format::AstcRgba5x5,
    Format::AstcRgba6x6,
    Format::AstcRgba8x8,
    Format::AstcRgba10x10,
    Format::AstcRgba12x12,
];

fn load() -> J {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/astc.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        write!(s, "{b:02x}").unwrap();
        s
    })
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn int(v: &J) -> i32 {
    i32::try_from(v.as_i64().unwrap()).unwrap()
}

/// A case's data (`texture.py`'s `build`): hex, zeros, or the first `len`
/// bytes of `SplitMix64` from `seed`, each output little-endian.
fn build(spec: &J) -> Vec<u8> {
    if let Some(h) = spec["hex"].as_str() {
        return unhex(h);
    }
    if let Some(n) = spec["zeros"].as_u64() {
        return vec![0; usize::try_from(n).unwrap()];
    }
    let len = usize::try_from(spec["len"].as_u64().unwrap()).unwrap();
    let mut rng = Rng::new(spec["seed"].as_u64().unwrap());
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        out.extend_from_slice(&rng.next_u64().to_le_bytes());
    }
    out.truncate(len);
    out
}

fn blob(c: &J) -> Option<Vec<u8>> {
    c["blob"].as_array().map(|b| {
        b.iter()
            .map(|v| u8::try_from(v.as_u64().unwrap()).unwrap())
            .collect()
    })
}

/// A side padded to whole blocks of `block`, as UnityPy pads it.
fn padded(side: i32, block: i32) -> i64 {
    let side = i64::from(side);
    let block = i64::from(block);
    side + (-side).rem_euclid(block)
}

/// The bytes mip 0 holds at its padded size, both sides positive.
fn needed(format: Format, width: i32, height: i32) -> u64 {
    let (bw, bh) = format.block_size().unwrap();
    let blocks = |side: i32, b: u8| u64::try_from(side).unwrap().div_ceil(u64::from(b));
    blocks(width, bw) * blocks(height, bh) * 16
}

/// What UnityPy raises for a negative side, by the check that meets it
/// first: Pillow's, on a side padding made 0 or the crop; astc-encoder-py's
/// size check, on a negative size; or the data check on a negative block
/// count's product, the two sides' both below 0. unityrs names each
/// [`DecodeError::NegativeSize`] (`PARITY.md` section 19.3).
fn negative(raises: &str, message: &str) -> bool {
    match raises {
        "ValueError" => {
            message == "tile cannot extend outside image"
                || message.starts_with("Invalid ASTC data size: ")
        }
        "ASTCError" => {
            message.starts_with("Compressed data size does not match the image dimensions.")
        }
        _ => false,
    }
}

/// The error the crate names for the exception the oracle recorded.
fn named(c: &J, format: Format, width: i32, height: i32, len: usize) -> DecodeError {
    let id = c["id"].as_str().unwrap();
    let (raises, message) = (
        c["raises"].as_str().unwrap(),
        c["message"].as_str().unwrap(),
    );
    if (width < 0 || height < 0) && message != "Texture2D has no image data" {
        assert!(negative(raises, message), "{id}: {raises}: {message}");
        return DecodeError::NegativeSize { width, height };
    }
    match (raises, message) {
        ("ValueError", "Texture2D has no image data") => DecodeError::NoImageData,
        ("ValueError", m) if m.starts_with("Invalid ASTC data size: ") => {
            let needed = needed(format, width, height);
            assert_eq!(
                m,
                format!("Invalid ASTC data size: {len} < {needed}"),
                "{id}"
            );
            DecodeError::NotEnoughData {
                needed,
                got: u64::try_from(len).unwrap(),
            }
        }
        // Pillow's raw decoder on astcenc's output at the padded width:
        // `PyErr_NoMemory`, no message.
        ("MemoryError", "") => {
            let (bw, _) = format.block_size().unwrap();
            DecodeError::PillowMemoryError {
                width: u32::try_from(padded(width, i32::from(bw))).unwrap(),
                max: 67_108_856,
            }
        }
        ("DecompressionBombError", _) => DecodeError::DecompressionBomb {
            pixels: u64::try_from(width).unwrap() * u64::try_from(height).unwrap(),
            max: MAX_CROP_PIXELS,
        },
        (t, m) => panic!("{id}: an exception the decoders do not name: {t}: {m}"),
    }
}

#[test]
fn every_astc_decode_is_the_oracle_image_or_error() {
    let doc = load();
    assert_eq!(doc["unitypy"], "1.25.2");
    assert_eq!(doc["pillow"], "9.5.0");
    assert_eq!(doc["astc_encoder"], "0.1.12");
    let cases = doc["format_cases"].as_array().unwrap();
    assert_eq!(cases.len(), CASES);
    let (mut images, mut formats, mut raises) = (0, BTreeSet::new(), BTreeSet::new());
    for c in cases {
        let id = c["id"].as_str().unwrap();
        let format = Format::from_texture_format(int(&c["format"])).unwrap();
        formats.insert(format.name());
        let (width, height) = (int(&c["width"]), int(&c["height"]));
        let data = build(&c["data"]);
        // The input is the one the oracle decoded.
        assert_eq!(
            hex(&Sha256::digest(&data)),
            c["data_sha256"].as_str().unwrap(),
            "{id}"
        );
        // Version 1 of the batch engine exports none of these formats.
        assert!(!format.exported(), "{id}");
        let platform_blob = blob(c);
        let selected = select(&TextureFields {
            texture_format: format.texture_format(),
            width,
            height,
            platform: int(&c["platform"]),
            platform_blob: platform_blob.as_deref(),
        });
        if width != 0 && height != 0 {
            assert_eq!(
                selected,
                Selection::Skip(Skip::UnsupportedFormat(format.texture_format())),
                "{id}"
            );
        }
        let meter = Meter::unlimited();
        let got = decode(format, &data, width, height, &meter);
        let planned = plan(format, data.len(), width, height);
        let Some(mode) = c["mode"].as_str() else {
            let err = got.err().unwrap_or_else(|| panic!("{id}: decoded"));
            assert_eq!(err, named(c, format, width, height, data.len()), "{id}");
            if c["raises"] == "DecompressionBombError" {
                assert_eq!(err.to_string(), c["message"].as_str().unwrap(), "{id}");
            }
            raises.insert(c["raises"].as_str().unwrap().to_owned());
            // The plan names it too, before any data is read.
            assert_eq!(planned.err(), Some(err), "{id}");
            assert_eq!(meter.used(), 0, "{id}");
            continue;
        };
        images += 1;
        let image = got.unwrap_or_else(|e| panic!("{id}: {e}"));
        assert_eq!(image.mode().name(), mode, "{id}");
        let want: Vec<u32> = c["size"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| u32::try_from(v.as_u64().unwrap()).unwrap())
            .collect();
        assert_eq!([image.width(), image.height()].as_slice(), want, "{id}");
        if let Some(pixels) = c["pixels"].as_str() {
            assert_eq!(hex(image.pixels()), pixels, "{id}");
        }
        assert_eq!(
            hex(&Sha256::digest(image.pixels())),
            c["sha256"].as_str().unwrap(),
            "{id}"
        );
        // What the plan said, and what the meter holds while the image
        // lives: the tables are returned with the decode.
        let p = planned.unwrap_or_else(|e| panic!("{id}: planned {e}"));
        if width != 0 && height != 0 {
            assert_eq!(p.input_bytes, needed(format, width, height), "{id}");
            assert!(p.scratch > 0, "{id}");
        }
        assert_eq!(p.output_bytes, u64::try_from(image.pixels().len()).unwrap());
        assert_eq!(image.charge().bytes(), p.charge(), "{id}");
        assert_eq!(meter.used(), image.charge().bytes(), "{id}");
        drop(image);
        assert_eq!(meter.used(), 0, "{id}");
    }
    assert_eq!(images, IMAGES);
    let all: BTreeSet<&str> = FORMATS.iter().map(|f| f.name()).collect();
    assert_eq!(formats, all);
    let kinds: BTreeSet<String> = [
        "ASTCError",
        "DecompressionBombError",
        "MemoryError",
        "ValueError",
    ]
    .map(str::to_owned)
    .into();
    assert_eq!(raises, kinds);
}

/// The twelve are every format of the crate whose decoder is astcenc.
#[test]
fn the_twelve_are_the_astcenc_formats() {
    let astc: BTreeSet<&str> = Format::ALL
        .iter()
        .filter(|f| f.name().starts_with("ASTC"))
        .map(|f| f.name())
        .collect();
    let ours: BTreeSet<&str> = FORMATS.iter().map(|f| f.name()).collect();
    assert_eq!(astc, ours);
}
