//! Exact HEAD 417d595 bundle writes, corrected Tier C saves and meter refusals.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use serde_json::{Value, json};
use std::sync::Arc;
use unityrs_core::files::{BundleFile, BundlePacker, BundleSaveOptions, SaveError, SaveLayout};
use unityrs_core::limits::Meter;
use unityrs_core::stream::ReaderKind;
use unityrs_core::{Bytes, FileContext, RawStr};

fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn fixture() -> Value {
    let text = include_str!("fixtures/head_save_bundle.json");
    assert!(text.len() < 1_500_000);
    let mut fixture: Value = serde_json::from_str(text).unwrap();
    assert_eq!(fixture["head"], "417d59522a998fdee9a9c2b29fe35f3063222457");
    let chunks = fixture["data"].as_array().unwrap();
    let bytes: Vec<Value> = fixture["bytes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pieces| {
            Value::String(
                pieces
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|at| {
                        chunks[usize::try_from(at.as_u64().unwrap()).unwrap()]
                            .as_str()
                            .unwrap()
                    })
                    .collect(),
            )
        })
        .collect();
    for case in fixture["cases"].as_array_mut().unwrap() {
        for field in ["input", "output"] {
            case[field] = bytes[usize::try_from(case[field].as_u64().unwrap()).unwrap()].clone();
        }
        for row in case["structure"]["directory"].as_array_mut().unwrap() {
            row[4] = bytes[usize::try_from(row[4].as_u64().unwrap()).unwrap()].clone();
        }
        if let Some(at) = case["actual"]["hex"].as_u64() {
            case["actual"]["hex"] = bytes[usize::try_from(at).unwrap()].clone();
        }
    }
    fixture
}

fn open(bytes: Bytes, meter: &Arc<Meter>, kind: ReaderKind) -> Arc<BundleFile> {
    BundleFile::open_with_reader(
        RawStr::from_static(b"head.bundle"),
        bytes,
        None,
        meter,
        &Arc::new(FileContext::unitypy()),
        kind,
    )
    .unwrap()
}

fn packer(value: &Value) -> BundlePacker {
    match value.as_str() {
        Some("none") => BundlePacker::None,
        Some("original") => BundlePacker::Original,
        Some("lz4") => BundlePacker::Lz4,
        Some("lz4hc") => BundlePacker::Lz4Hc,
        Some("lzma") => BundlePacker::Lzma,
        None => BundlePacker::Flags {
            data: u32::try_from(value[0].as_u64().unwrap()).unwrap(),
            blocks: u16::try_from(value[1].as_u64().unwrap()).unwrap(),
        },
        _ => panic!("unexpected packer {value}"),
    }
}

fn check_structure(bundle: &BundleFile, expected: &Value, name: &str) {
    assert_eq!(
        json!(bundle.header().data_flags),
        expected["flags"],
        "{name}"
    );
    let blocks: Vec<_> = bundle
        .blocks()
        .iter()
        .map(|block| json!([block.uncompressed, block.compressed, block.flags]))
        .collect();
    let rows: Vec<_> = expected["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| json!([row[0], row[1], row[2]]))
        .collect();
    assert_eq!(blocks, rows, "{name}: blocks");
    assert_eq!(
        bundle.directory().len(),
        expected["directory"].as_array().unwrap().len(),
        "{name}"
    );
    for (index, (row, want)) in bundle
        .directory()
        .iter()
        .zip(expected["directory"].as_array().unwrap())
        .enumerate()
    {
        assert_eq!(
            json!([row.offset, row.size, row.flags]),
            json!([want[0], want[1], want[2]]),
            "{name}"
        );
        assert_eq!(
            row.path.as_bytes(),
            unhex(want[3].as_str().unwrap()),
            "{name}"
        );
        assert_eq!(
            bundle.entry_bytes(index).unwrap().as_ref(),
            unhex(want[4].as_str().unwrap()),
            "{name}"
        );
    }
}

#[test]
fn exact_head_bundle_saves_and_tier_c_outputs_reload_and_are_idempotent() {
    let fixture = fixture();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 108, "HEAD write coverage must not shrink");
    let mut corrected = 0;
    for case in cases {
        let name = format!("{} / {}", case["name"], case["packer"]);
        corrected += usize::from(!case["actual"].as_object().unwrap().is_empty());
        for kind in [ReaderKind::Memory, ReaderKind::Stream] {
            for layout in [SaveLayout::UnityPy, SaveLayout::Semantic] {
                let meter = Meter::unlimited();
                let bundle = open(
                    Bytes::from(unhex(case["input"].as_str().unwrap())),
                    &meter,
                    kind,
                );
                let options = BundleSaveOptions {
                    layout,
                    packer: packer(&case["packer"]),
                };
                let bytes = bundle
                    .save(options)
                    .unwrap_or_else(|error| panic!("{name}: {error}"));
                // Exact bytes include the u64 offsets, every block's padding,
                // and liblz4 HC's deterministic compressed bytes.
                assert_eq!(
                    bytes.as_ref(),
                    unhex(case["output"].as_str().unwrap()),
                    "{name}: HEAD bytes"
                );
                let back = open(bytes.clone(), &Meter::unlimited(), kind);
                check_structure(&back, &case["structure"], &name);
                if case["same_packer_idempotent"] == true {
                    assert_eq!(back.save(options).unwrap(), bytes, "{name}: idempotence");
                }
                let canonical_options = BundleSaveOptions {
                    layout,
                    packer: BundlePacker::None,
                };
                let canonical = back.save(canonical_options).unwrap();
                assert_eq!(
                    open(canonical.clone(), &Meter::unlimited(), kind)
                        .save(canonical_options)
                        .unwrap(),
                    canonical,
                    "{name}: canonical"
                );
                drop(back);
                let used = meter.used();
                drop(bytes);
                assert!(meter.used() < used, "{name}: output owns its memory grant");
            }
        }
    }
    assert!(corrected >= 6, "HEAD's Tier C exceptions stay represented");
}

#[test]
fn head_bundle_packers_release_temporaries_on_memory_and_work_refusal() {
    use unityrs_core::budget::{Reservation, Unbounded};
    let fixture = fixture();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "v9-short")
        .unwrap();
    for packer in [
        BundlePacker::None,
        BundlePacker::Original,
        BundlePacker::Lz4,
        BundlePacker::Lz4Hc,
        BundlePacker::Lzma,
    ] {
        let meter =
            Meter::with_decode_work(1 << 20, 1 << 30, Arc::new(Unbounded), Reservation::Input);
        let bundle = open(
            Bytes::from(unhex(case["input"].as_str().unwrap())),
            &meter,
            ReaderKind::Memory,
        );
        bundle.load_all().unwrap();
        let options = BundleSaveOptions {
            packer,
            ..BundleSaveOptions::default()
        };
        let held = meter.charge(meter.limit() - meter.used()).unwrap();
        let used = meter.used();
        assert!(
            matches!(bundle.save(options), Err(SaveError::Charge(_))),
            "{packer:?}"
        );
        assert_eq!(meter.used(), used, "{packer:?}");
        drop(held);
        let used = meter.used();
        meter.count_decode((1 << 30) - meter.decoded()).unwrap();
        assert!(
            matches!(bundle.save(options), Err(SaveError::Charge(_))),
            "{packer:?}"
        );
        assert_eq!(meter.used(), used, "{packer:?}");
    }
}
