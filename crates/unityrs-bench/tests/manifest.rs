//! The gate scale, regenerated, is the manifest `benchmarks/fixtures.json`
//! records (`docs/plans/bench.md` decision 3): any change to a builder that
//! moves a fixture byte fails here until the manifest is regenerated with
//! `python tools/bench/bench.py gen --update`.

use std::path::Path;

use serde_json::Value;
use unityrs_bench::{Scale, generate};

fn recorded(scale: Scale) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/fixtures.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let doc: Value = serde_json::from_str(&text).unwrap();
    doc["scales"][scale.name()].clone()
}

fn check(scale: Scale) {
    let made = generate(scale, |_| Ok(())).unwrap();
    assert_eq!(
        made,
        recorded(scale),
        "{scale:?}: regenerate benchmarks/fixtures.json"
    );
}

#[test]
fn the_gate_scale_is_the_recorded_manifest() {
    check(Scale::Gate);
}

/// The full scale too (about 180 MB, built in memory one file at a time);
/// `bench.py run` checks it before every comparison.
#[test]
#[ignore = "builds the full set; bench.py run checks it"]
fn the_full_scale_is_the_recorded_manifest() {
    check(Scale::Full);
}
