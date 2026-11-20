//! Freeze v8's chain from the Animator node to the hips (plan 2.3, B4b):
//! the hips parent's transform `(t_p, R_p, sigma_p)` in the Animator
//! node's frame `A`, composed from the rig's own Transforms as the
//! reference's `tracked` and `link` compose the recorded ones
//! (`tools/parity/anim_body_tolerance.py`): each node below the Animator
//! node, from the identity, is `p + s_p R_p t` (Unity's quaternion-vector
//! product), with rotation `R_p q` and uniform scale `s_p s`. The Animator
//! node's own transform never enters `A`.
//!
//! `animator_chain` composes the chain for any depth. A nonuniform or
//! nonpositive scale on it refuses (`unsupported_animator_root`), as the
//! reference's `uniform` does, and so does a rotation more than
//! `UNIT_TOLERANCE` from unit length; a hips Transform outside the
//! Animator node's subtree refuses (`detached_bone`), and a non-finite
//! link `non_finite_body`. It then names the recorded rig the chain is, if
//! any: one of `RECORDED_RIGS`, the rigs freeze v8's E4 passed (the
//! compact rig and F5's four), each as its fixture built it
//! (`tools/fixtures/unity/Editor/UnityrsBodyFixtures.cs`). That needs the
//! Animator node's scale, the node between it and the hips (translation,
//! rotation and scale) and the hips' rest translation, rotation and scale
//! within `RECORDED_TOLERANCE` of the fixture's
//! (`anim_fixture_body.check_chain`'s `near`), every ancestor of the
//! Animator node at the identity, the Animator node at the identity or,
//! where E4 played the rig there, at `OFFSET_PLACEMENT`, and the Avatar's
//! rest skeleton (`m_SkeletonPose`) the fixture's: the compact rig's
//! bones (`COMPACT_LAYOUT`, `anim_fixture_rigs.rig_layout`) below the
//! hips, scaled by the rig's size and the chain's scales and turned by its
//! rotations, each within `LAYOUT_TOLERANCE`. That skeleton is the rig
//! size `m_Scale` follows, so an unrecorded human scale is an unrecorded
//! rig.
//!
//! `admit_chain` pairs the recorded rig with the clip's admitted root
//! settings as E4 did: the compact rig with every admitted variant, F5's
//! only with `CHAIN_VARIANT`. An unrecorded rig and every other pairing
//! refuse (`unsupported_animator_root`) until B5's predicate.
use super::{
    body::{BODY_SLOTS, BodyAvatar},
    body_model::{HIPS, Point, Points, Quaternion, multiply, refuse, uniform, unity_rotate, zip3},
    body_solve::HipsParent,
    root::{RootSettings, U32},
};
use crate::{AnimError, BodyModel, PathQuery, RigPaths};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_scene::Trs;

/// The F5 clip variant, `anim_fixture_body.CHAIN_VARIANT`.
pub const CHAIN_VARIANT: &str = "keep-o1-y1-xz1-motion";

/// How far a Transform's value may be from its fixture value:
/// `anim_fixture_body.check_chain`'s `near`.
pub const RECORDED_TOLERANCE: f64 = 1e-6;

/// How far a rest bone may be from its fixture position, in metres:
/// `anim_fixture_body.check_chain`'s tolerance on the `large` rig's hips.
pub const LAYOUT_TOLERANCE: f64 = 1e-5;

/// How far a chain rotation's length may be from one: an f32 unit
/// quaternion as Unity normalizes it in f32 (four squares and three sums,
/// a square root, a division) and stores it, eight f32 roundings at most,
/// `gamma_8`.
pub const UNIT_TOLERANCE: f64 = 8. * U32 / (1. - 8. * U32);

/// The compact rig's bones in `rig_layout`'s order: each with its parent
/// and its offset from it, in metres.
pub const COMPACT_LAYOUT: [(&str, &str, [f64; 3]); 22] = [
    ("Hips", "", [0., 0.88, 0.]),
    ("Spine", "Hips", [0., 0.16, 0.]),
    ("Chest", "Spine", [0., 0.13, 0.]),
    ("UpperChest", "Chest", [0., 0.08, 0.]),
    ("Neck", "UpperChest", [0., 0.14, 0.]),
    ("Head", "Neck", [0., 0.12, 0.]),
    ("LeftUpperLeg", "Hips", [-0.13, -0.05, 0.]),
    ("LeftLowerLeg", "LeftUpperLeg", [0., -0.36, 0.04]),
    ("LeftFoot", "LeftLowerLeg", [0., -0.37, -0.04]),
    ("LeftToes", "LeftFoot", [0., -0.04, 0.14]),
    ("LeftShoulder", "UpperChest", [-0.08, 0.08, 0.]),
    ("LeftUpperArm", "LeftShoulder", [-0.14, 0., 0.]),
    ("LeftLowerArm", "LeftUpperArm", [-0.26, 0., 0.]),
    ("LeftHand", "LeftLowerArm", [-0.23, 0., 0.]),
    ("RightUpperLeg", "Hips", [0.13, -0.05, 0.]),
    ("RightLowerLeg", "RightUpperLeg", [0., -0.36, 0.04]),
    ("RightFoot", "RightLowerLeg", [0., -0.37, -0.04]),
    ("RightToes", "RightFoot", [0., -0.04, 0.14]),
    ("RightShoulder", "UpperChest", [0.08, 0.08, 0.]),
    ("RightUpperArm", "RightShoulder", [0.14, 0., 0.]),
    ("RightLowerArm", "RightUpperArm", [0.26, 0., 0.]),
    ("RightHand", "RightLowerArm", [0.23, 0., 0.]),
];

/// A recorded node's local transform: translation, rotation, uniform scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecordedNode {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: f32,
}

/// One rig freeze v8's E4 passed, as its fixture built it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecordedRig {
    /// The rig's name in E4's configurations.
    pub rig: &'static str,
    /// The fixture's `BodyRig.Size`: the compact layout's scale.
    pub size: f32,
    /// The Animator node's uniform scale.
    pub animator_scale: f32,
    /// The node between the Animator node and the hips, if any.
    pub between: Option<RecordedNode>,
    /// The hips' rest local transform.
    pub hips: RecordedNode,
    /// Whether E4 also passed the rig at `OFFSET_PLACEMENT`.
    pub offset: bool,
    /// Whether E4 passed every admitted variant on the rig, or only
    /// `CHAIN_VARIANT`.
    pub every_variant: bool,
}

const IDENTITY: [f32; 4] = [0., 0., 0., 1.];
/// `sin` and `cos` of 45 degrees in f32: `Quaternion.Euler(+-90, 0, 0)`.
const HALF: f32 = 0.707_106_77;
const COMPACT_HIPS: RecordedNode = RecordedNode {
    position: [0., 0.88, 0.],
    rotation: IDENTITY,
    scale: 1.,
};

/// The Animator node's offset placement E4 played the compact rig at:
/// `(2, .25, -1)` and `Quaternion.Euler(0, 35, 0)`.
pub const OFFSET_PLACEMENT: ([f32; 3], [f32; 4]) =
    ([2., 0.25, -1.], [0., 0.300_705_8, 0., 0.953_716_9]);

/// The rigs E4 passed; a test binds them to freeze v8's `e4.passing` and
/// the fixtures' sources.
pub const RECORDED_RIGS: [RecordedRig; 5] = [
    RecordedRig {
        rig: "compact",
        size: 1.,
        animator_scale: 1.,
        between: None,
        hips: COMPACT_HIPS,
        offset: true,
        every_variant: true,
    },
    RecordedRig {
        rig: "scaled-animator",
        size: 1.,
        animator_scale: 1.25,
        between: None,
        hips: COMPACT_HIPS,
        offset: false,
        every_variant: false,
    },
    RecordedRig {
        rig: "large",
        size: 1.5,
        animator_scale: 1.,
        between: None,
        hips: RecordedNode {
            position: [0., 1.32, 0.],
            ..COMPACT_HIPS
        },
        offset: false,
        every_variant: false,
    },
    RecordedRig {
        rig: "armature",
        size: 1.,
        animator_scale: 1.,
        between: Some(RecordedNode {
            position: [0., 0.05, 0.1],
            rotation: [-HALF, 0., 0., HALF],
            scale: 0.8,
        }),
        hips: RecordedNode {
            position: [0., 0.125, 0.8375],
            rotation: [HALF, 0., 0., HALF],
            scale: 1.,
        },
        offset: false,
        every_variant: false,
    },
    RecordedRig {
        rig: "hips-scale",
        size: 1.,
        animator_scale: 1.,
        between: None,
        hips: RecordedNode {
            position: [0., 1.056, 0.],
            rotation: IDENTITY,
            scale: 1.2,
        },
        offset: false,
        every_variant: false,
    },
];

/// The hips parent's transform in `A`, the hips' own uniform scale `s_h`,
/// and the recorded rig the chain is with whether the Animator node is at
/// `OFFSET_PLACEMENT`. Only `animator_chain` makes one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimatorChain {
    parent: HipsParent,
    hips_scale: f64,
    recorded: Option<(&'static RecordedRig, bool)>,
}

impl AnimatorChain {
    #[must_use]
    pub const fn parent(&self) -> HipsParent {
        self.parent
    }

    #[must_use]
    pub const fn hips_scale(&self) -> f64 {
        self.hips_scale
    }

    /// The recorded rig, and whether at `OFFSET_PLACEMENT`; `None` for a
    /// rig E4 did not pass.
    #[must_use]
    pub const fn recorded(&self) -> Option<(&'static RecordedRig, bool)> {
        self.recorded
    }
}

fn near(got: &[f32], want: &[f32]) -> bool {
    got.len() == want.len()
        && got
            .iter()
            .zip(want)
            .all(|(g, w)| (f64::from(*g) - f64::from(*w)).abs() <= RECORDED_TOLERANCE)
}

fn node_is(local: &Trs, want: &RecordedNode) -> bool {
    near(&local.position, &want.position)
        && near(&local.rotation, &want.rotation)
        && near(&local.scale, &[want.scale; 3])
}

const AT_IDENTITY: RecordedNode = RecordedNode {
    position: [0.; 3],
    rotation: IDENTITY,
    scale: 1.,
};

/// A `COMPACT_LAYOUT` bone's offset from the hips, or `None` for a slot
/// the compact rig lacks.
fn layout_offset(slot: usize) -> Option<Point> {
    let name = BODY_SLOTS.get(slot)?;
    let mut at = COMPACT_LAYOUT.iter().find(|(bone, _, _)| bone == name)?;
    let mut sum = [0.; 3];
    for _ in 0..COMPACT_LAYOUT.len() {
        if at.1.is_empty() {
            return Some(sum);
        }
        sum = zip3(sum, at.2, |a, b| a + b);
        at = COMPACT_LAYOUT.iter().find(|(bone, _, _)| *bone == at.1)?;
    }
    None
}

/// Whether the Avatar's rest skeleton is the compact layout below the
/// hips at `hips` in `A`, turned by `turn` and scaled by `scale`.
fn layout_is(rest: &Points, hips: Point, turn: Quaternion, scale: f64) -> bool {
    let close = |got: Point, want: Point| {
        got.iter()
            .zip(&want)
            .all(|(g, w)| (g - w).abs() <= LAYOUT_TOLERANCE)
    };
    let Some(Some(at)) = rest.get(HIPS) else {
        return false;
    };
    close(*at, hips)
        && rest
            .iter()
            .enumerate()
            .all(|(slot, point)| match (point, layout_offset(slot)) {
                (Some(point), Some(offset)) => {
                    let want = unity_rotate(turn, offset).map(|v| scale * v);
                    close(zip3(*point, *at, |p, h| p - h), want)
                }
                (None, None) => true,
                _ => false,
            })
}

impl BodyAvatar<'_> {
    /// The chain from the Animator node (`paths`' root) to the hips'
    /// Transform, which the hips' `m_TOS` path names; the module documents
    /// its refusals and the recorded rigs.
    pub fn animator_chain(
        &self,
        paths: &RigPaths<'_>,
        meter: &Arc<Meter>,
    ) -> Result<AnimatorChain, AnimError> {
        let (graph, animator) = paths.graph();
        let skeleton = self.nodes[HIPS].ok_or(AnimError::InvalidAvatarIndex("m_HumanBoneIndex"))?;
        let id = *self
            .human()
            .skeleton
            .ids
            .as_slice()
            .get(skeleton)
            .ok_or(AnimError::InvalidAvatarIndex("m_HumanBoneIndex"))?;
        let Some(path) = self.avatar.tos_path(id, meter)? else {
            return refuse(BodyModel::MissingRestScale);
        };
        let hips = match paths.resolve(PathQuery::HashWithEvidence(id, path.as_bytes()), meter) {
            Ok(node) => node,
            Err(error) if error.resource_refusal() => return Err(error),
            Err(_) => return refuse(BodyModel::MissingRestScale),
        };
        let node = |id: usize| graph.nodes().get(id).ok_or(AnimError::MissingPath);
        // The nodes strictly between the Animator node and the hips, bottom up.
        let mut between: MeteredVec<usize> = MeteredVec::new(meter);
        let mut at = node(hips)?.parent;
        loop {
            meter.count_decode(1)?;
            match at {
                Some(id) if id == animator => break,
                Some(id) => {
                    if between.len() >= graph.nodes().len() {
                        return Err(AnimError::MissingPath);
                    }
                    between.push(id)?;
                    at = node(id)?.parent;
                }
                None => return refuse(BodyModel::DetachedBone),
            }
        }
        let mut parent = HipsParent::IDENTITY;
        for &id in between.as_slice().iter().rev() {
            let local = node(id)?.local;
            parent = link(
                &parent,
                local.position.map(f64::from),
                local.rotation.map(f64::from),
                uniform(local.scale)?,
            )?;
        }
        let hips_local = node(hips)?.local;
        let hips_scale = uniform(hips_local.scale)?;
        let placed = node(animator)?.local;
        uniform(placed.scale)?;
        let mut ancestors = true;
        let mut up = node(animator)?.parent;
        let mut steps = 0;
        while let Some(id) = up {
            meter.count_decode(1)?;
            steps += 1;
            if steps > graph.nodes().len() {
                return Err(AnimError::MissingPath);
            }
            ancestors &= node_is(&node(id)?.local, &AT_IDENTITY);
            up = node(id)?.parent;
        }
        // The hips in `A`, and the turn and scale of the bones below it.
        let hips_in_a = link(
            &parent,
            hips_local.position.map(f64::from),
            hips_local.rotation.map(f64::from),
            hips_scale,
        )?;
        let rest = self.rest_points(meter)?;
        let identity = near(&placed.position, &[0.; 3]) && near(&placed.rotation, &IDENTITY);
        let offset = near(&placed.position, &OFFSET_PLACEMENT.0)
            && near(&placed.rotation, &OFFSET_PLACEMENT.1);
        let middle = between.as_slice();
        let recorded = RECORDED_RIGS
            .iter()
            .find(|rig| {
                ancestors
                    && (identity || (offset && rig.offset))
                    && near(&placed.scale, &[rig.animator_scale; 3])
                    && node_is(&hips_local, &rig.hips)
                    && match (rig.between, middle) {
                        (None, []) => true,
                        (Some(want), &[id]) => node(id).is_ok_and(|n| node_is(&n.local, &want)),
                        _ => false,
                    }
                    && layout_is(
                        &rest,
                        hips_in_a.position,
                        hips_in_a.rotation,
                        hips_in_a.scale * f64::from(rig.size),
                    )
            })
            .map(|rig| (rig, !identity));
        Ok(AnimatorChain {
            parent,
            hips_scale,
            recorded,
        })
    }
}

/// The reference's `link`: `parent + s_p R_p t` (Unity's quaternion-vector
/// product), rotation `R_p q`, scale `s_p s`. A rotation not within
/// `UNIT_TOLERANCE` of unit length refuses.
fn link(
    parent: &HipsParent,
    translation: Point,
    rotation: Quaternion,
    scale: f64,
) -> Result<HipsParent, AnimError> {
    let length = rotation.iter().map(|v| v * v).sum::<f64>().sqrt();
    let off = (length - 1.).abs();
    if off.is_nan() || off > UNIT_TOLERANCE {
        return Err(AnimError::UnsupportedAnimatorRoot);
    }
    let turned = unity_rotate(parent.rotation, translation);
    let offset = turned.map(|v| parent.scale * v);
    let out = HipsParent {
        position: zip3(parent.position, offset, |p, o| p + o),
        rotation: multiply(parent.rotation, rotation),
        scale: parent.scale * scale,
    };
    if !out
        .position
        .iter()
        .chain(&out.rotation)
        .chain(&[out.scale])
        .all(|v| v.is_finite())
    {
        return refuse(BodyModel::NonFiniteBody);
    }
    Ok(out)
}

/// Whether freeze v8's E4 passed `settings` on `chain`'s rig: the compact
/// rig with every admitted variant, F5's only with `CHAIN_VARIANT`.
pub fn admit_chain(chain: &AnimatorChain, settings: &RootSettings) -> Result<(), AnimError> {
    match chain.recorded {
        Some((rig, _)) if rig.every_variant || settings.admitted().variant == CHAIN_VARIANT => {
            Ok(())
        }
        _ => Err(AnimError::UnsupportedAnimatorRoot),
    }
}

const _: () = crate::send_sync::<RecordedNode>();
const _: () = crate::send_sync::<RecordedRig>();
const _: () = crate::send_sync::<AnimatorChain>();
