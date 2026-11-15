//! B2a L4 oracle replay: layout bytes, compressed semantics and held B1 edits.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use serde_json::{Value, json};
use std::sync::Arc;
use unityrs_core::files::{BundleFile, BundlePacker, BundleSaveOptions, File, SaveLayout};
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, FileContext, RawStr};

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").unwrap();
        out
    })
}

fn unhex(text: &str) -> Vec<u8> {
    assert_eq!(text.len() % 2, 0);
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn fixture() -> Value {
    let text = include_str!("fixtures/bundle_save.json");
    assert!(text.len() < 1_200_000, "keep shared byte chunks compact");
    let mut fixture: Value = serde_json::from_str(text).unwrap();
    assert_eq!(fixture["reference"], "UnityPy 1.25.2");
    let chunks = fixture["data"].as_array().unwrap();
    let unique: std::collections::BTreeSet<_> =
        chunks.iter().map(|value| value.as_str().unwrap()).collect();
    assert_eq!(unique.len(), chunks.len(), "fixture chunks are interned");
    let data: Vec<Value> = fixture["bytes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pieces| {
            let text: String = pieces
                .as_array()
                .unwrap()
                .iter()
                .map(|at| {
                    chunks[usize::try_from(at.as_u64().unwrap()).unwrap()]
                        .as_str()
                        .unwrap()
                })
                .collect();
            Value::String(text)
        })
        .collect();
    for case in fixture["cases"].as_array_mut().unwrap() {
        for field in ["input", "output"] {
            case[field] = data[usize::try_from(case[field].as_u64().unwrap()).unwrap()].clone();
        }
        for row in case["structure"]["directory"].as_array_mut().unwrap() {
            row[4] = data[usize::try_from(row[4].as_u64().unwrap()).unwrap()].clone();
        }
        if let Some(at) = case["actual"]["hex"].as_u64() {
            case["actual"]["hex"] = data[usize::try_from(at).unwrap()].clone();
        }
    }
    fixture
}

fn open(bytes: Bytes) -> Arc<BundleFile> {
    BundleFile::open(
        RawStr::from_static(b"save.bundle"),
        bytes,
        None,
        &Meter::unlimited(),
        &Arc::new(FileContext::unitypy()),
    )
    .unwrap()
}

fn packer(value: &Value) -> BundlePacker {
    match value.as_str() {
        Some("none") => BundlePacker::None,
        Some("original") => BundlePacker::Original,
        Some("lz4") => BundlePacker::Lz4,
        Some("lzma") => BundlePacker::Lzma,
        None => BundlePacker::Flags {
            data: u32::try_from(value[0].as_u64().unwrap()).unwrap(),
            blocks: u16::try_from(value[1].as_u64().unwrap()).unwrap(),
        },
        _ => panic!("unexpected packer {value}"),
    }
}

fn mutate(bundle: &BundleFile, mutation: &str) -> Option<unityrs_core::ObjectRef> {
    if mutation == "none" {
        return None;
    }
    let slot = bundle
        .files()
        .iter()
        .find(|slot| slot.name.as_bytes() == b"CAB-file")
        .unwrap();
    let File::Serialized(file) = bundle.entry(slot.dir_index).unwrap() else {
        panic!("SerializedFile");
    };
    let object = file.object(1).unwrap();
    match mutation {
        "raw" => object.set_raw_data(b"replacement-\0\xff").unwrap(),
        "empty" => object.set_raw_data(b"").unwrap(),
        "typetree" => {
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
            object
                .save_typetree(&value, None, unityrs_core::walk::WriteRules::UnityPy)
                .unwrap();
        }
        _ => panic!("mutation {mutation}"),
    }
    assert!(file.is_changed());
    assert_eq!(object.raw_data().unwrap().as_ref(), 31i32.to_le_bytes());
    // Keep this handle alive across save to pin the published SerializedFile.
    Some(object)
}

fn structure(bundle: &BundleFile) -> Value {
    let header = bundle.header();
    let text = |raw: &RawStr| std::str::from_utf8(raw.as_bytes()).unwrap().to_owned();
    let rows: Vec<_> = bundle
        .directory()
        .iter()
        .enumerate()
        .map(|(i, row)| {
            json!([
                row.offset,
                row.size,
                row.flags,
                hex(row.path.as_bytes()),
                hex(&bundle.entry_bytes(i).unwrap())
            ])
        })
        .collect();
    let blocks: Vec<_> = bundle
        .blocks()
        .iter()
        .map(|block| json!([block.uncompressed, block.flags]))
        .collect();
    json!({"signature":text(&header.signature),"format":header.format,
        "player":text(&header.version_player),"engine":text(&header.version_engine),
        "flags":header.data_flags,"alignment":header.block_alignment,
        "blocks":blocks,"directory":rows})
}

#[test]
fn replay_bundle_save_layouts_packers_and_held_serialized_edits() {
    let fixture = fixture();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 287, "coverage must not silently shrink");
    let mut failures = 0;
    for case in cases {
        let name = format!("{} / {}", case["name"], case["packer"]);
        failures += usize::from(
            case["actual"].get("error").is_some() || case["actual"].get("reload_error").is_some(),
        );
        if let Some(malformed) = case["actual"]["hex"].as_str() {
            assert!(
                BundleFile::open(
                    RawStr::from_static(b"malformed.bundle"),
                    Bytes::from(unhex(malformed)),
                    None,
                    &Meter::unlimited(),
                    &Arc::new(FileContext::unitypy())
                )
                .is_err(),
                "{name}: oracle save stays encrypted"
            );
        }
        for layout in [SaveLayout::UnityPy, SaveLayout::Semantic] {
            let bundle = open(Bytes::from(unhex(case["input"].as_str().unwrap())));
            let held = mutate(&bundle, case["mutation"].as_str().unwrap());
            let options = BundleSaveOptions {
                layout,
                packer: packer(&case["packer"]),
            };
            let bytes = bundle
                .save(options)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            if layout == SaveLayout::UnityPy
                && (case["packer"] == "none"
                    || (case["packer"].is_array()
                        && case["packer"][1].as_u64().unwrap() & 0x3f == 3)
                    || case["structure"]["signature"] != "UnityFS")
            {
                assert_eq!(
                    bytes.as_ref(),
                    unhex(case["output"].as_str().unwrap()),
                    "{name}: exact layout"
                );
            }
            let back = open(bytes.clone());
            assert_eq!(
                structure(&back),
                case["structure"],
                "{name}: structure and decompressed bytes"
            );
            if case["same_packer_idempotent"] == true {
                assert_eq!(back.save(options).unwrap(), bytes, "{name}: idempotence");
            }
            let canonical_options = BundleSaveOptions {
                layout,
                packer: BundlePacker::None,
            };
            let canonical = back.save(canonical_options).unwrap();
            assert_eq!(
                open(canonical.clone()).save(canonical_options).unwrap(),
                canonical,
                "{name}: canonical idempotence"
            );
            if let Some(object) = held {
                assert_eq!(
                    object.raw_data().unwrap().as_ref(),
                    31i32.to_le_bytes(),
                    "{name}: original reader"
                );
            }
        }
    }
    assert!(
        failures > 20,
        "Tier C oracle failures must stay represented"
    );
}
