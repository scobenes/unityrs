//! Replays `tests/fixtures/astc.json`'s `footprint_cases` (written by
//! `tools/fixtures/texture_astc.py` with UnityPy 1.25.2 and astc-encoder-py
//! 0.1.12): `Texture2DConverter.astc` called alone at each of astcenc's
//! fourteen footprints, on sizes that cut the edge blocks, so with no
//! padding and no flip, its image bit for bit, as astcenc's AVX2 build
//! decodes it (the oracle's, an error block white) and as its scalar build
//! does (an error block magenta: `block`'s module docs). The blocks are
//! astcenc's encoder's, `SplitMix64`'s and crafted ones; `coverage` lists the tags the
//! generator's `classify` gives them, and every footprint has each tag the
//! module docs list.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value as J;
use sha2::{Digest, Sha256};
use unityrs_core::limits::Meter;
use unityrs_testkit::Rng;

use super::super::block::{ERROR, MAGENTA};
use super::super::context::{FOOTPRINTS, Footprint, Tables};
use super::super::decode_image;

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

/// A case's data: hex, or the first `len` bytes of `SplitMix64` from
/// `seed`, each output little-endian.
fn build(spec: &J) -> Vec<u8> {
    if let Some(h) = spec["hex"].as_str() {
        return (0..h.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
            .collect();
    }
    let len = usize::try_from(spec["len"].as_u64().unwrap()).unwrap();
    let mut rng = Rng::new(spec["seed"].as_u64().unwrap());
    let mut out = Vec::new();
    while out.len() < len {
        out.extend_from_slice(&rng.next_u64().to_le_bytes());
    }
    out.truncate(len);
    out
}

/// The tags every footprint's blocks have: each block-mode class, single
/// and dual plane, one to four partitions, the second plane on each
/// channel, every weight precision and endpoint mode, mixed modes, both
/// kinds of LDR void extent, and every error block but `error:grid`, which
/// a footprint whose every grid fits (12x12) cannot have.
fn required() -> BTreeSet<String> {
    let mut tags: BTreeSet<String> = [
        "single",
        "dual",
        "valid",
        "cem:mixed",
        "cem:hdr",
        "void:ldr",
        "void:allones",
        "error:bits",
        "error:color_quant",
        "error:colors",
        "error:dual4",
        "error:reserved",
        "error:void_extent",
        "error:void_hdr",
        "error:void_reserved",
    ]
    .map(str::to_owned)
    .into();
    tags.extend((1..=4).map(|p| format!("partitions:{p}")));
    tags.extend((0..4).map(|c| format!("plane2:{c}")));
    tags.extend((0..12).map(|q| format!("quant:{q}")));
    tags.extend((0..16).map(|c| format!("cem:{c}")));
    tags
}

/// The block-mode table's rows (`classify`'s layouts) whose grids fit
/// `w` x `h`: `lo0` (4-7 by 2-5) and `lo3a` (2-3 by 2-5) always; `lo1`
/// (8-11 wide), `lo2` (8-11 high), `lo3b` (6-7 high), `hi0` (12 wide),
/// `hi1` (12 high), `hi2` (6-9 by 6-9) and `hi3` (6x10 or 10x6) once one of
/// their grids fits.
fn layouts(w: u8, h: u8) -> BTreeSet<String> {
    let rows: [(&str, u8, u8); 9] = [
        ("lo0", 4, 2),
        ("lo3a", 2, 2),
        ("lo1", 8, 2),
        ("lo2", 2, 8),
        ("lo3b", 2, 6),
        ("hi0", 12, 2),
        ("hi1", 2, 12),
        ("hi2", 6, 6),
        ("hi3", 6, 10),
    ];
    let mut out: BTreeSet<String> = rows
        .iter()
        .filter(|(_, x, y)| *x <= w && *y <= h)
        .map(|(name, _, _)| format!("layout:{name}"))
        .collect();
    if w >= 10 && h >= 6 {
        out.insert("layout:hi3".to_owned());
    }
    out
}

#[test]
fn every_footprint_decodes_as_unitypys_astc() {
    let doc = load();
    assert_eq!(doc["unitypy"], "1.25.2");
    assert_eq!(doc["astc_encoder"], "0.1.12");
    let cases = doc["footprint_cases"].as_array().unwrap();
    assert_eq!(cases.len(), 5 * FOOTPRINTS.len());
    let mut seen = BTreeSet::new();
    for c in cases {
        let id = c["id"].as_str().unwrap();
        let block: Vec<u8> = c["block"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| u8::try_from(v.as_u64().unwrap()).unwrap())
            .collect();
        let (bw, bh) = (block[0], block[1]);
        seen.insert((bw, bh));
        let (w, h) = (
            usize::try_from(c["width"].as_u64().unwrap()).unwrap(),
            usize::try_from(c["height"].as_u64().unwrap()).unwrap(),
        );
        let data = build(&c["data"]);
        assert_eq!(
            hex(&Sha256::digest(&data)),
            c["data_sha256"].as_str().unwrap(),
            "{id}"
        );
        // Every case's data is whole: the image.
        assert_eq!(c["mode"], "RGBA", "{id}");
        assert_eq!(c["size"], serde_json::json!([w, h]), "{id}");
        // Neither side a whole number of blocks: each edge block is cut.
        assert!(w % usize::from(bw) != 0 && h % usize::from(bh) != 0, "{id}");
        let meter = Meter::unlimited();
        let mut tables = Tables::new(Footprint::new(bw, bh).unwrap(), &meter).unwrap();
        let mut out = vec![0; 4 * w * h];
        decode_image(&mut tables, &data, (w, h), false, ERROR, &mut out);
        if let Some(pixels) = c["pixels"].as_str() {
            assert_eq!(hex(&out), pixels, "{id}");
        }
        assert_eq!(
            hex(&Sha256::digest(&out)),
            c["sha256"].as_str().unwrap(),
            "{id}"
        );
        decode_image(&mut tables, &data, (w, h), false, MAGENTA, &mut out);
        assert_eq!(
            hex(&Sha256::digest(&out)),
            c["scalar_sha256"].as_str().unwrap(),
            "{id}: the scalar build"
        );
    }
    assert_eq!(seen, FOOTPRINTS.iter().map(|f| (f.0, f.1)).collect());
    // The generator's coverage, per footprint.
    let coverage = doc["coverage"].as_object().unwrap();
    assert_eq!(coverage.len(), FOOTPRINTS.len());
    for (w, h, _) in FOOTPRINTS {
        let tags: BTreeSet<String> = coverage[&format!("{w}x{h}")]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap().to_owned())
            .collect();
        let mut want = required();
        want.extend(layouts(w, h));
        if (w, h) != (12, 12) {
            want.insert("error:grid".to_owned());
        }
        let missing: Vec<_> = want.difference(&tags).collect();
        assert!(missing.is_empty(), "{w}x{h}: {missing:?}");
    }
}
