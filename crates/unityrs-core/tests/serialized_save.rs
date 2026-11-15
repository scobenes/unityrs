//! B1: file save, mutations and Tier C old trees.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Arc;
use unityrs_core::files::{SaveOptions, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::options::FileContext;
use unityrs_core::{Bytes, RawStr};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::serialized::{build, one_object};

fn load(data: Bytes) -> Arc<SerializedFile> {
    SerializedFile::parse(
        RawStr::from_static(b"CAB-save"),
        data,
        None,
        &Meter::unlimited(),
        &Arc::new(FileContext::unitypy()),
    )
    .unwrap()
}

#[test]
fn every_format_saves_reloads_and_is_idempotent() {
    for format in 1..=22 {
        for big_endian in [false, true] {
            let mut spec = one_object(
                49,
                vec![
                    NodeSpec::new(0, "Base", "Base"),
                    NodeSpec::new(1, "int", "value"),
                ],
                vec![1, 2, 3, 4],
            );
            spec.format = format;
            spec.big_endian = big_endian;
            if format < 16 {
                spec.objects[0].type_id = 49;
            }
            let file = load(Bytes::from(build(&spec)));
            let saved = file.save(SaveOptions::default()).unwrap();
            let back = load(saved.clone());
            assert_eq!(
                back.object(1).unwrap().raw_data().unwrap().as_ref(),
                [1, 2, 3, 4]
            );
            assert_eq!(
                back.save(SaveOptions::default()).unwrap(),
                saved,
                "format {format}"
            );
            assert_eq!(back.types()[0].nodes, file.types()[0].nodes);
        }
    }
}

#[test]
fn replacing_raw_data_changes_save_but_reads_still_read_the_original() {
    let file = load(Bytes::from(build(&one_object(
        49,
        vec![NodeSpec::new(0, "Base", "Base")],
        vec![1, 2, 3],
    ))));
    assert!(!file.is_changed());
    let object = file.object(1).unwrap();
    object.set_raw_data(b"replacement").unwrap();
    assert!(file.is_changed());
    assert_eq!(object.raw_data().unwrap().as_ref(), [1, 2, 3]);
    let back = load(file.save(SaveOptions::default()).unwrap());
    assert_eq!(
        back.object(1).unwrap().raw_data().unwrap().as_ref(),
        b"replacement"
    );
}

fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn formats_above_head_refuse_before_output_or_charges() {
    use unityrs_core::budget::{Reservation, Unbounded};
    use unityrs_core::files::{SaveError, SaveLayout};
    use unityrs_core::stream::ReaderKind;
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/head_read.json")).unwrap();
    let mut formats = std::collections::BTreeSet::new();
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["kind"] == "serialized" && case["result"].get("error").is_none())
    {
        let meter =
            Meter::with_decode_work(1 << 24, 1 << 24, Arc::new(Unbounded), Reservation::Input);
        let mut input = unhex(case["hex"].as_str().unwrap());
        if input[8..12] != 26u32.to_be_bytes() {
            continue;
        }
        input[8..12].copy_from_slice(&27u32.to_be_bytes());
        let file = SerializedFile::parse_with_reader(
            RawStr::from_static(b"CAB-head-save"),
            Bytes::from(input),
            None,
            &meter,
            &Arc::new(FileContext::unitypy()),
            if case["stream"] == true {
                ReaderKind::Stream
            } else {
                ReaderKind::Memory
            },
        )
        .unwrap();
        formats.insert(file.header().version);
        for exhausted in [false, true] {
            let held = exhausted.then(|| {
                meter.count_decode((1 << 24) - meter.decoded()).unwrap();
                meter.charge(meter.limit() - meter.used()).unwrap()
            });
            let before = (meter.used(), meter.granted(), meter.decoded());
            for layout in [SaveLayout::Semantic, SaveLayout::UnityPy] {
                let error = file.save(SaveOptions { layout }).unwrap_err();
                assert_eq!(error, SaveError::HeadTierWritesNotImplemented);
                assert!(
                    error
                        .to_string()
                        .contains("HEAD-tier writes are not implemented")
                );
                assert_eq!(
                    (meter.used(), meter.granted(), meter.decoded()),
                    before,
                    "{}: refusal must not build or charge output",
                    case["name"]
                );
            }
            drop(held);
        }
    }
    assert_eq!(formats, [27].into());
}

#[test]
fn save_fixture_interns_bytes_below_the_size_bound() {
    let text = include_str!("fixtures/serialized_save.json");
    assert!(
        text.len() < 1_500_000,
        "save fixture must stay below 1.5 MB"
    );
    let fixture: serde_json::Value = serde_json::from_str(text).unwrap();
    let table = fixture["data"].as_array().unwrap();
    let unique: std::collections::BTreeSet<_> = table.iter().map(|s| s.as_str().unwrap()).collect();
    assert_eq!(unique.len(), table.len(), "duplicate byte strings");
}

fn save_fixture() -> serde_json::Value {
    let mut fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/serialized_save.json")).unwrap();
    let data = fixture["data"].as_array().unwrap().clone();
    for (group, fields) in [
        (
            "cases",
            &["input", "replacement", "original", "layout", "semantic"][..],
        ),
        ("blobs", &["input", "layout", "semantic"][..]),
    ] {
        for case in fixture[group].as_array_mut().unwrap() {
            for &field in fields {
                if !case[field].is_null() {
                    let at = usize::try_from(case[field].as_u64().unwrap()).unwrap();
                    case[field] = data[at].clone();
                }
            }
            if let Some(at) = case["actual"]["hex"].as_u64() {
                case["actual"]["hex"] = data[usize::try_from(at).unwrap()].clone();
            }
        }
    }
    fixture
}

fn install_replacement(object: &unityrs_core::ObjectRef, case: &serde_json::Value) {
    if case["mutation"] == "typetree" {
        use unityrs_core::typetree::NodeId;
        use unityrs_core::value::{Value, ValueBuilder};
        let nodes = object
            .serialized_type()
            .unwrap()
            .nodes
            .as_ref()
            .unwrap()
            .clone();
        let builder = ValueBuilder::new(object.file().meter(), nodes).unwrap();
        let mut entries = builder.vec();
        entries.push((NodeId(1), Value::Int(-123_456))).unwrap();
        let value = builder.map(0, entries);
        let value = builder.finish(value, 4);
        let bytes = object
            .save_typetree(&value, None, unityrs_core::walk::WriteRules::UnityPy)
            .unwrap();
        assert_eq!(bytes.as_ref(), unhex(case["replacement"].as_str().unwrap()));
    } else if let Some(raw) = case["replacement"].as_str() {
        object.set_raw_data(unhex(raw)).unwrap();
    }
}

#[test]
fn replay_unitypy_file_saves_and_corrected_tier_c_saves() {
    use unityrs_core::files::SaveLayout;
    let fixture = save_fixture();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 374);
    let mut actual_saves = 0;
    let mut actual_errors = 0;
    let mut formats = std::collections::BTreeSet::new();
    for case in cases {
        let mut ctx = FileContext::unitypy();
        ctx.serialized_file_parse_typetree = case["parse_trees"].as_bool().unwrap();
        let ctx = Arc::new(ctx);
        let meter = Meter::unlimited();
        let file = SerializedFile::parse(
            RawStr::from_static(b"CAB-save"),
            Bytes::from(unhex(case["input"].as_str().unwrap())),
            None,
            &meter,
            &ctx,
        )
        .unwrap();
        assert!(file.header().version <= 22, "oracle covers B1 formats");
        formats.insert(file.header().version);
        let name = case["name"].as_str().unwrap();
        let object = file.object(9).unwrap();
        let original = unhex(case["original"].as_str().unwrap());
        assert_eq!(object.raw_data().unwrap().as_ref(), original, "{name}");
        install_replacement(&object, case);
        assert_eq!(
            file.is_changed(),
            case["changed"][0].as_bool().unwrap(),
            "{name}"
        );
        assert_eq!(
            object.raw_data().unwrap().as_ref(),
            original,
            "{name}: original reader"
        );
        for (layout, key) in [
            (SaveLayout::UnityPy, "layout"),
            (SaveLayout::Semantic, "semantic"),
        ] {
            let bytes = file.save(SaveOptions { layout }).unwrap();
            assert_eq!(
                bytes.as_ref(),
                unhex(case[key].as_str().unwrap()),
                "{name}: {key}"
            );
            if case["layout_reloads"] == false && layout == SaveLayout::UnityPy {
                assert_eq!(
                    bytes.as_ref(),
                    unhex(case["actual"]["hex"].as_str().unwrap())
                );
                actual_saves += 1;
                continue;
            }
            let back = SerializedFile::parse(
                RawStr::from_static(b"CAB-back"),
                bytes.clone(),
                None,
                &meter,
                &ctx,
            )
            .unwrap();
            assert_eq!(back.externals(), file.externals(), "{name}: externals");
            assert_eq!(
                back.save(SaveOptions { layout }).unwrap(),
                bytes,
                "{name}: idempotence"
            );
            assert_eq!(
                back.objects().map(|o| o.path_id()).collect::<Vec<_>>(),
                [-3, 4, 9]
            );
            let expected = case["replacement"]
                .as_str()
                .map_or_else(|| original.clone(), unhex);
            assert_eq!(
                back.object(9).unwrap().raw_data().unwrap().as_ref(),
                expected,
                "{name}"
            );
            if key == "layout" {
                if let Some(actual) = case["actual"]["hex"].as_str() {
                    actual_saves += 1;
                    if file.header().version >= 13
                        || (file.header().version >= 9 && case["parse_trees"] == true)
                    {
                        assert_eq!(bytes.as_ref(), unhex(actual), "{name}: unmodified UnityPy");
                    }
                } else {
                    actual_errors += 1;
                    assert_eq!(case["actual"]["error"], "AttributeError");
                }
            }
        }
    }
    assert_eq!((actual_saves, actual_errors), (293, 81));
    assert_eq!(formats, (1..=22).collect());
}

#[test]
fn replay_dump_blob_and_common_offset_zero_divergence() {
    use unityrs_core::files::SaveLayout;
    use unityrs_core::stream::{Endian, Reader};
    use unityrs_core::typetree::{NodeList, parse::parse_blob};
    let fixture = save_fixture();
    for case in fixture["blobs"].as_array().unwrap() {
        let data = Bytes::from(unhex(case["input"].as_str().unwrap()));
        let endian = if case["big"] == true {
            Endian::Big
        } else {
            Endian::Little
        };
        let format = u32::try_from(case["version"].as_u64().unwrap()).unwrap();
        let mut reader = Reader::new(&data, endian);
        let meter = Meter::unlimited();
        let limits = unityrs_core::limits::Limits::default();
        let nodes = NodeList::from_rows(
            parse_blob(&mut reader, &data, format, &meter, &limits, None).unwrap(),
        )
        .unwrap();
        for (layout, key) in [
            (SaveLayout::UnityPy, "layout"),
            (SaveLayout::Semantic, "semantic"),
        ] {
            let out = nodes.dump_blob(format, endian, layout, None).unwrap();
            assert_eq!(out.as_ref(), unhex(case[key].as_str().unwrap()));
            let mut reader = Reader::new(&out, endian);
            let back = NodeList::from_rows(
                parse_blob(&mut reader, &out, format, &meter, &limits, None).unwrap(),
            )
            .unwrap();
            assert_eq!(back, nodes);
            assert_eq!(back.dump_blob(format, endian, layout, None).unwrap(), out);
        }
        assert_ne!(case["layout"], case["semantic"]);
    }
}

#[test]
fn save_typetree_installs_the_reserialized_object() {
    use unityrs_core::walk::WriteRules;
    let file = load(Bytes::from(build(&one_object(
        49,
        vec![
            NodeSpec::new(0, "Base", "Base"),
            NodeSpec::new(1, "int", "value"),
        ],
        37i32.to_le_bytes().to_vec(),
    ))));
    let object = file.object(1).unwrap();
    let parsed = object.parse_as_dict(true).unwrap();
    assert_eq!(
        object
            .save_typetree(&parsed, None, WriteRules::UnityPy)
            .unwrap()
            .as_ref(),
        37i32.to_le_bytes()
    );
    assert!(file.is_changed());
    let back = load(file.save(SaveOptions::default()).unwrap());
    assert_eq!(
        back.object(1).unwrap().raw_data().unwrap().as_ref(),
        37i32.to_le_bytes()
    );
}

#[test]
fn mutations_propagate_through_bundle_and_web_parents() {
    use unityrs_core::files::{BundleFile, File};
    let asset = build(&one_object(
        49,
        vec![NodeSpec::new(0, "Base", "Base")],
        vec![1, 2, 3],
    ));
    let web = unityrs_testkit::web::plain("UnityWebData1.0", &[("CAB-save", &asset)]);
    let data = unityrs_testkit::env::bundle(&[("inner", web)]);
    let bundle = BundleFile::open(
        RawStr::from_static(b"outer"),
        Bytes::from(data),
        None,
        &Meter::unlimited(),
        &Arc::new(FileContext::unitypy()),
    )
    .unwrap();
    let File::Web(web) = bundle.entry(0).unwrap() else {
        panic!("web")
    };
    let File::Serialized(file) = web.entry(0).unwrap() else {
        panic!("serialized")
    };
    assert!(!file.is_changed() && !web.is_changed() && !bundle.is_changed());
    file.object(1).unwrap().set_raw_data(b"edited").unwrap();
    assert!(file.is_changed() && web.is_changed() && bundle.is_changed());
    assert!(File::Serialized(file.clone()).is_changed());
    // A dropped weak parent does not prevent marking or saving a child.
    drop((bundle, web));
    file.mark_changed();
    assert_eq!(
        load(file.save(SaveOptions::default()).unwrap())
            .object(1)
            .unwrap()
            .raw_data()
            .unwrap()
            .as_ref(),
        b"edited"
    );
}

#[test]
fn output_and_replacements_are_metered_and_refusals_are_atomic() {
    use unityrs_core::budget::{Reservation, Unbounded};
    use unityrs_core::files::SaveError;
    use unityrs_core::limits::ChargeError;
    let meter = Meter::new(1 << 20, Arc::new(Unbounded), Reservation::Input);
    let data = Bytes::from(build(&one_object(
        49,
        vec![NodeSpec::new(0, "Base", "Base")],
        vec![7; 4],
    )));
    let file = SerializedFile::parse(
        RawStr::from_static(b"CAB-save"),
        data,
        None,
        &meter,
        &Arc::new(FileContext::unitypy()),
    )
    .unwrap();
    let before = meter.used();
    let saved = file.save(SaveOptions::default()).unwrap();
    assert!(meter.used() >= before + saved.len() as u64);
    let cloned = saved.clone();
    drop(saved);
    assert!(meter.used() > before);
    drop(cloned);
    assert_eq!(meter.used(), before);
    let ballast = meter.charge(meter.limit() - meter.used()).unwrap();
    assert!(matches!(
        file.save(SaveOptions::default()),
        Err(SaveError::Charge(ChargeError::Limit(_)))
    ));
    assert!(
        file.object(1)
            .unwrap()
            .set_raw_data(b"replacement")
            .is_err()
    );
    assert!(!file.is_changed());
    assert_eq!(meter.used(), meter.limit());
    drop(ballast);
    let back = load(file.save(SaveOptions::default()).unwrap());
    assert_eq!(back.object(1).unwrap().raw_data().unwrap().as_ref(), [7; 4]);
    file.object(1).unwrap().set_raw_data(b"new").unwrap();
    assert!(meter.used() > before);
    drop((file, back));
    assert_eq!(meter.used(), 0);
}

#[test]
fn tree_writers_report_missing_fields_and_narrowing_failures() {
    use unityrs_core::files::{SaveError, SaveLayout};
    use unityrs_core::stream::Endian;
    use unityrs_core::typetree::{NodeList, NodeRow};
    let mut row = NodeRow::new(0, "int", "value");
    let nodes = NodeList::trusted(vec![row.clone()]).unwrap();
    assert!(matches!(
        nodes.dump_blob(22, Endian::Little, SaveLayout::UnityPy, None),
        Err(SaveError::Missing("m_TypeFlags"))
    ));
    assert!(matches!(
        nodes.dump_old(2, Endian::Little),
        Err(SaveError::Missing("m_VariableCount"))
    ));
    row.type_flags = Some(0);
    row.index = Some(0);
    row.meta_flag = Some(0);
    let nodes = NodeList::trusted(vec![row.clone()]).unwrap();
    assert!(
        nodes
            .dump_blob(18, Endian::Little, SaveLayout::UnityPy, None)
            .is_ok()
    );
    assert!(matches!(
        nodes.dump_blob(19, Endian::Little, SaveLayout::UnityPy, None),
        Err(SaveError::Missing("m_RefTypeHash"))
    ));
    row.level = 256;
    let nodes = NodeList::trusted(vec![row]).unwrap();
    assert!(matches!(
        nodes.dump_blob(18, Endian::Little, SaveLayout::UnityPy, None),
        Err(SaveError::Range("m_Level"))
    ));
}

#[test]
fn empty_files_save_in_every_supported_format() {
    use unityrs_testkit::serialized::SerializedSpec;
    for format in 1..=22 {
        let bytes = Bytes::from(build(&SerializedSpec {
            format,
            ..SerializedSpec::default()
        }));
        let file = load(bytes);
        let saved = file.save(SaveOptions::default()).unwrap();
        assert_eq!(
            load(saved.clone()).save(SaveOptions::default()).unwrap(),
            saved
        );
    }
}

#[test]
fn sequential_metadata_at_the_string_window_reloads_without_shifting_fields() {
    use unityrs_testkit::writer::ByteWriter;
    let mut meta = ByteWriter::le();
    meta.cstr(b"2020.3.0f1")
        .i32(13)
        .bool(false)
        .i32(0)
        .i32(0)
        .i32(0)
        .i32(1);
    meta.bytes(&vec![b'a'; 32_767])
        .bytes(&(1u8..=16).collect::<Vec<_>>())
        .i32(2)
        .cstr(b"file")
        .cstr(b"");
    let mut file = ByteWriter::be();
    let end = u32::try_from(20 + meta.len()).unwrap();
    file.u32(u32::try_from(meta.len()).unwrap())
        .u32(end)
        .u32(17)
        .u32(end)
        .bytes(&[0; 4])
        .bytes(meta.as_slice());
    let original = load(Bytes::from(file.into_inner()));
    assert_eq!(
        original.externals()[0].temp_empty.as_ref().unwrap().len(),
        32_767
    );
    let saved = original.save(SaveOptions::default()).unwrap();
    let back = load(saved.clone());
    assert_eq!(back.externals(), original.externals());
    assert_eq!(back.save(SaveOptions::default()).unwrap(), saved);
}

#[test]
fn a_full_window_final_string_keeps_an_eof_sentinel() {
    use unityrs_testkit::serialized::{ExternalSpec, SerializedSpec};
    for format in 1..=22 {
        for padding in 0..16 {
            let spec = SerializedSpec {
                format,
                externals: vec![ExternalSpec {
                    path: vec![b'x'; if format < 5 { 32_767 } else { padding }],
                    ..ExternalSpec::default()
                }],
                user_information: vec![b'a'; 32_767],
                ..SerializedSpec::default()
            };
            let file = load(Bytes::from(build(&spec)));
            let bytes = file.save(SaveOptions::default()).unwrap();
            let back = load(bytes.clone());
            assert_eq!(back.externals(), file.externals());
            assert_eq!(back.user_information(), file.user_information());
            assert_eq!(back.save(SaveOptions::default()).unwrap(), bytes);
        }
    }
}

#[test]
fn semantic_save_refuses_a_reader_accepted_short_guid() {
    use unityrs_core::files::{SaveError, SaveLayout};
    use unityrs_testkit::writer::ByteWriter;
    let mut input = ByteWriter::be();
    input
        .u32(21)
        .u32(0)
        .u32(5)
        .u32(0)
        .u32(0)
        .u32(1)
        .bytes(&[0; 8]);
    let file = load(Bytes::from(input.into_inner()));
    assert_eq!(file.externals()[0].guid.as_ref().unwrap().len(), 0);
    assert!(matches!(
        file.save(SaveOptions::default()),
        Err(SaveError::Range("guid"))
    ));
    assert!(
        file.save(SaveOptions {
            layout: SaveLayout::UnityPy
        })
        .is_ok()
    );
}
