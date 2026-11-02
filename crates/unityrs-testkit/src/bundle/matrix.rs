// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/BundleFile.py, UnityPy/files/ObjectReader.py, UnityPy/files/SerializedFile.py, UnityPy/helpers/ImportHelper.py, UnityPy/helpers/TypeTreeNode.py. See THIRD_PARTY_LICENSES.
//! Package C's synthetic matrix ([`matrix`]) for the L0-L1 parity check,
//! and the specs it is built from.

use super::{BlockOverride, BundleSpec, EntrySpec, TestCodec, build, build_with_signature};
use crate::writer::ByteWriter;

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
/// (files/SerializedFile.py:266; detection, helpers/ImportHelper.py:110-121,
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
