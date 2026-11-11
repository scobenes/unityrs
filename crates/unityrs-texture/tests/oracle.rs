//! Replays the oracle's fixtures (`tests/fixtures/`, written by
//! `tools/fixtures/texture.py` from UnityPy 1.25.2 and Pillow 9.5.0)
//! without Python: every decode case's image, bit for bit, or its
//! exception, by type and message, and every selection.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value as J;
use sha2::{Digest, Sha256};
use unityrs_core::limits::Meter;
use unityrs_testkit::Rng;
use unityrs_texture::{
    DecodeError, Format, MAX_CROP_PIXELS, Selection, TEXTURE_FORMATS, TextureFields,
    XBOX_SWAP_FORMATS, decode, plan, select,
};

/// The decode cases `texture.py` records, and how many are images.
const DECODE_CASES: usize = 523;
const IMAGES: usize = 317;
/// The selection cases.
const SELECT_CASES: usize = 723;

fn load(name: &str) -> J {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
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

/// The first `len` bytes of `SplitMix64` from `seed`, each output
/// little-endian, as `texture.py`'s `splitmix` makes them.
fn splitmix(seed: u64, len: usize) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        out.extend_from_slice(&rng.next_u64().to_le_bytes());
    }
    out.truncate(len);
    out
}

/// A case's data (`texture.py`'s `build`).
fn build(spec: &J) -> Vec<u8> {
    if let Some(h) = spec["hex"].as_str() {
        return unhex(h);
    }
    if let Some(n) = spec["zeros"].as_u64() {
        return vec![0; usize::try_from(n).unwrap()];
    }
    let len = usize::try_from(spec["len"].as_u64().unwrap()).unwrap();
    let mut data = splitmix(spec["seed"].as_u64().unwrap(), len);
    if let Some(mode) = spec["bc7_mode"].as_u64() {
        let full = data.len() - data.len() % 16;
        for b in data[..full].iter_mut().step_by(16) {
            *b = if mode == 8 {
                0
            } else {
                let low = (2u16 << mode) - 1;
                (*b & !u8::try_from(low).unwrap()) | (1 << mode)
            };
        }
    }
    data
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

/// The bytes mip 0 of a texture holds at its padded size (`texture.py`'s
/// `needed`), both sides positive.
fn needed(format: Format, width: i32, height: i32) -> u64 {
    let (w, h) = (
        u64::try_from(width).unwrap(),
        u64::try_from(height).unwrap(),
    );
    match format.block_bytes() {
        Some(block) => w.div_ceil(4) * h.div_ceil(4) * block,
        None => w * h * format.mode().bytes_per_pixel(),
    }
}

/// The error the crate names for the exception the oracle recorded.
fn named(c: &J, format: Format, width: i32, height: i32, len: usize) -> DecodeError {
    let id = c["id"].as_str().unwrap();
    let message = c["message"].as_str().unwrap();
    match (c["raises"].as_str().unwrap(), message) {
        ("ValueError", "Texture2D has no image data") => DecodeError::NoImageData,
        ("ValueError", "not enough image data") => DecodeError::NotEnoughData {
            needed: needed(format, width, height),
            got: u64::try_from(len).unwrap(),
        },
        ("ValueError", m) if NEGATIVE.contains(&m) => DecodeError::NegativeSize { width, height },
        // Pillow's raw decoder's width cap: `PyErr_NoMemory`, no message.
        ("MemoryError", "") => DecodeError::PillowMemoryError {
            width: width.unsigned_abs(),
            max: match format {
                Format::Rgba32 => 67_108_856,
                Format::Rgb24 => 89_478_478,
                _ => panic!("{id}: a block format's MemoryError"),
            },
        },
        ("DecompressionBombError", _) => DecodeError::DecompressionBomb {
            pixels: u64::try_from(width).unwrap() * u64::try_from(height).unwrap(),
            max: MAX_CROP_PIXELS,
        },
        (t, m) => panic!("{id}: an exception the decoders do not name: {t}: {m}"),
    }
}

#[test]
fn every_decode_is_the_oracle_image_or_error() {
    let doc = load("decode.json");
    assert_eq!(doc["unitypy"], "1.25.2");
    assert_eq!(doc["pillow"], "9.5.0");
    let cases = doc["cases"].as_array().unwrap();
    assert_eq!(cases.len(), DECODE_CASES);
    let mut images = 0;
    let mut errors = 0;
    let mut bc7_modes = BTreeSet::new();
    for c in cases {
        let id = c["id"].as_str().unwrap();
        let format = Format::from_texture_format(int(&c["format"])).unwrap();
        let (width, height) = (int(&c["width"]), int(&c["height"]));
        let data = build(&c["data"]);
        // The input is the one the oracle decoded.
        assert_eq!(
            hex(&Sha256::digest(&data)),
            c["data_sha256"].as_str().unwrap(),
            "{id}"
        );
        if let Some(mode) = c["data"]["bc7_mode"].as_u64() {
            bc7_modes.insert(mode);
        }
        let platform_blob = blob(c);
        // Every case is one the selection decodes, or an empty image.
        let selected = select(&TextureFields {
            texture_format: format.texture_format(),
            width,
            height,
            platform: int(&c["platform"]),
            platform_blob: platform_blob.as_deref(),
        });
        if width != 0 && height != 0 {
            assert_eq!(selected, Selection::Export(format), "{id}");
        }
        let meter = Meter::unlimited();
        let got = decode(format, &data, width, height, &meter);
        let planned = plan(format, data.len(), width, height);
        if let Some(mode) = c["mode"].as_str() {
            images += 1;
            let image = got.unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(image.mode().name(), mode, "{id}");
            let size = [image.width(), image.height()];
            let want: Vec<u32> = c["size"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| u32::try_from(v.as_u64().unwrap()).unwrap())
                .collect();
            assert_eq!(size.as_slice(), want.as_slice(), "{id}");
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
            assert_eq!(p.output_bytes, u64::try_from(image.pixels().len()).unwrap());
            assert_eq!(image.charge().bytes(), p.charge(), "{id}");
            assert_eq!(meter.used(), image.charge().bytes(), "{id}");
            drop(image);
            assert_eq!(meter.used(), 0, "{id}");
        } else {
            errors += 1;
            let err = got.err().unwrap_or_else(|| panic!("{id}: decoded"));
            assert_eq!(err, named(c, format, width, height, data.len()), "{id}");
            // Where the crate gives Pillow's message, it is Pillow's.
            if c["raises"] == "DecompressionBombError" {
                assert_eq!(err.to_string(), c["message"].as_str().unwrap(), "{id}");
            }
            // The plan names it too, before any data is read.
            assert_eq!(planned.err(), Some(err), "{id}");
            assert_eq!(meter.used(), 0, "{id}");
        }
    }
    assert_eq!((images, errors), (IMAGES, DECODE_CASES - IMAGES));
    assert_eq!(bc7_modes, (0..=8).collect::<BTreeSet<_>>());
}

/// The selection the oracle recorded. Its predicates are UnityPy's own,
/// measured (`TextureFormat`'s names, `is_switch_swizzled`,
/// `XBOX_SWAP_FORMATS`); the order between the skips is the recorder's rule
/// (`texture.py`'s `selected`, `docs/BATCH_SPEC.md` section 3), not a
/// measurement, so for it this test checks only that the crate keeps the
/// rule.
#[test]
fn every_selection_is_the_oracle_one() {
    let doc = load("select.json");
    let names: Vec<(i32, String)> = doc["names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| (int(&n["value"]), n["name"].as_str().unwrap().to_owned()))
        .collect();
    let ours: Vec<(i32, String)> = TEXTURE_FORMATS
        .iter()
        .map(|(v, n)| (*v, (*n).to_owned()))
        .collect();
    assert_eq!(ours, names);
    let xbox: Vec<i32> = doc["xbox_swap_formats"]
        .as_array()
        .unwrap()
        .iter()
        .map(int)
        .collect();
    assert_eq!(xbox, XBOX_SWAP_FORMATS);
    let cases = doc["cases"].as_array().unwrap();
    assert_eq!(cases.len(), SELECT_CASES);
    let mut seen = BTreeSet::new();
    for c in cases {
        let platform_blob = blob(c);
        let fields = TextureFields {
            texture_format: int(&c["format"]),
            width: int(&c["width"]),
            height: int(&c["height"]),
            platform: int(&c["platform"]),
            platform_blob: platform_blob.as_deref(),
        };
        let got = match select(&fields) {
            Selection::Export(f) => format!("export({})", f.name()),
            Selection::Skip(s) => s.to_string(),
        };
        assert_eq!(got, c["selected"].as_str().unwrap(), "{c}");
        // Each kind of answer, and each platform's skip.
        seen.insert(if got.starts_with("unsupported_platform") {
            got
        } else {
            got.split('(').next().unwrap().to_owned()
        });
    }
    let want: BTreeSet<String> = [
        "empty_image",
        "export",
        "unsupported_format",
        "unsupported_platform(switch)",
        "unsupported_platform(xbox360)",
    ]
    .map(str::to_owned)
    .into();
    assert_eq!(seen, want);
}
