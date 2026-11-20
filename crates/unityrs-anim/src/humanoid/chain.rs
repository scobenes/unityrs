//! Freeze v8's chain from the Animator node to the hips (plan 2.3, B4b):
//! the hips parent's transform `(t_p, R_p, sigma_p)` in the Animator
//! node's frame `A`, composed from the rig's own Transforms as the
//! reference's `tracked` and `link` compose the recorded ones
//! (`tools/parity/anim_body_tolerance.py`): each node below the Animator
//! node, from the identity, is `p + s_p R_p t` (Unity's quaternion-vector
//! product), with rotation `R_p q` and uniform scale `s_p s`. The Animator
//! node's own transform never enters `A`.
//!
//! `animator_chain` composes it for any depth, and names what the chain
//! carries beyond the plain one (the hips directly below the Animator
//! node, both at unit scale): a scaled Animator node, one node between the
//! Animator node and the hips, or a non-unit hips scale `s_h`, each a rig
//! freeze v8's F5 recorded alone. A nonuniform or nonpositive scale on the
//! chain refuses (`unsupported_animator_root`), as the reference's
//! `uniform` does, and so do the chains F5 did not record: two or more
//! nodes between the Animator node and the hips, or two of the features
//! together. A hips Transform outside the Animator node's subtree refuses
//! (`detached_bone`), and a non-finite link or result `non_finite_body`.
//!
//! `admit_chain` admits a plain chain with every root configuration
//! `root::root_settings` admits (E4's passing set) and an F5 chain only
//! with the configuration E4 passed on it: all three cells Original, from
//! the clip's motion slots. Every other pairing refuses
//! (`unsupported_animator_root`) until B5's predicate. The human scale
//! `m_Scale` is not restricted: E3a validated the solve on fifteen rigs.
use super::{
    body::BodyAvatar,
    body_model::{HIPS, Point, Quaternion, multiply, refuse, uniform, unity_rotate, zip3},
    body_solve::HipsParent,
    root::{Cell, RootSettings},
};
use crate::{AnimError, BodyModel, PathQuery, RigPaths};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

/// What an Animator chain carries beyond the plain one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainFeature {
    /// The hips directly below the Animator node, both at unit scale.
    Plain,
    /// A uniform non-unit scale on the Animator node (F5's
    /// `scaled-animator`).
    ScaledAnimator,
    /// One node between the Animator node and the hips (F5's `armature`).
    Intermediate,
    /// A uniform non-unit hips scale `s_h` (F5's `hips-scale`).
    HipsScale,
}

/// The hips parent's transform in `A`, the hips' own uniform scale `s_h`
/// and the chain's feature.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimatorChain {
    pub parent: HipsParent,
    pub hips_scale: f64,
    pub feature: ChainFeature,
}

impl BodyAvatar<'_> {
    /// The chain from the Animator node (`paths`' root) to the hips'
    /// Transform, which the hips' `m_TOS` path names; the module documents
    /// its refusals.
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
        let (hips_local, animator_local) = (node(hips)?.local.scale, node(animator)?.local.scale);
        let hips_scale = uniform(hips_local)?;
        uniform(animator_local)?;
        // Any component off one exactly: a scale within `uniform`'s
        // tolerance of the unit is still not the plain chain.
        let unit = |scale: [f32; 3]| scale.iter().all(|v| v.to_bits() == 1_f32.to_bits());
        let features = [
            (!unit(animator_local), ChainFeature::ScaledAnimator),
            (!between.is_empty(), ChainFeature::Intermediate),
            (!unit(hips_local), ChainFeature::HipsScale),
        ];
        let mut present = features
            .iter()
            .filter(|(on, _)| *on)
            .map(|&(_, feature)| feature);
        let feature = present.next().unwrap_or(ChainFeature::Plain);
        if between.len() > 1 || present.next().is_some() {
            return Err(AnimError::UnsupportedAnimatorRoot);
        }
        Ok(AnimatorChain {
            parent,
            hips_scale,
            feature,
        })
    }
}

/// The reference's `link`: `parent + s_p R_p t` (Unity's quaternion-vector
/// product), rotation `R_p q`, scale `s_p s`.
fn link(
    parent: &HipsParent,
    translation: Point,
    rotation: Quaternion,
    scale: f64,
) -> Result<HipsParent, AnimError> {
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

/// Whether freeze v8 verified `settings` on `chain`: a plain chain with
/// every admitted configuration, an F5 chain only all Original.
pub fn admit_chain(chain: &AnimatorChain, settings: &RootSettings) -> Result<(), AnimError> {
    let rules = settings.rules;
    let original = [rules.yaw, rules.height, rules.xz] == [Cell::Original; 3];
    if chain.feature == ChainFeature::Plain || original {
        Ok(())
    } else {
        Err(AnimError::UnsupportedAnimatorRoot)
    }
}

const _: () = crate::send_sync::<ChainFeature>();
const _: () = crate::send_sync::<AnimatorChain>();
