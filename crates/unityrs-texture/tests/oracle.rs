//! Replays the oracle's fixtures (`tests/fixtures/`, written by
//! `tools/fixtures/texture.py` from UnityPy 1.25.2 and Pillow 9.5.0)
//! without Python: every decode case's image, bit for bit, or its error, and
//! every selection.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value as J;
use sha2::{Digest, Sha256};
use unityrs_core::limits::Meter;
use unityrs_testkit::Rng;
use unityrs_texture::{
    DecodeError, Format, Selection, TEXTURE_FORMATS, TextureFields, XBOX_SWAP_FORMATS, decode,
    plan, select,
};

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

#[test]
fn every_decode_is_the_oracle_image_or_error() {
    let doc = load("decode.json");
    assert_eq!(doc["unitypy"], "1.25.2");
    assert_eq!(doc["pillow"], "9.5.0");
    let cases = doc["cases"].as_array().unwrap();
    assert!(cases.len() >= 400, "{} cases", cases.len());
    let mut images = 0;
    let mut errors = 0;
    let mut bc7_modes = BTreeSet::new();
    for c in cases {
        let id = c["id"].as_str().unwrap();
        let format = Format::from_texture_format(int(&c["format"])).unwrap();
        let (width, height) = (int(&c["width"]), int(&c["height"]));
        let data = build(&c["data"]);
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
            let p = plan(format, width, height).unwrap();
            assert_eq!(p.output_bytes, u64::try_from(image.pixels().len()).unwrap());
            assert!(image.charge().bytes() >= p.charge(), "{id}");
            assert_eq!(meter.used(), image.charge().bytes(), "{id}");
            drop(image);
            assert_eq!(meter.used(), 0, "{id}");
        } else {
            errors += 1;
            let message = c["message"].as_str().unwrap();
            assert_eq!(c["raises"], "ValueError", "{id}");
            let err = got.err().unwrap_or_else(|| panic!("{id}: decoded"));
            match message {
                "Texture2D has no image data" => assert_eq!(err, DecodeError::NoImageData, "{id}"),
                "not enough image data" => {
                    let needed = plan(format, width, height).unwrap().input_bytes;
                    assert_eq!(
                        err,
                        DecodeError::NotEnoughData {
                            needed,
                            got: u64::try_from(data.len()).unwrap()
                        },
                        "{id}"
                    );
                }
                m if NEGATIVE.contains(&m) => {
                    assert_eq!(err, DecodeError::NegativeSize { width, height }, "{id}");
                }
                m => panic!("{id}: an error the decoders do not name: {m}"),
            }
            assert_eq!(meter.used(), 0, "{id}");
        }
    }
    assert!(
        images >= 300 && errors >= 100,
        "{images} images, {errors} errors"
    );
    assert_eq!(bc7_modes, (0..=8).collect::<BTreeSet<_>>());
}

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
    assert!(cases.len() >= 700, "{} cases", cases.len());
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
        seen.insert(got.split('(').next().unwrap().to_owned());
    }
    let want: BTreeSet<String> = [
        "empty_image",
        "export",
        "unsupported_format",
        "unsupported_platform",
    ]
    .map(str::to_owned)
    .into();
    assert_eq!(seen, want);
}
