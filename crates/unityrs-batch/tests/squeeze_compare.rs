//! X13's own checks, checked (`docs/plans/extract.md` X13): the run one
//! below `M`'s need must be exactly the records the reference model derives
//! from its baseline (`squeeze_support::compare`), so a skip naming no
//! baseline output, a sibling crop's changed bytes, a member's changed path
//! or archive, a unit left as it was, a deleted, moved or added row and a
//! duplicated, moved or changed summary each fail it; a real failed-`M`
//! outcome is recognised by its typed reason; and the CR7 exclusions find a
//! dropped container by the core's own registry key (`simplify_name`).
//!
//! It reads the engine's test tap, so it is built with `test-hooks`.
#![cfg(feature = "test-hooks")]

mod squeeze_support;

use std::sync::{Mutex, OnceLock, PoisonError};

use serde_json::Value as J;
use squeeze_support::compare::below_m;
use squeeze_support::exclusions::{Gap, gaps};
use squeeze_support::{BIG, BIG_EVICTIONS, Outcome, catalogue, limits, refused_m, run};
use unityrs_testkit::env::bundle;
use unityrs_testkit::search::least;

/// The member variant of a scene with one atlas of four valid crops, `M`
/// skipping some of them and leaving a sibling crop and its member.
const SCENE: (&str, &str) = ("own_sheet_by_material_name", "x.bundle");

/// CR8's counts are the process's: one run at a time, or a run counts
/// another test's refusals.
static SERIAL: Mutex<()> = Mutex::new(());

fn at(m: u64) -> Outcome {
    let _one = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    let scene = catalogue()
        .into_iter()
        .find(|(n, _)| *n == SCENE.0)
        .unwrap()
        .1;
    run(&scene, SCENE.1, limits(m, BIG, BIG_EVICTIONS, BIG), true)
}

/// The real baseline, and the real run one below its `M` need, searched
/// once for every test.
fn pair() -> (Outcome, Outcome) {
    static PAIR: OnceLock<(Outcome, Outcome)> = OnceLock::new();
    PAIR.get_or_init(|| {
        let need = least(0, BIG, 0, |m| refused_m(&at(m))).unwrap().need;
        (at(BIG), at(need - 1))
    })
    .clone()
}

fn rows(o: &Outcome) -> Vec<J> {
    o.records
        .iter()
        .map(|r| serde_json::from_str(r).unwrap())
        .collect()
}

fn find(o: &Outcome, f: impl Fn(&J) -> bool) -> usize {
    rows(o).iter().position(f).expect("such a row")
}

fn m_skip(r: &J) -> bool {
    r["type"] == "skip"
        && r["reason"]
            .as_str()
            .is_some_and(|x| x.starts_with("limit("))
}

/// `outcome` with its row `row`'s `field` set to `value`.
fn with(mut outcome: Outcome, row: usize, field: &str, value: J) -> Outcome {
    let mut record: J = serde_json::from_str(&outcome.records[row]).unwrap();
    record[field] = value;
    outcome.records[row] = record.to_string();
    outcome
}

/// The surviving sibling crop, the atlas's first: its row and its member's.
fn sibling(below: &Outcome) -> (usize, usize) {
    let crop = find(below, |r| {
        r["type"] == "candidate" && r["kind"] == "atlas_sprite"
    });
    let key = rows(below)[crop]["key"].clone();
    assert_eq!(key, "ui/a.png");
    (
        crop,
        find(below, |r| r["type"] == "member" && r["key"] == key),
    )
}

/// The real run skips crops 1 to 3 of the atlas's sprites, the atlas's
/// crops by `mSprites` index (its sprites 4 and 5 the baseline's own
/// skips), and the model's records are exactly its.
#[test]
fn a_real_skip_of_three_crops_is_the_models_result() {
    let (base, below) = pair();
    let skipped: Vec<_> = rows(&below)
        .into_iter()
        .filter(m_skip)
        .map(|r| (r["output"].clone(), r["sprite"].clone()))
        .collect();
    let crop = |n: u64| (J::from("atlas_sprite"), J::from(n));
    assert_eq!(skipped, [crop(1), crop(2), crop(3)]);
    let _ = sibling(&below);
    assert_eq!(below_m(&below, &base), Ok("skipped"), "{below:?}");
}

/// A skip naming a sprite the baseline has no crop for takes no crop's
/// place, so the crop it stands for is unexplained.
#[test]
fn a_skip_of_no_baseline_sprite_is_not_a_skip() {
    let (base, below) = pair();
    let skip = find(&below, m_skip);
    let changed = with(below, skip, "sprite", J::from(999));
    assert!(below_m(&changed, &base).is_err());
}

/// The sibling crop's bytes, and its member's path and archive, are the
/// baseline's.
#[test]
fn a_sibling_crop_and_its_member_are_the_baselines() {
    let (base, below) = pair();
    let (crop, member) = sibling(&below);
    let sha = rows(&below)[crop]["sha256"].as_str().unwrap().to_owned();
    let flipped = format!(
        "{}{}",
        if sha.starts_with('0') { '1' } else { '0' },
        &sha[1..]
    );
    for (i, k, v) in [
        (crop, "sha256", J::from(flipped)),
        (member, "path", J::from("atlases/ui/elsewhere.png")),
        (member, "archive", J::from(1)),
    ] {
        let changed = with(below.clone(), i, k, v);
        assert!(below_m(&changed, &base).is_err(), "a sibling's {k} passed");
    }
}

#[test]
fn a_deleted_reordered_or_added_row_is_not_a_skip() {
    let (base, below) = pair();
    let (crop, _) = sibling(&below);
    let mut deleted = below.clone();
    deleted.records.remove(crop);
    let mut reordered = below.clone();
    reordered.records.swap(crop, crop + 1);
    let mut added = below.clone();
    added.records.insert(crop, below.records[crop].clone());
    for (what, o) in [
        ("deleted", deleted),
        ("reordered", reordered),
        ("added", added),
    ] {
        assert!(below_m(&o, &base).is_err(), "a {what} row passed");
    }
}

/// One summary, last, its fields the model's, its export work exact.
#[test]
fn a_duplicated_moved_or_changed_summary_is_not_a_skip() {
    let (base, below) = pair();
    let s = below.records.len() - 1;
    let summary = below.records[s].clone();
    let mut doubled = below.clone();
    doubled.records.push(summary.clone());
    let mut moved = below.clone();
    let last = moved.records.pop().unwrap();
    moved.records.insert(0, last);
    for (what, o) in [("duplicated", doubled), ("moved", moved)] {
        assert!(below_m(&o, &base).is_err(), "a {what} summary passed");
    }
    let skips = rows(&below)[s]["skips"].as_u64().unwrap();
    let work = rows(&below)[s]["export_work"].as_u64().unwrap();
    for (k, v) in [
        ("id", J::from("other.bundle")),
        ("status", J::from("failed")),
        ("reason", J::from("container_error")),
        ("skips", J::from(skips + 1)),
        ("unresolved_scripts", J::from(1)),
        ("export_work", J::from(0)),
        ("export_work", J::from(work - 1)),
    ] {
        let changed = with(below.clone(), s, k, v);
        assert!(below_m(&changed, &base).is_err(), "a summary's {k} passed");
    }
}

/// The export's skips follow the plan's (4.4's **Record order**): an
/// export-time `M` skip moved before the atlas's plan-time `empty_rect`
/// skips is refused.
#[test]
fn an_export_skip_before_the_plans_is_not_a_skip() {
    let (base, below) = pair();
    let plan = find(&below, |r| r["phase"] == "plan");
    assert_eq!(rows(&below)[plan]["reason"], "empty_rect");
    let skip = find(&below, m_skip);
    let mut moved = below.clone();
    let row = moved.records.remove(skip);
    moved.records.insert(plan, row);
    assert!(below_m(&moved, &base).is_err());
}

/// An `M` skip row's fields are the model's, derived from the baseline's
/// object and the run's `M`, never taken from the row.
#[test]
fn an_m_skips_fields_are_derived() {
    let (base, below) = pair();
    let skip = find(&below, m_skip);
    let m = below.m;
    for (k, v) in [
        ("input", J::from("other.bundle")),
        ("class", J::from("Texture2D")),
        ("detail", J::from("render")),
        ("cut", J::from(true)),
        ("reason", J::from(format!("limit(input_memory, {})", m + 1))),
        ("anchor", J::from(0)),
        ("work", serde_json::json!([["pixels", 1]])),
    ] {
        let changed = with(below.clone(), skip, k, v);
        assert!(below_m(&changed, &base).is_err(), "an M skip's {k} passed");
    }
}

/// A hand-derived run below `M` from the baseline: its summary's skips the
/// skip rows, its status theirs, its export work every row's and the
/// records file's (3000 bytes: one 4 KiB flush).
fn derived(base: &Outcome, mut records: Vec<J>) -> Outcome {
    let work: u64 = records
        .iter()
        .flat_map(|r| r["work"].as_array().cloned().unwrap_or_default())
        .map(|e| e[1].as_u64().unwrap())
        .sum();
    let mut summary = rows(base).pop().unwrap();
    summary["skips"] = J::from(records.iter().filter(|r| r["type"] == "skip").count());
    summary["export_work"] = J::from(work + 4096);
    records.push(summary);
    let mut o = base.clone();
    o.records = records.iter().map(J::to_string).collect();
    o.records_len = 3000;
    o.m = 1000;
    o.m_refusals = 1;
    o
}

/// The atlas's `M` skip row, at `phase`, of `output`.
fn atlas_skip(json: &J, output: &str, phase: &str) -> J {
    let mut skip = serde_json::json!({
        "type": "skip", "input": "x.bundle", "serialized_file": json["serialized_file"],
        "path_id": json["path_id"], "class": "MonoBehaviour", "output": output,
        "sprite": null, "reason": "limit(input_memory, 1000)", "detail": null,
        "cut": false, "work": [], "phase": phase,
    });
    if phase == "export" {
        skip["anchor"] = J::from(0);
    }
    skip
}

/// An export-time `M` skip of the atlas's JSON, derived by hand
/// (`docs/BATCH_SPEC.md` 4.5's **Units**): the JSON and its member leave,
/// and with neither the sheet's texture nor the JSON made, every crop is a
/// plain candidate. A crop left in the JSON's unit is refused.
#[test]
fn an_atlas_json_skip_makes_its_crops_plain() {
    let (base, _) = pair();
    let mut records = rows(&base);
    records.pop();
    let json = records[0].clone();
    assert_eq!(json["key"], "ui.json");
    records.retain(|r| r["key"] != "ui.json");
    for r in &mut records {
        if r["type"] == "candidate" {
            assert_eq!(r["unit"], "ui.json");
            r["unit"] = J::Null;
        }
    }
    records.push(atlas_skip(&json, "atlas_json", "export"));
    let o = derived(&base, records);
    assert_eq!(below_m(&o, &base), Ok("skipped"));
    let crop = find(&o, |r| r["kind"] == "atlas_sprite");
    let kept = with(o, crop, "unit", J::from("ui.json"));
    assert!(
        below_m(&kept, &base).is_err(),
        "a crop kept the JSON's unit"
    );
}

/// The atlas's read refused at plan, derived by hand: nothing of the atlas
/// is planned, so its two `empty_rect` skips are never found either, and
/// the summary counts the one `M` skip. Keeping them is refused.
#[test]
fn an_atlas_refused_at_plan_finds_none_of_its_sprites_skips() {
    let (base, _) = pair();
    let all = rows(&base);
    let json = all[0].clone();
    let mut records: Vec<J> = all
        .iter()
        .filter(|r| r["type"] == "diagnostic")
        .cloned()
        .collect();
    records.push(atlas_skip(&json, "atlas", "plan"));
    let o = derived(&base, records.clone());
    assert_eq!(below_m(&o, &base), Ok("skipped"), "{o:?}");
    let mut kept: Vec<J> = all
        .iter()
        .filter(|r| r["type"] == "skip")
        .cloned()
        .collect();
    kept.insert(0, atlas_skip(&json, "atlas", "plan"));
    assert!(below_m(&derived(&base, kept), &base).is_err());
}

/// A real input failed on `M`: its typed reason, spelled as records spell
/// it, is an `M` failure.
#[test]
fn a_real_failure_on_m_is_one() {
    let (base, small) = (at(BIG), at(1000));
    assert_eq!(small.state, "Failed", "{small:?}");
    assert_eq!(below_m(&small, &base), Ok("failed"), "{small:?}");
}

/// A top-level container, then a later row whose core registry key is the
/// container's: a Windows-style basename, lower-cased as Python does.
#[test]
fn a_dropped_container_is_found_by_the_cores_key() {
    let inner = || bundle(&[("resource", vec![0; 16])]);
    for (first, later) in [
        ("dir\\CAB-A", "cab-a"),
        ("dir/CAB-A", "CAB-a"),
        ("x\\y/CAB-\u{c9}", "cab-\u{e9}"),
    ] {
        let data = bundle(&[(first, inner()), (later, vec![1; 8])]);
        assert!(
            gaps(&data).contains(&Gap::DroppedContainer),
            "{first:?} then {later:?}"
        );
    }
    let distinct = bundle(&[("dir\\CAB-A", inner()), ("cab-b", vec![1; 8])]);
    assert!(!gaps(&distinct).contains(&Gap::DroppedContainer));
}
