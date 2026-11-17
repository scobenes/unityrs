//! Bundle writer invariants and refusals independent of oracle byte layout.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::Arc;
use unityrs_core::files::{
    BundleFile, BundlePacker, BundleSaveOptions, SaveError, SaveLayout, SaveOptions,
};
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build};

fn load(data: Bytes, meter: &Arc<Meter>) -> Arc<BundleFile> {
    BundleFile::open(
        RawStr::from_static(b"bundle-save"),
        data,
        None,
        meter,
        &Arc::new(FileContext::unitypy()),
    )
    .unwrap()
}

fn spec(format: u32) -> BundleSpec {
    BundleSpec {
        format,
        info_codec: TestCodec::None,
        data_codec: TestCodec::None,
        entries: vec![EntrySpec {
            name: b"data.resS".to_vec(),
            data: b"bundle payload".to_vec(),
            flags: 7,
        }],
        ..BundleSpec::default()
    }
}

#[test]
fn fs_formats_and_packers_reload_and_save_identically() {
    for format in 1..=8 {
        let b = load(Bytes::from(build(&spec(format))), &Meter::unlimited());
        for packer in [
            BundlePacker::None,
            BundlePacker::Original,
            BundlePacker::Lz4,
            BundlePacker::Lz4Hc,
            BundlePacker::Lzma,
            BundlePacker::Flags {
                data: 0x2c3,
                blocks: 3,
            },
        ] {
            let options = BundleSaveOptions {
                packer,
                ..BundleSaveOptions::default()
            };
            let saved = b.save(options).unwrap();
            let back = load(saved.clone(), &Meter::unlimited());
            assert_eq!(back.entry_bytes(0).unwrap().as_ref(), b"bundle payload");
            assert_eq!(back.directory()[0].flags, Some(7));
            assert_eq!(
                back.save(options).unwrap(),
                saved,
                "format {format}, {packer:?}"
            );
        }
    }
}

#[test]
fn original_without_blocks_reloads_declared_nonempty_entries_as_empty() {
    // Rust-only structural acceptance: extending the 1.25.2 oracle inputs
    // would require regenerating bundle_save.json with UnityPy.
    for format in 6..=8 {
        let name = b"missing.resS";
        let input = BundleSpec {
            entries: vec![EntrySpec {
                name: name.to_vec(),
                data: vec![],
                flags: 7,
            }],
            ..spec(format)
        };
        let mut bytes = build(&input);
        // With stored blocks-info and no payload, the directory ends the
        // file: replace its size just before the flags and NUL-ended name.
        let size_at = bytes.len() - (8 + 4 + name.len() + 1);
        bytes[size_at..size_at + 8].copy_from_slice(&37i64.to_be_bytes());
        let b = load(Bytes::from(bytes), &Meter::unlimited());
        assert_eq!(b.blocks().len(), 0);
        assert_eq!(b.directory()[0].size, 37);
        assert!(b.entry_bytes(0).unwrap().is_empty());
        for layout in [SaveLayout::UnityPy, SaveLayout::Semantic] {
            let options = BundleSaveOptions {
                packer: BundlePacker::Original,
                layout,
            };
            let saved = b.save(options).unwrap();
            let back = load(saved.clone(), &Meter::unlimited());
            assert_eq!(back.header().format, format);
            assert_eq!(back.header().data_flags, b.header().data_flags);
            assert_eq!(back.directory().len(), 1);
            assert_eq!(back.directory()[0].path.as_bytes(), name);
            assert_eq!(back.directory()[0].flags, Some(7));
            assert_eq!(back.directory()[0].size, 0);
            assert!(back.entry_bytes(0).unwrap().is_empty());
            assert_eq!(back.save(options).unwrap(), saved);
        }
    }
}

#[test]
fn missing_directory_bit_and_unknown_codec_are_typed_refusals() {
    let b = load(Bytes::from(build(&spec(8))), &Meter::unlimited());
    assert!(matches!(
        b.save_fs(0, 0, SaveOptions::default()),
        Err(SaveError::Missing("directory flag 0x40"))
    ));
    assert!(matches!(
        b.save_fs(0x44, 0, SaveOptions::default()),
        Err(SaveError::UnsupportedCodec(_))
    ));
}

#[test]
fn formats_above_head_refuse_before_output_or_work_charges() {
    let meter = Meter::unlimited();
    // A no-block input can also be read by the HEAD reader without offsets.
    let empty = BundleSpec {
        format: 10,
        entries: vec![],
        ..spec(10)
    };
    let bytes = build(&empty);
    let b = load(Bytes::from(bytes), &meter);
    let used = meter.used();
    let work = meter.decoded();
    assert_eq!(
        b.save(BundleSaveOptions::default()),
        Err(SaveError::HeadTierWritesNotImplemented)
    );
    assert_eq!(meter.used(), used);
    assert_eq!(meter.decoded(), work);
    assert_eq!(
        b.save_fs(0x40, 0x40, SaveOptions::default()),
        Err(SaveError::HeadTierWritesNotImplemented)
    );
    assert_eq!(meter.used(), used);
    assert_eq!(meter.decoded(), work);
}

#[test]
fn memory_refusal_releases_writer_temporaries() {
    let meter = Meter::unlimited();
    let b = load(Bytes::from(build(&spec(8))), &meter);
    b.load_all().unwrap();
    let before = meter.used();
    // A separate bounded meter pins the opened file, then denies output.
    let ctx = Arc::new(FileContext::unitypy());
    let bounded = Meter::new(
        before + (1 << 16),
        ctx.budget.clone(),
        unityrs_core::budget::Reservation::Input,
    );
    let b = load(Bytes::from(build(&spec(8))), &bounded);
    b.load_all().unwrap();
    let held = bounded.charge(bounded.limit() - bounded.used()).unwrap();
    let used = bounded.used();
    assert!(matches!(
        b.save(BundleSaveOptions::default()),
        Err(SaveError::Charge(_))
    ));
    assert_eq!(bounded.used(), used);
    drop(held);
    b.save(BundleSaveOptions::default()).unwrap();
}

#[test]
fn held_head_serialized_file_saves_and_reloads() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/head_read.json")).unwrap();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == "serialized" && c["result"].get("error").is_none())
        .unwrap();
    let data: Vec<_> = case["hex"]
        .as_str()
        .unwrap()
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect();
    let mut spec = spec(8);
    spec.entries = vec![EntrySpec {
        name: b"CAB-head".to_vec(),
        data,
        flags: 4,
    }];
    let b = load(Bytes::from(build(&spec)), &Meter::unlimited());
    assert!(matches!(
        b.entry(0).unwrap(),
        unityrs_core::files::File::Serialized(_)
    ));
    let saved = b.save(BundleSaveOptions::default()).unwrap();
    let back = load(saved.clone(), &Meter::unlimited());
    assert!(matches!(
        back.entry(0).unwrap(),
        unityrs_core::files::File::Serialized(_)
    ));
    assert_eq!(back.save(BundleSaveOptions::default()).unwrap(), saved);
}

#[test]
fn output_owns_its_charge_until_dropped_and_work_can_refuse_save() {
    use unityrs_core::budget::{Reservation, Unbounded};
    let meter = Meter::with_decode_work(1 << 20, 1 << 20, Arc::new(Unbounded), Reservation::Input);
    let b = load(Bytes::from(build(&spec(8))), &meter);
    b.load_all().unwrap();
    let used = meter.used();
    let output = b.save(BundleSaveOptions::default()).unwrap();
    assert!(meter.used() > used);
    drop(output);
    assert_eq!(meter.used(), used);
    let remaining = (1 << 20) - meter.decoded();
    meter.count_decode(remaining).unwrap();
    assert!(matches!(
        b.save(BundleSaveOptions::default()),
        Err(SaveError::Charge(_))
    ));
    assert_eq!(meter.used(), used);
}
