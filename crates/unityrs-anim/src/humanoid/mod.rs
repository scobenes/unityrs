//! Runtime Avatar tables, the humanoid clip slot table and the ordered
//! qualification refusals of `docs/plans/anim.md` section 2.4.
//!
//! The plan orders the refusals only partly: a missing Avatar or an
//! unreconstructable hierarchy comes before any slot is interpreted, and
//! the rest are independent causes. `humanoid_refusal` returns them in one
//! fixed order consistent with that; the qualification evidence lists
//! every cause a recorded clip has, and Rust's must be among them.
//!
//! A clip that passes every check still refuses as
//! `unsupported_humanoid_playback`: positive playback is deferred. Whether
//! such a clip's hips or root need a correction is measured on Editor
//! recordings, so only the qualification evidence reports it
//! (`unsupported_body_motion(hips_correction)` or `(root_projection)`).
//!
//! `body_avatar` validates the Avatar inputs of the humanoid body model
//! (`docs/plans/humanoid-body.md`); no refusal above calls it yet.
mod avatar;
mod body;
mod clip;

pub use avatar::{
    Avatar, Axes, BODY_BONES, HAND_BONES, Human, Skeleton, SkeletonNode, Twist, Xform, read_avatar,
};
pub use body::{
    BODY_SLOTS, BodyAvatar, RECORDED_ABSENT, body_avatar, mass_sum_bound, slot_bone, topology_scope,
};
pub use clip::{
    ABSENT_SLOT, ClipSetting, DOF_COUNT, DofKind, HumanoidClip, HumanoidSlot, MUSCLE_COUNT,
    ROOT_BASIS, SettingValue, TransformWriter,
};
pub(crate) use clip::{is_humanoid, read as read_humanoid_clip};

use crate::{AnimError, BodyMotion, Clip, PathQuery, RigPaths, read::Dict};
use std::sync::Arc;
use unityrs_core::{ObjectRef, env::Environment, limits::Meter, value::Value};
use unityrs_scene::Trs;

/// `HumanBodyBones` indices of the bones `HumanTrait` requires.
pub const REQUIRED_BONES: [usize; 15] = [0, 1, 2, 3, 4, 5, 6, 7, 10, 13, 14, 15, 16, 17, 18];

/// The `HumanBodyBones` index each `HumanTrait` muscle rotates, from the
/// public `HumanTrait.BoneFromMuscle` and `MuscleName` order.
#[must_use]
pub const fn muscle_bone(muscle: usize) -> Option<usize> {
    Some(match muscle {
        0..=2 => 7,
        3..=5 => 8,
        6..=8 => 54,
        9..=11 => 9,
        12..=14 => 10,
        15 | 16 => 21,
        17 | 18 => 22,
        19 | 20 => 23,
        21..=23 => 1,
        24 | 25 => 3,
        26 | 27 => 5,
        28 => 19,
        29..=31 => 2,
        32 | 33 => 4,
        34 | 35 => 6,
        36 => 20,
        37 | 38 => 11,
        39..=41 => 13,
        42 | 43 => 15,
        44 | 45 => 17,
        46 | 47 => 12,
        48..=50 => 14,
        51 | 52 => 16,
        53 | 54 => 18,
        55..MUSCLE_COUNT => {
            // Per hand and finger: 1 Stretched and Spread on the proximal
            // bone, then 2 and 3 Stretched on the intermediate and distal.
            let finger = muscle - 55;
            let step = match finger % 4 {
                0 | 1 => 0,
                2 => 1,
                _ => 2,
            };
            24 + finger / 4 * 3 + step
        }
        _ => return None,
    })
}

impl Human {
    /// The human skeleton node of a `HumanBodyBones` index, if mapped.
    #[must_use]
    pub fn bone_node(&self, bone: usize) -> Option<usize> {
        let raw = match bone {
            0..=8 => self.bones.as_slice().get(bone),
            54 => self.bones.as_slice().get(9),
            9..=23 => self.bones.as_slice().get(bone + 1),
            24..=38 => self.left_hand.as_slice().get(bone - 24),
            39..=53 => self.right_hand.as_slice().get(bone - 39),
            _ => None,
        };
        usize::try_from(*raw?).ok()
    }
}

/// The Avatar and hierarchy facts of one Animator.
#[derive(Debug)]
pub struct RigAvatar {
    pub avatar: Option<Avatar>,
    /// `m_HasTransformHierarchy`; an optimized or unknown hierarchy is false.
    pub transform_hierarchy: bool,
}

/// Follow an Animator's `m_Avatar` from its owning file. Other components,
/// such as legacy `Animation`, have no Avatar.
pub fn rig_avatar(
    env: &Environment,
    component: &ObjectRef,
    meter: &Arc<Meter>,
) -> Result<RigAvatar, AnimError> {
    if component.class_id() != 95 {
        return Ok(RigAvatar {
            avatar: None,
            transform_hierarchy: false,
        });
    }
    meter.count_decode(u64::from(component.info().byte_size))?;
    let parsed = component.parse_as_dict(false)?;
    let dict = Dict {
        parsed: &parsed,
        meter,
    };
    let transform_hierarchy = matches!(
        dict.root("m_HasTransformHierarchy")?,
        Some(Value::Bool(true))
    );
    let pointer = match dict.root("m_Avatar")? {
        Some(value) => dict.pointer(value)?,
        None => return Err(AnimError::Malformed("m_Avatar")),
    };
    if !pointer.is_set() {
        return Ok(RigAvatar {
            avatar: None,
            transform_hierarchy,
        });
    }
    let (object, _) = env.deref_held(component.file(), pointer, meter)?;
    Ok(RigAvatar {
        avatar: Some(read_avatar(&object, meter)?),
        transform_hierarchy,
    })
}

/// The Avatar twist and stretch values of the fixtures' neutral twin rig.
/// The qualification evidence records the twin's values and the recorded
/// test holds these to them; Unity documents no identity value.
pub const NEUTRAL_TWIST: f32 = 0.;
pub const NEUTRAL_STRETCH: f32 = 0.;

/// The `HumanBodyBones` each twist setting redistributes over: upper arm
/// twist between shoulder and elbow, lower arm twist between elbow and
/// wrist, and the same for the legs.
/// [Unity HumanDescription](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/HumanDescription.html)
pub const TWIST_BONES: [(&str, [usize; 4]); 4] = [
    ("m_ArmTwist", [13, 14, 15, 16]),
    ("m_ForeArmTwist", [15, 16, 17, 18]),
    ("m_UpperLegTwist", [1, 2, 3, 4]),
    ("m_LegTwist", [3, 4, 5, 6]),
];

/// The limbs each stretch setting lengthens: the arms, the legs, and the
/// feet's spacing on the legs.
pub const STRETCH_BONES: [(&str, [usize; 6]); 3] = [
    ("m_ArmStretch", [13, 14, 15, 16, 17, 18]),
    ("m_LegStretch", [1, 2, 3, 4, 5, 6]),
    ("m_FeetSpacing", [1, 2, 3, 4, 5, 6]),
];

/// The typed refusal for a humanoid clip on one rig, in the order the
/// module documents: Avatar and hierarchy, required bones, Animator root
/// and generic writers, other pose features, body slots, muscle bones and
/// axes, twist and stretch on the animated limbs, clip settings, root
/// basis, then the body offset. A read or resource failure is returned in
/// place of the refusal.
pub fn humanoid_refusal(
    clip: &Clip,
    rig: &RigAvatar,
    paths: &RigPaths<'_>,
    meter: &Arc<Meter>,
) -> AnimError {
    match refusal(clip, rig, paths, meter) {
        Ok(refusal) | Err(refusal) => refusal,
    }
}

/// The mapped muscles whose optional bone the Avatar leaves out. Playback
/// drops them; the evidence counts them.
#[must_use]
pub fn dropped_muscles(clip: &HumanoidClip, human: &Human) -> usize {
    clip.mapped(DofKind::Muscle)
        .filter(|slot| {
            muscle_bone(slot.ordinal).is_some_and(|bone| {
                !REQUIRED_BONES.contains(&bone) && human.bone_node(bone).is_none()
            })
        })
        .count()
}

#[allow(clippy::too_many_lines, reason = "one ordered list of refusals")]
fn refusal(
    clip: &Clip,
    rig: &RigAvatar,
    paths: &RigPaths<'_>,
    meter: &Arc<Meter>,
) -> Result<AnimError, AnimError> {
    let Some(slots) = clip.humanoid() else {
        return Ok(clip
            .issue()
            .cloned()
            .unwrap_or(AnimError::UnsupportedHumanoidPlayback));
    };
    let Some(avatar) = &rig.avatar else {
        return Ok(AnimError::MissingAvatar);
    };
    if !rig.transform_hierarchy {
        return Ok(AnimError::UnsupportedHumanoidHierarchy(0));
    }
    if let Some(node) = hierarchy(avatar, paths, meter)? {
        return Ok(AnimError::UnsupportedHumanoidHierarchy(node));
    }
    let human = &avatar.human;
    meter.count_decode(REQUIRED_BONES.len() as u64)?;
    if let Some(&bone) = REQUIRED_BONES
        .iter()
        .find(|&&bone| human.bone_node(bone).is_none())
    {
        return Ok(AnimError::MissingHumanBone(bone));
    }
    if !identity_root(paths, meter)? {
        return Ok(AnimError::UnsupportedAnimatorRoot);
    }
    let ids = human.skeleton.ids.as_slice();
    meter
        .count_decode((slots.transform_writers().len() as u64).saturating_mul(ids.len() as u64))?;
    if let Some(writer) = slots
        .transform_writers()
        .iter()
        .find(|writer| writer.path != 0 && ids.contains(&writer.path))
    {
        return Ok(AnimError::ConflictingHumanoidWriter(writer.binding));
    }
    meter.count_decode(DOF_COUNT as u64 * 5)?;
    if slots.mapped(DofKind::Motion).next().is_some()
        || avatar.root_motion_bone >= 0
        || slots.has_generic_root()
    {
        return Ok(AnimError::UnsupportedLocomotion);
    }
    if slots.mapped(DofKind::Goal).next().is_some() {
        return Ok(AnimError::UnsupportedHumanoidIk);
    }
    if slots.mapped(DofKind::Translation).next().is_some() || human.has_translation_dof {
        return Ok(AnimError::UnsupportedTranslationDof);
    }
    if slots.has_reference_pose() {
        return Ok(AnimError::UnsupportedAdditivePose);
    }
    if slots.mapped(DofKind::Body).next().is_some() {
        return Ok(AnimError::UnsupportedBodyMotion(BodyMotion::BodySlots));
    }
    // The animated bones, without the muscles of absent optional bones.
    let mut animated = [false; 55];
    for slot in slots.mapped(DofKind::Muscle) {
        let bone = muscle_bone(slot.ordinal).ok_or(AnimError::UnknownHumanDof(slot.dof))?;
        let Some(node) = human.bone_node(bone) else {
            if REQUIRED_BONES.contains(&bone) {
                return Ok(AnimError::MissingHumanBone(bone));
            }
            continue;
        };
        let has_axes = human
            .skeleton
            .nodes
            .as_slice()
            .get(node)
            .is_some_and(|node| node.axes >= 0);
        if !has_axes {
            return Ok(AnimError::UnsupportedHumanoidAxes(bone));
        }
        if let Some(flag) = animated.get_mut(bone) {
            *flag = true;
        }
    }
    let moves = |bones: &[usize]| bones.iter().any(|&bone| animated.get(bone) == Some(&true));
    for ((field, value), (_, bones)) in human.twist.fields().into_iter().zip(TWIST_BONES) {
        if value != NEUTRAL_TWIST && moves(&bones) {
            return Ok(AnimError::UnsupportedHumanoidTwist(field));
        }
    }
    for ((field, bones), value) in
        STRETCH_BONES
            .into_iter()
            .zip([human.arm_stretch, human.leg_stretch, human.feet_spacing])
    {
        if value != NEUTRAL_STRETCH && moves(&bones) {
            return Ok(AnimError::UnsupportedHumanoidAvatarSetting(field));
        }
    }
    if let Some(setting) = slots
        .settings()
        .iter()
        .find(|s| !s.is_root_basis() && !s.neutral())
    {
        return Ok(AnimError::UnsupportedHumanoidClipSettings(setting.field));
    }
    if let Some(field) = slots.root_basis_refusal() {
        return Ok(AnimError::UnsupportedHumanoidRootBasis(field));
    }
    Ok(AnimError::UnsupportedHumanoidPlayback)
}

/// Whether the Animator node and its ancestors all have identity local
/// TRS, the only root configuration the fixtures verify.
fn identity_root(paths: &RigPaths<'_>, meter: &Arc<Meter>) -> Result<bool, AnimError> {
    let (graph, mut node) = paths.graph();
    loop {
        meter.count_decode(1)?;
        let Some(row) = graph.nodes().get(node) else {
            return Err(AnimError::MissingPath);
        };
        let Trs {
            position,
            rotation,
            scale,
        } = row.local;
        // Exact: one is the only identity value, either sign of w.
        let one = |v: f32| v.abs().to_bits() == 1f32.to_bits();
        let [x, y, z, w] = rotation;
        let unit = x == 0. && y == 0. && z == 0. && one(w);
        let unscaled = scale.iter().all(|&v| v > 0. && one(v));
        if !position.iter().all(|&v| v == 0.) || !unit || !unscaled {
            return Ok(false);
        }
        match row.parent {
            Some(parent) => node = parent,
            None => return Ok(true),
        }
    }
}

/// The first human skeleton node whose `m_TOS` path is missing, does not
/// extend its parent's path, or names no node below the Animator.
fn hierarchy(
    avatar: &Avatar,
    paths: &RigPaths<'_>,
    meter: &Arc<Meter>,
) -> Result<Option<usize>, AnimError> {
    let skeleton = &avatar.human.skeleton;
    for (i, (node, &id)) in skeleton
        .nodes
        .as_slice()
        .iter()
        .zip(skeleton.ids.as_slice())
        .enumerate()
    {
        let Some(path) = avatar.tos_path(id, meter)? else {
            return Ok(Some(i));
        };
        let path = path.as_bytes();
        if let Ok(parent) = usize::try_from(node.parent) {
            let parent = skeleton
                .ids
                .as_slice()
                .get(parent)
                .ok_or(AnimError::InvalidAvatarIndex("m_Human.m_Skeleton"))?;
            let Some(prefix) = avatar.tos_path(*parent, meter)? else {
                return Ok(Some(i));
            };
            let prefix = prefix.as_bytes();
            let extends = if prefix.is_empty() {
                !path.is_empty() && !path.contains(&b'/')
            } else {
                path.strip_prefix(prefix)
                    .and_then(|rest| rest.strip_prefix(b"/"))
                    .is_some_and(|name| !name.is_empty() && !name.contains(&b'/'))
            };
            if !extends {
                return Ok(Some(i));
            }
        } else if !path.is_empty() {
            return Ok(Some(i));
        }
        match paths.resolve(PathQuery::HashWithEvidence(id, path), meter) {
            Ok(_) => {}
            Err(error) if error.resource_refusal() => return Err(error),
            Err(_) => return Ok(Some(i)),
        }
    }
    Ok(None)
}

const _: () = crate::send_sync::<RigAvatar>();
