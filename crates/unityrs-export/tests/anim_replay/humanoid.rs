//! Same-built-bundle humanoid qualification. The evidence comes from
//! `tools/parity/anim_humanoid_qualify.py` on a completed Editor run: Rust
//! reads the same digest-pinned bundles, checks the recorded slot
//! inventory, storage, settings, Avatar twist and stretch and dropped
//! muscles, and its typed refusal for each clip must be one of the causes
//! the evidence lists (cross-check obligation 3). The clip set is the
//! run's: every required humanoid clip and every optional one `run.json`
//! marks recorded, each with its completed replay.
use crate::{J, Result, address, index, list, load, object, require, same, string};
use std::{path::Path, sync::Arc};
use unityrs_anim::{
    RigPaths, StorageKind, discover,
    humanoid::{
        DofKind, NEUTRAL_STRETCH, NEUTRAL_TWIST, SettingValue, dropped_muscles, humanoid_refusal,
        rig_avatar,
    },
    read_clip,
};
use unityrs_scene::{GraphOptions, Unit};

/// The humanoid clips every run records (`REPLAY_CASES`).
const REQUIRED: &[&str] = &[
    "humanoid-compact-body-motion",
    "humanoid-compact-muscles",
    "humanoid-compact-muscles-offset",
    "humanoid-tall-muscles",
    "humanoid-tall-muscles-offset",
];
/// The optional humanoid coverage clips (`COVERAGE_CASES`).
const OPTIONAL: &[&str] = &[
    "humanoid-non-neutral-setting-refusal",
    "humanoid-root-basis-o0-y0-xz0",
    "humanoid-root-basis-o0-y0-xz1",
    "humanoid-root-basis-o0-y1-xz0",
    "humanoid-root-basis-o0-y1-xz1",
    "humanoid-root-basis-o1-y0-xz0",
    "humanoid-root-basis-o1-y0-xz1",
    "humanoid-root-basis-o1-y1-xz0",
    "humanoid-root-basis-o1-y1-xz1",
    "humanoid-twist-refusal",
];

/// The run's clip set, from `run.json` beside the bundles folder, each
/// with its completed replay.
fn expected_cases(dir: &Path) -> Result<Vec<String>> {
    let run = dir.parent().ok_or("bundles folder has no run folder")?;
    let manifest: J = serde_json::from_slice(&std::fs::read(run.join("run.json"))?)?;
    let recorded = list(&manifest["cases"])?
        .iter()
        .filter(|row| row["status"] == "recorded")
        .map(|row| string(&row["name"]))
        .collect::<Result<Vec<_>>>()?;
    let mut cases = REQUIRED
        .iter()
        .map(|&name| name.to_owned())
        .collect::<Vec<_>>();
    cases.extend(
        OPTIONAL
            .iter()
            .filter(|name| recorded.contains(*name))
            .map(|&name| name.to_owned()),
    );
    cases.sort();
    for case in &cases {
        let replay = run.join("replays").join(format!("{case}.json"));
        if !replay.is_file() {
            return Err(format!("{case}: missing completed replay").into());
        }
    }
    Ok(cases)
}

fn kind(kind: DofKind) -> &'static str {
    match kind {
        DofKind::Motion => "motion",
        DofKind::Body => "body",
        DofKind::Goal => "goal",
        DofKind::Muscle => "muscle",
        DofKind::Translation => "translation",
    }
}

fn storage(storage: Option<(StorageKind, usize)>) -> &'static str {
    match storage {
        None => "absent",
        Some((StorageKind::Streamed, _)) => "streamed",
        Some((StorageKind::Dense, _)) => "dense",
        Some((StorageKind::Constant, _)) => "constant",
    }
}

fn same_setting(value: Option<SettingValue>, recorded: &J) -> bool {
    match value {
        None => recorded.is_null(),
        Some(SettingValue::Bool(value)) => recorded.as_bool() == Some(value),
        Some(SettingValue::Float(value)) => recorded.as_f64() == Some(f64::from(value)),
    }
}

/// Check one clip's recorded inventory and refusal; returns the refusal.
pub(crate) fn check_clip(entry: &J, dir: &Path) -> Result<String> {
    let spec = &entry["spec"];
    let (env, files) = load(spec, dir)?;
    let prefab = object(&files, &spec["prefab"])?;
    let clip_object = object(&files, &spec["clip"])?;
    let binding = object(&files, &spec["binding"])?;
    let unit = Unit::Prefab(prefab);
    let meter: &Arc<_> = unit.meter();
    let graph = unit.graph(&env, &GraphOptions::default(), meter)?;
    let discovery = discover(&env, &graph, meter)?;
    let node = address(&graph, &spec["binding_address"])?;
    let rig = discovery
        .rigs()
        .iter()
        .filter(|rig| rig.node == node && rig.component.class_id() == 95)
        .nth(index(&spec["binding_component"])?)
        .ok_or("missing recorded Animator")?;
    require(same(&rig.component, &binding), "Animator identity")?;
    let clip = read_clip(&clip_object, meter)?;
    let human = clip.humanoid().ok_or("clip has no humanoid slot table")?;
    let counts = human.counts();
    require(
        [counts.streamed, counts.dense, counts.constant]
            .iter()
            .zip(["streamed", "dense", "constant"])
            .all(|(&count, name)| entry["storage_counts"][name].as_u64() == Some(count as u64)),
        "storage counts",
    )?;
    let recorded = list(&entry["slots"])?;
    let slots = human.slots().collect::<Vec<_>>();
    require(
        recorded.len() == slots.len()
            && recorded.iter().zip(&slots).all(|(row, slot)| {
                row["dof"].as_u64() == Some(slot.dof as u64)
                    && row["kind"] == kind(slot.kind)
                    && row["index_value"].as_i64() == Some(i64::from(slot.index_value))
                    && row["storage"] == storage(slot.storage)
            }),
        "humanoid slot inventory",
    )?;
    require(
        entry["absent_slot"].as_i64() == Some(i64::from(unityrs_anim::humanoid::ABSENT_SLOT)),
        "absent sentinel",
    )?;
    let settings = list(&entry["settings"])?;
    require(
        settings.len() == human.settings().len()
            && settings.iter().zip(human.settings()).all(|(row, setting)| {
                row["field"] == setting.field && same_setting(setting.value, &row["serialized"])
            }),
        "humanoid clip settings",
    )?;
    let avatar = rig_avatar(&env, &rig.component, meter)?;
    let read = avatar.avatar.as_ref().ok_or("missing Avatar")?;
    require(
        same(&read.object, &object(&files, &spec["avatar"])?),
        "Avatar identity",
    )?;
    let stretch = [
        ("m_ArmStretch", read.human.arm_stretch),
        ("m_LegStretch", read.human.leg_stretch),
        ("m_FeetSpacing", read.human.feet_spacing),
    ];
    require(
        read.human
            .twist
            .fields()
            .iter()
            .chain(&stretch)
            .all(|(field, value)| {
                entry["avatar_settings"][field].as_f64() == Some(f64::from(*value))
            }),
        "Avatar twist and stretch",
    )?;
    require(
        entry["dropped_muscles"]["count"].as_u64()
            == Some(dropped_muscles(human, &read.human) as u64),
        "dropped optional-bone muscles",
    )?;
    let paths = RigPaths::new(&graph, rig.node, meter)?;
    Ok(humanoid_refusal(&clip, &avatar, &paths, meter).to_string())
}

/// The recorded neutral twin's twist and stretch are the values playback
/// treats as neutral.
fn check_neutral_twin(document: &J) -> Result<()> {
    let values = &document["neutral_twin"]["values"];
    let neutral = |field: &str, value: f32| values[field].as_f64() == Some(f64::from(value));
    require(
        [
            "m_ArmTwist",
            "m_ForeArmTwist",
            "m_UpperLegTwist",
            "m_LegTwist",
        ]
        .iter()
        .all(|field| neutral(field, NEUTRAL_TWIST))
            && ["m_ArmStretch", "m_LegStretch", "m_FeetSpacing"]
                .iter()
                .all(|field| neutral(field, NEUTRAL_STRETCH)),
        "neutral twin twist and stretch",
    )
}

pub(crate) fn check_document(document: &J, dir: &Path) -> Result<()> {
    require(
        document["format"] == "unityrs.anim_humanoid_qualification" && document["version"] == 2,
        "expected humanoid qualification version 2",
    )?;
    let qualified = list(&document["qualified"])?;
    // Positive playback waits for a qualified clip (plan section 2.4).
    require(
        qualified.is_empty(),
        "qualified humanoid clips need playback",
    )?;
    check_neutral_twin(document)?;
    let expected = expected_cases(dir)?;
    let listed = list(&document["expected_cases"])?
        .iter()
        .map(string)
        .collect::<Result<Vec<_>>>()?;
    let clips = list(&document["clips"])?;
    let cases = clips
        .iter()
        .map(|entry| string(&entry["case"]))
        .collect::<Result<Vec<_>>>()?;
    require(
        !expected.is_empty() && listed == expected && cases == expected,
        "the evidence covers another clip set than the run",
    )?;
    for entry in clips {
        let case = string(&entry["case"])?;
        let refusal = check_clip(entry, dir).map_err(|error| format!("{case}: {error}"))?;
        let recorded = list(&entry["causes"])?
            .iter()
            .map(string)
            .collect::<Result<Vec<_>>>()?;
        if !recorded.contains(&refusal.as_str()) {
            return Err(
                format!("{case}: Rust refused {refusal}, evidence causes {recorded:?}").into(),
            );
        }
        let probe = string(&entry["expected_refusal"])?;
        let code = refusal.split('(').next().unwrap_or_default();
        if !probe.is_empty() && code != probe {
            return Err(
                format!("{case}: Rust refused {refusal}, the probe expects {probe}").into(),
            );
        }
    }
    Ok(())
}
