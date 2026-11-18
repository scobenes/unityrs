//! Same-built-bundle humanoid qualification. The evidence comes from
//! `tools/parity/anim_humanoid_qualify.py` on a completed Editor run: Rust
//! reads the same digest-pinned bundles, checks the recorded slot
//! inventory, storage, settings and Avatar twist, and must give each clip
//! the recorded typed refusal (cross-check obligation 3).
use crate::{J, Result, address, index, list, load, object, require, same, string};
use std::{path::Path, sync::Arc};
use unityrs_anim::{
    RigPaths, StorageKind, discover,
    humanoid::{DofKind, SettingValue, humanoid_refusal, rig_avatar},
    read_clip,
};
use unityrs_scene::{GraphOptions, Unit};

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
    require(
        read.human.twist.fields().iter().all(|(field, value)| {
            entry["avatar_settings"][field].as_f64() == Some(f64::from(*value))
        }),
        "Avatar twist",
    )?;
    let paths = RigPaths::new(&graph, rig.node, meter)?;
    Ok(humanoid_refusal(&clip, &avatar, &paths, meter).to_string())
}

pub(crate) fn check_document(document: &J, dir: &Path) -> Result<()> {
    require(
        document["format"] == "unityrs.anim_humanoid_qualification" && document["version"] == 1,
        "expected humanoid qualification version 1",
    )?;
    let qualified = list(&document["qualified"])?;
    // Positive playback waits for a qualified clip (plan section 2.4).
    require(
        qualified.is_empty(),
        "qualified humanoid clips need playback",
    )?;
    for entry in list(&document["clips"])? {
        let refusal = check_clip(entry, dir)
            .map_err(|error| format!("{}: {error}", string(&entry["case"]).unwrap_or("?")))?;
        if refusal != string(&entry["refusal"])? {
            return Err(format!(
                "{}: Rust refused {refusal}, evidence {}",
                string(&entry["case"])?,
                entry["refusal"]
            )
            .into());
        }
    }
    Ok(())
}
