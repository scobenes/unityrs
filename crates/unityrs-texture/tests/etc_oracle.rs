//! Replays `tests/fixtures/etc.json` (written by `tools/fixtures/texture.py`
//! from `tools/fixtures/texture_etc.py`'s cases, with UnityPy 1.25.2,
//! texture2ddecoder 1.0.5 and Pillow 9.5.0) without Python: every ETC and
//! EAC case's image, bit for bit, or its exception, by type and message;
//! and for the formats with a Tier C patch (`PARITY.md` section 19.1), the
//! divergence, pixel by pixel: `ETC2_RGBA1`'s transparent pixels are
//! (0, 0, 0, 0) here, and a signed EAC base of -128 reads as -127.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value as J;
use sha2::{Digest, Sha256};
use unityrs_core::limits::Meter;
use unityrs_testkit::Rng;
use unityrs_texture::{
    DecodeError, Format, MAX_CROP_PIXELS, Selection, Skip, TextureFields, decode, plan, select,
};

/// The cases `texture_etc.py` lists, and how many are images.
const CASES: usize = 706;
const IMAGES: usize = 608;
/// Each Tier C format's images, and how many of them diverge.
const TIER_C: [(Format, usize, usize); 3] = [
    (Format::Etc2Rgba1, 77, 40),
    (Format::EacRSigned, 73, 9),
    (Format::EacRgSigned, 73, 14),
];

fn load() -> J {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/etc.json");
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

/// The error the crate names for the exception the oracle recorded.
fn named(c: &J, width: i32, height: i32) -> DecodeError {
    let id = c["id"].as_str().unwrap();
    match (
        c["raises"].as_str().unwrap(),
        c["message"].as_str().unwrap(),
    ) {
        ("ValueError", "Texture2D has no image data") => DecodeError::NoImageData,
        // Padding made a side 0: Pillow's tile check on texture2ddecoder's
        // empty output.
        ("ValueError", "tile cannot extend outside image") => {
            DecodeError::NegativeSize { width, height }
        }
        // Pillow's raw decoder on texture2ddecoder's output at the padded
        // width: `PyErr_NoMemory`, no message.
        ("MemoryError", "") => DecodeError::PillowMemoryError {
            width: width.unsigned_abs().div_ceil(4) * 4,
            max: 67_108_856,
        },
        ("DecompressionBombError", _) => DecodeError::DecompressionBomb {
            pixels: u64::try_from(width).unwrap() * u64::try_from(height).unwrap(),
            max: MAX_CROP_PIXELS,
        },
        (t, m) => panic!("{id}: an exception the decoders do not name: {t}: {m}"),
    }
}

/// Whether `ours` is the Khronos reference for a pixel UnityPy decodes as
/// `theirs` that it differs from: for `ETC2_RGBA1` a transparent pixel
/// UnityPy keeps the colour of, made (0, 0, 0, 0) (section 22.9); for signed
/// EAC a channel whose block's base of -128 UnityPy computes with, 1 higher
/// as -127 (sections 22.7 and 22.8), the other channels equal.
fn diverges_as_specified(format: Format, ours: [u8; 4], theirs: [u8; 4]) -> bool {
    match format {
        Format::Etc2Rgba1 => theirs[3] == 0 && theirs != [0; 4] && ours == [0; 4],
        Format::EacRSigned | Format::EacRgSigned => {
            let channels = if format == Format::EacRSigned { 1 } else { 2 };
            (0..4).all(|i| {
                ours[i] == theirs[i]
                    || (i < channels && theirs[i] < 255 && ours[i] == theirs[i] + 1)
            })
        }
        _ => false,
    }
}

/// A Tier C format's image: the reference's, and where the oracle recorded
/// the pixels, exactly the pixels it names diverging, each as the
/// specification says. Returns whether it diverges from UnityPy's.
fn tier_c_matches(c: &J, format: Format, pixels: &[u8]) -> bool {
    let id = c["id"].as_str().unwrap();
    let ours = hex(&Sha256::digest(pixels));
    let diverging = usize::try_from(c["diverging"].as_u64().unwrap()).unwrap();
    if diverging > 0 {
        assert_ne!(ours, c["sha256"].as_str().unwrap(), "{id}");
    }
    assert_eq!(ours, c["reference_sha256"].as_str().unwrap(), "{id}");
    if let Some(theirs) = c["pixels"].as_str() {
        let theirs = unhex(theirs);
        let pairs = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .zip(theirs.as_chunks::<4>().0);
        let mut differ = 0;
        for (ours, theirs) in pairs.filter(|(o, t)| o != t) {
            differ += 1;
            assert!(
                diverges_as_specified(format, *ours, *theirs),
                "{id}: {ours:?} {theirs:?}"
            );
        }
        assert_eq!(differ, diverging, "{id}");
    }
    diverging > 0
}

#[test]
fn every_etc_and_eac_decode_is_the_oracle_image_or_error() {
    let doc = load();
    assert_eq!(doc["unitypy"], "1.25.2");
    assert_eq!(doc["pillow"], "9.5.0");
    assert_eq!(doc["texture2ddecoder"], "1.0.5");
    let cases = doc["cases"].as_array().unwrap();
    assert_eq!(cases.len(), CASES);
    let (mut images, mut formats) = (0, BTreeSet::new());
    let mut tier_c = TIER_C.map(|(format, _, _)| (format, 0, 0));
    for c in cases {
        let id = c["id"].as_str().unwrap();
        let format = Format::from_texture_format(int(&c["format"])).unwrap();
        formats.insert(format.name());
        let (width, height) = (int(&c["width"]), int(&c["height"]));
        let data = build(&c["data"]);
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
            assert_eq!(err, named(c, width, height), "{id}");
            if c["raises"] == "DecompressionBombError" {
                assert_eq!(err.to_string(), c["message"].as_str().unwrap(), "{id}");
            }
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
        let ours = hex(&Sha256::digest(image.pixels()));
        if let Some(seen) = tier_c.iter_mut().find(|(f, _, _)| *f == format) {
            seen.1 += 1;
            if tier_c_matches(c, format, image.pixels()) {
                seen.2 += 1;
            }
        } else {
            if let Some(pixels) = c["pixels"].as_str() {
                assert_eq!(hex(image.pixels()), pixels, "{id}");
            }
            assert_eq!(ours, c["sha256"].as_str().unwrap(), "{id}");
        }
        let p = planned.unwrap_or_else(|e| panic!("{id}: planned {e}"));
        assert_eq!(p.output_bytes, u64::try_from(image.pixels().len()).unwrap());
        assert_eq!(image.charge().bytes(), p.charge(), "{id}");
        assert_eq!(meter.used(), image.charge().bytes(), "{id}");
        drop(image);
        assert_eq!(meter.used(), 0, "{id}");
    }
    assert_eq!(images, IMAGES);
    assert_eq!(tier_c, TIER_C);
    let all: BTreeSet<&str> = Format::ALL
        .iter()
        .filter(|f| f.name().starts_with("ETC") || f.name().starts_with("EAC"))
        .map(|f| f.name())
        .collect();
    assert_eq!(all.len(), 8);
    assert_eq!(formats, all);
}

/// The pinned divergence test of `PARITY.md` section 19.1's `ETC2_RGBA1`
/// row: on the oracle's punch-through cases, UnityPy (texture2ddecoder
/// 1.0.5) returns transparent pixels whose colour is not black, and unityrs
/// returns each of them as (0, 0, 0, 0), the Khronos Data Format
/// Specification 1.3, section 22.9; every other pixel is UnityPy's.
#[test]
fn etc2_rgba1_transparent_pixels_are_zero_where_unitypy_keeps_their_colour() {
    let doc = load();
    let mut kept_total = 0;
    for c in doc["cases"].as_array().unwrap() {
        let id = c["id"].as_str().unwrap();
        if c["format"] != 46 || !id.contains("-punch-") {
            continue;
        }
        let Some(pixels) = c["pixels"].as_str() else {
            continue;
        };
        let theirs = unhex(pixels);
        let image = decode(
            Format::Etc2Rgba1,
            &build(&c["data"]),
            int(&c["width"]),
            int(&c["height"]),
            &Meter::unlimited(),
        )
        .unwrap();
        let (ours, theirs) = (image.pixels().as_chunks::<4>().0, theirs.as_chunks::<4>().0);
        for (ours, theirs) in ours.iter().zip(theirs) {
            if theirs[3] == 0 {
                assert_eq!(*ours, [0, 0, 0, 0], "{id}");
                if theirs[..3] != [0, 0, 0] {
                    kept_total += 1;
                }
            } else {
                assert_eq!(ours, theirs, "{id}");
            }
        }
    }
    // The hex-recorded punch-through cases (4x4 and 6x7 of the
    // differential, T and H modes, 3 + 18, 3 + 12 and 5 + 11; the planar
    // mode is opaque) hold this many pixels UnityPy returns transparent with
    // their colour kept.
    assert_eq!(kept_total, 52);
}

/// The pinned divergence test of `PARITY.md` section 19.1's signed EAC row,
/// on the oracle's hand-built blocks of `tools/fixtures/texture_etc.py`
/// (every index 7, table 0's +14): with a base of -128, UnityPy
/// (texture2ddecoder 1.0.5) gives 1 at a multiplier of 0 and 13 at one of
/// 1, and unityrs 2 and 14, what a base of -127 gives on both sides (the
/// Khronos Data Format Specification 1.3, sections 22.7 and 22.8), in
/// `EAC_R_SIGNED`'s red and either of `EAC_RG_SIGNED`'s channels.
#[test]
fn signed_eac_reads_a_base_of_minus_128_as_minus_127() {
    let doc = load();
    let mut seen = 0;
    for c in doc["cases"].as_array().unwrap() {
        let id = c["id"].as_str().unwrap();
        let Some(rest) = id
            .strip_prefix("EAC_R_SIGNED-base-")
            .or_else(|| id.strip_prefix("EAC_RG_SIGNED-base-"))
        else {
            continue;
        };
        seen += 1;
        let format = Format::from_texture_format(int(&c["format"])).unwrap();
        let (unitypy, khronos) = if rest.contains("-x0-") {
            (1, 2)
        } else {
            (13, 14)
        };
        // The channels' bases, R then G: -128 or -127.
        let bases: Vec<bool> = rest
            .split("-x")
            .next()
            .unwrap()
            .split('-')
            .map(|b| b == "min")
            .collect();
        let image = decode(format, &build(&c["data"]), 4, 4, &Meter::unlimited()).unwrap();
        let theirs = unhex(c["pixels"].as_str().unwrap());
        for (ours, theirs) in image
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .zip(theirs.as_chunks::<4>().0)
        {
            for (channel, &min) in bases.iter().enumerate() {
                assert_eq!(theirs[channel], if min { unitypy } else { khronos }, "{id}");
                assert_eq!(ours[channel], khronos, "{id}");
            }
            assert_eq!(ours[bases.len()..], theirs[bases.len()..], "{id}");
        }
    }
    // R: -128 and -127; RG: both -128, and -127 with -128; at each multiplier.
    assert_eq!(seen, 8);
}
