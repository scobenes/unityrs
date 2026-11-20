//! The closed-form hips solve of freeze v8 (`docs/plans/humanoid-body.md`
//! 2.3) and the free-combination refusal, on `body_model`'s centre of mass
//! and frame, transcribed from the f64 reference's `solve_hips`
//! (`tools/parity/anim_body_model.py`) in its operation order.
//!
//! - **The body in the hips frame.** `BodyAvatar::hips_points` walks the
//!   hips' subtree from the hips at the identity: each node below takes its
//!   `m_SkeletonPose` translation, its rest scale and the given local
//!   rotation (the reference's `body_states` from the hips, on `posed`
//!   rows), so `c_h` excludes the hips' own scale and every ancestor's.
//!   The rest scales, the hips' own among them, are the rig's Transforms'
//!   as the bundle stores them (`BodyAvatar::rest_scales`, through `m_TOS`
//!   and the rig's paths): the scales E3a and E3b validated, which
//!   `m_SkeletonPose`'s can miss by an ulp. A node without one refuses
//!   (`missing_rest_scale`), and so does a mapped slot outside the hips'
//!   subtree (`detached_bone`).
//! - **The solve.** `H_A = B_A F(T) F_h^-1`, `P_A = h T_A - k H_A c_h` with
//!   `k = sigma_p s_h`, and the local hips `t = sigma_p^-1 R_p^-1 (P_A -
//!   t_p)`, `q = R_p^-1 H_A`, after the twist scope of `body_model`.
//! - **Free combinations.** Freeze v8's design has full rank (29 of 29), so
//!   `FREE_COMBINATIONS` is empty and no Avatar projects onto one; an edge
//!   with no frozen coefficient already refuses (`unfitted_edge`).
//!   `free_scope` refuses an Avatar with any edge in a free combination's
//!   support, more than the plan's geometric projection asks: a refusal is
//!   the safe side of that difference.
//!
//! Departures from the reference, each refusing where it would answer: a
//! non-finite or nonpositive parent or hips scale refuses
//! (`unsupported_animator_root`); a non-finite input, local state or
//! centre refuses (`non_finite_body`), as do `body_model`'s degenerate
//! frame and invalid rest frame; and so does a non-finite result or a
//! quaternion whose length is zero or overflows (`degenerate_solve`; the
//! reference returns NaN or raises, or `math.hypot` scales where the square
//! root of the squares overflows). The final quaternion is normalized by
//! that square root, within an ulp of the reference's `math.hypot`.
use super::{
    avatar::BODY_BONES,
    body::BodyAvatar,
    body_model::{
        COEFFICIENTS, Edges, HIPS, Matrix, Point, Points, Quaternion, body_frame, centre, finite,
        finite_state, frame_scope, multiply, quaternion, refuse, rest_frame_scope, sub, uniform,
    },
};
use crate::{AnimError, BodyModel, PathQuery, RigPaths};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

/// Freeze v8's `scope.free`: the coefficient combinations the training
/// design left free, as `(i, j, weight)` terms. Its design has full rank.
pub const FREE_COMBINATIONS: [&[(usize, usize, f64)]; 0] = [];

/// The hips parent's transform in the Animator node's frame: the composed
/// position, rotation and uniform scale of the nodes between the Animator
/// node and the hips.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HipsParent {
    pub position: Point,
    pub rotation: Quaternion,
    pub scale: f64,
}

impl HipsParent {
    /// The Animator node itself.
    pub const IDENTITY: Self = Self {
        position: [0.; 3],
        rotation: [0., 0., 0., 1.],
        scale: 1.,
    };
}

impl BodyAvatar<'_> {
    /// Each `m_Skeleton` node's rest local scale as the rig's own Transform
    /// stores it in the bundle: the node's `m_ID` through `m_TOS` to the
    /// graph node `paths` resolves, and that node's local scale. These are
    /// the scales freeze v8's E3a and E3b validated (the recorder's captured
    /// rest scales; `m_SkeletonPose`'s can differ by an ulp). `None` where
    /// the node has no path or the path names no rig node.
    pub fn rest_scales(
        &self,
        paths: &RigPaths<'_>,
        meter: &Arc<Meter>,
    ) -> Result<MeteredVec<Option<[f32; 3]>>, AnimError> {
        let ids = self.human().skeleton.ids.as_slice();
        let graph = paths.graph().0;
        let mut out = MeteredVec::with_capacity(meter, ids.len())?;
        for &id in ids {
            let scale = match self.avatar.tos_path(id, meter)? {
                Some(path) => {
                    match paths.resolve(PathQuery::HashWithEvidence(id, path.as_bytes()), meter) {
                        Ok(node) => graph.nodes().get(node).map(|node| node.local.scale),
                        Err(error) if error.resource_refusal() => return Err(error),
                        Err(_) => None,
                    }
                }
                None => None,
            };
            out.push(scale)?;
        }
        Ok(out)
    }

    /// The hips' own uniform rest scale from `rest_scale`, the reference's
    /// `hips_scale`. An unavailable scale refuses (`missing_rest_scale`),
    /// a nonuniform or nonpositive one `unsupported_animator_root`.
    pub fn hips_scale(
        &self,
        rest_scale: &dyn Fn(usize) -> Option<[f32; 3]>,
    ) -> Result<f64, AnimError> {
        let hips = self.nodes[HIPS].ok_or(AnimError::InvalidAvatarIndex("m_HumanBoneIndex"))?;
        uniform(rest_scale(hips).map_or_else(|| refuse(BodyModel::MissingRestScale), Ok)?)
    }

    /// The mapped slots' positions in the hips frame (the hips at the
    /// origin, unrotated, at unit scale), each node below the hips at its
    /// `m_SkeletonPose` translation, its rest scale `rest_scale(node)` (the
    /// rig's, `rest_scales`) and the local rotation `rotation(node)`: the
    /// reference's `body_states` from the hips on `posed` rows. A node
    /// below the hips without a rest scale refuses (`missing_rest_scale`;
    /// no other scale stands in), and so does a mapped slot outside the
    /// hips' subtree (`detached_bone`: the reference has no hips-local state
    /// for it). A nonuniform, nonpositive or non-finite rest scale refuses
    /// (`unsupported_animator_root`).
    pub fn hips_points(
        &self,
        rotation: &dyn Fn(usize) -> Quaternion,
        rest_scale: &dyn Fn(usize) -> Option<[f32; 3]>,
        meter: &Arc<Meter>,
    ) -> Result<Points, AnimError> {
        let human = self.human();
        let skeleton = human.skeleton.nodes.as_slice();
        let pose = human.pose.as_slice();
        let hips = self.nodes[HIPS].ok_or(AnimError::InvalidAvatarIndex("m_HumanBoneIndex"))?;
        meter.count_decode(skeleton.len() as u64)?;
        let mut states: MeteredVec<Option<(Point, Quaternion, f64)>> =
            MeteredVec::with_capacity(meter, skeleton.len())?;
        for (k, node) in skeleton.iter().enumerate() {
            let state = if k == hips {
                Some(([0.; 3], [0., 0., 0., 1.], 1.))
            } else {
                let parent = usize::try_from(node.parent)
                    .ok()
                    .filter(|&parent| parent < k)
                    .and_then(|parent| states.as_slice().get(parent).copied().flatten());
                match parent {
                    Some(parent) => {
                        let x = pose
                            .get(k)
                            .ok_or(AnimError::InvalidAvatarIndex("m_SkeletonPose"))?;
                        let s = rest_scale(k)
                            .map_or_else(|| refuse(BodyModel::MissingRestScale), Ok)?;
                        Some(finite_state(super::body_model::link(
                            parent,
                            x.t.map(f64::from),
                            rotation(k),
                            uniform(s)?,
                        ))?)
                    }
                    None => None,
                }
            };
            states.push(state)?;
        }
        let mut out = [None; BODY_BONES];
        for (point, node) in out.iter_mut().zip(&self.nodes) {
            if let Some(node) = node {
                let state = states.as_slice().get(*node).copied().flatten();
                *point = Some(state.map_or_else(|| refuse(BodyModel::DetachedBone), Ok)?.0);
            }
        }
        Ok(out)
    }
}

/// Refuses an Avatar with an edge in a free combination's support
/// (`free_combination`).
pub fn free_scope(edges: &Edges) -> Result<(), AnimError> {
    for combination in FREE_COMBINATIONS {
        let touches = combination
            .iter()
            .any(|&(i, j, weight)| weight != 0. && edges.get(j).copied().flatten() == Some(i));
        if touches {
            return refuse(BodyModel::FreeCombination);
        }
    }
    Ok(())
}

/// `(-x, -y, -z, w)`.
#[allow(clippy::many_single_char_names, reason = "the reference's names")]
#[must_use]
pub const fn conjugate(q: Quaternion) -> Quaternion {
    let [x, y, z, w] = q;
    [-x, -y, -z, w]
}

/// `q v q^-1` by two Hamilton products, the reference's `rotate`.
#[allow(clippy::many_single_char_names, reason = "the reference's names")]
#[must_use]
pub fn rotate(q: Quaternion, v: Point) -> Point {
    let [vx, vy, vz] = v;
    let [x, y, z, _] = multiply(multiply(q, [vx, vy, vz, 0.]), conjugate(q));
    [x, y, z]
}

/// The rotation matrix of a unit quaternion, the reference's
/// `quaternion_matrix`.
#[must_use]
#[allow(clippy::many_single_char_names, reason = "the reference's names")]
pub fn quaternion_matrix(q: Quaternion) -> Matrix {
    let [x, y, z, w] = q;
    [
        [
            1. - 2. * (y * y + z * z),
            2. * (x * y - z * w),
            2. * (x * z + y * w),
        ],
        [
            2. * (x * y + z * w),
            1. - 2. * (x * x + z * z),
            2. * (y * z - x * w),
        ],
        [
            2. * (x * z - y * w),
            2. * (y * z + x * w),
            1. - 2. * (x * x + y * y),
        ],
    ]
}

/// `a b`, the reference's `product(a, b)`.
#[must_use]
pub fn product(a: &Matrix, b: &Matrix) -> Matrix {
    let [b0, b1, b2] = *b;
    let columns = [0, 1, 2].map(|c| [b0, b1, b2].map(|row| row.get(c).copied().unwrap_or(0.)));
    a.map(|row| columns.map(|column| super::body_model::dot(row, column)))
}

fn turn(m: &Matrix, v: Point) -> Point {
    m.map(|row| super::body_model::dot(row, v))
}

/// 2.3: the hips' local translation and rotation placing the body at
/// `(T_A, B_A) = (target, rotation)`, with `local` the body in the hips
/// frame (`BodyAvatar::hips_points`), `rest_frame` the Avatar's `F(T)`
/// (`body_frame` of `BodyAvatar::rest_points`), `human_scale` `m_Scale`
/// and `hips_scale` the hips' own uniform rest scale. Freeze v8's
/// coefficients; `solve_hips_with` takes others.
#[allow(clippy::too_many_arguments, reason = "the reference's signature")]
pub fn solve_hips(
    target: Point,
    rotation: Quaternion,
    human_scale: f64,
    local: &Points,
    weights: &[f64; BODY_BONES],
    edges: &Edges,
    rest_frame: &Matrix,
    parent: &HipsParent,
    hips_scale: f64,
) -> Result<(Point, Quaternion), AnimError> {
    solve_hips_with(
        &COEFFICIENTS,
        target,
        rotation,
        human_scale,
        local,
        weights,
        edges,
        rest_frame,
        parent,
        hips_scale,
    )
}

/// `solve_hips` with the coefficients of `table`.
#[allow(clippy::many_single_char_names, reason = "the reference's names")]
#[allow(clippy::too_many_arguments, reason = "the reference's signature")]
pub fn solve_hips_with(
    table: &[(usize, usize, f64)],
    target: Point,
    rotation: Quaternion,
    human_scale: f64,
    local: &Points,
    weights: &[f64; BODY_BONES],
    edges: &Edges,
    rest_frame: &Matrix,
    parent: &HipsParent,
    hips_scale: f64,
) -> Result<(Point, Quaternion), AnimError> {
    let positive = |s: f64| s.is_finite() && s > 0.;
    if !(positive(parent.scale) && positive(hips_scale)) {
        return Err(AnimError::UnsupportedAnimatorRoot);
    }
    finite(&target)?;
    finite(&rotation)?;
    finite(&parent.position)?;
    finite(&parent.rotation)?;
    finite(&[human_scale])?;
    free_scope(edges)?;
    rest_frame_scope(rest_frame)?;
    frame_scope(local)?;
    let c_h = centre(local, weights, edges, table)?;
    let solved = super::body_model::product_transposed(
        &product(&quaternion_matrix(rotation), rest_frame),
        &body_frame(local)?,
    );
    let k = parent.scale * hips_scale;
    let turned = turn(&solved, c_h);
    let p_a = super::body_model::zip3(target, turned, |t, h| human_scale * t - k * h);
    let inverse = conjugate(parent.rotation);
    let t = rotate(inverse, sub(p_a, parent.position)).map(|v| v / parent.scale);
    let q = multiply(inverse, quaternion(&solved));
    let [x, y, z, w] = q;
    // An overflowing square would make every component a finite zero.
    let length = (0. + x * x + y * y + z * z + w * w).sqrt();
    if !(length.is_finite() && length > 0.) {
        return refuse(BodyModel::DegenerateSolve);
    }
    let q = q.map(|v| v / length);
    if !t.iter().chain(&q).all(|v| v.is_finite()) {
        return refuse(BodyModel::DegenerateSolve);
    }
    Ok((t, q))
}

const _: () = crate::send_sync::<HipsParent>();
