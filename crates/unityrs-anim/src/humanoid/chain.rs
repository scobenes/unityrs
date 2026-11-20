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
//! reference's `uniform` does, and so do a rotation more than
//! `UNIT_TOLERANCE` from unit length and a composed scale that is zero or
//! subnormal; a hips Transform outside the Animator node's subtree refuses
//! (`detached_bone`), and a non-finite link `non_finite_body`.
//!
//! It then states the chain's scope: whether it lies within what freeze
//! v8's evidence validated, as checkable ranges and structure, never as a
//! rig's identity. In scope, every one of these holds:
//! - the Avatar's human scale `m_Scale` within `HUMAN_SCALE_RANGE`, the
//!   range over the training rigs E3a and E4 validated;
//! - every rest scale below the hips (the skeleton nodes' Transforms the
//!   solve reads, `rest_scales`) exactly one, as on every validated rig;
//! - every ancestor of the Animator node exactly at the identity;
//! - the plain chain (the hips directly below the Animator node, both at
//!   unit scale) or one of F5's features alone, at its fixture value
//!   exactly: the Animator node scaled `ANIMATOR_SCALE`, the one node
//!   `BETWEEN` the Animator node and the hips, or the hips scaled
//!   `HIPS_SCALE` (`tools/fixtures/unity/Editor/UnityrsBodyFixtures.cs`);
//! - the Animator node exactly at the identity, or, for the plain chain
//!   only, exactly at `OFFSET_PLACEMENT`, where E4 played the compact rig.
//!
//! `admit_chain` pairs the scope with the clip's admitted root settings as
//! E4 did: the plain chain with every admitted variant, an F5 feature only
//! with `CHAIN_VARIANT`. A chain out of scope and every other pairing
//! refuse (`unsupported_animator_root`) until B5's predicate.
use super::{
    body::BodyAvatar,
    body_model::{HIPS, Point, Quaternion, multiply, refuse, uniform, unity_rotate, zip3},
    body_solve::HipsParent,
    root::{RootSettings, U32},
};
use crate::{AnimError, BodyModel, PathQuery, RigPaths};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_scene::{Graph, NodeId, Trs};

/// The F5 clip variant, `anim_fixture_body.CHAIN_VARIANT`.
pub const CHAIN_VARIANT: &str = "keep-o1-y1-xz1-motion";

/// `m_Scale`'s range over the training rigs freeze v8's E3a and E4
/// validated, inclusive: the armature rig's and the large rig's, measured
/// on the recordings of `HUMAN_SCALE_EVIDENCE`. The hold-out rigs are
/// sealed and not measured.
pub const HUMAN_SCALE_RANGE: [f32; 2] = [0.781_154_33, 1.434_663_9];

/// Freeze v8's `training.digest`: the recordings `HUMAN_SCALE_RANGE` and
/// the rest scales below the hips were measured on.
pub const HUMAN_SCALE_EVIDENCE: &str =
    "d99e541ac7db99c4b71c4ca2ef4c5cdc9f602abb2fa4ef9b4a26bf5207d0b7e2";

/// How far a chain rotation's length may be from one: an f32 unit
/// quaternion as Unity normalizes it in f32 (four squares and three sums,
/// a square root, a division) and stores it, eight f32 roundings at most,
/// `gamma_8`.
pub const UNIT_TOLERANCE: f64 = 8. * U32 / (1. - 8. * U32);

/// F5's scaled Animator node: `AnimatorScale`.
pub const ANIMATOR_SCALE: f32 = 1.25;

/// F5's hips scale: the `hips-scale` rig's `HipsScale`.
pub const HIPS_SCALE: f32 = 1.2;

/// `sin` and `cos` of 45 degrees as Unity's `Quaternion.Euler(-90, 0, 0)`
/// records them (`0x3f3504f4`), one ulp above √½ rounded to f32.
const HALF: f32 = f32::from_bits(0x3f35_04f4);

/// F5's node between the Animator node and the hips, the armature:
/// translation `(0, .05, .1)`, `Quaternion.Euler(-90, 0, 0)` and scale
/// `ArmatureScale`.
pub const BETWEEN: ([f32; 3], [f32; 4], f32) = ([0., 0.05, 0.1], [-HALF, 0., 0., HALF], 0.8);

/// The Animator node's offset placement E4 played the compact rig at:
/// `(2, .25, -1)` and `Quaternion.Euler(0, 35, 0)`.
pub const OFFSET_PLACEMENT: ([f32; 3], [f32; 4]) =
    ([2., 0.25, -1.], [0., 0.300_705_8, 0., 0.953_716_93]);

/// What the chain carries beyond the plain one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainFeature {
    /// The hips directly below the Animator node, both at unit scale.
    Plain,
    /// The Animator node at `ANIMATOR_SCALE` (F5's `scaled-animator`).
    ScaledAnimator,
    /// The one node `BETWEEN` (F5's `armature`).
    Intermediate,
    /// The hips at `HIPS_SCALE` (F5's `hips-scale`).
    HipsScale,
}

/// The hips parent's transform in `A`, the hips' own uniform scale `s_h`,
/// and the chain's scope: its feature and whether the Animator node is at
/// `OFFSET_PLACEMENT`, or `None` out of scope. Only `animator_chain`
/// makes one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimatorChain {
    parent: HipsParent,
    hips_scale: f64,
    scope: Option<(ChainFeature, bool)>,
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

    /// The chain's feature and whether it is at `OFFSET_PLACEMENT`;
    /// `None` out of the validated scope.
    #[must_use]
    pub const fn validated(&self) -> Option<(ChainFeature, bool)> {
        self.scope
    }
}

/// Exactly equal, either zero's sign aside.
fn same(got: &[f32], want: &[f32]) -> bool {
    got.len() == want.len()
        && got
            .iter()
            .zip(want)
            .all(|(g, w)| g.to_bits() == w.to_bits() || (g.to_bits() | w.to_bits()) << 1 == 0)
}

const IDENTITY: [f32; 4] = [0., 0., 0., 1.];

fn at_identity(local: &Trs) -> bool {
    same(&local.position, &[0.; 3])
        && same(&local.rotation, &IDENTITY)
        && same(&local.scale, &[1.; 3])
}

/// Whether every ancestor of the Animator node is exactly at the identity.
fn ancestors_at_identity(
    graph: &Graph,
    animator: NodeId,
    meter: &Meter,
) -> Result<bool, AnimError> {
    let node = |id: usize| graph.nodes().get(id).ok_or(AnimError::MissingPath);
    let (mut all, mut up, mut steps) = (true, node(animator)?.parent, 0);
    while let Some(id) = up {
        meter.count_decode(1)?;
        steps += 1;
        if steps > graph.nodes().len() {
            return Err(AnimError::MissingPath);
        }
        all &= at_identity(&node(id)?.local);
        up = node(id)?.parent;
    }
    Ok(all)
}

/// A chain outside the validated scope: composed, but with no scope.
const fn unscoped(parent: HipsParent, hips_scale: f64) -> AnimatorChain {
    AnimatorChain {
        parent,
        hips_scale,
        scope: None,
    }
}

impl BodyAvatar<'_> {
    /// The chain from the Animator node (`paths`' root) to the hips'
    /// Transform, which the hips' `m_TOS` path names, and its scope; the
    /// module documents its refusals.
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
        let ancestors = ancestors_at_identity(graph, animator, meter)?;
        let features = [
            match placed.scale {
                s if same(&s, &[1.; 3]) => None,
                s if same(&s, &[ANIMATOR_SCALE; 3]) => Some(ChainFeature::ScaledAnimator),
                _ => return Ok(unscoped(parent, hips_scale)),
            },
            match between.as_slice() {
                [] => None,
                &[id] => {
                    let local = node(id)?.local;
                    let (t, q, s) = BETWEEN;
                    if !(same(&local.position, &t)
                        && same(&local.rotation, &q)
                        && same(&local.scale, &[s; 3]))
                    {
                        return Ok(unscoped(parent, hips_scale));
                    }
                    Some(ChainFeature::Intermediate)
                }
                _ => return Ok(unscoped(parent, hips_scale)),
            },
            match hips_local.scale {
                s if same(&s, &[1.; 3]) => None,
                s if same(&s, &[HIPS_SCALE; 3]) => Some(ChainFeature::HipsScale),
                _ => return Ok(unscoped(parent, hips_scale)),
            },
        ];
        let mut present = features.iter().flatten();
        let feature = present.next().copied().unwrap_or(ChainFeature::Plain);
        let identity = same(&placed.position, &[0.; 3]) && same(&placed.rotation, &IDENTITY);
        let offset = same(&placed.position, &OFFSET_PLACEMENT.0)
            && same(&placed.rotation, &OFFSET_PLACEMENT.1);
        let placement = identity || (offset && feature == ChainFeature::Plain);
        let [low, high] = HUMAN_SCALE_RANGE;
        let scale = self.scale();
        let in_scope = present.next().is_none()
            && ancestors
            && placement
            && (low..=high).contains(&scale)
            && self.unit_below_hips(paths, meter)?;
        Ok(AnimatorChain {
            parent,
            hips_scale,
            scope: in_scope.then_some((feature, !identity)),
        })
    }

    /// Whether every skeleton node below the hips has a Transform at rest
    /// scale exactly one: the scales `rest_scales` gives the solve. A
    /// lookup `rest_scales` refuses is out of scope; a resource refusal
    /// stands.
    fn unit_below_hips(&self, paths: &RigPaths<'_>, meter: &Arc<Meter>) -> Result<bool, AnimError> {
        let scales = match self.rest_scales(paths, meter) {
            Ok(scales) => scales,
            Err(error) if error.resource_refusal() => return Err(error),
            Err(_) => return Ok(false),
        };
        let hips = self.nodes[HIPS].ok_or(AnimError::InvalidAvatarIndex("m_HumanBoneIndex"))?;
        let skeleton = self.human().skeleton.nodes.as_slice();
        meter.count_decode(skeleton.len() as u64)?;
        let mut below: MeteredVec<bool> = MeteredVec::with_capacity(meter, skeleton.len())?;
        for (k, node) in skeleton.iter().enumerate() {
            let parent = usize::try_from(node.parent)
                .ok()
                .filter(|&parent| parent < k);
            let inside =
                parent.is_some_and(|p| p == hips || below.as_slice().get(p) == Some(&true));
            if inside
                && !scales
                    .as_slice()
                    .get(k)
                    .copied()
                    .flatten()
                    .is_some_and(|s| same(&s, &[1.; 3]))
            {
                return Ok(false);
            }
            below.push(inside)?;
        }
        Ok(true)
    }
}

/// The reference's `link`: `parent + s_p R_p t` (Unity's quaternion-vector
/// product), rotation `R_p q`, scale `s_p s`. A rotation not within
/// `UNIT_TOLERANCE` of unit length refuses, and so does a composed scale
/// that is zero or subnormal.
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
    if !out.scale.is_normal() {
        return Err(AnimError::UnsupportedAnimatorRoot);
    }
    Ok(out)
}

/// Whether freeze v8's E4 validated `settings` on `chain`'s scope: the
/// plain chain with every admitted variant, an F5 feature only with
/// `CHAIN_VARIANT`.
pub fn admit_chain(chain: &AnimatorChain, settings: &RootSettings) -> Result<(), AnimError> {
    match chain.scope {
        Some((ChainFeature::Plain, _)) => Ok(()),
        Some(_) if settings.admitted().variant == CHAIN_VARIANT => Ok(()),
        _ => Err(AnimError::UnsupportedAnimatorRoot),
    }
}

const _: () = crate::send_sync::<ChainFeature>();
const _: () = crate::send_sync::<AnimatorChain>();
