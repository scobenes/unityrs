//! Read every Avatar and humanoid clip slot table of a mutated rig, then
//! the ordered humanoid refusal of each discovered rig's clips.
use crate::scene::{Digest, Outcome, anim_refusal};
use std::{mem::discriminant, sync::Arc};
use unityrs_anim::{
    AnimError, Clip, RigPaths, discover,
    humanoid::{
        Avatar, DofKind, HumanoidClip, RigAvatar, SettingValue, Xform, dropped_muscles,
        humanoid_refusal, read_avatar, rig_avatar,
    },
    read_clip,
};
use unityrs_core::{
    env::Environment,
    files::SerializedFile,
    limits::{Meter, MeteredVec},
};
use unityrs_scene::Graph;

/// The most humanoid clips of the file kept to pair with every rig.
const CROSS: usize = 16;

/// Panics on a nondeterministic result or retained operation charge.
/// Flags: 1 selects a prefab graph, 2 pairs every rig with every humanoid
/// clip of the file (at most [`CROSS`]) as well as with its own slots.
pub fn check(input: &[u8]) -> bool {
    crate::scene::check(input, |case, limit| {
        crate::scene::run_root(case, limit, |env, _, file, meter| {
            let mut digest = Digest::default();
            let clips = match sweep(file, case.flags, meter, &mut digest) {
                Ok(clips) => clips,
                Err(e) => return Outcome::anim_error(e),
            };
            let graph = match crate::scene::graph(env, file, case.flags, meter) {
                Ok(g) => g,
                Err(outcome) => return outcome,
            };
            match rigs(env, &graph, clips.as_slice(), meter, &mut digest) {
                Ok(()) => Outcome::Done(digest.finish()),
                Err(e) => Outcome::anim_error(e),
            }
        })
    })
}

/// A content error is hashed; a resource refusal stops the run.
fn soft(e: AnimError, digest: &mut Digest) -> Result<(), AnimError> {
    if anim_refusal(&e) {
        return Err(e);
    }
    digest.tag(format!("{e:?}"));
    Ok(())
}

/// Every Avatar and clip of the file, read directly; the humanoid clips
/// kept for pairing when flag 2 is set.
fn sweep(
    file: &Arc<SerializedFile>,
    flags: u8,
    meter: &Arc<Meter>,
    digest: &mut Digest,
) -> Result<MeteredVec<Clip>, AnimError> {
    let mut kept = MeteredVec::new(meter);
    for object in file.objects() {
        meter.count_decode(1)?;
        match object.class_id() {
            90 => match read_avatar(&object, meter) {
                Ok(avatar) => avatar_digest(&avatar, meter, digest)?,
                Err(e) => soft(e, digest)?,
            },
            74 => match read_clip(&object, meter) {
                Ok(clip) => {
                    digest.tag(clip.issue().map(|e| format!("{e:?}")));
                    let Some(slots) = clip.humanoid() else {
                        continue;
                    };
                    slots_digest(slots, meter, digest)?;
                    if flags & 2 != 0 && kept.len() < CROSS {
                        kept.push(clip)?;
                    }
                }
                Err(e) => soft(e, digest)?,
            },
            _ => {}
        }
    }
    Ok(kept)
}

fn xform_digest(x: &Xform, digest: &mut Digest) {
    for v in x.t.iter().chain(&x.q).chain(&x.s) {
        digest.tag(v.to_bits());
    }
}

fn avatar_digest(
    avatar: &Avatar,
    meter: &Arc<Meter>,
    digest: &mut Digest,
) -> Result<(), AnimError> {
    let human = &avatar.human;
    digest.tag((
        avatar.skeleton.nodes.len(),
        avatar.skeleton_pose.len(),
        avatar.default_pose.len(),
        avatar.name_ids.len(),
        human.skeleton.nodes.len(),
        human.skeleton.axes.len(),
        human.pose.len(),
        avatar.root_motion_bone,
        avatar.root_motion_skeleton.nodes.len(),
        avatar.tos.len(),
    ));
    digest.tag((
        human.has_left_hand,
        human.has_right_hand,
        human.has_translation_dof,
        human.scale.to_bits(),
        human.arm_stretch.to_bits(),
        human.leg_stretch.to_bits(),
        human.feet_spacing.to_bits(),
    ));
    for (field, value) in human.twist.fields() {
        digest.tag((field, value.to_bits()));
    }
    xform_digest(&human.root, digest);
    xform_digest(&avatar.root_motion_x, digest);
    meter.count_decode(55)?;
    for bone in 0..55 {
        digest.tag(human.bone_node(bone));
    }
    for axes in human.skeleton.axes.as_slice() {
        meter.count_decode(1)?;
        digest.tag((axes.kind, axes.length.to_bits()));
    }
    for (node, &id) in human
        .skeleton
        .nodes
        .as_slice()
        .iter()
        .zip(human.skeleton.ids.as_slice())
    {
        let path = avatar.tos_path(id, meter)?;
        digest.tag((node.parent, node.axes, id, path.map(|p| p.as_bytes().len())));
    }
    Ok(())
}

fn slots_digest(
    slots: &HumanoidClip,
    meter: &Arc<Meter>,
    digest: &mut Digest,
) -> Result<(), AnimError> {
    let counts = slots.counts();
    digest.tag((counts.streamed, counts.dense, counts.constant));
    digest.tag((
        slots.has_reference_pose(),
        slots.has_generic_root(),
        slots.root_basis_refusal(),
    ));
    meter.count_decode(unityrs_anim::humanoid::DOF_COUNT as u64)?;
    for slot in slots.slots() {
        digest.tag((
            slot.dof,
            discriminant(&slot.kind),
            slot.ordinal,
            slot.index_value,
        ));
        digest.tag(slot.storage.map(|(kind, at)| (discriminant(&kind), at)));
    }
    for kind in [
        DofKind::Motion,
        DofKind::Body,
        DofKind::Goal,
        DofKind::Muscle,
        DofKind::Translation,
    ] {
        meter.count_decode(unityrs_anim::humanoid::DOF_COUNT as u64)?;
        digest.tag(slots.mapped(kind).count());
    }
    for setting in slots.settings() {
        let value = setting.value.map(|value| match value {
            SettingValue::Bool(b) => (0, u32::from(b)),
            SettingValue::Float(f) => (1, f.to_bits()),
        });
        digest.tag((
            setting.field,
            value,
            setting.neutral(),
            setting.is_root_basis(),
        ));
    }
    for writer in slots.transform_writers() {
        meter.count_decode(1)?;
        digest.tag((writer.binding, writer.path));
    }
    Ok(())
}

/// Each discovered rig's Avatar, then the refusal of its own clips and,
/// with flag 2, of every kept clip.
fn rigs(
    env: &Environment,
    graph: &Graph,
    kept: &[Clip],
    meter: &Arc<Meter>,
    digest: &mut Digest,
) -> Result<(), AnimError> {
    let discovery = discover(env, graph, meter)?;
    for rig in discovery.rigs() {
        meter.count_decode(1)?;
        digest.tag((rig.node, rig.slots().len(), rig.issues().len()));
        let avatar = match rig_avatar(env, &rig.component, meter) {
            Ok(avatar) => avatar,
            Err(e) => {
                soft(e, digest)?;
                continue;
            }
        };
        digest.tag((avatar.avatar.is_some(), avatar.transform_hierarchy));
        let paths = RigPaths::new(graph, rig.node, meter)?;
        for slot in rig.slots() {
            meter.count_decode(1)?;
            let Ok(object) = slot.clip() else {
                digest.tag("unresolved");
                continue;
            };
            match read_clip(object, meter) {
                Ok(clip) => refuse(&clip, &avatar, &paths, meter, digest)?,
                Err(e) => soft(e, digest)?,
            }
        }
        for clip in kept {
            meter.count_decode(1)?;
            refuse(clip, &avatar, &paths, meter, digest)?;
        }
    }
    Ok(())
}

fn refuse(
    clip: &Clip,
    avatar: &RigAvatar,
    paths: &RigPaths<'_>,
    meter: &Arc<Meter>,
    digest: &mut Digest,
) -> Result<(), AnimError> {
    let refusal = humanoid_refusal(clip, avatar, paths, meter);
    if anim_refusal(&refusal) {
        return Err(refusal);
    }
    digest.tag(refusal.to_string());
    if let (Some(slots), Some(rig)) = (clip.humanoid(), &avatar.avatar) {
        meter.count_decode(unityrs_anim::humanoid::DOF_COUNT as u64)?;
        digest.tag(dropped_muscles(slots, &rig.human));
    }
    Ok(())
}
