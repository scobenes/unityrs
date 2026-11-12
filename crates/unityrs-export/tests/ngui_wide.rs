//! Section 3's answer 11 through the cutter (`docs/BATCH_SPEC.md` section 3
//! and section 5's sheet; review round 4's A1 and A2): a DXT1 texture wider
//! than Pillow 9.5.0's PNG encoder takes in RGBA (67,108,856 pixels), its
//! data inline. A plain candidate is skipped `pillow_memory_error` from its
//! decode's plan, with no export work and no pixel charge, after the bomb;
//! a sheet is decoded, its texture output alone is skipped, and its atlas
//! is cut, its JSON giving the sheet's size; a sheet whose decode is
//! refused takes the decode's answer, before 11 (review round 5's D1). The
//! images are big (a sheet decodes to 1 GiB), so these tests are a binary
//! of their own.

use std::sync::Arc;

use unityrs_core::FileContext;
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::Meter;
use unityrs_export::ngui::OutputKind;
use unityrs_testkit::env::bundle;
use unityrs_testkit::ngui::{
    AtlasScene, CutOptions, SceneBundle, atlas, file, format, mono_script, sprite, texture,
};

mod common;
use common::{Engine, Event, Ran, run_in};

/// One past the PNG encoder's RGBA width, a whole number of 4 x 4 blocks.
const WIDE: i32 = 67_108_860;

/// The DXT1 data of mip 0 of a `WIDE` x 4 texture (and of `WIDE` x 3,
/// padded to 4): 8 bytes a block.
fn data() -> Vec<u8> {
    vec![0; 8 * (WIDE as usize / 4)]
}

/// One bundle of `objects`, its options `options`.
fn scene(objects: &[unityrs_testkit::ngui::Object], options: CutOptions) -> AtlasScene {
    let cab = file(&[], objects, true).unwrap();
    AtlasScene {
        bundles: vec![SceneBundle {
            id: "x.bundle".to_owned(),
            data: bundle(&[("CAB-xw", cab)]),
            deps: Vec::new(),
        }],
        inputs: vec!["x.bundle".to_owned()],
        options,
    }
}

fn skips(ran: &Ran) -> Vec<(i64, &'static str, &str, Option<&str>)> {
    ran.skips
        .iter()
        .map(|s| {
            let r = &s.reason;
            (
                s.path_id,
                s.output.name(),
                r.reason.as_str(),
                r.detail.as_deref(),
            )
        })
        .collect()
}

#[test]
fn a_plain_texture_too_wide_to_write_is_skipped_before_its_decode() {
    // Under a meter that holds the data but not the 1 GiB decode, so a
    // pixel charge would have been refused (`limit(input_memory, ..)`).
    let meter = || Meter::new(768 << 20, Arc::new(Unbounded), Reservation::Input);
    let options = || CutOptions::textures("{name}.png");
    for (height, detail) in [(4, "pillow_memory_error"), (3, "decompression_bomb")] {
        let s = scene(
            &[texture(2, b"wide", format::DXT1, (WIDE, height), data())],
            options(),
        );
        let engine = Engine::default();
        let ran = run_in(&s, "x.bundle", FileContext::default(), &meter(), &engine).unwrap();
        // 4 x 3 crops the padded image: its area is the bomb's, answer 10,
        // which comes first.
        assert_eq!(
            skips(&ran),
            [(2, "texture", "decode_error", Some(detail))],
            "{height}"
        );
        assert_eq!(ran.handed.len(), 0, "{height}: nothing handed");
        assert_eq!(engine.work.get(), 0, "{height}: export work");
        let work = engine.log.borrow();
        assert!(!work.iter().any(|e| matches!(e, Event::Work(_))));
    }
}

/// A sheet `WIDE` x 4 and its atlas of one 2 x 2 sprite.
fn sheet_objects() -> [unityrs_testkit::ngui::Object; 3] {
    [
        mono_script(1, "NGUIAtlas"),
        texture(2, b"wide", format::DXT1, (WIDE, 4), data()),
        atlas(
            3,
            b"wide",
            (0, 1),
            (0, 0),
            vec![sprite(b"s", (0, 0, 2, 2), [0; 8])],
            Vec::new(),
        ),
    ]
}

#[test]
fn a_sheet_too_wide_to_write_is_cut_and_its_texture_output_alone_skipped() {
    // Review round 4's A1: the sheet's own PNG is its texture output's
    // alone. With textures on, that output is skipped and the atlas cut the
    // decoded sheet; with them off, nothing is skipped.
    for textures in [true, false] {
        let options = if textures {
            CutOptions::textures("{name}.png")
        } else {
            CutOptions::default()
        };
        let s = scene(&sheet_objects(), options);
        let engine = Engine::default();
        let ran = run_in(
            &s,
            "x.bundle",
            FileContext::default(),
            &Meter::unlimited(),
            &engine,
        )
        .unwrap();
        let want: &[_] = if textures {
            &[(2, "texture", "decode_error", Some("pillow_memory_error"))]
        } else {
            &[]
        };
        assert_eq!(skips(&ran), want, "textures {textures}");
        let handed: Vec<_> = ran
            .handed
            .iter()
            .map(|h| (h.kind, h.key.as_str()))
            .collect();
        assert_eq!(
            handed,
            [
                (OutputKind::AtlasJson, "wide.json"),
                (OutputKind::AtlasSprite, "wide/s.png"),
            ],
            "textures {textures}"
        );
        ran.assert_units();
        // The atlas JSON gives the sheet's size, its texture output skipped
        // or not planned (review round 5's T3).
        let json = ran
            .outputs
            .iter()
            .find(|o| o.kind == OutputKind::AtlasJson)
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&json.bytes).unwrap();
        assert_eq!(v["texture_width"], WIDE, "textures {textures}");
        assert_eq!(v["texture_height"], 4, "textures {textures}");
        // The sheet decoded once, at its padded size, and the crop.
        assert_eq!(engine.work.get(), WIDE as u64 * 4 * 4 + 2 * 2 * 4);
    }
}

#[test]
fn a_sheet_too_wide_to_write_whose_decode_is_refused_takes_that_answer() {
    // Review round 5's D1: a sheet's texture output asks answers 4 to 10,
    // then the sheet's decode, then 11. Under a meter that holds the data
    // but not the 1 GiB decode, the decode's refused charge (answer 12) is
    // the sheet's answer, taken by its texture output and its atlas alike,
    // not 11's `pillow_memory_error`.
    let max = 768 << 20;
    let meter = Meter::new(max, Arc::new(Unbounded), Reservation::Input);
    let s = scene(&sheet_objects(), CutOptions::textures("{name}.png"));
    let engine = Engine::default();
    let ran = run_in(&s, "x.bundle", FileContext::default(), &meter, &engine).unwrap();
    let limit = format!("limit(input_memory, {max})");
    assert_eq!(
        skips(&ran),
        [
            (2, "texture", limit.as_str(), None),
            (3, "atlas", limit.as_str(), None),
        ]
    );
    assert_eq!(ran.handed.len(), 0, "nothing handed");
    // The decode's work was counted before its charge was refused.
    assert_eq!(engine.work.get(), WIDE as u64 * 4 * 4);
}
