//! The tag walk's real work against the work it counts (`docs/BATCH_SPEC.md`
//! 4.4 step 2's bounds; B3's review rounds 4 and 5, R1 and Q3): many
//! atlases of short names and a long tag no name equals. The walk looks the
//! tag up once and compares each name it reads with it, so the bytes hashed
//! and compared (this crate's `testing` counts, under its `test-counters`
//! feature: every table map's hashing, counted by its hasher where it
//! hashes, and the walk's comparisons) grow with the atlases, not with the
//! atlases times the tag, and stay within the decode work the attempt
//! counted. A walk that hashed the tag again at each atlas (round 4's bug)
//! hashes `ATLASES x TAG` bytes and fails. Built under the feature only; the
//! count is the process's, so this binary holds one test.

#![cfg(feature = "test-counters")]

use unityrs_core::FileContext;
use unityrs_core::limits::Meter;
use unityrs_export::testing;
use unityrs_testkit::ngui::{
    AtlasScene, CutOptions, Object, SceneBundle, SpriteSpec, file, format, sheet, sprite_atlas,
    texture,
};

mod common;
use common::{Engine, run_in};

/// `SpriteAtlas`'s class id.
const SPRITE_ATLAS: i32 = 687_078_895;
/// The atlases.
const ATLASES: usize = 200;
/// The tag's length.
const TAG: usize = 4000;
/// The bytes a cut of this scene hashes and compares for each atlas,
/// whatever the tag, at most: its name's lookup and insert, each with its
/// length, its comparison, and its file's list looked up.
const EACH: usize = 128;

fn scene() -> AtlasScene {
    let tag = "z".repeat(TAG);
    let mut objects = vec![texture(
        1,
        b"own",
        format::RGBA32,
        (2, 2),
        sheet(0x21, 2, 2, false),
    )];
    for i in 0..ATLASES {
        let path_id = 10 + i64::try_from(i).unwrap();
        let name = format!("n{i}");
        objects.push(Object::new(
            path_id,
            SPRITE_ATLAS,
            sprite_atlas(Some(&name), &[]),
        ));
    }
    objects.push(
        SpriteSpec::rect("tagged", [0.0, 0.0, 2.0, 2.0], (0, 1))
            .tagged(&[&tag])
            .object(1000),
    );
    let cab = file(&[], &objects, true).unwrap();
    AtlasScene {
        bundles: vec![SceneBundle {
            id: "x.bundle".to_owned(),
            data: unityrs_testkit::env::bundle(&[("CAB-w1", cab)]),
            deps: Vec::new(),
        }],
        inputs: vec!["x.bundle".to_owned()],
        options: CutOptions {
            sprites_key: Some("{name}.png".to_owned()),
            ..CutOptions::default()
        },
    }
}

#[test]
fn a_long_unmatched_tag_is_hashed_once_and_its_walk_counted() {
    let s = scene();
    let meter = Meter::unlimited();
    let before = testing::tag_walk_bytes();
    let ran = run_in(
        &s,
        "x.bundle",
        FileContext::default(),
        &meter,
        &Engine::default(),
    )
    .unwrap();
    let walked = testing::tag_walk_bytes() - before;
    assert_eq!(ran.handed.len(), 1, "the Sprite renders its own m_RD");
    // The tag hashed once (with its length), then each atlas's own: no
    // atlas's turn hashes the tag again, which would be `ATLASES x TAG`.
    let most = u64::try_from(2 * TAG + EACH * ATLASES).unwrap();
    eprintln!("{walked} bytes hashed and compared, at most {most}");
    assert!(walked <= most, "{walked} bytes walked, at most {most}");
    assert!(most < u64::try_from(ATLASES * TAG).unwrap() / 10);
    assert!(
        walked <= meter.decoded(),
        "{walked} bytes walked, {} counted",
        meter.decoded()
    );
}
