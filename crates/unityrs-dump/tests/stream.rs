//! The record writer and the dump driver on hand-built records (plan 7.6,
//! wave 1: the core's files are not built yet, so the records stand in for
//! them): every record type of `docs/CANONICAL_DUMP.md` section 4 and every
//! value of `tests/fixtures/dump/encode.json` through `tools/parity/compare.py`'s
//! canonical checks, at each level set; the output's independence of `-j`;
//! gzip with mtime 0; the `.partial` file and its rename; the stops.

mod common;

use std::sync::Arc;

use common::{Tmp, compare, fixture, gunzip, line_problems, parsed, repo, s};
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
use unityrs_dump::run::{RunError, RunOptions, run_with, write_dump};
use unityrs_dump::select::{Input, Selection};
use unityrs_dump::{DumpOptions, InputDump, Levels};

const LEVEL_SETS: [Levels; 4] = [
    Levels {
        l1: false,
        l2: false,
        l3: false,
    },
    Levels {
        l1: true,
        l2: false,
        l3: false,
    },
    Levels {
        l1: true,
        l2: true,
        l3: false,
    },
    Levels::ALL,
];

fn raw(s: &[u8]) -> RawStr {
    RawStr::from(s)
}

fn l0(ty: Option<FileType>, parsed_as: ParsedAs, bytes: &[u8]) -> L0 {
    L0 {
        ty,
        parsed_as,
        size: bytes.len() as u64,
        sha256: sha256(bytes),
    }
}

fn file(dump: &mut InputDump, path: &[RawStr], l0: L0, l1: Option<FileL1>, levels: Levels) {
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

fn serialized_l1(object_order: Vec<i64>, version_packed: Option<Packed>) -> SerializedL1 {
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
        container: vec![ContainerRow {
            path: raw(b"assets/a\xff.prefab"),
            preload_index: 0,
            preload_size: -1,
            file_id: 0,
            path_id: i64::MAX,
        }],
    }
}

fn l1_of(path_id: i64) -> ObjectL1 {
    ObjectL1 {
        type_id: 0,
        class_id: 114,
        byte_start: i128::from(path_id.unsigned_abs()) + 4096,
        byte_size: 16,
        is_destroyed: None,
        is_stripped: Some(0),
    }
}

fn l3_of(i: usize, resave: bool) -> L3 {
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

fn error(kind: L2Kind, bytes_read: Option<i64>, value: Option<String>) -> L2 {
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
fn values() -> Vec<String> {
    let f = fixture("encode.json");
    f["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| encode(&parsed(&c["value"])).unwrap())
        .collect()
}

/// The hand-built records of the input named `name` (its bytes are `data`).
fn records(name: &[u8], data: &[u8], levels: Levels, values: &[String]) -> InputDump {
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
fn corpus(extra: usize) -> (Tmp, Vec<Input>) {
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
fn stream(inputs: &[Input], levels: Levels, jobs: usize) -> Vec<u8> {
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

#[test]
fn every_record_type_passes_compare_py_at_every_level_set() {
    let (tmp, inputs) = corpus(3);
    for levels in LEVEL_SETS {
        let text = stream(&inputs, levels, 1);
        let path = tmp
            .0
            .join(format!("dump-{}.jsonl", levels.names().join("")));
        std::fs::write(&path, &text).unwrap();
        let (status, report) = compare(&path, &path);
        assert_eq!(status, 0, "{levels:?}:\n{report}");
        assert_eq!(line_problems(&path), "", "{levels:?}");
        // The header and the end record.
        let lines: Vec<&[u8]> = text.split(|b| *b == b'\n').collect();
        assert!(lines[0].starts_with(b"{\"rec\":\"dump\""));
        assert_eq!(
            lines.last(),
            Some(&&b""[..]),
            "the stream ends with a newline"
        );
        let end = lines[lines.len() - 2];
        let files = lines
            .iter()
            .filter(|l| l.starts_with(b"{\"rec\":\"file\""))
            .count();
        let objects = lines
            .iter()
            .filter(|l| l.starts_with(b"{\"rec\":\"object\""))
            .count();
        assert_eq!(
            String::from_utf8_lossy(end),
            format!("{{\"rec\":\"end\",\"files\":{files},\"objects\":{objects}}}")
        );
        assert!(objects >= 70, "{objects} objects");
    }
}

#[test]
fn a_record_compare_py_refuses_is_caught() {
    // The check above must be able to fail: a dump whose end record
    // miscounts, and one with a summary of 64 bytes, are each refused.
    let (tmp, inputs) = corpus(0);
    let text = String::from_utf8(stream(&inputs, Levels::ALL, 1)).unwrap();
    let bad_end = text.replace("\"rec\":\"end\",\"files\":", "\"rec\":\"end\",\"files\":1");
    let path = tmp.0.join("bad-end.jsonl");
    std::fs::write(&path, bad_end).unwrap();
    let (status, report) = compare(&path, &path);
    assert_eq!(status, 1, "{report}");
    let short = format!(
        "{{\"t\":\"bytes\",\"len\":64,\"sha256\":\"{}\"}}",
        "0".repeat(64)
    );
    let bad_value = text.replacen("{\"t\":\"dict\",\"v\":[]}", &short, 1);
    let path = tmp.0.join("bad-value.jsonl");
    std::fs::write(&path, bad_value).unwrap();
    assert!(line_problems(&path).contains("summary_too_short"));
}

#[test]
fn an_encode_error_is_canonical_in_a_candidate() {
    let tmp = Tmp::new("encode");
    let me = [raw(b"x")];
    let rec = ObjectRecord {
        path_id: 1,
        l1: Some(l1_of(1)),
        l2: Some(error(L2Kind::Encode, None, None)),
        l3: Some(l3_of(0, false)),
    };
    let mut d = InputDump::default();
    file(
        &mut d,
        &me,
        l0(Some(FileType::AssetsFile), ParsedAs::SerializedFile, b"x"),
        Some(FileL1::Serialized(Box::new(serialized_l1(vec![1], None)))),
        Levels::ALL,
    );
    d.lines.push(object_line(&me, &rec));
    let text = format!(
        "{}\n{}{}\n",
        unityrs_dump::header_line(Levels::ALL, "unityrs"),
        d.text(),
        unityrs_dump::end_line(1, 1)
    );
    let path = tmp.0.join("encode.jsonl");
    std::fs::write(&path, text).unwrap();
    assert_eq!(line_problems(&path), "");
}

#[test]
fn the_output_does_not_depend_on_jobs() {
    let (_tmp, inputs) = corpus(60);
    let one = stream(&inputs, Levels::ALL, 1);
    for jobs in [2, 3, 8] {
        assert!(
            stream(&inputs, Levels::ALL, jobs) == one,
            "-j {jobs} differs from -j 1"
        );
    }
}

fn options(tmp: &Tmp, output: &str, jobs: usize) -> RunOptions {
    RunOptions {
        selection: Selection {
            inputs: vec![tmp.0.join("in")],
            ..Selection::default()
        },
        output: tmp.0.join("out").join(output),
        dump: DumpOptions {
            jobs,
            ..DumpOptions::default()
        },
        stats: None,
        quiet: true,
        allow_index_mismatch: false,
    }
}

#[test]
fn a_run_writes_gzip_with_mtime_0_through_a_partial_file_at_any_jobs() {
    let (tmp, inputs) = corpus(20);
    let vals = values();
    let dumper = |name: &[u8], data: Bytes| records(name, &data, Levels::ALL, &vals);
    let want = stream(&inputs, Levels::ALL, 1);
    for jobs in [1, 4] {
        let opts = options(&tmp, &format!("j{jobs}.jsonl.gz"), jobs);
        // A .partial left by a killed run is removed first.
        let partial = tmp.0.join("out").join(format!("j{jobs}.jsonl.gz.partial"));
        std::fs::create_dir_all(partial.parent().unwrap()).unwrap();
        std::fs::write(&partial, b"stale").unwrap();
        let summary = run_with(&opts, &dumper).unwrap();
        assert!(!partial.exists(), "the .partial file was renamed");
        let gz = std::fs::read(&opts.output).unwrap();
        assert_eq!(&gz[..10], b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x02\xff");
        assert!(gunzip(&gz) == want, "-j {jobs}");
        assert_eq!(summary.inputs, inputs.len() as u64);
    }
    // Without .gz the stream is plain.
    let opts = options(&tmp, "plain.jsonl", 2);
    run_with(&opts, &dumper).unwrap();
    assert_eq!(sha256(&std::fs::read(&opts.output).unwrap()), sha256(&want));
}

#[test]
fn a_panic_stops_the_run_and_writes_no_dump() {
    let (tmp, _) = corpus(10);
    for jobs in [1, 3] {
        let opts = options(&tmp, "panic.jsonl", jobs);
        let dumper = |name: &[u8], _: Bytes| {
            assert!(name != b"e/plain-004", "a bug");
            InputDump::default()
        };
        let err = run_with(&opts, &dumper).unwrap_err();
        assert!(matches!(err, RunError::Failed(_)), "{err:?}");
        assert!(err.to_string().contains("a bug"), "{err}");
        assert!(!opts.output.exists());
        assert!(!tmp.0.join("out/panic.jsonl.partial").exists());
    }
}

#[test]
fn an_index_mismatch_stops_the_run_unless_allowed() {
    let tmp = Tmp::new("index");
    std::fs::create_dir_all(tmp.0.join("corpus")).unwrap();
    std::fs::write(tmp.0.join("corpus/one.bin"), b"one").unwrap();
    std::fs::write(tmp.0.join("corpus/two.bin"), b"two").unwrap();
    let good = unityrs_dump::json::sha256_hex(b"one");
    let index = serde_json::json!({"files": [
        {"path": "one.bin", "kind": "bundle", "sha256": good},
        {"path": "two.bin", "kind": "bundle", "sha256": "00".repeat(32)},
    ]});
    std::fs::write(tmp.0.join("corpus/index.json"), index.to_string()).unwrap();
    let mut opts = options(&tmp, "index.jsonl", 2);
    opts.selection = Selection {
        inputs: vec![tmp.0.join("corpus/index.json")],
        index: true,
        ..Selection::default()
    };
    let dumper = |name: &[u8], data: Bytes| records(name, &data, Levels::ALL, &[]);
    let err = run_with(&opts, &dumper).unwrap_err();
    assert!(err.to_string().contains("two.bin: sha256"), "{err}");
    assert_eq!(err.status(), 1);
    assert!(!opts.output.exists());
    opts.allow_index_mismatch = true;
    let summary = run_with(&opts, &dumper).unwrap();
    assert_eq!(summary.index_sha256_mismatch, 1);
    assert_eq!(summary.files, 2);
}

#[test]
fn paths_the_oracle_refuses_are_refused() {
    let (tmp, _) = corpus(1);
    let dumper = |_: &[u8], _: Bytes| InputDump::default();
    // Inside the repository.
    let mut opts = options(&tmp, "x.jsonl", 1);
    opts.output = repo().join("target/never-written.jsonl");
    let err = run_with(&opts, &dumper).unwrap_err();
    assert_eq!(err.status(), 2, "{err}");
    assert!(!opts.output.exists());
    // A symlink at the output.
    let opts = options(&tmp, "link.jsonl", 1);
    std::fs::create_dir_all(opts.output.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(tmp.0.join("elsewhere"), &opts.output).unwrap();
    let err = run_with(&opts, &dumper).unwrap_err();
    assert!(err.to_string().contains("symlink"), "{err}");
    // An input as the output.
    let mut opts = options(&tmp, "x.jsonl", 1);
    opts.output = tmp.0.join("in/e/plain-000");
    let err = run_with(&opts, &dumper).unwrap_err();
    assert!(err.to_string().contains("would overwrite"), "{err}");
    assert_eq!(std::fs::read(&opts.output).unwrap(), b"e/plain-000");
    // The stats over the dump.
    let mut opts = options(&tmp, "x.jsonl", 1);
    opts.stats = Some(opts.output.clone());
    assert_eq!(run_with(&opts, &dumper).unwrap_err().status(), 2);
    // No input at all.
    let mut opts = options(&tmp, "x.jsonl", 1);
    opts.selection.names = vec![b"nothing*".to_vec()];
    let err = run_with(&opts, &dumper).unwrap_err();
    assert_eq!(err, RunError::Usage("no inputs matched".into()));
}

#[test]
fn stats_are_written_beside_the_dump() {
    let (tmp, _) = corpus(2);
    let mut opts = options(&tmp, "s.jsonl", 2);
    opts.stats = Some(tmp.0.join("out/stats.json"));
    let dumper = |name: &[u8], data: Bytes| records(name, &data, Levels::ALL, &[]);
    run_with(&opts, &dumper).unwrap();
    let stats: serde_json::Value =
        serde_json::from_slice(&std::fs::read(tmp.0.join("out/stats.json")).unwrap()).unwrap();
    assert_eq!(stats["summary"]["inputs"], 9);
    assert_eq!(stats["inputs"].as_array().unwrap().len(), 9);
    assert_eq!(s(&stats["inputs"][0]["path"]), "a/crash.bin");
}
