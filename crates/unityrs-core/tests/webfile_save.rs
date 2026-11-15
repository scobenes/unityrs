//! Bidirectional oracle parity plus WebFile writer state and metering invariants.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#[path = "support/webfile_save.rs"]
mod support;
use serde_json::Value;
use support::{load, mutate, packer, unhex};
use unityrs_core::Bytes;
use unityrs_core::files::{SaveError, SaveLayout, WebPacker, WebSaveOptions};
use unityrs_core::limits::Meter;

#[test]
fn semantic_plain_save_refuses_brotli_detector_collision() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/webfile_save.json")).unwrap();
    let case = &fixture["collision"];
    let web = load(
        Bytes::from(unhex(case["input"].as_str().unwrap())),
        &Meter::unlimited(),
    );
    let oracle = web
        .save(WebSaveOptions {
            layout: SaveLayout::UnityPy,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(oracle.as_ref(), unhex(case["output"].as_str().unwrap()));
    assert_eq!(&oracle[32..38], b"brotli");
    assert_eq!(
        web.save(WebSaveOptions::default()),
        Err(SaveError::Range("WebFile brotli marker collision"))
    );
    for packer in [WebPacker::Gzip, WebPacker::Brotli] {
        let saved = web
            .save(WebSaveOptions {
                packer,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            load(saved, &Meter::unlimited()).files()[0].name.as_bytes(),
            b"brotli"
        );
    }
}

#[test]
fn signature_scan_is_metered_and_oversize_refuses_before_scanning() {
    use std::sync::Arc;
    use unityrs_core::budget::{Reservation, Unbounded};
    let meter = Meter::with_decode_work(1 << 20, 1 << 20, Arc::new(Unbounded), Reservation::Input);
    let source = unityrs_testkit::web::plain("UnityWebData1.0", &[]);
    let web = load(Bytes::from(source), &meter);
    let before = meter.decoded();
    assert_eq!(
        web.save_with_signature(WebSaveOptions::default(), "UnityWebData\0bad"),
        Err(SaveError::Range("WebFile signature"))
    );
    assert!(meter.decoded() >= before + 16);
    meter.count_decode((1 << 20) - meter.decoded()).unwrap();
    let oversized = format!(
        "UnityWebData{}",
        "x".repeat(unityrs_core::stream::STRING_TO_NULL_WINDOW)
    );
    assert_eq!(
        web.save_with_signature(WebSaveOptions::default(), &oversized),
        Err(SaveError::Range("WebFile signature"))
    );
    assert!(matches!(
        web.save_with_signature(WebSaveOptions::default(), "UnityWebData1.0"),
        Err(SaveError::Charge(_))
    ));
}

#[test]
fn replay_webfile_save_signatures_packers_duplicates_nested_edits_and_block_boundaries() {
    let text = include_str!("fixtures/webfile_save.json");
    assert!(text.len() < 1_500_000);
    let fixture: Value = serde_json::from_str(text).unwrap();
    assert_eq!(fixture["reference"], "UnityPy 1.25.2");
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 234, "coverage must not silently shrink");
    let mut brotli_failures = 0;
    for case in cases {
        let bytes = |key: &str| {
            Bytes::from(unhex(
                fixture["data"][usize::try_from(case[key].as_u64().unwrap()).unwrap()]
                    .as_str()
                    .unwrap(),
            ))
        };
        let input = bytes("input");
        let plain = bytes("plain");
        let name = case["name"].as_str().unwrap();
        let signature = case["signature"].as_str().unwrap();
        let compression = packer(case["packer"].as_str().unwrap());
        brotli_failures += usize::from(!case["oracle_reload_error"].is_null());
        for layout in [SaveLayout::UnityPy, SaveLayout::Semantic] {
            let web = load(input.clone(), &Meter::unlimited());
            let original = web.data().clone();
            let held = mutate(&web, case["mutation"].as_str().unwrap());
            let old_raw = held.as_ref().map(|obj| obj.raw_data().unwrap());
            let options = WebSaveOptions {
                layout,
                packer: compression,
            };
            let saved = web.save_with_signature(options, signature).unwrap();
            let back = load(saved.clone(), &Meter::unlimited());
            assert_eq!(back.packer(), compression, "{name}");
            assert_eq!(back.data(), &plain, "{name}: exact decompressed layout");
            assert_eq!(
                back.save_with_signature(options, signature).unwrap(),
                saved,
                "{name}: idempotence"
            );
            assert_eq!(web.data(), &original, "{name}: original bytes");
            if let Some(object) = held {
                assert_eq!(
                    object.raw_data().unwrap(),
                    old_raw.unwrap(),
                    "{name}: held reader"
                );
                assert!(
                    object.file().is_changed(),
                    "{name}: save retains change flag"
                );
            }
        }
    }
    assert_eq!(
        brotli_failures, 78,
        "oracle brotli output remains undetectable"
    );
}

#[test]
fn defaults_match_oracle_and_explicit_signatures_have_typed_refusals() {
    let source = unityrs_testkit::web::plain("TuanjieWebData1.0", &[("raw", b"data")]);
    let web = load(
        Bytes::from(unityrs_testkit::web::gzip(&source)),
        &Meter::unlimited(),
    );
    let saved = web.save(WebSaveOptions::default()).unwrap();
    let back = load(saved, &Meter::unlimited());
    assert_eq!(back.signature().as_bytes(), b"UnityWebData1.0");
    assert_eq!(back.packer(), WebPacker::None);
    let before = (web.meter().used(), web.meter().decoded());
    for signature in ["invalid", "UnityWebData\0bad"] {
        assert_eq!(
            web.save_with_signature(WebSaveOptions::default(), signature),
            Err(SaveError::Range("WebFile signature"))
        );
    }
    let long = format!(
        "UnityWebData{}",
        "x".repeat(unityrs_core::stream::STRING_TO_NULL_WINDOW)
    );
    assert_eq!(
        web.save_with_signature(WebSaveOptions::default(), &long),
        Err(SaveError::Range("WebFile signature"))
    );
    assert_eq!(web.meter().used(), before.0);
    assert_eq!(web.meter().decoded(), before.1 + 16);
}

#[test]
fn every_packer_charges_output_and_releases_temporary_memory_on_refusal() {
    use std::sync::Arc;
    use unityrs_core::budget::{Reservation, Unbounded};
    let source = unityrs_testkit::web::plain("UnityWebData1.0", &[("raw", b"payload")]);
    for packer in [WebPacker::None, WebPacker::Gzip, WebPacker::Brotli] {
        let meter =
            Meter::with_decode_work(1 << 20, 1 << 20, Arc::new(Unbounded), Reservation::Input);
        let web = load(Bytes::from(source.clone()), &meter);
        web.load_all().unwrap();
        let before = meter.used();
        let options = WebSaveOptions {
            packer,
            ..WebSaveOptions::default()
        };
        let saved = web.save(options).unwrap();
        assert!(meter.used() > before);
        drop(saved);
        assert_eq!(meter.used(), before);
        let held = meter.charge(meter.limit() - meter.used()).unwrap();
        let used = meter.used();
        assert!(matches!(web.save(options), Err(SaveError::Charge(_))));
        assert_eq!(meter.used(), used);
        drop(held);
        meter.count_decode((1 << 20) - meter.decoded()).unwrap();
        assert!(matches!(web.save(options), Err(SaveError::Charge(_))));
        assert_eq!(meter.used(), before);
    }
}
