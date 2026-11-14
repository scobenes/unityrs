//! Replays `tests/fixtures/raw.json` (written by `tools/fixtures/texture.py`
//! from `tools/fixtures/texture_raw.py`'s cases, with UnityPy 1.25.2 and
//! Pillow 9.5.0) without Python: every case of the eleven formats UnityPy
//! decodes with Pillow besides the five of version 1 (Alpha8, ARGB4444,
//! ARGB32, RGB565, BGR24, DXT3, RGBA4444, BGRA32, BC4, BC5 and R8), its
//! image bit for bit, or its exception, by type and message, with the plan
//! giving the decode's answer and its charge.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value as J;
use sha2::{Digest, Sha256};
use unityrs_core::limits::Meter;
use unityrs_testkit::Rng;
use unityrs_texture::{
    DecodeError, Format, MAX_CROP_PIXELS, Selection, Skip, TextureFields, decode, plan, select,
};

/// The cases `texture_raw.py` lists, and how many are images.
const CASES: usize = 743;
const IMAGES: usize = 448;

/// The eleven formats, each with the bytes Pillow's raw decoder reads a
/// pixel (its raw mode's bits / 8), or `None` for a `bcn` format.
const FORMATS: [(Format, Option<u64>); 11] = [
    (Format::Alpha8, Some(1)),
    (Format::Argb4444, Some(2)),
    (Format::Argb32, Some(4)),
    (Format::Rgb565, Some(2)),
    (Format::Bgr24, Some(3)),
    (Format::Dxt3, None),
    (Format::Rgba4444, Some(2)),
    (Format::Bgra32, Some(4)),
    (Format::Bc4, None),
    (Format::Bc5, None),
    (Format::R8, Some(1)),
];

fn load() -> J {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/raw.json");
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

/// A case's data (`texture.py`'s `build`): hex, zeros, a ramp (the values
/// 0, 1, 2, ... each `ramp` bytes little-endian), or the first `len` bytes
/// of `SplitMix64` from `seed`, each output little-endian.
fn build(spec: &J) -> Vec<u8> {
    if let Some(h) = spec["hex"].as_str() {
        return unhex(h);
    }
    if let Some(n) = spec["zeros"].as_u64() {
        return vec![0; usize::try_from(n).unwrap()];
    }
    let len = usize::try_from(spec["len"].as_u64().unwrap()).unwrap();
    let mut out = Vec::with_capacity(len + 8);
    if let Some(width) = spec["ramp"].as_u64() {
        let width = usize::try_from(width).unwrap();
        let mut i = 0u64;
        while out.len() < len {
            out.extend_from_slice(&i.to_le_bytes()[..width]);
            i += 1;
        }
    } else {
        let mut rng = Rng::new(spec["seed"].as_u64().unwrap());
        while out.len() < len {
            out.extend_from_slice(&rng.next_u64().to_le_bytes());
        }
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

/// The negative-size messages Pillow raises, whichever of its checks meets
/// the size first.
const NEGATIVE: [&str; 3] = [
    "Width and height must be >= 0",
    "Coordinate 'right' is less than 'left'",
    "tile cannot extend outside image",
];

/// The bytes Pillow's raw decoder reads a pixel of `format`, if it is raw.
fn raw_bytes(format: Format) -> Option<u64> {
    FORMATS.iter().find(|(f, _)| *f == format).unwrap().1
}

/// The bytes mip 0 holds at its padded size, both sides positive.
fn needed(format: Format, width: i32, height: i32) -> u64 {
    let (w, h) = (
        u64::try_from(width).unwrap(),
        u64::try_from(height).unwrap(),
    );
    match raw_bytes(format) {
        Some(bytes) => w * h * bytes,
        None => w.div_ceil(4) * h.div_ceil(4) * format.block_bytes().unwrap(),
    }
}

/// The error the crate names for the exception the oracle recorded.
fn named(c: &J, format: Format, width: i32, height: i32, len: usize) -> DecodeError {
    let id = c["id"].as_str().unwrap();
    match (
        c["raises"].as_str().unwrap(),
        c["message"].as_str().unwrap(),
    ) {
        ("ValueError", "Texture2D has no image data") => DecodeError::NoImageData,
        ("ValueError", "not enough image data") => DecodeError::NotEnoughData {
            needed: needed(format, width, height),
            got: u64::try_from(len).unwrap(),
        },
        ("ValueError", m) if NEGATIVE.contains(&m) => DecodeError::NegativeSize { width, height },
        // Pillow's raw decoder's width cap, `INT_MAX / bits - 7`:
        // `PyErr_NoMemory`, no message.
        ("MemoryError", "") => {
            let bits = u32::try_from(raw_bytes(format).unwrap() * 8).unwrap();
            DecodeError::PillowMemoryError {
                width: width.unsigned_abs(),
                max: 0x7FFF_FFFF / bits - 7,
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
fn every_raw_and_bcn_decode_is_the_oracle_image_or_error() {
    let doc = load();
    assert_eq!(doc["unitypy"], "1.25.2");
    assert_eq!(doc["pillow"], "9.5.0");
    let cases = doc["cases"].as_array().unwrap();
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
        // lives.
        let p = planned.unwrap_or_else(|e| panic!("{id}: planned {e}"));
        if width != 0 && height != 0 {
            assert_eq!(p.input_bytes, needed(format, width, height), "{id}");
        }
        assert_eq!(p.output_bytes, u64::try_from(image.pixels().len()).unwrap());
        assert_eq!(image.charge().bytes(), p.charge(), "{id}");
        assert_eq!(meter.used(), image.charge().bytes(), "{id}");
        drop(image);
        assert_eq!(meter.used(), 0, "{id}");
    }
    assert_eq!(images, IMAGES);
    let all: BTreeSet<&str> = FORMATS.iter().map(|(f, _)| f.name()).collect();
    assert_eq!(formats, all);
    let kinds: BTreeSet<String> = ["DecompressionBombError", "MemoryError", "ValueError"]
        .map(str::to_owned)
        .into();
    assert_eq!(raises, kinds);
}

/// Every format the crate decodes is one of the five, the eleven here, an
/// ETC or EAC format (`tests/etc_oracle.rs`) or an ASTC LDR format
/// (`tests/astc_oracle.rs`): the oracle's four fixtures together replay
/// every one.
#[test]
fn every_format_is_replayed_by_an_oracle_fixture() {
    let ours: BTreeSet<&str> = FORMATS.iter().map(|(f, _)| f.name()).collect();
    for format in Format::ALL {
        let name = format.name();
        assert!(
            format.exported()
                || ours.contains(name)
                || name.starts_with("ETC")
                || name.starts_with("EAC")
                || name.starts_with("ASTC"),
            "{name}"
        );
    }
    assert_eq!(Format::ALL.len(), 5 + 11 + 8 + 12);
}
