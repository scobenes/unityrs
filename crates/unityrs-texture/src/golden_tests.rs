//! Golden digests of the ASTC, ETC and EAC decoders on seeded random and
//! hostile blocks, recorded from the decoders before their rewrite for
//! speed (`tests/fixtures/golden_decode.json`), which every later decoder
//! must reproduce byte for byte.
//!
//! Each case is one image: a target (a texture format through [`decode`],
//! or an ASTC footprint through `astc::decode_image`, which reaches the
//! eight footprints no format names), a generator, a seed and a size. The
//! sizes cut the edge blocks. The blocks are drawn with the testkit's
//! `SplitMix64` from the case's seed:
//!
//! - every family: `random`, uniform bytes;
//! - ASTC: `void`, void-extent blocks (LDR and HDR, reserved bits and
//!   extents valid or not); `mode`, a random payload under each block's
//!   mode drawn from the modes legal at the footprint, so the partitions,
//!   colour endpoint modes (HDR ones in the LDR profile among them),
//!   precisions and planes vary over decodable blocks; `mutate`, the
//!   legal and crafted blocks of `tests/fixtures/astc.json` for the
//!   footprint with 1 to 8 bits flipped;
//! - ETC: `diff`, the differential bit set, so ETC2's T, H and planar
//!   modes and ETC2 A1's punch-through occur often;
//! - EAC and ETC2 A8's alpha: `edge`, multipliers of 0 and bases of 0,
//!   255 and the signed -128.
//!
//! A case records the first 128 bits of the sha256 of the image (its
//! pixels, top row first) or
//! the decode error's text; an HDR format's case, where one reserved block
//! refuses the image, also each block's alone. `UNITYRS_GOLDEN_WRITE=1` rewrites the file
//! instead of checking it.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde_json::{Value as J, json};
use sha2::{Digest, Sha256};
use unityrs_core::limits::Meter;
use unityrs_testkit::Rng;

use crate::Format;
use crate::astc::{self, Footprint, Tables};

const FIXTURE: &str = "tests/fixtures/golden_decode.json";

/// The image sizes, each cutting edge blocks at every footprint.
const SIZES: [(usize, usize); 7] = [
    (1, 1),
    (5, 3),
    (13, 11),
    (37, 29),
    (67, 45),
    (129, 97),
    (257, 131),
];

/// The formats decoded through [`decode`]: name, format, generators.
const FORMATS: [(&str, Format, &[&str]); 26] = {
    const ASTC: &[&str] = &["random", "void", "mode", "mutate"];
    const ETC: &[&str] = &["random", "diff"];
    const EAC: &[&str] = &["random", "edge"];
    const A8: &[&str] = &["random", "diff", "edge"];
    [
        ("ASTC_RGB_4x4", Format::AstcRgb4x4, ASTC),
        ("ASTC_RGB_5x5", Format::AstcRgb5x5, ASTC),
        ("ASTC_RGB_6x6", Format::AstcRgb6x6, ASTC),
        ("ASTC_RGB_8x8", Format::AstcRgb8x8, ASTC),
        ("ASTC_RGB_10x10", Format::AstcRgb10x10, ASTC),
        ("ASTC_RGB_12x12", Format::AstcRgb12x12, ASTC),
        ("ASTC_RGBA_4x4", Format::AstcRgba4x4, ASTC),
        ("ASTC_RGBA_5x5", Format::AstcRgba5x5, ASTC),
        ("ASTC_RGBA_6x6", Format::AstcRgba6x6, ASTC),
        ("ASTC_RGBA_8x8", Format::AstcRgba8x8, ASTC),
        ("ASTC_RGBA_10x10", Format::AstcRgba10x10, ASTC),
        ("ASTC_RGBA_12x12", Format::AstcRgba12x12, ASTC),
        ("ASTC_HDR_4x4", Format::AstcHdr4x4, ASTC),
        ("ASTC_HDR_5x5", Format::AstcHdr5x5, ASTC),
        ("ASTC_HDR_6x6", Format::AstcHdr6x6, ASTC),
        ("ASTC_HDR_8x8", Format::AstcHdr8x8, ASTC),
        ("ASTC_HDR_10x10", Format::AstcHdr10x10, ASTC),
        ("ASTC_HDR_12x12", Format::AstcHdr12x12, ASTC),
        ("ETC_RGB4", Format::EtcRgb4, ETC),
        ("ETC2_RGB", Format::Etc2Rgb, ETC),
        ("ETC2_RGBA1", Format::Etc2Rgba1, ETC),
        ("ETC2_RGBA8", Format::Etc2Rgba8, A8),
        ("EAC_R", Format::EacR, EAC),
        ("EAC_R_SIGNED", Format::EacRSigned, EAC),
        ("EAC_RG", Format::EacRg, EAC),
        ("EAC_RG_SIGNED", Format::EacRgSigned, EAC),
    ]
};

/// What a case decodes.
#[derive(Clone, Copy)]
enum Target {
    Format(&'static str, Format),
    /// An ASTC footprint through `decode_image`, flipped or not.
    Footprint(u8, u8, bool),
}

impl Target {
    fn name(self) -> String {
        match self {
            Self::Format(name, _) => name.to_owned(),
            Self::Footprint(w, h, flip) => {
                format!("astc-{w}x{h}{}", if flip { "-flip" } else { "" })
            }
        }
    }

    /// The block's width, height and bytes, and whether it is ASTC.
    fn block(self) -> (u8, u8, usize, bool) {
        match self {
            Self::Footprint(w, h, _) => (w, h, 16, true),
            Self::Format(name, f) => {
                let (w, h) = f.block_size().unwrap();
                let bytes = usize::try_from(f.block_bytes().unwrap()).unwrap();
                (w, h, bytes, name.starts_with("ASTC"))
            }
        }
    }

    fn family(self) -> &'static str {
        match self {
            Self::Format(_, f) if f.is_astc_hdr() => "astc-hdr",
            Self::Format(name, _) if name.starts_with("EAC") => "eac",
            Self::Format(name, _) if name.starts_with("ETC") => "etc",
            _ => "astc",
        }
    }
}

/// Every case, in order, each with its seed.
fn cases() -> Vec<(Target, &'static str, u64, (usize, usize))> {
    let mut targets: Vec<(Target, &[&str])> = FORMATS
        .iter()
        .map(|&(name, f, gens)| (Target::Format(name, f), gens))
        .collect();
    for (w, h, _) in astc::context::FOOTPRINTS {
        for flip in [false, true] {
            targets.push((
                Target::Footprint(w, h, flip),
                &["random", "void", "mode", "mutate"],
            ));
        }
    }
    let mut out = Vec::new();
    for (target, gens) in targets {
        for &generator in gens {
            for &size in &SIZES {
                let seed = 0x601d_0000 + u64::try_from(out.len()).unwrap();
                out.push((target, generator, seed, size));
            }
        }
    }
    out
}

/// The legal and crafted blocks of `tests/fixtures/astc.json` at a
/// footprint (its hex cases).
fn pool(fixture: &J, (w, h): (u8, u8)) -> Vec<[u8; 16]> {
    let mut out = Vec::new();
    for case in fixture["footprint_cases"].as_array().unwrap() {
        // Its seeded cases are random bytes, which `random` draws already.
        let Some(hex) = case["data"]["hex"]
            .as_str()
            .filter(|_| case["block"] == json!([w, h]))
        else {
            continue;
        };
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        out.extend(bytes.as_chunks::<16>().0.iter().copied());
    }
    assert!(!out.is_empty(), "no fixture blocks at {w}x{h}");
    out
}

/// The block modes a footprint decodes: those (void extents aside, which
/// `void` draws) under which a block of zeros, one partition of
/// luminance, is not an error block.
fn modes(w: u8, h: u8) -> Vec<u16> {
    let blocks: Vec<u8> = (0..2048_u16)
        .flat_map(|m| (u128::from(m)).to_le_bytes())
        .collect();
    let errors = crate::astc_ldr_error_blocks(w, h, &blocks).unwrap();
    (0..2048_u16)
        .zip(errors)
        .filter(|&(m, error)| m & 0x1ff != 0x1fc && !error)
        .map(|(m, _)| m)
        .collect()
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "masked to the field's width"
)]
fn astc_block(rng: &mut Rng, generator: &str, pool: &[[u8; 16]], modes: &[u16]) -> [u8; 16] {
    let mut b = [0_u8; 16];
    for byte in &mut b {
        *byte = rng.next_u64() as u8;
    }
    match generator {
        "void" => {
            let hdr = rng.below(2) as u16;
            let reserved = if rng.below(4) == 0 {
                rng.below(4) as u16
            } else {
                3
            };
            let head = 0x1fc | hdr << 9 | reserved << 10;
            let mut v = u128::from_le_bytes(b) & !0xfff | u128::from(head);
            if rng.below(2) == 0 {
                // All four extents all ones: the constant-colour block.
                v |= ((1_u128 << 52) - 1) << 12;
            }
            b = v.to_le_bytes();
        }
        "mode" => {
            let m = modes[rng.below(modes.len())];
            let v = u128::from_le_bytes(b) & !0x7ff | u128::from(m);
            b = v.to_le_bytes();
        }
        "mutate" => {
            b = pool[rng.below(pool.len())];
            let flips = rng.below(8) + 1;
            rng.flip_bits(&mut b, flips);
        }
        _ => {}
    }
    b
}

#[allow(clippy::cast_possible_truncation, reason = "a random byte")]
fn etc_block(rng: &mut Rng, generator: &str, bytes: usize, format: Format) -> Vec<u8> {
    let mut b: Vec<u8> = (0..bytes).map(|_| rng.next_u64() as u8).collect();
    // The colour block: the last 8 bytes of a 16-byte ETC2 A8 block.
    let colour = if format == Format::Etc2Rgba8 { 8 } else { 0 };
    let alpha_blocks: &[usize] = match format {
        Format::EacR | Format::EacRSigned | Format::Etc2Rgba8 => &[0],
        Format::EacRg | Format::EacRgSigned => &[0, 8],
        _ => &[],
    };
    match generator {
        "diff" => b[colour + 3] |= 2,
        "edge" => {
            for &at in alpha_blocks {
                b[at] = [0, 255, 0x80, b[at]][rng.below(4)];
                if rng.below(2) == 0 {
                    b[at + 1] &= 0x0f;
                }
            }
        }
        _ => {}
    }
    b
}

/// A footprint's fixture blocks and decodable modes, each made once.
type Pools = BTreeMap<(u8, u8), (Vec<[u8; 16]>, Vec<u16>)>;

/// The case's image digest, or its error's text, and its block count.
fn run(
    (fixture, pools): (&J, &mut Pools),
    target: Target,
    generator: &str,
    seed: u64,
    (w, h): (usize, usize),
) -> (String, usize) {
    let (bw, bh, bytes, is_astc) = target.block();
    let blocks = w.div_ceil(usize::from(bw)) * h.div_ceil(usize::from(bh));
    let mut rng = Rng::new(seed);
    let mut data = Vec::with_capacity(blocks * bytes);
    if is_astc {
        let (pool, modes) = pools
            .entry((bw, bh))
            .or_insert_with(|| (pool(fixture, (bw, bh)), modes(bw, bh)));
        for _ in 0..blocks {
            data.extend(astc_block(&mut rng, generator, pool, modes));
        }
    } else if let Target::Format(_, f) = target {
        for _ in 0..blocks {
            data.extend(etc_block(&mut rng, generator, bytes, f));
        }
    }
    let meter = Meter::unlimited();
    let decode = |data: &[u8], w: usize, h: usize, f: Format| {
        let (w, h) = (i32::try_from(w).unwrap(), i32::try_from(h).unwrap());
        match crate::decode(f, data, w, h, &meter) {
            Ok(image) => hex(&Sha256::digest(image.pixels())[..16]),
            Err(e) => format!("error: {e}"),
        }
    };
    let digest = match target {
        // One reserved HDR block refuses the whole image, so each block is
        // also decoded alone, its digest or error hashed in turn.
        Target::Format(_, f) if f.is_astc_hdr() => {
            let mut all = Sha256::new();
            all.update(decode(&data, w, h, f));
            for block in data.chunks(bytes) {
                all.update(decode(block, usize::from(bw), usize::from(bh), f));
            }
            hex(&all.finalize()[..16])
        }
        Target::Format(_, f) => decode(&data, w, h, f),
        Target::Footprint(fw, fh, flip) => {
            let mut tables = Tables::new(Footprint::new(fw, fh).unwrap(), &meter).unwrap();
            let mut out = vec![0; w * h * 4];
            astc::decode_image(&mut tables, &data, (w, h), flip, astc::ERROR, &mut out);
            hex(&Sha256::digest(&out)[..16])
        }
    };
    (digest, blocks)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Every case's digest is the recorded one (or, with
/// `UNITYRS_GOLDEN_WRITE`, is recorded), and each family has at least
/// 10,000 blocks.
#[test]
fn the_decoders_reproduce_the_golden_digests() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let fixture: J =
        serde_json::from_slice(&std::fs::read(root.join("tests/fixtures/astc.json")).unwrap())
            .unwrap();
    let (mut got, mut counts, mut pools) = (Vec::new(), BTreeMap::new(), Pools::new());
    for (target, generator, seed, size) in cases() {
        let (digest, blocks) = run((&fixture, &mut pools), target, generator, seed, size);
        *counts.entry(target.family()).or_insert(0) += blocks;
        got.push(json!([
            target.name(),
            generator,
            seed,
            size.0,
            size.1,
            digest
        ]));
    }
    let got = json!({ "blocks": counts, "cases": got });
    let path = root.join(FIXTURE);
    if std::env::var_os("UNITYRS_GOLDEN_WRITE").is_some() {
        // One case a line: target, generator, seed, width, height, digest.
        let lines: Vec<String> = got["cases"]
            .as_array()
            .unwrap()
            .iter()
            .map(J::to_string)
            .collect();
        let text = format!(
            "{{\n\"blocks\": {},\n\"cases\": [\n{}\n]\n}}\n",
            got["blocks"],
            lines.join(",\n")
        );
        std::fs::write(&path, text).unwrap();
        return;
    }
    let want: J = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for family in ["astc", "astc-hdr", "etc", "eac"] {
        assert!(
            counts[family] >= 10_000,
            "{family}: {} blocks",
            counts[family]
        );
    }
    assert_eq!(got["blocks"], want["blocks"]);
    let (got, want) = (
        got["cases"].as_array().unwrap(),
        want["cases"].as_array().unwrap(),
    );
    assert_eq!(got.len(), want.len());
    for (g, w) in got.iter().zip(want) {
        assert_eq!(g, w);
    }
}
