//! The fuzz targets' seed corpora (`fuzz/`, the fuzz plan of
//! `docs/plans/core.md` section 6), written from the testkit's builders:
//! every output `unityrs_testkit::mutate::builder_outputs` lists (the format
//! matrix, every format, codec and flag set, and every file of every
//! Environment scene), the files each bundle holds, every object's tree and
//! bytes (at each of the value targets' memory bounds, the case's
//! `TreeCase::SMALL_MEMORY` clear and set), every tree stored in each format
//! both ways round, the deep values of `unityrs_testkit::depth`, LZ4
//! blocks of every output, textures of every size class the decoders tell
//! apart, ASTC blocks of every kind `unityrs-texture`'s oracle fixture
//! crafts (`tools/fixtures/texture_astc.py`), and every mesh and sprite case `unityrs-meshdecode`'s oracle
//! fixture records (`tools/fixtures/meshdecode.py`'s synthetic values).
//! Nothing comes from a corpus. One folder per target:
//!
//! ```text
//! cargo run -p unityrs-testkit --example fuzz_seeds -- DIR
//! cargo +nightly fuzz run TARGET CORPUS DIR/TARGET    # in fuzz/
//! ```
//!
//! `DIR` must be empty or new. The seeds are generated, never committed:
//! CI's fuzz workflow runs this before each target.
//!
//! The core splits what the builders made (a bundle into its files, a file
//! into its objects); the inputs are the builders' alone.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use unityrs_core::budget::Reservation;
use unityrs_core::detect::{FileType, check_file_type};
use unityrs_core::files::{BundleFile, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::stream::Endian;
use unityrs_core::typetree::NodeList;
use unityrs_core::typetree::commonstring::COMMON_STRINGS;
use unityrs_core::{FileContext, RawStr};
use unityrs_testkit::Rng;
use unityrs_testkit::blob::{NodeSpec, Strings, blob_tree, old_tree};
use unityrs_testkit::bundle::{TestCodec, compress};
use unityrs_testkit::depth::{Depth, Wrap, Z_ROWS};
use unityrs_testkit::fuzz::{MeshCase, RefType, Row, TextureCase, TreeCase, blob_case, lz4_case};

/// The targets, each a folder of `DIR`.
const TARGETS: [&str; 13] = [
    "detect",
    "bundle",
    "serialized",
    "typetree",
    "boost_read",
    "python_read",
    "write_after_read",
    "lz4",
    "texture",
    "astc",
    "meshdecode",
    "zlib_match",
    "unitycn",
];

/// The `texture` seeds' formats: the five `unityrs-texture` decodes (RGB24,
/// RGBA32, DXT1, DXT5, BC7), a named one it skips (`ETC_RGB4`) and a value
/// with no name.
const TEXTURE_FORMATS: [i32; 7] = [3, 4, 10, 12, 25, 34, 1000];

/// The `texture` seeds' sizes: a side of 0 or below, the size classes, the
/// bound (16384 x 16384, padded) and one column past it, Pillow's raw width
/// caps and one past each, the fewest pixels past its crop bound, and the
/// end of `i32`.
const TEXTURE_SIZES: [(i32, i32); 17] = [
    (0, 4),
    (4, 0),
    (-1, 4),
    (1, 1),
    (3, 5),
    (4, 4),
    (7, 9),
    (16, 16),
    (33, 2),
    (16384, 16384),
    (16385, 16384),
    (67_108_856, 1),
    (67_108_857, 1),
    (89_478_478, 1),
    (89_478_479, 1),
    (59, 3_033_169),
    (i32::MAX, 1),
];

/// The most data a `texture` seed holds.
const TEXTURE_DATA: u64 = 4096;

/// The `astc` seeds' source: `unityrs-texture`'s ASTC oracle fixture.
const ASTC_CASES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../unityrs-texture/tests/fixtures/astc.json"
);

/// Old-format and blob-format tree formats a stored tree is written in:
/// format 2 (with `m_VariableCount`), 3 (no index, no meta flag), the other
/// old formats, the blob at 10 and from 12, with `m_RefTypeHash` from 19.
const TREE_FORMATS: [u8; 10] = [2, 3, 5, 9, 11, 10, 12, 15, 19, 22];

/// Bytes an LZ4 seed compresses at most of an output.
const LZ4_MAX: usize = 16 << 10;

/// The `meshdecode` seeds' source: `unityrs-meshdecode`'s handler fixture.
const MESH_CASES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../unityrs-meshdecode/tests/fixtures/process.json"
);

/// The `meshdecode` seeds' meters: the case's limits alone, then a meter of
/// 64 bytes (refusing the first array), 16 KiB and 256 KiB (most of the
/// fixture's arrays fit, the largest do not).
pub(crate) const MESH_METERS: [u8; 4] = [0, 1, 16, 64];

struct Seeds {
    dir: PathBuf,
    count: [usize; TARGETS.len()],
    trees: BTreeSet<Vec<u8>>,
    nodes: Vec<Vec<NodeSpec>>,
}

impl Seeds {
    fn add(&mut self, target: &str, name: &str, bytes: &[u8]) -> std::io::Result<()> {
        let t = TARGETS.iter().position(|x| *x == target).unwrap_or(0);
        let n = self.count[t];
        self.count[t] += 1;
        let name: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .take(96)
            .collect();
        std::fs::write(self.dir.join(target).join(format!("{n:05}-{name}")), bytes)
    }

    /// One object's tree and bytes, for the three value targets, at each
    /// memory bound (`TreeCase::SMALL_MEMORY` clear and set).
    fn value(&mut self, name: &str, case: &TreeCase) -> std::io::Result<()> {
        for (tag, memory) in [("", 0), ("-small", TreeCase::SMALL_MEMORY)] {
            let mut case = case.clone();
            case.flags |= memory;
            let name = format!("{name}{tag}");
            let bytes = case.encode();
            self.add("boost_read", &name, &bytes)?;
            self.add("python_read", &name, &bytes)?;
            self.add("write_after_read", &name, &bytes)?;
            case.flags |= TreeCase::PYTHON;
            self.add("write_after_read", &format!("{name}-py"), &case.encode())?;
        }
        Ok(())
    }

    /// A tree, kept once, for the `typetree` target.
    fn tree(&mut self, nodes: Vec<NodeSpec>) {
        let key: Vec<u8> = nodes
            .iter()
            .flat_map(|n| {
                let mut k = vec![n.level];
                k.extend_from_slice(&n.ty);
                k.push(0);
                k.extend_from_slice(&n.name);
                k.push(0);
                k
            })
            .collect();
        if self.trees.insert(key) {
            self.nodes.push(nodes);
        }
    }

    /// A file the builders made, or one a bundle holds: the targets that
    /// read it, and what it holds.
    fn file(&mut self, name: &str, bytes: &[u8], depth: u32) -> std::io::Result<()> {
        self.add("detect", name, bytes)?;
        let ctx = Arc::new(FileContext::unitypy());
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        let data = unityrs_core::Bytes::copy_from_slice(bytes);
        match check_file_type(bytes, &ctx.limits) {
            Ok(FileType::BundleFile) => {
                self.add("bundle", name, bytes)?;
                let Ok(b) = BundleFile::open(RawStr::from(name), data, None, &meter, &ctx) else {
                    return Ok(());
                };
                for (i, e) in b.directory().iter().enumerate() {
                    if let (Ok(entry), true) = (b.entry_bytes(i), depth < 4) {
                        let path = String::from_utf8_lossy(e.path.as_bytes()).into_owned();
                        self.file(&format!("{name}~{path}"), &entry, depth + 1)?;
                    }
                }
            }
            // A resource file too: detection takes an old format's
            // SerializedFile for one, and the target parses whatever it gets.
            Ok(FileType::AssetsFile | FileType::ResourceFile) => {
                self.add("serialized", name, bytes)?;
                if let Ok(f) = SerializedFile::parse(RawStr::from(name), data, None, &meter, &ctx) {
                    self.objects(name, &f)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn objects(&mut self, name: &str, f: &Arc<SerializedFile>) -> std::io::Result<()> {
        let mut flags = 0;
        if f.header().endian == Endian::Big {
            flags |= TreeCase::BIG_ENDIAN;
        }
        if f.ref_types().is_none() {
            flags |= TreeCase::REFS_ABSENT;
        }
        let refs: Vec<RefType> = f
            .ref_types()
            .unwrap_or(&[])
            .iter()
            .map(|t| RefType {
                class: raw(t.class_name.as_ref()),
                namespace: raw(t.namespace.as_ref()),
                assembly: raw(t.assembly.as_ref()),
                rows: t.nodes.as_deref().map(rows),
            })
            .collect();
        for t in f.types().iter().chain(f.ref_types().unwrap_or(&[])) {
            if let Some(nodes) = &t.nodes {
                self.tree(specs(nodes));
            }
        }
        for o in f.objects() {
            let (Some(nodes), Ok(data)) = (
                o.serialized_type().and_then(|t| t.nodes.as_deref()),
                o.raw_data(),
            ) else {
                continue;
            };
            let case = TreeCase {
                flags,
                start: 0,
                rows: rows(nodes),
                refs: refs.clone(),
                data: data.to_vec(),
            };
            self.value(&format!("{name}~{}", o.path_id()), &case)?;
        }
        Ok(())
    }
}

fn raw(s: Option<&RawStr>) -> Vec<u8> {
    s.map(|s| s.as_bytes().to_vec()).unwrap_or_default()
}

/// A meta flag's low 32 bits, as a blob or an old-format tree stores it.
fn low32(m: i64) -> i32 {
    let b = m.to_le_bytes();
    i32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn rows(nodes: &NodeList) -> Vec<Row> {
    nodes
        .rows()
        .iter()
        .map(|r| {
            Row::new(
                u8::try_from(r.level).unwrap_or(u8::MAX),
                r.ty.as_bytes(),
                r.name.as_bytes(),
                r.meta_flag.map_or(0, low32),
            )
        })
        .collect()
}

fn specs(nodes: &NodeList) -> Vec<NodeSpec> {
    nodes
        .rows()
        .iter()
        .map(|r| {
            let mut n = NodeSpec::new(u8::try_from(r.level).unwrap_or(u8::MAX), "", "");
            n.ty = r.ty.as_bytes().to_vec();
            n.name = r.name.as_bytes().to_vec();
            n.byte_size = r.byte_size;
            n.version = r.version;
            n.type_flags = r.type_flags.unwrap_or(0);
            n.meta_flag = r.meta_flag.map_or(0, low32);
            n.ref_type_hash = r.ref_type_hash.unwrap_or(0);
            n.index = r.index;
            n
        })
        .collect()
}

/// `depth`'s shapes, a few levels deep, through ref types `A` and `Z`.
fn depth_cases() -> Vec<(String, TreeCase)> {
    let to_rows = |rows: &[(u8, &str, &str, i32)]| -> Vec<Row> {
        rows.iter()
            .map(|(l, t, n, m)| Row::new(*l, t.as_bytes(), n.as_bytes(), *m))
            .collect()
    };
    let shapes = [
        Wrap::Class,
        Wrap::Vector,
        Wrap::Aligned,
        Wrap::ClassArray,
        Wrap::Pair,
        Wrap::PairArray,
    ];
    let mut out = Vec::new();
    for (i, w) in shapes.iter().enumerate() {
        let next = shapes[(i + 1) % shapes.len()];
        for wraps in [vec![*w], vec![*w, next]] {
            let d = Depth::new(&wraps);
            let a = to_rows(&d.rows);
            let refs = vec![
                RefType {
                    class: b"A".to_vec(),
                    namespace: b"n".to_vec(),
                    assembly: b"a".to_vec(),
                    rows: Some(a.clone()),
                },
                RefType {
                    class: b"Z".to_vec(),
                    namespace: b"n".to_vec(),
                    assembly: b"a".to_vec(),
                    rows: Some(to_rows(&Z_ROWS)),
                },
            ];
            for n in [1, 3] {
                let case = TreeCase {
                    flags: 0,
                    start: 0,
                    rows: a.clone(),
                    refs: refs.clone(),
                    data: d.data(n),
                };
                out.push((format!("depth-{wraps:?}-{n}"), case));
            }
        }
    }
    out
}

/// The format the `texture` target's check decodes `format` as
/// (`fuzz/src/texture.rs`): itself when it is one of the five, else the
/// five's `format mod 5`th, in `TextureFormat` order.
fn texture_decoded_as(format: i32) -> i32 {
    match format {
        3 | 4 | 10 | 12 | 25 => format,
        _ => match format.rem_euclid(5) {
            0 => 3,
            1 => 4,
            2 => 10,
            3 => 12,
            _ => 25,
        },
    }
}

/// What mip 0 of a `width` x `height` texture of `format` holds, decoded
/// as the check decodes it: 4x4 blocks of 8 bytes (DXT1) or 16 (DXT5,
/// BC7), else 3 bytes a pixel (RGB24) or 4 (RGBA32); 0 for a side of 0 or
/// below.
fn texture_need(format: i32, width: i32, height: i32) -> u64 {
    let (Ok(w), Ok(h)) = (u64::try_from(width), u64::try_from(height)) else {
        return 0;
    };
    let blocks = w.div_ceil(4) * h.div_ceil(4);
    match texture_decoded_as(format) {
        10 => blocks * 8,
        12 | 25 => blocks * 16,
        3 => w * h * 3,
        _ => w * h * 4,
    }
}

/// The `texture` target's seeds: each of [`TEXTURE_FORMATS`] on Windows at
/// each of [`TEXTURE_SIZES`], with data a byte short of mip 0, exact and a
/// byte long (at most [`TEXTURE_DATA`]; one seed for those of one length,
/// as where mip 0 holds nothing or more than that), under the memory bound
/// and a meter of the charge less 1; the five at 7x9 on Switch, with no
/// blob, one GOB per block and more, and on Xbox 360; and one block down
/// each decoder path ([`texture_blocks`]).
fn texture(s: &mut Seeds) -> std::io::Result<()> {
    let mut rng = Rng::new(0x7e57_7e57_0000_0001);
    let mut bytes = |len: u64| {
        let mut out = Vec::new();
        while u64::try_from(out.len()).unwrap_or(u64::MAX) < len {
            out.extend_from_slice(&rng.next_u64().to_le_bytes());
        }
        out.truncate(usize::try_from(len).unwrap_or(0));
        out
    };
    for format in TEXTURE_FORMATS {
        for (width, height) in TEXTURE_SIZES {
            let need = texture_need(format, width, height);
            let mut last = None;
            for (tag, len) in [
                ("short", need.saturating_sub(1)),
                ("exact", need),
                ("long", need + 1),
            ] {
                let len = len.min(TEXTURE_DATA);
                if last == Some(len) {
                    continue;
                }
                last = Some(len);
                let data = bytes(len);
                for meter in [0, 1] {
                    let case = TextureCase {
                        format,
                        width,
                        height,
                        platform: 19,
                        meter,
                        blob: None,
                        data: data.clone(),
                    };
                    let name = format!("f{format}-{width}x{height}-{tag}-m{meter}");
                    s.add("texture", &name, &case.encode())?;
                }
            }
        }
    }
    let swizzled = [0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0];
    let platforms = [
        ("switch-none", 38, None),
        ("switch-one-gob", 38, Some(vec![0; 12])),
        ("switch-swizzled", 38, Some(swizzled.to_vec())),
        ("xbox360", 11, None),
    ];
    for format in &TEXTURE_FORMATS[..5] {
        for (tag, platform, blob) in &platforms {
            let case = TextureCase {
                format: *format,
                width: 7,
                height: 9,
                platform: *platform,
                meter: 0,
                blob: blob.clone(),
                data: bytes(texture_need(*format, 7, 9)),
            };
            s.add("texture", &format!("f{format}-7x9-{tag}"), &case.encode())?;
        }
    }
    texture_blocks(s)
}

/// The rest of a `texture_blocks` BC7 block after its first byte.
const BC7_TAIL: [u8; 15] = [
    0x5a, 0xc3, 0x96, 0x3c, 0xa5, 0x0f, 0xf0, 0x69, 0x87, 0x1e, 0xe1, 0x2d, 0xd2, 0x4b, 0xb4,
];

/// A BC1 colour block: `c0` and `c1` (RGB565, little-endian), then indices
/// 0 to 3 along each row.
const fn bc1_block(c0: u16, c1: u16) -> [u8; 8] {
    let (a, b) = (c0.to_le_bytes(), c1.to_le_bytes());
    [a[0], a[1], b[0], b[1], 0xe4, 0xe4, 0xe4, 0xe4]
}

/// One 4x4 block down each decoder path, so the `texture` target's seeds
/// reach every one whatever the random seeds hold: a BC7 block of each
/// mode (its first byte's lowest set bit) and of none (a zero first byte,
/// Pillow's degenerate block); a DXT1 block with `c0 > c1` (four colours)
/// and with `c0 <= c1` (three and black); a DXT5 block with `a0 > a1`
/// (eight alphas) and with `a0 <= a1` (six, 0 and 255), its alpha indices
/// 0 to 7 twice over. Each at the memory bound, so it decodes.
fn texture_blocks(s: &mut Seeds) -> std::io::Result<()> {
    let mut add = |name: &str, format: i32, data: Vec<u8>| {
        let case = TextureCase {
            format,
            width: 4,
            height: 4,
            platform: 19,
            meter: 0,
            blob: None,
            data,
        };
        s.add("texture", name, &case.encode())
    };
    for mode in 0..=8 {
        let first = if mode == 8 { 0 } else { 1u8 << mode };
        let mut block = vec![first];
        block.extend_from_slice(&BC7_TAIL);
        add(&format!("bc7-mode{mode}"), 25, block)?;
    }
    let (red, green) = (0xf800, 0x07e0);
    add("dxt1-c0-above-c1", 10, bc1_block(red, green).to_vec())?;
    add("dxt1-c0-not-above-c1", 10, bc1_block(green, red).to_vec())?;
    let alpha_indices = [0x88, 0xc6, 0xfa, 0x88, 0xc6, 0xfa];
    for (name, a0, a1) in [
        ("dxt5-a0-above-a1", 0xff, 0x00),
        ("dxt5-a0-not-above-a1", 0x00, 0xff),
    ] {
        let mut block = vec![a0, a1];
        block.extend_from_slice(&alpha_indices);
        block.extend_from_slice(&bc1_block(red, green));
        add(name, 12, block)?;
    }
    Ok(())
}

/// The `astc` target's seeds: for each of the twelve ASTC LDR formats
/// (`TextureFormat` 48 to 59), the blocks the oracle fixture records for its
/// footprint (astcenc's encoder's, `SplitMix64`'s and the crafted ones), at
/// the fixture's size, under the memory bound and a meter of the peak
/// charge less 1; then a size of 0, a negative side and a byte short of the
/// crafted blocks.
fn astc(s: &mut Seeds) -> std::io::Result<()> {
    let text = std::fs::read_to_string(ASTC_CASES)?;
    let body: serde_json::Value = serde_json::from_str(&text).map_err(std::io::Error::other)?;
    let bad = || std::io::Error::other("astc.json: a case is not as the script writes it");
    for (i, side) in [4, 5, 6, 8, 10, 12].into_iter().enumerate() {
        for c in body["footprint_cases"].as_array().ok_or_else(bad)? {
            let id = c["id"].as_str().ok_or_else(bad)?;
            if c["block"] != serde_json::json!([side, side]) {
                continue;
            }
            let data = if let Some(h) = c["data"]["hex"].as_str() {
                h.as_bytes()
                    .chunks(2)
                    .map(|p| {
                        std::str::from_utf8(p)
                            .ok()
                            .and_then(|p| u8::from_str_radix(p, 16).ok())
                            .ok_or_else(bad)
                    })
                    .collect::<std::io::Result<Vec<u8>>>()?
            } else {
                let len = c["data"]["len"].as_u64().ok_or_else(bad)?;
                let mut rng = Rng::new(c["data"]["seed"].as_u64().ok_or_else(bad)?);
                let mut out = Vec::new();
                while u64::try_from(out.len()).unwrap_or(u64::MAX) < len {
                    out.extend_from_slice(&rng.next_u64().to_le_bytes());
                }
                out.truncate(usize::try_from(len).unwrap_or(0));
                out
            };
            let side_of = |key: &str| {
                c[key]
                    .as_i64()
                    .and_then(|v| i32::try_from(v).ok())
                    .ok_or_else(bad)
            };
            let (width, height) = (side_of("width")?, side_of("height")?);
            for format in [48 + i, 54 + i] {
                let format = i32::try_from(format).unwrap_or(48);
                let mut add = |tag: &str, width, height, meter, data: &[u8]| {
                    let case = TextureCase {
                        format,
                        width,
                        height,
                        platform: 19,
                        meter,
                        blob: None,
                        data: data[..data.len().min(4096)].to_vec(),
                    };
                    s.add("astc", &format!("f{format}-{id}-{tag}"), &case.encode())
                };
                add("m0", width, height, 0, &data)?;
                add("m1", width, height, 1, &data)?;
                if id.ends_with("-crafted") {
                    add("zero", 0, height, 0, &data)?;
                    add("negative", -width, height, 0, &data)?;
                    add("short", width, height, 0, &data[..data.len() - 1])?;
                }
            }
        }
    }
    Ok(())
}

/// The `meshdecode` target's seeds: every case of `unityrs-meshdecode`'s
/// handler fixture (`tests/fixtures/process.json`), its tree, value,
/// version, byte order, resource and reader as recorded, under each of
/// [`MESH_METERS`].
fn meshdecode(s: &mut Seeds) -> std::io::Result<()> {
    let text = std::fs::read_to_string(MESH_CASES)?;
    let body: serde_json::Value = serde_json::from_str(&text).map_err(std::io::Error::other)?;
    let bad = || std::io::Error::other("process.json: a case is not as the script writes it");
    let int = |v: &serde_json::Value| v.as_i64().ok_or_else(bad);
    let hex = |v: &serde_json::Value| -> std::io::Result<Vec<u8>> {
        let h = v.as_str().ok_or_else(bad)?.as_bytes();
        h.chunks(2)
            .map(|p| {
                std::str::from_utf8(p)
                    .ok()
                    .and_then(|p| u8::from_str_radix(p, 16).ok())
                    .ok_or_else(bad)
            })
            .collect()
    };
    for c in body["cases"].as_array().ok_or_else(bad)? {
        let tree = &body["trees"][c["tree"].as_str().ok_or_else(bad)?];
        let mut rows = Vec::new();
        for r in tree.as_array().ok_or_else(bad)? {
            let text = |i: usize| r[i].as_str().map(str::as_bytes).ok_or_else(bad);
            rows.push(Row::new(
                u8::try_from(int(&r[0])?).map_err(|_| bad())?,
                text(1)?,
                text(2)?,
                i32::try_from(int(&r[3])?).map_err(|_| bad())?,
            ));
        }
        let v = c["version"].as_array().ok_or_else(bad)?;
        let part = |i: usize| v.get(i).ok_or_else(bad).and_then(int);
        let mut flags = 0;
        if c["source"] == "sprite" {
            flags |= MeshCase::SPRITE;
        }
        if c["endian"] == ">" {
            flags |= MeshCase::BIG_ENDIAN;
        }
        if c["reader"].as_bool() == Some(true) && !c["resource"].is_null() {
            flags |= MeshCase::RESOURCES;
        }
        let mut case = MeshCase {
            flags,
            meter: 0,
            version: [
                u16::try_from(part(0)?).map_err(|_| bad())?,
                u16::try_from(part(1)?).map_err(|_| bad())?,
                u16::try_from(part(2)?).map_err(|_| bad())?,
            ],
            version_type: u8::try_from(part(3)?).map_err(|_| bad())?,
            version_number: u8::try_from(part(4)?).map_err(|_| bad())?,
            resource: if c["resource"].is_null() {
                Vec::new()
            } else {
                hex(&c["resource"])?
            },
            value: TreeCase {
                rows,
                data: hex(&c["hex"])?,
                ..TreeCase::default()
            },
        };
        let name = c["name"].as_str().ok_or_else(bad)?;
        for meter in MESH_METERS {
            case.meter = meter;
            s.add("meshdecode", &format!("{name}-m{meter}"), &case.encode())?;
        }
    }
    Ok(())
}

/// Every target's seeds, in a folder of `dir` each, and how many. Crate-wide:
/// `tests/fuzz_seeds.rs` includes this file and checks what it writes.
pub(crate) fn write(dir: &Path) -> std::io::Result<[usize; TARGETS.len()]> {
    for t in TARGETS {
        std::fs::create_dir_all(dir.join(t))?;
    }
    let mut s = Seeds {
        dir: dir.to_path_buf(),
        count: [0; TARGETS.len()],
        trees: BTreeSet::new(),
        nodes: Vec::new(),
    };
    let outputs = unityrs_testkit::mutate::builder_outputs();
    for (name, bytes) in &outputs {
        s.file(name, bytes, 0)?;
        s.add("zlib_match", name, &bytes[..bytes.len().min(4 << 20)])?;
        let plain = &bytes[..bytes.len().min(LZ4_MAX)];
        let block = compress(TestCodec::Lz4, plain);
        s.add("unitycn", name, &block)?;
        let exact = u32::try_from(plain.len()).unwrap_or(u32::MAX);
        for (tag, declared) in [
            ("exact", exact),
            ("over", exact.saturating_add(1)),
            ("short", exact.saturating_sub(1)),
            ("zero", 0),
        ] {
            s.add("lz4", &format!("{name}-{tag}"), &lz4_case(declared, &block))?;
        }
    }
    // `lz4.block.compress(b"")`, the empty block, declared 0 and 1.
    s.add("lz4", "empty", &lz4_case(0, &[0]))?;
    s.add("lz4", "empty-over", &lz4_case(1, &[0]))?;
    for (name, bytes) in [
        ("empty", b"".as_slice()),
        ("zero", &[0]),
        ("offset", &[0, 0]),
        ("length", &[0xf0]),
        ("extensions", &[0xff; 32]),
    ] {
        s.add("unitycn", name, bytes)?;
    }
    texture(&mut s)?;
    astc(&mut s)?;
    meshdecode(&mut s)?;
    for len in [2, 3, 257, 258, 259, 262, 32768, 65536, 65537, 131_333] {
        s.add("zlib_match", &format!("zero-{len}"), &vec![0; len])?;
        let periodic: Vec<_> = (0..len).map(|i| (i % 17).to_le_bytes()[0]).collect();
        s.add("zlib_match", &format!("periodic-{len}"), &periodic)?;
    }
    for (name, case) in depth_cases() {
        for r in case.refs.iter().filter_map(|r| r.rows.as_ref()) {
            let nodes = r
                .iter()
                .map(|x| {
                    let mut n = NodeSpec::new(x.level, "", "");
                    n.ty.clone_from(&x.ty);
                    n.name.clone_from(&x.name);
                    n.meta_flag = x.meta_flag;
                    n
                })
                .collect();
            s.tree(nodes);
        }
        s.value(&name, &case)?;
    }
    let nodes = std::mem::take(&mut s.nodes);
    for (i, tree) in nodes.iter().enumerate() {
        for format in TREE_FORMATS {
            for big in [false, true] {
                let f = u32::from(format);
                let bytes = if unityrs_core::typetree::parse::is_blob_format(f) {
                    blob_tree(tree, f, big, Strings::Local, &[])
                } else {
                    old_tree(tree, f, big)
                };
                let end = if big { "be" } else { "le" };
                s.add(
                    "typetree",
                    &format!("t{i}-f{format}-{end}"),
                    &blob_case(format, big, &bytes),
                )?;
            }
        }
        let common = blob_tree(tree, 19, false, Strings::Common, COMMON_STRINGS);
        s.add(
            "typetree",
            &format!("t{i}-f19-common"),
            &blob_case(19, false, &common),
        )?;
    }
    Ok(s.count)
}

fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [dir] = args.as_slice() else {
        eprintln!("usage: fuzz_seeds DIR (an empty or new folder)");
        return ExitCode::from(2);
    };
    if std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some()) {
        eprintln!("fuzz_seeds: {} is not empty", dir.display());
        return ExitCode::from(2);
    }
    match write(dir) {
        Ok(count) => {
            for (t, n) in TARGETS.iter().zip(count) {
                println!("{t}: {n} seeds");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("fuzz_seeds: {e}");
            ExitCode::FAILURE
        }
    }
}
