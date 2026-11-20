//! Read every Avatar and humanoid clip slot table of a mutated rig, with
//! each Avatar's body-model inputs (masses, slot table, scope and rest
//! body), then the ordered humanoid refusal of each discovered rig's clips.
use crate::scene::{Digest, Outcome, anim_refusal};
use std::{mem::discriminant, sync::Arc};
use unityrs_anim::{
    AnimError, Clip, RigPaths, discover,
    humanoid::{
        Avatar, BODY_SLOTS, BodyAvatar, BoneMass, DofKind, HumanoidClip, RigAvatar, SettingValue,
        Xform, body_avatar, body_model, body_solve, chain, dropped_muscles, humanoid_refusal,
        read_avatar, rig_avatar, root,
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
                Ok(_) => Outcome::Done(digest.finish()),
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
                    root_digest(&clip, meter, digest)?;
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

/// Freeze v8's root projection of a humanoid clip: its admitted settings,
/// then the target body pose at the interval's start, middle and stop
/// under a unit and a scaled human, each held to a finite translation and
/// a unit quaternion. The number of poses projected.
pub(crate) fn root_digest(
    clip: &Clip,
    meter: &Arc<Meter>,
    digest: &mut Digest,
) -> Result<usize, AnimError> {
    let settings = match root::root_settings(clip, meter) {
        Ok(settings) => settings,
        Err(e) => return soft(e, digest).map(|()| 0),
    };
    let rules = settings.rules();
    digest.tag((
        settings.admitted().variant,
        discriminant(&rules.yaw),
        discriminant(&rules.height),
        discriminant(&rules.xz),
        settings.start().0.map(f64::to_bits),
        settings.start().1.map(f64::to_bits),
    ));
    let timing = match root::root_timing(clip, meter) {
        Ok(timing) => timing,
        Err(e) => return soft(e, digest).map(|()| 0),
    };
    let (start, stop) = (timing.start(), timing.stop());
    let mut projected = 0;
    for time in [start, start + (stop - start) / 2., stop] {
        for scale in [1., 1.75] {
            meter.count_decode(1)?;
            match root::target_body_pose(clip, time, scale, meter) {
                Ok(pose) => {
                    let (t, q) = (pose.translation, pose.rotation);
                    assert!(t.iter().all(|v| v.is_finite()), "finite T_A");
                    let length = q.iter().map(|v| v * v).sum::<f64>().sqrt();
                    assert!((length - 1.).abs() <= 1e-12, "unit B_A");
                    digest.tag((t.map(f64::to_bits), q.map(f64::to_bits)));
                    projected += 1;
                }
                Err(e) => soft(e, digest)?,
            }
        }
    }
    Ok(projected)
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
    body_digest(avatar, meter, digest)
}

/// The body-model inputs, or their typed refusal.
fn body_digest(avatar: &Avatar, meter: &Arc<Meter>, digest: &mut Digest) -> Result<(), AnimError> {
    match avatar.human.bone_mass {
        Some(BoneMass::Slots(masses)) => {
            meter.count_decode(masses.len() as u64)?;
            for mass in masses {
                digest.tag(mass.to_bits());
            }
        }
        Some(BoneMass::Length(n)) => digest.tag(n),
        None => {}
    }
    let body = match body_avatar(avatar, meter) {
        Ok(body) => body,
        Err(e) => return soft(e, digest),
    };
    for slot in 0..BODY_SLOTS.len() {
        meter.count_decode(1)?;
        let path = body.slot_path(slot, meter)?;
        digest.tag((
            body.nodes[slot],
            body.masses[slot].to_bits(),
            body.rest(slot).map(|x| x.t.map(f32::to_bits)),
            path.map(|p| p.as_bytes().len()),
        ));
    }
    digest.tag((body.scale().to_bits(), body.root().t.map(f32::to_bits)));
    model_digest(&body, meter, digest)
}

/// Freeze v8's model on the Avatar's rest body: the weights, edges, rest
/// points, centre of mass, frame and twist scope, or their typed refusal.
/// The hips solve needs the rig's rest scales, so `rigs` runs it.
fn model_digest(
    body: &BodyAvatar<'_>,
    meter: &Arc<Meter>,
    digest: &mut Digest,
) -> Result<(), AnimError> {
    let bits = |p: Option<[f64; 3]>| p.map(|p| p.map(f64::to_bits));
    let weights = body.weights();
    digest.tag(weights.map(f64::to_bits));
    let (edges, rest) = match body
        .edges(meter)
        .and_then(|e| Ok((e, body.rest_points(meter)?)))
    {
        Ok(found) => found,
        Err(e) => return soft(e, digest),
    };
    digest.tag((edges, rest.map(bits)));
    match body_model::centre(&rest, &weights, &edges, &body_model::COEFFICIENTS) {
        Ok(centre) => digest.tag(bits(Some(centre))),
        Err(e) => soft(e, digest)?,
    }
    match body_model::body_frame(&rest)
        .and_then(|f| Ok((f, body_model::shoulder_hip_twist(&rest)?)))
    {
        Ok((frame, twist)) => {
            digest.tag((frame.map(|row| row.map(f64::to_bits)), twist.to_bits()));
            if let Err(e) = body_model::frame_scope(&rest) {
                return soft(e, digest);
            }
            Ok(())
        }
        Err(e) => soft(e, digest),
    }
}

/// The requests and hips parents each rig's rest body is solved for: the
/// identity parent, then turned, moved and scaled ones.
const SOLVES: [([f64; 3], [f64; 4], [f64; 3], [f64; 4], f64); 3] = [
    (
        [0.25, 1., -0.5],
        [0., 0.6, 0., 0.8],
        [0.; 3],
        [0., 0., 0., 1.],
        1.,
    ),
    (
        [-1.5, 0.75, 2.],
        [0.5, -0.5, 0.5, 0.5],
        [1., 2., 3.],
        [0., 0.8, 0., 0.6],
        2.,
    ),
    (
        [0., 0.5, 0.],
        [0.28, 0., 0.96, 0.],
        [-0.5, 0., 0.25],
        [0.6, 0., 0., 0.8],
        0.5,
    ),
];

/// Freeze v8's hips solve on a rig's rest body: its weights, edges and
/// rest frame, the hips frame from each node's rest rotation and the rig's
/// own rest scales, then the solve for each of `SOLVES`, or their typed
/// refusal. Panics on a solve with a non-finite translation or a quaternion
/// off unit length. The number of solves that succeeded.
fn solve_digest(
    body: &BodyAvatar<'_>,
    paths: &RigPaths<'_>,
    meter: &Arc<Meter>,
    digest: &mut Digest,
) -> Result<usize, AnimError> {
    let weights = body.weights();
    let found = body.edges(meter).and_then(|edges| {
        let rest = body.rest_points(meter)?;
        let frame = body_model::body_frame(&rest)?;
        let scales = body.rest_scales(paths, meter)?;
        Ok((edges, frame, scales))
    });
    let (edges, rest_frame, scales) = match found {
        Ok(found) => found,
        Err(e) => return soft(e, digest).map(|()| 0),
    };
    let scales = scales.as_slice();
    let rest_scale = |k: usize| scales.get(k).copied().flatten();
    let pose = body.human().pose.as_slice();
    let rotation = |k: usize| pose.get(k).map_or([0., 0., 0., 1.], |x| x.q.map(f64::from));
    let found = body
        .hips_points(&rotation, &rest_scale, meter)
        .and_then(|local| Ok((local, body.hips_scale(&rest_scale)?)));
    let (local, hips_scale) = match found {
        Ok(found) => found,
        Err(e) => return soft(e, digest).map(|()| 0),
    };
    digest.tag(local.map(|p| p.map(|p| p.map(f64::to_bits))));
    let mut solved = 0;
    for (target, request, position, rotation, scale) in SOLVES {
        meter.count_decode(1)?;
        let parent = body_solve::HipsParent {
            position,
            rotation,
            scale,
        };
        // `m_Scale` applied once, as root projection applies it.
        let human_scale = f64::from(body.scale());
        let pose = body_solve::AnimatorBodyPose {
            translation: target.map(|t| human_scale * t),
            rotation: request,
        };
        let found = body_solve::solve_hips(
            pose,
            &local,
            &weights,
            &edges,
            &rest_frame,
            &parent,
            hips_scale,
        );
        match found {
            Ok((t, q)) => {
                let length = q.iter().map(|v| v * v).sum::<f64>().sqrt();
                assert!(
                    t.iter().all(|v| v.is_finite()) && (length - 1.).abs() <= 1e-12,
                    "a solve off its contract: t {t:?}, q {q:?}"
                );
                digest.tag((t.map(f64::to_bits), q.map(f64::to_bits)));
                solved += 1;
            }
            Err(e) => soft(e, digest)?,
        }
    }
    Ok(solved)
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

/// Each discovered rig's Avatar, its rest body's hips solves, then the
/// refusal of its own clips and, with flag 2, of every kept clip. The
/// number of hips solves that succeeded.
pub(crate) fn rigs(
    env: &Environment,
    graph: &Graph,
    kept: &[Clip],
    meter: &Arc<Meter>,
    digest: &mut Digest,
) -> Result<usize, AnimError> {
    let discovery = discover(env, graph, meter)?;
    let mut solved = 0;
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
        if let Some(found) = &avatar.avatar {
            match body_avatar(found, meter) {
                Ok(body) => {
                    solved += solve_digest(&body, &paths, meter, digest)?;
                    chain_digest(&body, &paths, kept, meter, digest)?;
                }
                Err(e) => soft(e, digest)?,
            }
        }
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
    Ok(solved)
}

/// The rig's Animator chain (`humanoid::chain`), held to a finite parent
/// with a positive scale (the identity for a recorded rig with no node
/// between), the recorded rig it names, then its admission with each kept
/// clip's admitted root settings.
fn chain_digest(
    body: &BodyAvatar<'_>,
    paths: &RigPaths<'_>,
    kept: &[Clip],
    meter: &Arc<Meter>,
    digest: &mut Digest,
) -> Result<(), AnimError> {
    let found = match body.animator_chain(paths, meter) {
        Ok(found) => found,
        Err(e) => return soft(e, digest),
    };
    let parent = found.parent();
    assert!(
        parent
            .position
            .iter()
            .chain(&parent.rotation)
            .all(|v| v.is_finite())
            && parent.scale > 0.
            && found.hips_scale() > 0.,
        "a finite chain"
    );
    let recorded = found.recorded();
    if recorded.is_some_and(|(rig, _)| rig.between.is_none()) {
        assert_eq!(parent, body_solve::HipsParent::IDENTITY, "a plain chain");
    }
    digest.tag((
        recorded.map(|(rig, offset)| (rig.rig, offset)),
        parent.position.map(f64::to_bits),
        parent.rotation.map(f64::to_bits),
        parent.scale.to_bits(),
        found.hips_scale().to_bits(),
    ));
    for clip in kept {
        meter.count_decode(1)?;
        match root::root_settings(clip, meter) {
            Ok(settings) => {
                digest.tag(chain::admit_chain(&found, &settings).map_err(|e| e.to_string()));
            }
            Err(e) => soft(e, digest)?,
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
