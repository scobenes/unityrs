//! The hand-built records the stream tests dump, and the inputs that carry
//! them: every record type of `docs/CANONICAL_DUMP.md` section 4 and every
//! value of `tests/fixtures/dump/encode.json`.

use std::sync::Arc;

use crate::common::{Tmp, fixture, parsed};
use unityrs_core::detect::{FileType, ParsedAs};
use unityrs_core::typetree::{NodeList, NodeRow};
use unityrs_core::version::{FlagsClass, Packed};
use unityrs_core::walk::L2Kind;
use unityrs_core::{Bytes, RawStr};
use unityrs_dump::encode::encode;
use unityrs_dump::json::sha256;
use unityrs_dump::record::{
    BlockRow, BundleL1, ContainerRow, DirRow, ExternalRow, FileError, FileErrorKind, FileL1,
    FileRecord, L0, L2, L2Error, L3, ObjectL1, ObjectRecord, Resave, SerializedHeaderRow,
    SerializedL1, TypeRecord, file_line, object_line,
};
use unityrs_dump::run::write_dump;
use unityrs_dump::select::Input;
use unityrs_dump::{DumpOptions, InputDump, Levels};

pub(crate) fn raw(s: &[u8]) -> RawStr {
    RawStr::from(s)
}

pub(crate) fn l0(ty: Option<FileType>, parsed_as: ParsedAs, bytes: &[u8]) -> L0 {
    L0 {
        ty,
        parsed_as,
        size: bytes.len() as u64,
        sha256: sha256(bytes),
    }
}

pub(crate) fn file(
    dump: &mut InputDump,
    path: &[RawStr],
    l0: L0,
    l1: Option<FileL1>,
    levels: Levels,
) {
    dump.push_file(file_line(&FileRecord {
        file: path.to_vec(),
        l0,
        l1: if levels.l1 { l1 } else { None },
        error: None,
    }));
}

fn tree(rows: &[(i32, &[u8], &[u8])]) -> Arc<NodeList> {
    let rows = rows
        .iter()
        .enumerate()
        .map(|(i, (level, ty, name))| NodeRow {
            level: *level,
            ty: raw(ty),
            name: raw(name),
            byte_size: -1,
            version: 1,
            type_flags: Some(0),
            variable_count: None,
            index: Some(i32::try_from(i).unwrap()),
            meta_flag: Some(0x4000),
            ref_type_hash: Some(u64::MAX),
        })
        .collect();
    Arc::new(NodeList::trusted(rows).unwrap())
}

pub(crate) fn serialized_l1(
    object_order: Vec<i64>,
    version_packed: Option<Packed>,
) -> SerializedL1 {
    let plain = TypeRecord {
        class_id: 114,
        is_stripped_type: Some(false),
        script_type_index: -1,
        script_id: Some([0xab; 16]),
        old_type_hash: Some([0x01; 16]),
        nodes: Some(tree(&[
            (0, b"MonoBehaviour", b"Base"),
            (1, b"int", b"m_X"),
            (1, b"\xffT", b"n\xfe"),
        ])),
        type_dependencies: Some(vec![0, -1]),
        class_name: None,
        namespace: None,
        assembly: None,
    };
    let stripped = TypeRecord {
        class_id: 28,
        is_stripped_type: None,
        script_type_index: 3,
        script_id: None,
        old_type_hash: None,
        nodes: None,
        type_dependencies: None,
        class_name: None,
        namespace: None,
        assembly: None,
    };
    let reference = TypeRecord {
        class_id: 114,
        is_stripped_type: Some(true),
        script_type_index: -1,
        script_id: None,
        old_type_hash: Some([0; 16]),
        nodes: Some(tree(&[(0, b"Foo", b"Base")])),
        type_dependencies: None,
        class_name: Some(raw(b"Foo")),
        namespace: Some(raw(b"")),
        assembly: Some(raw(b"Asm\xff.dll")),
    };
    SerializedL1 {
        header: SerializedHeaderRow {
            metadata_size: 100,
            file_size: 4096,
            version: 22,
            data_offset: 4096,
            big_endian: false,
            reserved: Some([0, 0, 0]),
            unknown: 0,
        },
        unity_version: raw(b"2022.3.1f1"),
        version_packed,
        target_platform: Some(19),
        enable_type_tree: true,
        big_id_enabled: 0,
        types: vec![plain, stripped],
        object_order,
        script_types: vec![(0, 11_400_000), (-1, i64::MIN)],
        externals: vec![
            ExternalRow {
                temp_empty: Some(raw(b"")),
                guid: Some([0x0f; 16]),
                kind: Some(0),
                path: raw(b"archive:/CAB-other/CAB-other"),
            },
            ExternalRow {
                temp_empty: None,
                guid: None,
                kind: None,
                path: raw(b"lib\xff"),
            },
        ],
        ref_types: Some(vec![reference]),
        user_information: Some(raw(b"")),
        container: vec![
            ContainerRow {
                path: Some(raw(b"assets/a\xff.prefab")),
                preload_index: Some(0),
                preload_size: Some(-1),
                file_id: Some(0),
                path_id: Some(i128::from(u64::MAX)),
            },
            ContainerRow {
                path: None,
                preload_index: None,
                preload_size: Some(1),
                file_id: None,
                path_id: Some(i128::from(i64::MIN)),
            },
        ],
    }
}

pub(crate) fn l1_of(path_id: i64) -> ObjectL1 {
    ObjectL1 {
        type_id: 0,
        class_id: 114,
        byte_start: i128::from(path_id.unsigned_abs()) + 4096,
        byte_size: 16,
        is_destroyed: None,
        is_stripped: Some(0),
    }
}

pub(crate) fn l3_of(i: usize, resave: bool) -> L3 {
    L3 {
        raw_size: 16,
        raw_sha256: sha256(&i.to_le_bytes()),
        resave: if resave {
            match i % 3 {
                0 => Resave::Equal,
                1 => Resave::Differs {
                    size: 20,
                    sha256: sha256(b"x"),
                    first_diff: 5,
                },
                _ => Resave::Error {
                    note: Some("KeyError: 'data'".into()),
                },
            }
        } else {
            Resave::Skipped
        },
    }
}

fn object(
    dump: &mut InputDump,
    path: &[RawStr],
    rec: ObjectRecord,
    levels: Levels,
    i: usize,
    resave: bool,
) {
    let rec = ObjectRecord {
        l1: if levels.l1 { rec.l1 } else { None },
        l2: if levels.l2 { rec.l2 } else { None },
        l3: if levels.l3 {
            Some(l3_of(i, resave))
        } else {
            None
        },
        ..rec
    };
    dump.lines.push(object_line(path, &rec));
    dump.objects += 1;
}

pub(crate) fn error(kind: L2Kind, bytes_read: Option<i64>, value: Option<String>) -> L2 {
    L2 {
        root_type: Some(raw(b"MonoBehaviour")),
        source: Some("embedded"),
        error: Some(L2Error {
            kind,
            bytes_read,
            note: Some(format!("a {} note", kind.as_str())),
        }),
        value,
    }
}

/// The encoded values of `encode.json`, encoded here.
pub(crate) fn values() -> Vec<String> {
    let f = fixture("encode.json");
    f["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| encode(&parsed(&c["value"])).unwrap())
        .collect()
}

/// The hand-built records of the input named `name` (its bytes are `data`).
pub(crate) fn records(name: &[u8], data: &[u8], levels: Levels, values: &[String]) -> InputDump {
    let mut d = InputDump {
        sha256: sha256(data),
        bytes_in: data.len() as u64,
        ..InputDump::default()
    };
    let me = vec![raw(name)];
    let fail = |d: &mut InputDump, ty, kind, note: Option<&str>| {
        d.push_file(file_line(&FileRecord {
            file: me.clone(),
            l0: l0(ty, ParsedAs::Error, data),
            l1: None,
            error: Some(FileError {
                kind,
                note: note.map(str::to_owned),
            }),
        }));
    };
    match name {
        b"a/detect.bin" => fail(&mut d, None, FileErrorKind::Detect, None),
        b"a/crash.bin" => fail(
            &mut d,
            Some(FileType::AssetsFile),
            FileErrorKind::Crash,
            Some("the process dumping this input died of SIGSEGV"),
        ),
        b"b/bad.bundle" => fail(
            &mut d,
            Some(FileType::BundleFile),
            FileErrorKind::Parse,
            Some("Lzma block: \u{1}corrupt \"data\"\\"),
        ),
        b"a/zip.zip" => file(
            &mut d,
            &me,
            l0(Some(FileType::Zip), ParsedAs::Zip, data),
            None,
            levels,
        ),
        b"b/raw.resS" => file(
            &mut d,
            &me,
            l0(Some(FileType::AssetsFile), ParsedAs::Raw, data),
            None,
            levels,
        ),
        b"c/bundle" => bundle(&mut d, &me, data, levels),
        b"d/\xff.assets" => serialized(&mut d, &me, data, levels, values),
        _ => file(
            &mut d,
            &me,
            l0(Some(FileType::ResourceFile), ParsedAs::Raw, data),
            None,
            levels,
        ),
    }
    d
}

/// A `SerializedFile` input: every value of `values`, then an object of each
/// L2 failure kind, its objects given out of `path_id` order in the table.
fn serialized(d: &mut InputDump, me: &[RawStr], data: &[u8], levels: Levels, values: &[String]) {
    let order: Vec<i64> = (0..values.len() + 6)
        .map(|i| 100 - i64::try_from(i).unwrap())
        .collect();
    let mut sorted = order.clone();
    sorted.sort_unstable();
    let big = Packed::from_limbs(&[1, 2, 3]);
    file(
        d,
        me,
        l0(Some(FileType::AssetsFile), ParsedAs::SerializedFile, data),
        Some(FileL1::Serialized(Box::new(serialized_l1(
            order,
            Some(big),
        )))),
        levels,
    );
    let mut ids = sorted.into_iter();
    for (i, v) in values.iter().enumerate() {
        let path_id = ids.next().unwrap();
        let l2 = L2 {
            root_type: Some(raw(b"MonoBehaviour")),
            source: Some(if i % 2 == 0 { "embedded" } else { "tpk" }),
            error: None,
            value: Some(v.clone()),
        };
        let rec = ObjectRecord {
            path_id,
            l1: Some(l1_of(path_id)),
            l2: Some(l2),
            l3: None,
        };
        object(d, me, rec, levels, i, true);
    }
    let failures = [
        L2 {
            root_type: None,
            source: None,
            error: Some(L2Error {
                kind: L2Kind::NoTypetree,
                bytes_read: None,
                note: Some("no typetree for class 28".into()),
            }),
            value: None,
        },
        error(L2Kind::CheckRead, Some(20), values.first().cloned()),
        error(L2Kind::OutOfBounds, None, None),
        error(L2Kind::NegativeLength, None, None),
        error(L2Kind::RefType, None, None),
        error(L2Kind::Other, None, None),
    ];
    for (k, l2) in failures.into_iter().enumerate() {
        let path_id = ids.next().unwrap();
        let rec = ObjectRecord {
            path_id,
            l1: Some(l1_of(path_id)),
            l2: Some(l2),
            l3: None,
        };
        object(d, me, rec, levels, k, false);
    }
}

fn dir(offset: i64, size: i64, path: &[u8]) -> DirRow {
    DirRow {
        offset,
        size,
        flags: Some(4),
        path: raw(path),
    }
}

fn bundle(d: &mut InputDump, me: &[RawStr], data: &[u8], levels: Levels) {
    let packed =
        |major: u128| Packed::from_u128((major << 48) | (3 << 32) | (1 << 16) | (3 << 8) | 1);
    let l1 = BundleL1 {
        signature: raw(b"UnityFS"),
        format: 8,
        version_player: raw(b"5.x.x"),
        version_engine: raw(b"2022.3.1f1"),
        engine_version_packed: Some(packed(2022)),
        flags: Some(0x243),
        flags_class: Some(FlagsClass::New),
        block_alignment: true,
        encrypted: false,
        hash: None,
        crc: None,
        blocks_info: Some(BlockRow {
            uncompressed: 200,
            compressed: 120,
            flags: 0x243,
        }),
        blocks: Some(vec![
            BlockRow {
                uncompressed: 131_072,
                compressed: 40_000,
                flags: 3,
            },
            BlockRow {
                uncompressed: 10,
                compressed: 7,
                flags: 0,
            },
        ]),
        // A repeated path: `files` keeps it at its first position.
        directory: vec![
            dir(0, 4096, b"CAB-1"),
            dir(4096, 16, b"CAB-1.resS"),
            dir(4112, 300, b"nested"),
            dir(4412, 4096, b"CAB-1"),
        ],
    };
    file(
        d,
        me,
        l0(Some(FileType::BundleFile), ParsedAs::BundleFile, data),
        Some(FileL1::Bundle(l1)),
        levels,
    );
    let cab: Vec<RawStr> = [me, &[raw(b"CAB-1")]].concat();
    file(
        d,
        &cab,
        l0(Some(FileType::AssetsFile), ParsedAs::SerializedFile, b"cab"),
        Some(FileL1::Serialized(Box::new(serialized_l1(
            vec![7, -2],
            None,
        )))),
        levels,
    );
    for (i, path_id) in [-2i64, 7].into_iter().enumerate() {
        let rec = ObjectRecord {
            path_id,
            l1: Some(l1_of(path_id)),
            l2: Some(L2 {
                root_type: Some(raw(b"AssetBundle")),
                source: Some("embedded"),
                error: None,
                value: Some("{\"t\":\"dict\",\"v\":[]}".into()),
            }),
            l3: None,
        };
        object(d, &cab, rec, levels, i, true);
    }
    let res: Vec<RawStr> = [me, &[raw(b"CAB-1.resS")]].concat();
    file(
        d,
        &res,
        l0(Some(FileType::AssetsFile), ParsedAs::Raw, b"res"),
        None,
        levels,
    );
    nested(d, me, levels);
}

/// A bundle inside the bundle, its one entry named in bytes that are not UTF-8.
fn nested(d: &mut InputDump, me: &[RawStr], levels: Levels) {
    let nested: Vec<RawStr> = [me, &[raw(b"nested")]].concat();
    let inner_l1 = BundleL1 {
        directory: vec![dir(0, 10, b"in\xffner")],
        blocks: Some(Vec::new()),
        engine_version_packed: None,
        flags_class: Some(FlagsClass::Old),
        block_alignment: false,
        ..match nested_template() {
            FileL1::Bundle(b) => b,
            _ => unreachable!(),
        }
    };
    file(
        d,
        &nested,
        l0(Some(FileType::BundleFile), ParsedAs::BundleFile, b"nested"),
        Some(FileL1::Bundle(inner_l1)),
        levels,
    );
    let inner: Vec<RawStr> = [nested.as_slice(), &[raw(b"in\xffner")]].concat();
    file(
        d,
        &inner,
        l0(Some(FileType::ResourceFile), ParsedAs::Raw, b"x"),
        None,
        levels,
    );
}

fn nested_template() -> FileL1 {
    FileL1::Bundle(BundleL1 {
        signature: raw(b"UnityFS"),
        format: 6,
        version_player: raw(b"5.x.x"),
        version_engine: raw(b"0.0.0"),
        engine_version_packed: None,
        flags: Some(0x40),
        flags_class: None,
        block_alignment: false,
        encrypted: false,
        hash: None,
        crc: None,
        blocks_info: Some(BlockRow {
            uncompressed: 0,
            compressed: 0,
            flags: 0x40,
        }),
        blocks: None,
        directory: Vec::new(),
    })
}

/// The names of the hand-built inputs, plus `extra` plain ones.
fn names(extra: usize) -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = [
        &b"a/detect.bin"[..],
        b"a/crash.bin",
        b"a/zip.zip",
        b"b/bad.bundle",
        b"b/raw.resS",
        b"c/bundle",
        b"d/\xff.assets",
    ]
    .iter()
    .map(|n| n.to_vec())
    .collect();
    v.extend((0..extra).map(|i| format!("e/plain-{i:03}").into_bytes()));
    v
}

/// A folder holding a file for each name, its bytes the name.
pub(crate) fn corpus(extra: usize) -> (Tmp, Vec<Input>) {
    use std::os::unix::ffi::OsStrExt;
    let tmp = Tmp::new("stream");
    let mut inputs = Vec::new();
    for n in names(extra) {
        let path = tmp.0.join("in").join(std::ffi::OsStr::from_bytes(&n));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &n).unwrap();
        inputs.push(Input {
            path,
            name: n,
            sha256: None,
        });
    }
    inputs.sort_by(|a, b| a.name.cmp(&b.name));
    (tmp, inputs)
}

/// A whole dump of the hand-built inputs, as `write_dump` writes it.
pub(crate) fn stream(inputs: &[Input], levels: Levels, jobs: usize) -> Vec<u8> {
    let vals = values();
    let opts = DumpOptions {
        levels,
        jobs,
        ..DumpOptions::default()
    };
    let mut out = Vec::new();
    write_dump(
        inputs,
        &opts,
        &mut out,
        &mut |_, _| Ok(()),
        &|name, data: Bytes| records(name, &data, levels, &vals),
    )
    .unwrap();
    out
}
