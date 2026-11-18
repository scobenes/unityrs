//! Budget and clock rules of the per-recording tolerance derivation.
use super::fixture::{bits, budget, rejects, with_clip, with_fixture};
use crate::anim_support::{GenericClip, generic_clip};
use crate::{J, compare};
use serde_json::json;

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
    for (ramp, own_ranges, passes) in [
        (true, true, true),
        (false, true, false),
        (false, false, true),
    ] {
        with_clip(
            false,
            generic_clip(GenericClip::RampBesideLarge),
            |document| {
                let mut row = budget("translation", 0, "streamed", &envelope);
                if own_ranges {
                    row["component_ranges"] = components.clone();
                }
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
                } else {
                    // A replay that leaves X at rest must not pass on Y's
                    // f32 rounding at 2^20.
                    rejects(dir, valid, frozen, "translation");
                }
            },
        );
    }
}
