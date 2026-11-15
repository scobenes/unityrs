//! Synthetic HEAD 417d595 writer replay. Unity 6000.4/6000.5 Editor
//! measurements remain a target; this tests the vendored MIT writer.
use std::sync::Arc;
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::files::{SaveError, SaveLayout, SaveOptions, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::stream::ReaderKind;
use unityrs_core::{Bytes, FileContext, RawStr};

fn unhex(s: &str) -> Bytes {
    Bytes::from(
        s.as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect::<Vec<_>>(),
    )
}

fn fixture() -> serde_json::Value {
    let text = include_str!("fixtures/head_save_serialized.json");
    assert!(text.len() < 1_500_000);
    serde_json::from_str(text).unwrap()
}

#[test]
fn replay_head_saves_reload_both_readers_and_remain_idempotent() {
    let fixture = fixture();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 188);
    let mut versions = std::collections::BTreeSet::new();
    let mut refusals = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let mut ctx = FileContext::unitypy();
        ctx.serialized_file_parse_typetree = case["parse_trees"].as_bool().unwrap();
        let ctx = Arc::new(ctx);
        for kind in [ReaderKind::Memory, ReaderKind::Stream] {
            let meter = Meter::unlimited();
            let file = SerializedFile::parse_with_reader(
                RawStr::from_static(b"CAB-head"),
                unhex(case["input"].as_str().unwrap()),
                None,
                &meter,
                &ctx,
                kind,
            )
            .unwrap();
            versions.insert(file.header().version);
            if case["mutation"] == true {
                for obj in file.objects() {
                    obj.set_raw_data(b"edited payload").unwrap();
                }
            }
            for layout in [SaveLayout::Semantic, SaveLayout::UnityPy] {
                let before = (meter.used(), meter.granted(), meter.decoded());
                let saved = file.save(SaveOptions { layout });
                if case.get("error").is_some() {
                    assert!(matches!(saved, Err(SaveError::Missing("HEAD type node"))));
                    assert_eq!((meter.used(), meter.granted()), (before.0, before.1));
                    assert!(meter.decoded() > before.2);
                    refusals += 1;
                    continue;
                }
                let saved = saved.unwrap();
                assert_eq!(
                    saved,
                    unhex(case["output"].as_str().unwrap()),
                    "{} {kind:?} {layout:?}",
                    case["name"]
                );
                let back = SerializedFile::parse_with_reader(
                    RawStr::from_static(b"CAB-back"),
                    saved.clone(),
                    None,
                    &meter,
                    &ctx,
                    kind,
                )
                .unwrap();
                assert_eq!(
                    back.save(SaveOptions { layout }).unwrap(),
                    saved,
                    "{}",
                    case["name"]
                );
                assert_eq!(file.types().len(), back.types().len());
                if case["parse_trees"] == true {
                    for (a, b) in file.types().iter().zip(back.types()) {
                        assert_eq!(a.nodes, b.nodes, "{}", case["name"]);
                    }
                }
                for obj in file.objects() {
                    let expected = if case["mutation"] == true {
                        Bytes::from_static(b"edited payload")
                    } else {
                        obj.raw_data().unwrap()
                    };
                    assert_eq!(
                        back.object(obj.path_id()).unwrap().raw_data().unwrap(),
                        expected
                    );
                }
            }
        }
    }
    assert_eq!(versions, [23, 24, 25, 26].into());
    assert!(refusals > 0);
}

#[test]
fn head_output_is_charged_and_memory_refusal_releases_temporaries() {
    let fixture = fixture();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "shared-<" && c["parse_trees"] == true && c["mutation"] == false)
        .unwrap();
    let meter = Meter::with_decode_work(1 << 20, 1 << 24, Arc::new(Unbounded), Reservation::Input);
    let file = SerializedFile::parse(
        RawStr::from_static(b"CAB-budget"),
        unhex(case["input"].as_str().unwrap()),
        None,
        &meter,
        &Arc::new(FileContext::unitypy()),
    )
    .unwrap();
    let before = meter.used();
    let saved = file.save(SaveOptions::default()).unwrap();
    assert!(meter.used() >= before + saved.len() as u64);
    let clone = saved.clone();
    drop(saved);
    assert!(meter.used() > before);
    drop(clone);
    assert_eq!(meter.used(), before);
    let held = meter.charge(meter.limit() - before).unwrap();
    assert!(matches!(
        file.save(SaveOptions::default()),
        Err(SaveError::Charge(_))
    ));
    assert_eq!(meter.used(), meter.limit());
    drop(held);
    assert!(file.save(SaveOptions::default()).is_ok());
    meter.count_decode((1 << 24) - meter.decoded()).unwrap();
    assert!(matches!(
        file.save(SaveOptions::default()),
        Err(SaveError::Charge(_))
    ));
    assert_eq!(meter.used(), before);
    drop(file);
    assert_eq!(meter.used(), 0);
}
