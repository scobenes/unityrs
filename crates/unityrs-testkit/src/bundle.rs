//! `UnityFS` bundles (formats 6 to 8) and `UnityWeb`/`UnityRaw` (P3), built
//! from a spec, with every block codec the core reads.
//!
//! The layout is the one UnityPy 1.25.2's `BundleFile.read_fs` reads
//! (`files/BundleFile.py:96-186`), big-endian throughout: the signature, the
//! format, both version strings, the total size, blocks-info's stored and
//! decoded sizes and the data flags; the 16-byte alignment where UnityPy
//! applies it (format 7 and up, or a 2019 engine from 2019.4.15); blocks-info
//! (hash, blocks, directory) right there or at the end of the file (flag
//! 0x80); the 16-byte padding before the data under the new flag set with
//! 0x200; and the data blocks, each compressed on its own.
//!
//! Owner: package C.

use crate::writer::ByteWriter;

/// A block codec, as the flags' low six bits name it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TestCodec {
    /// 0.
    #[default]
    None,
    /// 1: LZMA "alone" props + data, as Unity writes it (no size field).
    Lzma,
    /// 2: LZ4 block.
    Lz4,
    /// 3: written as plain LZ4 (the decoder is the same).
    Lz4Hc,
    /// Any id, with the data stored as is (for refusal tests).
    Raw(u8),
}

impl TestCodec {
    /// The compression type number.
    #[must_use]
    pub fn id(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Lzma => 1,
            Self::Lz4 => 2,
            Self::Lz4Hc => 3,
            Self::Raw(n) => n,
        }
    }
}

/// Overrides for one block, so a test can place a malformed block before the
/// entry it reads (plan section 7.4, "one layout").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockOverride {
    /// Which block, in file order.
    pub block: usize,
    /// This block's codec instead of [`BundleSpec::data_codec`].
    pub codec: Option<TestCodec>,
    /// The declared decoded size written to blocks-info (an LZ4 block that
    /// decodes short, an LZMA block that outgrows it, a stored block whose
    /// declaration differs from its bytes).
    pub uncompressed: Option<u32>,
    /// The declared stored size written to blocks-info.
    pub compressed: Option<u32>,
}

/// One directory entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntrySpec {
    /// The path.
    pub name: Vec<u8>,
    /// The bytes.
    pub data: Vec<u8>,
    /// The flags (4 marks a `SerializedFile`).
    pub flags: u32,
}

/// A `UnityFS` bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleSpec {
    /// The format (6, 7 or 8).
    pub format: u32,
    /// `version_player`.
    pub version_player: Vec<u8>,
    /// `version_engine`.
    pub version_engine: Vec<u8>,
    /// The blocks-info codec.
    pub info_codec: TestCodec,
    /// The data codec.
    pub data_codec: TestCodec,
    /// Uncompressed block size; the data is cut into blocks of this size.
    pub block_size: usize,
    /// Blocks info at the end of the file (flag 0x80).
    pub info_at_end: bool,
    /// Entries, in directory order.
    pub entries: Vec<EntrySpec>,
    /// Extra data flags, combined with a bitwise or (0x200 block-info padding, 0x400 the old
    /// encryption bit, ...).
    pub extra_flags: u32,
    /// Per-block overrides.
    pub block_overrides: Vec<BlockOverride>,
    /// Cut the finished file to this length (a truncated last block).
    pub truncate_to: Option<usize>,
}

impl Default for BundleSpec {
    fn default() -> Self {
        Self {
            format: 7,
            version_player: b"5.x.x".to_vec(),
            version_engine: b"2022.3.0f1".to_vec(),
            info_codec: TestCodec::Lz4,
            data_codec: TestCodec::Lz4,
            block_size: 128 << 10,
            info_at_end: false,
            entries: Vec::new(),
            extra_flags: 0,
            block_overrides: Vec::new(),
            truncate_to: None,
        }
    }
}

/// `data` compressed by `codec`: LZMA as Unity stores it (the property byte
/// `0x5D`, the 4-byte dictionary size, the raw LZMA1 stream), LZ4 and LZ4HC
/// as one LZ4 block, anything else stored as it is.
#[must_use]
pub fn compress(codec: TestCodec, data: &[u8]) -> Vec<u8> {
    match codec {
        TestCodec::None | TestCodec::Raw(_) => data.to_vec(),
        TestCodec::Lz4 | TestCodec::Lz4Hc => lz4_flex::block::compress(data),
        TestCodec::Lzma => lzma(data),
    }
}

/// The dictionary size the LZMA blocks declare (Unity's 512 KiB).
pub const LZMA_DICT: u32 = 1 << 19;

thread_local! {
    /// Each LZMA stream by its input. G3b's mutator builds every output again
    /// for each field it replaces (`mutate`), so most blocks repeat, and the
    /// encoder's set-up (its dictionary and match finder) costs more than the
    /// rest of a build.
    static LZMA_MEMO: std::cell::RefCell<std::collections::HashMap<Vec<u8>, Vec<u8>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// An LZMA block as Unity stores it: the properties byte, the dictionary size
/// (a writer field, written on every call, so a memo hit numbers the same
/// fields), then the raw stream.
fn lzma(data: &[u8]) -> Vec<u8> {
    // lc 3, lp 0, pb 2: (2 * 5 + 0) * 9 + 3.
    let mut out = ByteWriter::le();
    out.u8(0x5D).u32(LZMA_DICT);
    let mut out = out.into_inner();
    if let Some(hit) = LZMA_MEMO.with(|m| m.borrow().get(data).cloned()) {
        out.extend_from_slice(&hit);
        return out;
    }
    let stream = lzma_encode(data);
    LZMA_MEMO.with(|m| {
        let mut m = m.borrow_mut();
        // A bound, so a long-lived thread does not keep every block forever.
        if m.len() >= 4096 {
            m.clear();
        }
        m.insert(data.to_vec(), stream.clone());
    });
    out.extend_from_slice(&stream);
    out
}

/// The raw LZMA stream of `data` (no header), empty when the encoder fails.
fn lzma_encode(data: &[u8]) -> Vec<u8> {
    use liblzma::stream::{Action, Filters, LzmaOptions, Status, Stream};
    let mut opts = LzmaOptions::new_preset(1).unwrap_or_else(|_| LzmaOptions::new());
    opts.dict_size(LZMA_DICT)
        .literal_context_bits(3)
        .literal_position_bits(0)
        .position_bits(2);
    let mut filters = Filters::new();
    filters.lzma1(&opts);
    let Ok(mut stream) = Stream::new_raw_encoder(&filters) else {
        return Vec::new();
    };
    let mut buf = vec![0u8; data.len() + data.len() / 2 + 1024];
    loop {
        let (read, written) = (stream.total_in(), stream.total_out());
        let input = data.get(as_usize(read)..).unwrap_or_default();
        let window = buf.get_mut(as_usize(written)..).unwrap_or_default();
        match stream.process(input, window, Action::Finish) {
            Ok(Status::StreamEnd) | Err(_) => break,
            Ok(_) if window_full(&stream, buf.len()) => buf.resize(buf.len() * 2, 0),
            Ok(_) => {}
        }
    }
    buf.truncate(as_usize(stream.total_out()));
    buf
}

fn window_full(stream: &liblzma::stream::Stream, len: usize) -> bool {
    as_usize(stream.total_out()) >= len
}

fn as_usize(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// `(major, minor, build)` of an ASCII engine string, as far as it parses.
fn engine_triple(engine: &[u8]) -> Option<(u32, u32, u32)> {
    let s = std::str::from_utf8(engine).ok()?;
    let mut parts = s.splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let rest = parts.next()?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some((major, minor, digits.parse().ok()?))
}

/// Whether UnityPy reads these flags as `ArchiveFlags` (the new set) for
/// this engine (`BundleFile.py:113-121`), on the engine's ASCII digits.
fn new_flags(engine: &[u8]) -> bool {
    let Some(v) = engine_triple(engine) else {
        return false;
    };
    !(v < (2020, 0, 0)
        || (v.0 == 2020 && v < (2020, 3, 34))
        || (v.0 == 2021 && v < (2021, 3, 2))
        || (v.0 == 2022 && v < (2022, 1, 1)))
}

/// Whether UnityPy aligns to 16 after the header (`BundleFile.py:129-131`).
fn aligned(format: u32, engine: &[u8]) -> bool {
    format >= 7 || engine_triple(engine).is_some_and(|v| v.0 == 2019 && v >= (2019, 4, 15))
}

/// The bundle's bytes.
#[must_use]
pub fn build(spec: &BundleSpec) -> Vec<u8> {
    build_with_signature(spec, b"UnityFS")
}

/// The bundle's bytes under another signature: `UnityWeb` or `UnityRaw` at
/// format 6 take the UnityFS layout with one more byte after the data flags
/// (`BundleFile.py:104-106`), which this writes for any signature but
/// `UnityFS`.
#[must_use]
pub fn build_with_signature(spec: &BundleSpec, signature: &[u8]) -> Vec<u8> {
    // The decoded data and the directory.
    let mut data = Vec::new();
    let mut directory = ByteWriter::be();
    directory.i32(i32::try_from(spec.entries.len()).unwrap_or(i32::MAX));
    for e in &spec.entries {
        directory
            .i64(i64::try_from(data.len()).unwrap_or(i64::MAX))
            .i64(i64::try_from(e.data.len()).unwrap_or(i64::MAX))
            .u32(e.flags)
            .cstr(&e.name);
        data.extend_from_slice(&e.data);
    }

    // The blocks.
    let size = spec.block_size.max(1);
    let mut stored = Vec::new();
    let mut rows = ByteWriter::be();
    let chunks: Vec<&[u8]> = data.chunks(size).collect();
    let count = chunks.len().max(
        spec.block_overrides
            .iter()
            .map(|o| o.block + 1)
            .max()
            .unwrap_or(0),
    );
    rows.bytes(&[0; 16]);
    rows.i32(i32::try_from(count).unwrap_or(i32::MAX));
    for i in 0..count {
        let chunk = chunks.get(i).copied().unwrap_or_default();
        let over = spec.block_overrides.iter().find(|o| o.block == i);
        let codec = over.and_then(|o| o.codec).unwrap_or(spec.data_codec);
        let bytes = compress(codec, chunk);
        let uncompressed = over
            .and_then(|o| o.uncompressed)
            .unwrap_or_else(|| u32::try_from(chunk.len()).unwrap_or(u32::MAX));
        let compressed = over
            .and_then(|o| o.compressed)
            .unwrap_or_else(|| u32::try_from(bytes.len()).unwrap_or(u32::MAX));
        rows.u32(uncompressed)
            .u32(compressed)
            .u16(u16::from(codec.id()));
        stored.extend_from_slice(&bytes);
    }
    rows.bytes(directory.as_slice());
    let info = rows.into_inner();
    let info_stored = compress(spec.info_codec, &info);

    let mut flags = u32::from(spec.info_codec.id()) | 0x40 | spec.extra_flags;
    if spec.info_at_end {
        flags |= 0x80;
    }

    let mut w = ByteWriter::be();
    w.cstr(signature)
        .u32(spec.format)
        .cstr(&spec.version_player)
        .cstr(&spec.version_engine);
    let size_at = w.len();
    w.i64(0)
        .u32(u32::try_from(info_stored.len()).unwrap_or(u32::MAX))
        .u32(u32::try_from(info.len()).unwrap_or(u32::MAX))
        .u32(flags);
    if signature != b"UnityFS" {
        w.u8(0);
    }
    if aligned(spec.format, &spec.version_engine) {
        w.align(16);
    }
    let pad = flags & 0x200 != 0 && new_flags(&spec.version_engine);
    if !spec.info_at_end {
        w.bytes(&info_stored);
    }
    if pad {
        w.align(16);
    }
    w.bytes(&stored);
    if spec.info_at_end {
        w.bytes(&info_stored);
    }
    let mut file = w.into_inner();
    // Through a writer, so the total is a field (`writer::with_fields`).
    let mut total = ByteWriter::be();
    total.i64(i64::try_from(file.len()).unwrap_or(i64::MAX));
    if let Some(at) = file.get_mut(size_at..size_at + 8) {
        at.copy_from_slice(total.as_slice());
    }
    if let Some(n) = spec.truncate_to {
        file.truncate(n);
    }
    file
}

/// Package C's synthetic matrix for the L0-L1 parity check (plan section
/// 7.4, "Done when"): `(name, bytes)` inputs covering every `SerializedFile`
/// format both endians, UnityFS 6-8 with every codec, blocks-info at either
/// end, both alignment rules, padding, both flag sets, UnityWeb and UnityRaw
/// at format 6, nested bundles, repeated paths, block overrides, and an
/// AssetBundle (class 142) loose and inside a bundle, whose container rows
/// the parse reads (C request 9). `tools/fixtures/files.py` records UnityPy's
/// L0 and L1 records for each.
#[must_use]
// A table of cases, one per line.
#[allow(clippy::too_many_lines)]
pub fn matrix() -> Vec<(String, Vec<u8>)> {
    use crate::serialized::build as file;
    let mut out = Vec::new();
    for format in 1..=22 {
        for big in [false, true] {
            let end = if big { "be" } else { "le" };
            out.push((
                format!("CAB-f{format}-{end}"),
                file(&matrix_file(format, big)),
            ));
        }
    }
    let mut no_trees = matrix_file(17, false);
    no_trees.type_trees = false;
    out.push(("CAB-f17-notrees".into(), file(&no_trees)));
    out.push(("sample.resS".into(), file(&matrix_file(22, false))));
    let mut blank = matrix_file(22, false);
    blank.unity_version = Vec::new();
    out.push(("CAB-blank-version".into(), file(&blank)));
    // An AssetBundle (class 142), whose container the parse reads (C9).
    let assetbundle = |hostile: bool| {
        use crate::env::{Bundle, Obj, row};
        crate::env::serialized(
            &["archive:/CAB-x/CAB-x"],
            &[
                Obj::named(1, "n", &[(0, 2), (1, 5)]),
                Obj::asset_bundle(
                    2,
                    Bundle {
                        preload: vec![(0, 1), (1, 5)],
                        rows: vec![
                            row("assets/n.prefab", (0, 2), (0, 1)),
                            row("assets/n.prefab", (1, 1), (0, 2)),
                        ],
                        hostile,
                        ..Bundle::default()
                    },
                ),
            ],
        )
    };
    out.push(("CAB-assetbundle".into(), assetbundle(false)));
    out.push((
        "assetbundle.bundle".into(),
        crate::env::bundle(&[("CAB-assetbundle", assetbundle(false))]),
    ));
    // The same AssetBundle's container as a damaged tree holds it
    // (`env::Bundle::hostile`): UnityPy loads it, each row's members it
    // holds as no string or `int` dumped as `null`.
    out.push(("CAB-assetbundle-hostile".into(), assetbundle(true)));

    let codecs = [
        ("none", TestCodec::None),
        ("lzma", TestCodec::Lzma),
        ("lz4", TestCodec::Lz4),
        ("lz4hc", TestCodec::Lz4Hc),
    ];
    for format in [6, 7, 8] {
        for (cname, codec) in codecs {
            for at_end in [false, true] {
                let spec = BundleSpec {
                    format,
                    data_codec: codec,
                    info_codec: codec,
                    info_at_end: at_end,
                    block_size: 160,
                    entries: matrix_entries(),
                    ..BundleSpec::default()
                };
                let end = if at_end { "end" } else { "front" };
                out.push((format!("b{format}-{cname}-{end}.bundle"), build(&spec)));
            }
        }
    }
    let base = || BundleSpec {
        block_size: 160,
        entries: matrix_entries(),
        ..BundleSpec::default()
    };
    let mut cases: Vec<(&str, BundleSpec)> = vec![
        (
            "padding.bundle",
            BundleSpec {
                extra_flags: 0x200,
                ..base()
            },
        ),
        (
            "padding-f6.bundle",
            BundleSpec {
                format: 6,
                extra_flags: 0x200,
                ..base()
            },
        ),
        (
            "old-flags-encrypted.bundle",
            BundleSpec {
                version_engine: b"2020.3.33f1".to_vec(),
                extra_flags: 0x200,
                ..base()
            },
        ),
        (
            "new-flags-encrypted.bundle",
            BundleSpec {
                extra_flags: 0x400,
                ..base()
            },
        ),
        (
            "f6-2019.4.15.bundle",
            BundleSpec {
                format: 6,
                version_engine: b"2019.4.15f1".to_vec(),
                ..base()
            },
        ),
        (
            "f6-2019.4.14.bundle",
            BundleSpec {
                format: 6,
                version_engine: b"2019.4.14f1".to_vec(),
                ..base()
            },
        ),
        (
            "f6-2018.bundle",
            BundleSpec {
                format: 6,
                version_engine: b"2018.4.36f1".to_vec(),
                ..base()
            },
        ),
        (
            "engine-big-major.bundle",
            BundleSpec {
                version_engine: b"133091.1.1f1".to_vec(),
                ..base()
            },
        ),
        (
            "engine-major-0.bundle",
            BundleSpec {
                version_engine: b"0.5.1f1".to_vec(),
                ..base()
            },
        ),
        (
            "engine-nomatch.bundle",
            BundleSpec {
                version_engine: b"abc".to_vec(),
                ..base()
            },
        ),
        (
            "engine-type-byte.bundle",
            BundleSpec {
                version_engine: b"2020.3.34f1024".to_vec(),
                ..base()
            },
        ),
        (
            "engine-2021.3.2.bundle",
            BundleSpec {
                version_engine: b"2021.3.2ff1".to_vec(),
                ..base()
            },
        ),
        (
            "one-block.bundle",
            BundleSpec {
                block_size: 1 << 20,
                ..base()
            },
        ),
        (
            "tiny-blocks.bundle",
            BundleSpec {
                block_size: 7,
                ..base()
            },
        ),
        (
            "lz4-declared-larger.bundle",
            BundleSpec {
                block_overrides: vec![over(0, None, Some(500), None)],
                ..base()
            },
        ),
        (
            "lz4-declared-smaller.bundle",
            BundleSpec {
                block_overrides: vec![over(0, None, Some(100), None)],
                ..base()
            },
        ),
        (
            "lzma-overlong.bundle",
            BundleSpec {
                data_codec: TestCodec::Lzma,
                block_overrides: vec![over(0, None, Some(100), None)],
                ..base()
            },
        ),
        (
            "stored-declared-differs.bundle",
            BundleSpec {
                data_codec: TestCodec::None,
                block_overrides: vec![over(1, None, Some(40), None)],
                ..base()
            },
        ),
        (
            "stored-compressed-larger.bundle",
            BundleSpec {
                data_codec: TestCodec::None,
                block_overrides: vec![over(0, None, None, Some(200))],
                ..base()
            },
        ),
        (
            "lzham-block.bundle",
            BundleSpec {
                block_overrides: vec![over(2, Some(TestCodec::Raw(4)), None, None)],
                ..base()
            },
        ),
        (
            "unknown-codec-block.bundle",
            BundleSpec {
                block_overrides: vec![over(1, Some(TestCodec::Raw(9)), None, None)],
                ..base()
            },
        ),
        (
            "unknown-codec-info.bundle",
            BundleSpec {
                info_codec: TestCodec::Raw(7),
                ..base()
            },
        ),
        (
            "repeated-path.bundle",
            BundleSpec {
                entries: {
                    let mut e = matrix_entries();
                    e.push(EntrySpec {
                        name: b"CAB-entry".to_vec(),
                        data: file(&matrix_file(21, true)),
                        flags: 4,
                    });
                    e
                },
                ..base()
            },
        ),
        (
            "blank-version-entry.bundle",
            BundleSpec {
                entries: vec![EntrySpec {
                    name: b"CAB-blank".to_vec(),
                    data: file(&blank),
                    flags: 4,
                }],
                ..base()
            },
        ),
        (
            "nested.bundle",
            BundleSpec {
                entries: vec![
                    EntrySpec {
                        name: b"inner".to_vec(),
                        data: build(&base()),
                        flags: 0,
                    },
                    EntrySpec {
                        name: b"CAB-outer".to_vec(),
                        data: file(&matrix_file(19, false)),
                        flags: 4,
                    },
                ],
                ..base()
            },
        ),
        (
            "bad-entry.bundle",
            BundleSpec {
                entries: vec![
                    EntrySpec {
                        name: b"CAB-good".to_vec(),
                        data: file(&matrix_file(22, false)),
                        flags: 4,
                    },
                    EntrySpec {
                        name: b"CAB-bad".to_vec(),
                        data: file(&blank)[..200].to_vec(),
                        flags: 4,
                    },
                ],
                ..base()
            },
        ),
        (
            "empty.bundle",
            BundleSpec {
                entries: Vec::new(),
                ..base()
            },
        ),
    ];
    let whole = build(&base());
    cases.push((
        "truncated.bundle",
        BundleSpec {
            truncate_to: Some(whole.len() - 30),
            data_codec: TestCodec::None,
            ..base()
        },
    ));
    for (name, spec) in cases {
        out.push((name.to_owned(), build(&spec)));
    }
    for sig in [&b"UnityWeb"[..], b"UnityRaw"] {
        let spec = BundleSpec {
            format: 6,
            ..base()
        };
        let name = format!("{}-f6.bundle", String::from_utf8_lossy(sig));
        out.push((name, build_with_signature(&spec, sig)));
    }
    out.extend(negative_position_sites(&base()));
    out
}

/// Plan 3.3's negative-position sites, each patched into a well-formed
/// input: a negative `node_count` and `stringbuffer_size` (UnityPy
/// helpers/TypeTreeNode.py:113-118 reads `read(24 * node_count)`), a
/// format-8 header whose `metadata_size` exceeds `file_size`
/// (files/SerializedFile.py:266; detection, helpers/ImportHelper.py:110-120,
/// already keeps such a file raw, so the oracle records a raw file),
/// blocks-info at the end whose stored size
/// exceeds the file (files/BundleFile.py:134-135), and a format-22 object
/// whose `byte_start` plus `data_offset` passes `i64::MAX`
/// (files/ObjectReader.py:82-86).
fn negative_position_sites(bundle: &BundleSpec) -> Vec<(String, Vec<u8>)> {
    use crate::blob::{Strings, blob_tree};
    use crate::serialized::build as file;
    let mut out = Vec::new();
    for (name, field, value) in [
        ("CAB-negative-node-count", 0usize, -1i32),
        ("CAB-negative-stringbuffer", 4, -5),
    ] {
        let spec = matrix_file(22, false);
        let mut bytes = file(&spec);
        let nodes = spec.types.first().map_or(&[][..], |t| t.nodes.as_slice());
        let blob = blob_tree(nodes, 22, false, Strings::Local, &[]);
        if let Some(at) = bytes.windows(blob.len()).position(|w| w == blob.as_slice()) {
            bytes[at + field..at + field + 4].copy_from_slice(&value.to_le_bytes());
            out.push((name.to_owned(), bytes));
        }
    }
    let mut f8 = file(&matrix_file(8, false));
    let past = u32::try_from(f8.len())
        .unwrap_or(u32::MAX)
        .saturating_add(64);
    f8[0..4].copy_from_slice(&past.to_be_bytes());
    out.push(("CAB-f8-metadata-past-start".into(), f8));
    let mut end = build(&BundleSpec {
        info_at_end: true,
        ..bundle.clone()
    });
    // The header: signature and format (12 bytes), the two version strings,
    // the total size (8 bytes), then the stored blocks-info size.
    let nul =
        |b: &[u8], from: usize| from + b[from..].iter().position(|c| *c == 0).unwrap_or(0) + 1;
    let at = nul(&end, nul(&end, 12)) + 8;
    let past = u32::try_from(end.len())
        .unwrap_or(u32::MAX)
        .saturating_add(100);
    end[at..at + 4].copy_from_slice(&past.to_be_bytes());
    out.push(("info-end-past-start.bundle".into(), end));
    // The first object's row (path id 1, start 0, size 12) given the start
    // `i64::MAX`: detection bounds `data_offset` by the file, not a row.
    let mut big = file(&matrix_file(22, false));
    let row: Vec<u8> = [
        &1i64.to_le_bytes()[..],
        &0i64.to_le_bytes(),
        &12u32.to_le_bytes(),
    ]
    .concat();
    if let Some(at) = big.windows(row.len()).position(|w| w == row.as_slice()) {
        big[at + 8..at + 16].copy_from_slice(&i64::MAX.to_le_bytes());
        out.push(("CAB-f22-byte-start-past-i64".into(), big));
    }
    out
}

fn over(
    block: usize,
    codec: Option<TestCodec>,
    uncompressed: Option<u32>,
    compressed: Option<u32>,
) -> BlockOverride {
    BlockOverride {
        block,
        codec,
        uncompressed,
        compressed,
    }
}

/// A bundle's entries in the matrix: a `SerializedFile` and its resource.
fn matrix_entries() -> Vec<EntrySpec> {
    vec![
        EntrySpec {
            name: b"CAB-entry".to_vec(),
            data: crate::serialized::build(&matrix_file(22, false)),
            flags: 4,
        },
        EntrySpec {
            name: b"CAB-entry.resS".to_vec(),
            data: (0..300u32)
                .map(|i| u8::try_from(i % 251).unwrap_or(0))
                .collect(),
            flags: 0,
        },
    ]
}

/// A `SerializedFile` of `format` exercising each table: two types (one
/// that reads a script id), repeated path ids, script indexes, an external,
/// a ref type from 20, user information.
fn matrix_file(format: u32, big_endian: bool) -> crate::serialized::SerializedSpec {
    use crate::blob::NodeSpec;
    use crate::serialized::{ExternalSpec, ObjectSpec, SerializedSpec, TypeSpec};
    let nodes = vec![
        NodeSpec::new(0, "GameObject", "Base"),
        NodeSpec::new(1, "int", "m_A").size(4),
        NodeSpec::new(1, "UInt8", "m_B").size(1).aligned(),
        NodeSpec::new(1, "vector", "m_C"),
        NodeSpec::new(2, "Array", "Array").size(-1),
        NodeSpec::new(3, "int", "size").size(4),
        NodeSpec::new(3, "char", "data").size(1),
    ];
    let script_class = if format < 16 { -3 } else { 114 };
    let types = vec![
        TypeSpec {
            class_id: 1,
            script_type_index: -1,
            old_type_hash: [7; 16],
            nodes: nodes.clone(),
            dependencies: vec![1, 2],
            ..TypeSpec::default()
        },
        TypeSpec {
            class_id: script_class,
            script_type_index: 0,
            script_id: [5; 16],
            old_type_hash: [6; 16],
            nodes: vec![
                NodeSpec::new(0, "MonoBehaviour", "Base"),
                NodeSpec::new(1, "int", "m_X").size(4),
            ],
            ..TypeSpec::default()
        },
    ];
    let type_id = |i: usize| -> i32 {
        if format >= 16 {
            i32::try_from(i).unwrap_or(0)
        } else {
            types.get(i).map_or(0, |t| t.class_id)
        }
    };
    let class = |i: usize| {
        u16::try_from(types.get(i).map_or(0, |t| t.class_id.rem_euclid(65536))).unwrap_or(0)
    };
    let object = |path_id: i64, t: usize, script: Option<i16>| ObjectSpec {
        path_id,
        type_id: type_id(t),
        class_id: class(t),
        // m_A, m_B aligned, and m_C's count (0), each written as a field
        // (`writer::with_fields`).
        data: {
            let mut w = ByteWriter::le();
            w.i32(1).u8(2).align(4).i32(0);
            w.into_inner()
        },
        script_type_index: script,
        ..ObjectSpec::default()
    };
    let objects = vec![
        object(1, 0, None),
        object(-3, 1, Some(0)),
        object(2, 0, None),
        object(-3, 1, Some(1)),
    ];
    SerializedSpec {
        format,
        big_endian,
        unity_version: b"2022.3.0f1".to_vec(),
        platform: 19,
        types,
        objects,
        scripts: vec![(0, 11), (1, 1 << 40)],
        // Below format 6 the external has no `temp_empty` (UnityPy leaves it
        // unset and the oracle records `null`, C request 3).
        externals: vec![ExternalSpec {
            temp_empty: Vec::new(),
            guid: [3; 16],
            kind: 3,
            path: b"library/unity default resources".to_vec(),
        }],
        ref_types: vec![TypeSpec {
            class_id: 114,
            script_type_index: 0,
            script_id: [9; 16],
            old_type_hash: [8; 16],
            nodes: vec![
                NodeSpec::new(0, "Ref", "Base"),
                NodeSpec::new(1, "float", "m_F").size(4),
            ],
            ref_names: Some((b"Foo".to_vec(), b"Ns".to_vec(), b"Asm.dll".to_vec())),
            ..TypeSpec::default()
        }],
        user_information: b"ui".to_vec(),
        big_id_enabled: i32::from(format == 10 || format == 12),
        ..SerializedSpec::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockOverride, BundleSpec, EntrySpec, TestCodec, build, compress, new_flags};

    fn spec(codec: TestCodec) -> BundleSpec {
        BundleSpec {
            data_codec: codec,
            info_codec: codec,
            block_size: 64,
            entries: vec![
                EntrySpec {
                    name: b"CAB-a".to_vec(),
                    data: (0..100u8).collect(),
                    flags: 4,
                },
                EntrySpec {
                    name: b"CAB-a.resS".to_vec(),
                    data: vec![9; 50],
                    flags: 0,
                },
            ],
            ..BundleSpec::default()
        }
    }

    #[test]
    fn the_header_is_big_endian_and_aligned_at_format_7() {
        let b = build(&spec(TestCodec::None));
        assert!(b.starts_with(b"UnityFS\0\0\0\0\x07"));
        let at = 8 + 4 + 6 + 11;
        let total = i64::from_be_bytes(b[at..at + 8].try_into().unwrap());
        assert_eq!(usize::try_from(total).unwrap(), b.len());
        let flags = u32::from_be_bytes(b[at + 16..at + 20].try_into().unwrap());
        assert_eq!(flags, 0x40);
        // Header 49 bytes, aligned to 64; blocks-info: 16 + 4 + 3 * 10 + 4
        // + 2 rows; then 150 data bytes.
        let info = 16 + 4 + 30 + 4 + (20 + 6) + (20 + 11);
        assert_eq!(b.len(), 64 + info + 150);
    }

    #[test]
    fn every_codec_round_trips() {
        let data: Vec<u8> = (0..5000u32).map(|i| u8::try_from(i % 7).unwrap()).collect();
        let lz4 = compress(TestCodec::Lz4, &data);
        assert_eq!(lz4_flex::block::decompress(&lz4, data.len()).unwrap(), data);
        let lzma = compress(TestCodec::Lzma, &data);
        assert_eq!(lzma[0], 0x5D);
        assert!(lzma.len() < data.len());
        for c in [TestCodec::Lzma, TestCodec::Lz4, TestCodec::Lz4Hc] {
            let b = build(&spec(c));
            assert!(b.len() > 64, "{c:?}");
        }
    }

    #[test]
    fn overrides_and_truncation() {
        let mut s = spec(TestCodec::None);
        s.block_overrides = vec![BlockOverride {
            block: 1,
            codec: Some(TestCodec::Raw(9)),
            uncompressed: Some(7),
            compressed: None,
        }];
        s.truncate_to = Some(100);
        assert_eq!(build(&s).len(), 100);
        assert!(new_flags(b"2022.3.0f1"));
        assert!(!new_flags(b"2020.3.33f1"));
        assert!(!new_flags(b"2019.4.40f1"));
        assert!(new_flags(b"2021.3.2f1"));
    }

    #[test]
    fn an_lzma_dictionary_size_is_a_field_memo_or_not() {
        use super::{LZMA_DICT, compress};
        use crate::writer::{Field, with_fields};
        let data: Vec<u8> = (0..300u32).map(|i| (i % 7) as u8).collect();
        let dict = Field {
            width: 4,
            big_endian: false,
            value: u64::from(LZMA_DICT),
        };
        // Encoded, then served from the memo: the same bytes and fields.
        let (first, f1) = with_fields(&[], || compress(TestCodec::Lzma, &data));
        let (again, f2) = with_fields(&[], || compress(TestCodec::Lzma, &data));
        assert_eq!((&first, &f1), (&again, &f2));
        assert_eq!(f1, [dict]);
        assert_eq!(first[1..5], LZMA_DICT.to_le_bytes());
        // Replaced, it is written as given, the stream unchanged.
        let (huge, _) = with_fields(&[(0, 0xFFFF_FFFF)], || compress(TestCodec::Lzma, &data));
        assert_eq!(huge[1..5], [0xFF; 4]);
        assert_eq!((huge[0], &huge[5..]), (first[0], &first[5..]));
    }
}
