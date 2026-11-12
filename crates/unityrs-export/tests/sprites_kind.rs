//! The `sprites` kind beside the other kinds and under the engine's
//! bounds, whose answers the spec gives and the reference does not record
//! (`docs/BATCH_SPEC.md` 4.4's plan and export orders, 4.5, 7.3;
//! `docs/plans/batch.md` 2.6 and 2.7): a texture takes its key before a
//! Sprite, and a Sprite before an atlas's sprite; the sprite groups are
//! exported first; every pixel buffer is counted as export work before the
//! next output; a stop ends where the spec says (the bounds:
//! `sprites_bounds.rs`).

use unityrs_core::FileContext;
use unityrs_core::limits::Meter;
use unityrs_export::ngui::{Failure, OutputKind};
use unityrs_testkit::ngui::{
    AtlasScene, CutOptions, SpriteSpec, atlas, file, format, mono_script, sheet, sprite,
    sprite_scenes, texture,
};

mod common;
use common::{Engine, Event, Ran, run, run_in};

fn scene(name: &str) -> AtlasScene {
    sprite_scenes()
        .unwrap()
        .into_iter()
        .find(|(n, _)| *n == name)
        .unwrap()
        .1
}

fn keys(ran: &Ran) -> Vec<&str> {
    ran.handed.iter().map(|o| o.key.as_str()).collect()
}

fn skips(ran: &Ran) -> Vec<(i64, &'static str, &str)> {
    ran.skips
        .iter()
        .map(|s| (s.path_id, s.output.name(), s.reason.reason.as_str()))
        .collect()
}

/// A texture `x` exported as `x.png`, Sprites `x` and `y` on it under
/// `{name}.png`, and an atlas `x` cutting the texture (by its own name) into
/// a sprite `y` under `{sprite}.png`, its JSON `x.json`.
fn three_kinds() -> AtlasScene {
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            texture(2, b"x", format::RGBA32, (4, 4), sheet(0x20, 4, 4, false)),
            SpriteSpec::rect("x", [0.0, 0.0, 2.0, 2.0], (0, 2)).object(3),
            SpriteSpec::rect("y", [1.0, 1.0, 2.0, 2.0], (0, 2)).object(4),
            atlas(
                5,
                b"x",
                (0, 1),
                (0, 0),
                vec![sprite(b"y", (0, 0, 1, 1), [0; 8])],
                Vec::new(),
            ),
        ],
        true,
    )
    .unwrap();
    AtlasScene {
        bundles: vec![unityrs_testkit::ngui::SceneBundle {
            id: "x.bundle".to_owned(),
            data: unityrs_testkit::env::bundle(&[("CAB-k1", cab)]),
            deps: Vec::new(),
        }],
        inputs: vec!["x.bundle".to_owned()],
        options: CutOptions {
            sprite_key: "{sprite}.png".to_owned(),
            json_key: "{atlas}.json".to_owned(),
            texture_key: Some("{name}.png".to_owned()),
            sprites_key: Some("{name}.png".to_owned()),
            ..CutOptions::default()
        },
    }
}

#[test]
fn a_texture_then_a_sprite_then_an_atlas_takes_a_key() {
    // Plan order (4.4): the textures kind, then the Sprites, then the
    // atlases. The Sprite `x` loses `x.png` to the texture, and the atlas's
    // sprite `y` loses `y.png` to the Sprite; the Sprite's group is
    // exported before the sheet the atlas cuts.
    let ran = run(&three_kinds(), "x.bundle");
    assert_eq!(keys(&ran), ["y.png", "x.png", "x.json"]);
    let kinds: Vec<OutputKind> = ran.handed.iter().map(|o| o.kind).collect();
    assert_eq!(
        kinds,
        [
            OutputKind::Sprite,
            OutputKind::Texture,
            OutputKind::AtlasJson
        ]
    );
    assert_eq!(ran.handed[0].unit, None, "a Sprite is a plain candidate");
    assert_eq!(
        skips(&ran),
        [
            (3, "sprite", "duplicate_key"),
            (5, "atlas_sprite", "duplicate_key")
        ]
    );
    assert_eq!(ran.skips[0].class, "Sprite");
    ran.assert_units();
}

#[test]
fn without_a_sprites_key_no_sprite_is_planned() {
    let mut s = three_kinds();
    s.options.sprites_key = None;
    let ran = run(&s, "x.bundle");
    assert_eq!(keys(&ran), ["x.png", "x.json", "y.png"]);
    assert!(ran.skips.is_empty(), "{:?}", ran.skips);
}

#[test]
fn export_work_counts_each_sheet_then_each_image_before_its_png() {
    // own_sheets: the 8 x 8 RGBA sheet, then `a` (4 x 3), `c` (3 x 5),
    // `mask` (4 x 4) and `flip` (3 x 2), RGBA; then the 4 x 4 RGB sheet,
    // `b` (RGB) and `mask_rgb` (RGBA). Each group's check comes before its
    // sheet, each Sprite's before its render, and each image's count before
    // its PNG is made and handed over.
    let s = scene("own_sheets");
    let engine = Engine::default();
    run_in(
        &s,
        "x.bundle",
        FileContext::default(),
        &Meter::unlimited(),
        &engine,
    )
    .unwrap();
    let log = engine.log.borrow();
    let first = log
        .iter()
        .position(|e| matches!(e, Event::Work(_)))
        .unwrap();
    // Each Sprite: its check, its image's count, then the check every
    // output's hand-over makes.
    let image = |w: u64, key: &str| {
        vec![
            Event::Check,
            Event::Work(w),
            Event::Check,
            Event::Sink(format!("Sprite/{key}.png")),
        ]
    };
    let mut want = vec![Event::Work(8 * 8 * 4)];
    for (w, key) in [(48, "a"), (60, "c"), (64, "mask"), (24, "flip")] {
        want.extend(image(w, key));
    }
    want.extend([Event::Check, Event::Work(4 * 4 * 3)]);
    for (w, key) in [(48, "b"), (64, "mask_rgb")] {
        want.extend(image(w, key));
    }
    assert_eq!(log.get(first - 1), Some(&Event::Check), "the group's check");
    assert_eq!(log[first..first + want.len()], want[..]);
    assert!(
        !log[first + want.len()..]
            .iter()
            .any(|e| matches!(e, Event::Work(_) | Event::Sink(_))),
        "{log:?}"
    );
    let total = 256 + 48 + 60 + 64 + 24 + 48 + 48 + 64;
    assert_eq!(engine.work.get(), total);
    for max in [0, 255, 256, 303, total - 1] {
        let engine = Engine {
            max_work: Some(max),
            ..Engine::default()
        };
        let got = run_in(
            &s,
            "x.bundle",
            FileContext::default(),
            &Meter::unlimited(),
            &engine,
        );
        let Err(Failure::Input(r)) = got else {
            panic!("{max}: not refused");
        };
        assert_eq!(r.reason, format!("limit(export_work, {max})"));
    }
    // At the total, every count fits.
    let engine = Engine {
        max_work: Some(total),
        ..Engine::default()
    };
    let ran = run_in(
        &s,
        "x.bundle",
        FileContext::default(),
        &Meter::unlimited(),
        &engine,
    )
    .unwrap();
    assert_eq!(ran.handed.len(), 6);
    assert_eq!(engine.work.get(), total);
}

#[test]
fn an_alpha_sheet_counts_its_merge_once_for_its_group() {
    // alpha_textures: the RGB24 sheet with its alpha texture (8 x 8 RGB,
    // then 8 x 8 RGBA, then the merge, 8 x 8 RGBA) once for `merged`,
    // `merged2` and `alpha_mask`; then the sheet alone for `plain`; then
    // the mismatched alpha texture (8 x 8 RGB and 8 x 4 RGBA), whose merge
    // fails on the sizes before its count (B3's review round 1, R5).
    let s = scene("alpha_textures");
    let engine = Engine::default();
    let ran = run_in(
        &s,
        "x.bundle",
        FileContext::default(),
        &Meter::unlimited(),
        &engine,
    )
    .unwrap();
    let work: Vec<u64> = engine
        .log
        .borrow()
        .iter()
        .filter_map(|e| match e {
            Event::Work(w) => Some(*w),
            _ => None,
        })
        .collect();
    assert_eq!(work, [192, 256, 256, 64, 256, 64, 192, 48, 192, 128]);
    assert_eq!(skips(&ran), [(7, "sprite", "decode_error")]);
}

#[test]
fn a_stop_ends_the_export_at_the_check_that_sees_it() {
    // Every check in turn raises the stop: the attempt ends `Stopped`, its
    // outputs so far a prefix of the whole run's, until a check past the
    // last one, where it runs whole.
    let s = scene("own_sheets");
    let whole = run(&s, "x.bundle");
    let all = keys(&whole);
    for at in 0.. {
        let engine = Engine {
            stop_at: Some(at),
            ..Engine::default()
        };
        let got = run_in(
            &s,
            "x.bundle",
            FileContext::default(),
            &Meter::unlimited(),
            &engine,
        );
        match got {
            Err(Failure::Stopped) => {
                let handed: Vec<String> = engine
                    .log
                    .borrow()
                    .iter()
                    .filter_map(|e| match e {
                        Event::Sink(k) => Some(k.clone()),
                        _ => None,
                    })
                    .collect();
                assert_eq!(handed[..], all[..handed.len()], "stop at {at}");
            }
            Ok(ran) => {
                assert_eq!(keys(&ran), all);
                assert!(at > 6, "stopped too late to matter: {at}");
                break;
            }
            Err(f) => panic!("{at}: {f:?}"),
        }
    }
}
