//! A late-start clip whose playable time is source time (`zero_origin`).
#![allow(clippy::cast_possible_truncation)]
use super::fixture::{bits, rejects, with_recording};
use crate::anim_support::bundle;
use crate::compare;
use serde_json::json;

#[test]
fn zero_origin_replay_reads_playable_time_as_source_time() {
    // Playable time runs from 0 to the stop at 3. Below the start at 2 the
    // clip holds its start pose, and the held weight jumps at 2.25.
    let below = f64::from(2.25_f32.next_down());
    let local = [0., below, 2.5, 3.];
    with_recording(
        false,
        bundle(false),
        |document| {
            document["spec"]["source_clock"] = json!("zero_origin");
            document["spec"]["schedule"] = json!(local.map(|l| json!({ "local": l })));
            document["capture"]["playback"]["clip_length_f32"] = json!(bits(3.));
            let samples = document["capture"]["samples"].as_array_mut().unwrap();
            for (sample, l) in samples.iter_mut().zip(local) {
                sample["local"] = json!(l);
                sample["local_f32"] = json!(bits(l as f32));
                sample["source"] = json!(l.max(2.));
                sample["effective_source"] = json!(l.max(2.));
            }
        },
        |dir, valid, frozen| {
            assert_eq!(frozen["clock_discriminated"], json!(true));
            assert_eq!(frozen["samples"][0]["source"], json!(2.));
            compare(valid, dir, frozen).unwrap();
            // Read with the interval-start clock, the schedule overruns the clip.
            let mut manifest = valid.clone();
            manifest["spec"]["source_clock"] = json!("widened");
            rejects(dir, &manifest, frozen, "replay endpoint schedule");
            let mut manifest = valid.clone();
            manifest["capture"]["samples"][1]["source"] = json!(2. + below);
            rejects(dir, &manifest, frozen, "bundle sample clocks");
        },
    );
}
