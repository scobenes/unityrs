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
//!   255 and the signed -128;
//! - the BC formats: `edge`, colour endpoints equal or swapped and alpha endpoints
//!   equal, swapped, 0 or 255; BC7's `mode`, the first byte naming each
//!   of the eight modes, or none (0), equally often; BC7's `partition`,
//!   not drawn but walked in turn, every legal mode and partition (so
//!   every anchor-index layout) and modes 4 and 5's rotations and index
//!   selections, their other bits random;
//! - PVRTC: `mode`, the modulation-mode bit set, and for 2bpp the
//!   interpolation bits varied, so the missing weights occur often.
//!
//! The BC and PVRTC cases follow the rest (their seeds continue after
//! them), PVRTC at sizes of its own, whose block counts are powers of two.
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

/// The BC and PVRTC formats, after every other case: name, format,
/// generators.
const LATER: [(&str, Format, &[&str]); 9] = {
    const BCN: &[&str] = &["random", "edge"];
    const PVRTC: &[&str] = &["random", "mode"];
    [
        ("DXT1", Format::Dxt1, BCN),
        ("DXT3", Format::Dxt3, BCN),
        ("DXT5", Format::Dxt5, BCN),
        ("BC4", Format::Bc4, BCN),
        ("BC5", Format::Bc5, BCN),
        ("BC7", Format::Bc7, &["random", "mode"]),
        ("PVRTC_RGB2", Format::PvrtcRgb2, PVRTC),
        ("PVRTC_RGBA4", Format::PvrtcRgba4, PVRTC),
        ("BC7", Format::Bc7, &["partition"]),
    ]
};

/// PVRTC's sizes, whose block counts must be powers of two at both bit
/// rates: edge blocks cut, and Morton's order square and not.
const PVRTC_SIZES: [(usize, usize); 9] = [
    (1, 1),
    (5, 3),
    (13, 15),
    (29, 13),
    (61, 30),
    (128, 32),
    (16, 256),
    (254, 127),
    (256, 256),
];

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
            Self::Format(name, _) if name.starts_with("PVRTC") => "pvrtc",
            Self::Format(name, _) if !name.starts_with("ASTC") => "bcn",
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
    let later = LATER
        .iter()
        .map(|&(name, f, gens)| (Target::Format(name, f), gens));
    let mut out = Vec::new();
    for (target, gens) in targets.into_iter().chain(later) {
        let pvrtc = target.family() == "pvrtc";
        for &generator in gens {
            for &size in if pvrtc { &PVRTC_SIZES[..] } else { &SIZES } {
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

/// A BC or PVRTC block of `bytes` bytes under `generator`.
#[allow(clippy::cast_possible_truncation, reason = "a random byte")]
fn bcn_block(rng: &mut Rng, generator: &str, bytes: usize, format: Format) -> Vec<u8> {
    let mut b: Vec<u8> = (0..bytes).map(|_| rng.next_u64() as u8).collect();
    match (generator, format) {
        ("mode", Format::Bc7) => {
            // Mode `m` is the first byte's lowest set bit; 8 is none.
            let m = rng.below(9);
            b[0] = if m == 8 {
                0
            } else {
                (b[0] & !((2_u16 << m) - 1) as u8) | 1 << m
            };
        }
        ("mode", _) => {
            // The modulation-mode bit (bit 0 of colour A).
            b[4] |= 1;
            if rng.below(2) == 0 {
                b[0] |= 1;
            }
        }
        ("edge", _) => {
            let (colour, alpha): (&[usize], &[usize]) = match format {
                Format::Dxt1 => (&[0], &[]),
                Format::Dxt3 => (&[8], &[]),
                Format::Dxt5 => (&[8], &[0]),
                Format::Bc4 => (&[], &[0]),
                _ => (&[], &[0, 8]),
            };
            for &at in colour {
                let c0 = u16::from_le_bytes([b[at], b[at + 1]]);
                let c1 = u16::from_le_bytes([b[at + 2], b[at + 3]]);
                // Equal, or c0 below c1: BC1's three-colour form.
                let (c0, c1) = match rng.below(3) {
                    0 => (c0, c0),
                    1 => (c0.min(c1), c0.max(c1)),
                    _ => (c0, c1),
                };
                b[at..at + 2].copy_from_slice(&c0.to_le_bytes());
                b[at + 2..at + 4].copy_from_slice(&c1.to_le_bytes());
            }
            for &at in alpha {
                match rng.below(4) {
                    0 => b[at + 1] = b[at],
                    1 => b[at] = 0,
                    2 => b[at + 1] = 255,
                    _ => {}
                }
            }
        }
        _ => {}
    }
    b
}

/// BC7's every legal block layout, in the order `partition` walks
/// them: mode, partition (0 to 15 in mode 0, 0 to 63 in modes 1, 2, 3
/// and 7, the partition fixing the anchor indices), rotation (modes 4 and
/// 5) and index selection (mode 4).
fn bc7_layouts() -> Vec<[u8; 4]> {
    let mut out = Vec::new();
    for mode in 0..8_u8 {
        let (partitions, rotations, selections) = match mode {
            0 => (16, 1, 1),
            1 | 2 | 3 | 7 => (64, 1, 1),
            4 => (1, 4, 2),
            5 => (1, 4, 1),
            _ => (1, 1, 1),
        };
        for p in 0..partitions {
            for r in 0..rotations {
                for s in 0..selections {
                    out.push([mode, p, r, s]);
                }
            }
        }
    }
    out
}

/// The `partition` generator's block `i`: layout `i` (cycling) of
/// [`bc7_layouts`] over random bits.
#[allow(clippy::cast_possible_truncation, reason = "a random byte")]
fn bc7_layout_block(rng: &mut Rng, layouts: &[[u8; 4]], i: usize) -> Vec<u8> {
    let random: Vec<u8> = (0..16).map(|_| rng.next_u64() as u8).collect();
    let mut bits = u128::from_le_bytes(random.try_into().unwrap());
    let [mode, p, r, s] = layouts[i % layouts.len()];
    // The mode's unary prefix, then its partition or rotation and index
    // selection fields.
    let fields: &[(u8, u32)] = match mode {
        0 => &[(p, 4)],
        4 => &[(r, 2), (s, 1)],
        5 => &[(r, 2)],
        6 => &[],
        _ => &[(p, 6)],
    };
    let mut at = u32::from(mode) + 1;
    bits = (bits & !((1 << at) - 1)) | (1 << mode);
    for &(value, width) in fields {
        bits = (bits & !(((1 << width) - 1) << at)) | (u128::from(value) << at);
        at += width;
    }
    bits.to_le_bytes().to_vec()
}

/// A block's BC7 layout, read from its bits as the specification lays
/// them out (independently of [`bc7_layouts`]); `None` for mode 8.
fn bc7_layout_of(block: &[u8]) -> Option<[u8; 4]> {
    let bits = u128::from_le_bytes(block.try_into().unwrap());
    let mode = bits.trailing_zeros();
    let field = |at: u32, width: u32| u8::try_from((bits >> (mode + 1 + at)) & ((1 << width) - 1));
    let m = u8::try_from(mode).ok()?;
    Some(match mode {
        0 => [m, field(0, 4).unwrap(), 0, 0],
        1 | 2 | 3 | 7 => [m, field(0, 6).unwrap(), 0, 0],
        4 => [m, 0, field(0, 2).unwrap(), field(2, 1).unwrap()],
        5 => [m, 0, field(0, 2).unwrap(), 0],
        6 => [m, 0, 0, 0],
        _ => return None,
    })
}

/// A footprint's fixture blocks and decodable modes, each made once.
type Pools = BTreeMap<(u8, u8), (Vec<[u8; 16]>, Vec<u16>)>;

/// The case's blocks and their count.
fn blocks(
    (fixture, pools): (&J, &mut Pools),
    target: Target,
    generator: &str,
    seed: u64,
    (w, h): (usize, usize),
) -> (Vec<u8>, usize) {
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
        let make = match target.family() {
            "bcn" | "pvrtc" => bcn_block,
            _ => etc_block,
        };
        let layouts = bc7_layouts();
        for i in 0..blocks {
            if generator == "partition" {
                data.extend(bc7_layout_block(&mut rng, &layouts, i));
            } else {
                data.extend(make(&mut rng, generator, bytes, f));
            }
        }
    }
    (data, blocks)
}

/// The case's image digest, or its error's text, and its block count.
fn run(
    inputs: (&J, &mut Pools),
    target: Target,
    generator: &str,
    seed: u64,
    (w, h): (usize, usize),
) -> (String, usize) {
    let (bw, bh, bytes, _) = target.block();
    let (data, blocks) = blocks(inputs, target, generator, seed, (w, h));
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
    for family in ["astc", "astc-hdr", "etc", "eac", "bcn", "pvrtc"] {
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

/// The `partition` cases cover every legal BC7 layout, by construction
/// and not by chance: 16 partitions of mode 0, 64 of modes 1, 2, 3 and 7,
/// 8 rotations and index selections of mode 4, 4 rotations of mode 5 and
/// mode 6, 285 in all; every case of 285 blocks or more covers them all.
#[test]
fn the_bc7_cases_cover_every_mode_and_partition() {
    let fixture = J::Null;
    let mut pools = Pools::new();
    let (mut all, mut full) = (std::collections::BTreeSet::new(), 0);
    for (target, generator, seed, size) in cases() {
        if generator != "partition" {
            continue;
        }
        let (data, blocks) = blocks((&fixture, &mut pools), target, generator, seed, size);
        let mut seen = std::collections::BTreeSet::new();
        for block in data.chunks(16) {
            seen.insert(bc7_layout_of(block).unwrap());
        }
        assert_eq!(seen.len(), blocks.min(285));
        full += usize::from(seen.len() == 285);
        all.extend(seen);
    }
    let mut per_mode = [0; 8];
    for [mode, ..] in &all {
        per_mode[usize::from(*mode)] += 1;
    }
    assert_eq!(per_mode, [16, 64, 64, 64, 8, 4, 1, 64]);
    assert_eq!(all.len(), 285);
    assert!(full >= 2, "{full} cases cover every layout");
}
