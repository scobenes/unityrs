//! Budget and clock rules of the per-recording tolerance derivation.
use super::fixture::{bits, budget, legacy_euler_clip, rejects, with_clip, with_fixture};
use crate::anim_support::{GenericClip, generic_clip, legacy_clip};
use crate::{J, Path, compare};
use serde_json::json;
use std::process::Command;
use unityrs_testkit::{
    child,
    ngui::vector,
    scene3d::anim::playback::{self as p, Key},
    scratch::Scratch,
};

/// Move the interior sample of the generic fixture, whose pose is the same
/// throughout the held boundary's left side.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the local times are f32 values"
)]
pub(crate) fn move_interior(valid: &J, tolerance: &J, local: f64, effective: f64) -> (J, J) {
    let mut manifest = valid.clone();
    let mut frozen = tolerance.clone();
    manifest["spec"]["schedule"][1]["local"] = json!(local);
    manifest["capture"]["samples"][1]["local"] = json!(local);
    manifest["capture"]["samples"][1]["local_f32"] = json!(bits(local as f32));
    for row in [
        &mut manifest["capture"]["samples"][1],
        &mut frozen["samples"][1],
    ] {
        row["source"] = json!(2. + local);
        row["effective_source"] = json!(effective);
    }
    (manifest, frozen)
}

#[test]
fn a_replay_whose_clocks_agree_at_every_sample_is_clock_independent() {
    with_fixture(false, |dir, valid, tolerance| {
        // 2 + 0.125 is exact in f32, so neither clock moves any sample and
        // the held probe cannot, and need not, tell them apart.
        let (manifest, mut frozen) = move_interior(valid, tolerance, 0.125, 2.125);
        rejects(dir, &manifest, &frozen, "bundle clock independence");
        frozen["clock_discriminated"] = json!("independent");
        compare(&manifest, dir, &frozen).unwrap();
    });
}

#[test]
#[allow(clippy::cast_possible_truncation)]
fn a_held_segment_of_a_streamed_track_is_streamed_storage() {
    with_clip(
        false,
        generic_clip(GenericClip::StreamedThenHeld),
        |document| {
            document["spec"]["evidence"]["clock"]["key_times"] = json!([2, 2.25, 2.5, 3]);
            // The held key's value lies in the c0 range of the streamed stage.
            document["spec"]["budgets"][0] = budget(
                "translation",
                0,
                "streamed",
                &json!([[0, 0], [0, 0], [0, 1], [0, 0.5], [0, 0.5]]),
            );
            for sample in document["capture"]["samples"].as_array_mut().unwrap() {
                let x = sample["local"].as_f64().unwrap().min(0.5) as f32;
                sample["pose"]["transforms"][0]["translation"] =
                    json!([bits(x), bits(0.), bits(0.)]);
            }
        },
        |dir, valid, frozen| {
            compare(valid, dir, frozen).unwrap();
            let mut held = valid.clone();
            held["spec"]["budgets"][0]["track_kind"] = json!("held");
            held["spec"]["budgets"][0]["stages"] = json!([{"stage":"held","range":[[0, 0.5]]}]);
            rejects(dir, &held, frozen, "bundle track");
        },
    );
}

#[test]
#[allow(clippy::cast_possible_truncation)]
fn each_component_is_bounded_by_its_own_ranges() {
    let large = 2f64.powi(20);
    let envelope = json!([[0, 0], [0, 0], [0, 1], [0, large], [0, 1]]);
    let components = json!([
        [[0, 0], [0, 0], [0, 1], [0, 1], [0, 1]],
        [[0, 0], [0, 0], [0, 0], [large, large], [0, 1]],
        [[0, 0], [0, 0], [0, 0], [0, 0], [0, 1]]
    ]);
    for (ramp, passes) in [(true, true), (false, false)] {
        with_clip(
            false,
            generic_clip(GenericClip::RampBesideLarge),
            |document| {
                let mut row = budget("translation", 0, "streamed", &envelope);
                row["component_ranges"] = components.clone();
                document["spec"]["budgets"][0] = row;
                for sample in document["capture"]["samples"].as_array_mut().unwrap() {
                    let local = sample["local"].as_f64().unwrap();
                    let x = if ramp { local as f32 } else { 0. };
                    sample["pose"]["transforms"][0]["translation"] =
                        json!([bits(x), bits(large as f32), bits(0.)]);
                }
            },
            |dir, valid, frozen| {
                if passes {
                    compare(valid, dir, frozen).unwrap();
                    // A vector row without its own component ranges is
                    // refused, never bounded by the shared envelope.
                    let mut shared = valid.clone();
                    shared["spec"]["budgets"][0]
                        .as_object_mut()
                        .unwrap()
                        .remove("component_ranges");
                    rejects(dir, &shared, frozen, "bundle component range coverage");
                } else {
                    // A replay that leaves X at rest must not pass on Y's
                    // f32 rounding at 2^20.
                    rejects(dir, valid, frozen, "translation");
                }
            },
        );
    }
}

/// The budget rows the Python author writes for a replay from saved
/// `unityrs show` documents of the same synthetic bundle.
fn authored_rows(document: &J, shows: &str) -> J {
    let scratch = Scratch::new("anim-authored-budgets");
    let mut unbudgeted = document.clone();
    unbudgeted["spec"]
        .as_object_mut()
        .unwrap()
        .remove("budgets");
    let path = scratch.join("replay.json");
    std::fs::write(&path, serde_json::to_vec(&unbudgeted).unwrap()).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/parity");
    let output = child::output(
        Command::new("python3")
            .arg("-I")
            .arg(root.join("anim_replay_budgets.py"))
            .arg(&path)
            .arg("--show-dir")
            .arg(root.join("tests/anim_budget_show").join(shows)),
    )
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let authored: J = serde_json::from_slice(&output.stdout).unwrap();
    authored["spec"]["budgets"].clone()
}

#[test]
#[allow(clippy::cast_possible_truncation)]
fn a_fast_single_axis_turn_authored_in_python_passes_the_rust_audit() {
    // The saved documents in tools/parity/tests/anim_budget_show/turn are
    // this bundle's: a Z-only Euler turn from 0 to 3000 degrees.
    let clip = legacy_euler_clip(&[Key::new(2., [0.; 3]), Key::new(3., [0., 0., 3000.])]);
    with_clip(
        true,
        clip,
        |document| {
            for sample in document["capture"]["samples"].as_array_mut().unwrap() {
                let t = sample["local"].as_f64().unwrap();
                let half = (1500. * t * t * (3. - 2. * t)).to_radians();
                sample["pose"]["transforms"][0]["rotation"] = json!([
                    bits(0.),
                    bits(0.),
                    bits(half.sin() as f32),
                    bits(half.cos() as f32)
                ]);
            }
            document["spec"]["budgets"] = authored_rows(document, "turn");
        },
        |dir, valid, frozen| compare(valid, dir, frozen).unwrap(),
    );
}

#[test]
fn a_raw_path_through_the_origin_is_too_close_to_bound() {
    // W runs linearly from 1 to -1 while X runs from 2^-14 to -2^-14, so
    // the raw quaternion passes through the origin at 2.5 s.
    let tiny = 2_f32.powi(-14);
    let mut keys = [
        Key::new(2., [tiny, 0., 0., 1.]),
        Key::new(3., [-tiny, 0., 0., -1.]),
    ];
    for key in &mut keys {
        key.incoming = [-2. * tiny, 0., 0., -2.];
        key.outgoing = key.incoming;
    }
    let mut clip = legacy_clip();
    let row = p::curve("", &keys, [2, 2], None, true, None);
    unityrs_testkit::scene3d::set(
        &mut clip.value,
        "m_RotationCurves",
        vector(row.clone(), vec![row]),
    );
    with_clip(
        true,
        clip,
        |document| {
            let mut rotation = budget(
                "rotation",
                0,
                "hermite",
                &json!([[-1, 1], [-1, 1], [-2, 0], [-2, 0], [1, 1]]),
            );
            rotation["stages"][0]["norm_ranges"] = json!([
                [[-1, 1], [-1, 1], [-1, 1], [0.5, 1]],
                [[-1, 1], [-1, 1], [-1, 1], [-1, -0.5]]
            ]);
            document["spec"]["budgets"][1] = rotation;
        },
        |dir, valid, frozen| {
            rejects(
                dir,
                valid,
                frozen,
                "rotation path too close to the origin to bound",
            );
        },
    );
}
