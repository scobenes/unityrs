//! The Avatar inputs of the humanoid body model
//! (`docs/plans/humanoid-body.md`): the per-slot masses of 2.1 (decision 1),
//! the slot-to-node mapping and the rest body that 2.1 to 2.3 read, checked
//! as far as E0 needs no model coefficient, and the frozen topology scope
//! of 2.1 and section 8.
//!
//! - **E0a.** `m_HumanBoneMass` and `m_HumanBoneIndex` hold one entry per
//!   slot of the 25-slot order (`HumanBodyBones` 0-8, `UpperChest`, 9-23).
//!   The reader already holds each index in range and no two slots on one
//!   node; here each mapped slot's node must also have an `m_TOS` path
//!   (`m_Skeleton.m_ID` through `m_TOS`), and no two mapped slots may
//!   resolve to one path. A swap of two valid slots leaves every path
//!   distinct, so only a capture of the rig's bones can show it: that part
//!   of E0a is the reference checker's comparison with the recordings
//!   (`tools/parity/anim_body_check.py`), a verification of the plan's
//!   reading of the Avatar, not a runtime gate.
//! - **E0b**, the normalization: every mass is finite and non-negative, an
//!   absent slot's is exactly zero, and the masses sum to one within
//!   `mass_sum_bound`. Each stored mass is the E0b fold of `HumanTrait`'s
//!   default masses over their sum, within `gamma(50)` at f32 of that fold
//!   (Unity's sums of up to 25 terms, the division and the store; the
//!   checker's bound), and the folds sum to one, so the stored masses sum
//!   to one within `gamma(50)`; their f64 sum here errs by at most
//!   `gamma(25)` at f64 of it. The model then takes the masses as
//!   serialized (decision 1). Each slot's comparison with the fold of the
//!   Editor's `HumanTrait` defaults is likewise the reference checker's,
//!   on the recorder's table: it verifies the plan's understanding of the
//!   field, is no runtime refusal, and keeps Unity's default table out of
//!   this crate.
//! - **Scope.** An Avatar giving a zero-mass leaf (`LEAVES`: the Eyes and
//!   the Jaw) any mass refuses, and so does one with Chest or Neck absent,
//!   or exactly one Shoulder mapped, or any pattern of absent body slots
//!   but the leaves that no in-scope training rig recorded
//!   (`RECORDED_ABSENT`, the reference's `anim_body_model.RECORDED_ABSENT`
//!   and freeze v8's topology scope). Every recorded Avatar gives the
//!   leaves zero mass, so absent they fold nothing into the Head and mapped
//!   their edges' fitted fractions vanish (plan 2.5): they take no part in
//!   the pattern only at zero mass. The check reads `m_HumanBoneIndex` and
//!   the leaves' masses, before any forward kinematics.
//! - **E0c**, its model-free part: `m_Scale` is positive and is
//!   `m_RootX.t.y` within two f32 roundings.
//!
//! The centre of mass, the body frame and the closed-form hips solve, with
//! the free-combination and shoulder-hip twist refusals, come in PR B3b on
//! these inputs, with the coefficients, limits and scope of freeze v8
//! (`tools/parity/anim_body_freeze_v8.json`), which E3b validated.
use super::avatar::{Avatar, BODY_BONES, Human, Xform};
use crate::{AnimError, BodyAvatarFault, BodyModel};
use unityrs_core::{RawStr, limits::Meter};

/// The 25 body slots of `m_HumanBoneIndex` and `m_HumanBoneMass`, in order.
pub const BODY_SLOTS: [&str; BODY_BONES] = [
    "Hips",
    "LeftUpperLeg",
    "RightUpperLeg",
    "LeftLowerLeg",
    "RightLowerLeg",
    "LeftFoot",
    "RightFoot",
    "Spine",
    "Chest",
    "UpperChest",
    "Neck",
    "Head",
    "LeftShoulder",
    "RightShoulder",
    "LeftUpperArm",
    "RightUpperArm",
    "LeftLowerArm",
    "RightLowerArm",
    "LeftHand",
    "RightHand",
    "LeftToes",
    "RightToes",
    "LeftEye",
    "RightEye",
    "Jaw",
];

const CHEST: usize = 8;
const UPPER_CHEST: usize = 9;
const NECK: usize = 10;
const LEFT_SHOULDER: usize = 12;
const RIGHT_SHOULDER: usize = 13;
const LEFT_TOES: usize = 20;
const RIGHT_TOES: usize = 21;
const LEFT_EYE: usize = 22;
const RIGHT_EYE: usize = 23;
const JAW: usize = 24;

/// The `HumanBodyBones` index of a body slot.
#[must_use]
pub const fn slot_bone(slot: usize) -> usize {
    match slot {
        0..=8 => slot,
        9 => 54,
        _ => slot - 1,
    }
}

const fn mask(mut slots: &[usize]) -> u32 {
    let mut out = 0;
    while let [slot, rest @ ..] = slots {
        out |= 1 << *slot;
        slots = rest;
    }
    out
}

/// The zero-mass leaves, as a slot bit mask: the Eyes and the Jaw.
pub const LEAVES: u32 = mask(&[LEFT_EYE, RIGHT_EYE, JAW]);

/// The absent body slots but the leaves of each in-scope training rig, as
/// slot bit masks: the only admitted patterns.
pub const RECORDED_ABSENT: [u32; 9] = [
    mask(&[]),
    mask(&[UPPER_CHEST]),
    mask(&[LEFT_TOES]),
    mask(&[RIGHT_TOES]),
    mask(&[LEFT_TOES, RIGHT_TOES]),
    mask(&[LEFT_SHOULDER, RIGHT_SHOULDER]),
    mask(&[UPPER_CHEST, LEFT_TOES, RIGHT_TOES]),
    mask(&[UPPER_CHEST, LEFT_TOES]),
    mask(&[UPPER_CHEST, RIGHT_TOES]),
];

const U32: f64 = 1. / 16_777_216.;
const U64: f64 = 1. / 9_007_199_254_740_992.;

/// Higham's `gamma_n = n u / (1 - n u)`, for `n u < 1`.
const fn gamma(n: f64, u: f64) -> f64 {
    n * u / (1. - n * u)
}

/// The bound on `|sum - 1|` of the stored masses' f64 sum (module doc):
/// `gamma(50)` at f32 plus `gamma(25)` at f64 of the sum.
#[must_use]
pub fn mass_sum_bound(sum: f64) -> f64 {
    gamma(50., U32) + gamma(25., U64) * sum
}

/// The topology refusal for an Avatar's mapped body slots, from
/// `m_HumanBoneIndex` and the masses: a leaf's mass, then Chest, then Neck,
/// then one Shoulder, then the recorded patterns, as the reference's
/// `topology_scope` orders them.
pub fn topology_scope(
    human: &Human,
    masses: &[f32; BODY_BONES],
    meter: &Meter,
) -> Result<(), AnimError> {
    meter.count_decode(BODY_BONES as u64)?;
    let mut absent = 0_u32;
    for slot in 0..BODY_BONES {
        let mapped = human
            .bones
            .as_slice()
            .get(slot)
            .is_some_and(|&node| node >= 0);
        if !mapped {
            absent |= 1 << slot;
        }
    }
    let has = |slot: usize| absent & (1 << slot) == 0;
    let heavy = masses
        .iter()
        .enumerate()
        .any(|(slot, &mass)| LEAVES & (1 << slot) != 0 && mass != 0.);
    let cause = if heavy {
        BodyModel::LeafMass
    } else if !has(CHEST) {
        BodyModel::ChestAbsent
    } else if !has(NECK) {
        BodyModel::NeckAbsent
    } else if has(LEFT_SHOULDER) != has(RIGHT_SHOULDER) {
        BodyModel::OneShoulder
    } else if !RECORDED_ABSENT.contains(&(absent & !LEAVES)) {
        BodyModel::UnrecordedTopology
    } else {
        return Ok(());
    };
    Err(AnimError::UnsupportedHumanoidBodyModel(cause))
}

/// One Avatar's validated body-model inputs.
#[derive(Debug)]
pub struct BodyAvatar<'a> {
    pub avatar: &'a Avatar,
    /// The human skeleton node of each body slot, `None` when unmapped.
    pub nodes: [Option<usize>; BODY_BONES],
    /// `m_HumanBoneMass`, exactly as stored.
    pub masses: [f32; BODY_BONES],
}

impl BodyAvatar<'_> {
    /// `m_Human`: `m_SkeletonPose` and the human skeleton's parents.
    #[must_use]
    pub const fn human(&self) -> &Human {
        &self.avatar.human
    }

    /// `m_RootX`, the rest body pose in the Animator node's frame.
    #[must_use]
    pub const fn root(&self) -> Xform {
        self.avatar.human.root
    }

    /// `m_Scale`, the human scale.
    #[must_use]
    pub const fn scale(&self) -> f32 {
        self.avatar.human.scale
    }

    /// A mapped slot's rest local transform in `m_SkeletonPose`.
    #[must_use]
    pub fn rest(&self, slot: usize) -> Option<&Xform> {
        let node = (*self.nodes.get(slot)?)?;
        self.avatar.human.pose.as_slice().get(node)
    }

    /// A mapped slot's `m_TOS` path below the Animator node.
    pub fn slot_path(&self, slot: usize, meter: &Meter) -> Result<Option<&RawStr>, AnimError> {
        let Some(node) = self.nodes.get(slot).copied().flatten() else {
            return Ok(None);
        };
        match self.avatar.human.skeleton.ids.as_slice().get(node) {
            Some(&id) => self.avatar.tos_path(id, meter),
            None => Ok(None),
        }
    }
}

/// Validate an Avatar's body-model inputs in the module's order: E0a, the
/// masses (E0b), the topology scope, then `m_Scale` (E0c). Each failure is
/// a typed refusal; nothing here panics on malformed tables.
pub fn body_avatar<'a>(avatar: &'a Avatar, meter: &Meter) -> Result<BodyAvatar<'a>, AnimError> {
    let human = &avatar.human;
    let fault = |fault| Err(AnimError::InvalidBodyAvatar(fault));
    // E0a: the slot table, its nodes' paths and their skeleton IDs.
    if human.bones.len() != BODY_BONES {
        return Err(AnimError::InvalidAvatarIndex("m_HumanBoneIndex"));
    }
    let mut nodes = [None; BODY_BONES];
    let mut paths: [Option<&RawStr>; BODY_BONES] = [None; BODY_BONES];
    let ids = human.skeleton.ids.as_slice();
    meter.count_decode(BODY_BONES as u64 * BODY_BONES as u64)?;
    for (slot, &node) in human.bones.as_slice().iter().enumerate() {
        let Ok(node) = usize::try_from(node) else {
            continue;
        };
        let Some(&id) = ids.get(node) else {
            return Err(AnimError::InvalidAvatarIndex("m_HumanBoneIndex"));
        };
        let Some(path) = avatar.tos_path(id, meter)? else {
            return fault(BodyAvatarFault::SlotPath(slot));
        };
        let aliased = paths
            .iter()
            .take(slot)
            .flatten()
            .any(|earlier: &&RawStr| earlier.as_bytes() == path.as_bytes());
        if aliased {
            return fault(BodyAvatarFault::SlotAlias(slot));
        }
        if let (Some(entry), Some(seen)) = (nodes.get_mut(slot), paths.get_mut(slot)) {
            *entry = Some(node);
            *seen = Some(path);
        }
    }
    // E0b: one finite, non-negative mass per slot, zero where absent,
    // summing to one.
    let Some(stored) = &human.bone_mass else {
        return fault(BodyAvatarFault::MassMissing);
    };
    let stored = stored.as_slice();
    meter.count_decode(stored.len() as u64)?;
    let Ok(masses) = <[f32; BODY_BONES]>::try_from(stored) else {
        return fault(BodyAvatarFault::MassLength(stored.len()));
    };
    let mut sum = 0_f64;
    for (slot, (&mass, node)) in masses.iter().zip(&nodes).enumerate() {
        if !mass.is_finite() {
            return fault(BodyAvatarFault::MassNonFinite(slot));
        }
        if mass < 0. {
            return fault(BodyAvatarFault::MassNegative(slot));
        }
        if node.is_none() && mass != 0. {
            return fault(BodyAvatarFault::MassAbsentSlot(slot));
        }
        sum += f64::from(mass);
    }
    let off = (sum - 1.).abs();
    if !(off.is_finite() && off <= mass_sum_bound(sum)) {
        return fault(BodyAvatarFault::MassSum);
    }
    topology_scope(human, &masses, meter)?;
    // E0c without the model: m_Scale against the rest body height. The
    // reader holds both finite; a caller's edit may not, so NaN refuses.
    let scale = f64::from(human.scale);
    if !(scale.is_finite() && scale > 0.) {
        return fault(BodyAvatarFault::Scale);
    }
    let off = (scale - f64::from(human.root.t[1])).abs();
    if !(off.is_finite() && off <= 2. * U32 * scale) {
        return fault(BodyAvatarFault::RootHeight);
    }
    Ok(BodyAvatar {
        avatar,
        nodes,
        masses,
    })
}

const _: () = crate::send_sync::<BodyAvatar<'static>>();
