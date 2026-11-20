//! The frozen body model of freeze v8 (`docs/plans/humanoid-body.md` 2.1
//! and 2.2; `tools/parity/anim_body_freeze_v8.json`, which E3b validated):
//! its 29 edge coefficients, the mass weights, the edges, the rest body
//! from `m_SkeletonPose`, the centre of mass, the body frame and rotation,
//! and the shoulder-hip twist and spine-lateral scope refusals.
//!
//! Each formula is the f64 reference's (`tools/parity/anim_body_model.py`
//! and `anim_body_tolerance.py`), in its operation and summation order:
//! values are f64 from the f32 inputs, taken exactly, and every sum runs
//! in `HumanBodyBones` order with `UpperChest` last (`SUM_ORDER`, the
//! reference's `NAMES`). One step differs: the reference normalizes a
//! quaternion with `math.hypot`, this module by the square root of the sum
//! of squares, within an ulp of it. Every constant here is the freeze's,
//! and the integration tests bind each one to the JSON.
//!
//! - **2.1.** `c = sum_i m_i (p_i + sum_j a_ij (p_j - p_i))` over the
//!   mapped slots `i` and their edges to `j`, the nearest mapped body-slot
//!   descendants in the Avatar's skeleton (`BodyAvatar::edges`), with
//!   `m_i` the Avatar's mass over the masses' sum (decision 1,
//!   `BodyAvatar::weights`). An edge the freeze fitted no coefficient for
//!   refuses (`unfitted_edge`): the reference would take it as 0, a value
//!   no training rig tested. The leaves' Head edges keep their small
//!   nonzero frozen fractions.
//! - **2.2.** `up = (UpperArms - UpperLegs) / 2`, `lateral = (R - L
//!   UpperArm) + (R - L UpperLeg)`, `forward = lateral x up`, `right = up x
//!   forward`, each of `up` and `forward` normalized; `F` has the columns
//!   `(right, up, forward)` and the body rotation is `F(pose) F(T)^-1`.
//! - **Scope.** A shoulder-hip twist (the angle about `up` between the
//!   upper-arm and upper-leg laterals) over `TWIST_LIMIT` refuses
//!   (`shoulder_hip_twist`), and an undefined one (no `up`, or a lateral
//!   along it) refuses as `undefined_twist`. A pose whose largest spine
//!   Left-Right muscle exceeds `LATERAL_LIMIT` refuses
//!   (`spine_lateral_range`).
//! - **Refusals the reference lacks**, each refusing where the reference
//!   would answer: a non-finite state, point, weight, frame or result
//!   (`non_finite_body`; a chain of large scales overflows, and an infinite
//!   scale times a zero translation is NaN); an `up` within its own
//!   rounding of zero (`undefined_twist`, not only an exact zero); a
//!   lateral along `up` within `frame_threshold` (`degenerate_frame`,
//!   where the reference would take a rounding residual as the forward
//!   axis); and a rest frame that is not finite and orthonormal
//!   (`invalid_rest_frame`).
//!
//! The hips solve of 2.3 and the free-combination refusal are
//! `body_solve`'s.
use super::{avatar::BODY_BONES, body::BodyAvatar};
use crate::{AnimError, BodyModel};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

/// A position in f64.
pub type Point = [f64; 3];
/// A position per body slot, `None` where the slot is unmapped.
pub type Points = [Option<Point>; BODY_BONES];
/// Each slot's edge parent: the nearest mapped body-slot ancestor.
pub type Edges = [Option<usize>; BODY_BONES];
/// A rotation matrix by rows.
pub type Matrix = [[f64; 3]; 3];
/// A quaternion `(x, y, z, w)`.
pub type Quaternion = [f64; 4];

pub const HIPS: usize = 0;
pub const LEFT_UPPER_LEG: usize = 1;
pub const RIGHT_UPPER_LEG: usize = 2;
pub const LEFT_LOWER_LEG: usize = 3;
pub const RIGHT_LOWER_LEG: usize = 4;
pub const LEFT_FOOT: usize = 5;
pub const RIGHT_FOOT: usize = 6;
pub const SPINE: usize = 7;
pub const CHEST: usize = 8;
pub const UPPER_CHEST: usize = 9;
pub const NECK: usize = 10;
pub const HEAD: usize = 11;
pub const LEFT_SHOULDER: usize = 12;
pub const RIGHT_SHOULDER: usize = 13;
pub const LEFT_UPPER_ARM: usize = 14;
pub const RIGHT_UPPER_ARM: usize = 15;
pub const LEFT_LOWER_ARM: usize = 16;
pub const RIGHT_LOWER_ARM: usize = 17;
pub const LEFT_HAND: usize = 18;
pub const RIGHT_HAND: usize = 19;
pub const LEFT_TOES: usize = 20;
pub const RIGHT_TOES: usize = 21;
pub const LEFT_EYE: usize = 22;
pub const RIGHT_EYE: usize = 23;
pub const JAW: usize = 24;

/// The reference's order of every sum over slots (`NAMES`):
/// `HumanBodyBones` 0-23, then `UpperChest`.
pub const SUM_ORDER: [usize; BODY_BONES] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 9,
];

/// The frame bones of 2.2: the upper arms, then the upper legs, left first.
pub const FRAME_BONES: [usize; 4] = [
    LEFT_UPPER_ARM,
    RIGHT_UPPER_ARM,
    LEFT_UPPER_LEG,
    RIGHT_UPPER_LEG,
];

/// Freeze v8's `coefficients`: `(i, j, a_ij)` per edge, as the JSON
/// holds them.
#[allow(clippy::unreadable_literal, reason = "the freeze's values verbatim")]
pub const COEFFICIENTS: [(usize, usize, f64); 29] = [
    (CHEST, LEFT_SHOULDER, 0.2499960304099545),
    (CHEST, NECK, 0.25000641568003523),
    (CHEST, RIGHT_SHOULDER, 0.2499961743207566),
    (CHEST, UPPER_CHEST, 0.5000011989858576),
    (HEAD, JAW, -9.62427800769327e-06),
    (HEAD, LEFT_EYE, 1.8527665225685173e-06),
    (HEAD, RIGHT_EYE, 3.878443976525228e-06),
    (HIPS, LEFT_UPPER_LEG, 0.3333318775263505),
    (HIPS, RIGHT_UPPER_LEG, 0.33333139539940737),
    (HIPS, SPINE, 0.33333663431945154),
    (LEFT_FOOT, LEFT_TOES, 1.7133203945231105e-05),
    (LEFT_LOWER_ARM, LEFT_HAND, 0.4999958960503324),
    (LEFT_LOWER_LEG, LEFT_FOOT, 0.49999975542770464),
    (LEFT_SHOULDER, LEFT_UPPER_ARM, 0.500006991831585),
    (LEFT_UPPER_ARM, LEFT_LOWER_ARM, 0.12500288535174703),
    (LEFT_UPPER_LEG, LEFT_LOWER_LEG, 0.4999996633923184),
    (NECK, HEAD, 0.4999839367400359),
    (RIGHT_FOOT, RIGHT_TOES, 2.3133957541958807e-05),
    (RIGHT_LOWER_ARM, RIGHT_HAND, 0.49999592961078526),
    (RIGHT_LOWER_LEG, RIGHT_FOOT, 0.49999921915890544),
    (RIGHT_SHOULDER, RIGHT_UPPER_ARM, 0.5000070758834383),
    (RIGHT_UPPER_ARM, RIGHT_LOWER_ARM, 0.5000041007057056),
    (RIGHT_UPPER_LEG, RIGHT_LOWER_LEG, 0.4999996332453748),
    (SPINE, CHEST, 0.5000014870086255),
    (UPPER_CHEST, LEFT_SHOULDER, 0.2499978354750374),
    (UPPER_CHEST, LEFT_UPPER_ARM, 0.11458183184076098),
    (UPPER_CHEST, NECK, 0.25000366967295573),
    (UPPER_CHEST, RIGHT_SHOULDER, 0.2499981526754456),
    (UPPER_CHEST, RIGHT_UPPER_ARM, 0.11458203760427024),
];

/// Freeze v8's `bounds.twist_limit`: 4.25 degrees in radians.
#[allow(clippy::unreadable_literal, reason = "the freeze's value verbatim")]
pub const TWIST_LIMIT: f64 = 0.07417649320975901;

/// Freeze v8's `scope.lateral_limit`.
pub const LATERAL_LIMIT: f64 = 0.193_359_375;

/// The spine Left-Right muscles in `HumanTrait` muscle order: Spine,
/// Chest and `UpperChest`.
pub const LATERAL_MUSCLES: [usize; 3] = [1, 4, 7];

/// Freeze v8's `bounds.unit_roundoff_f64`.
pub const U64: f64 = 1. / 9_007_199_254_740_992.;

/// The reference's `shoulder_hip_twist`: a lateral within `ALONG_UP * U64`
/// of its own length of `up` is along it.
pub const ALONG_UP: f64 = 64.;

/// The reference's `uniform`: three scales are one within this fraction of
/// the largest.
pub const UNIFORM_TOLERANCE: f64 = 1e-6;

/// Higham's `gamma_n = n u / (1 - n u)` at f64.
#[must_use]
pub const fn gamma(n: f64) -> f64 {
    n * U64 / (1. - n * U64)
}

/// The degenerate-frame threshold's operation depth (`frame_threshold`).
pub const FRAME_DEPTH: f64 = 16.;

/// The rest frame's orthonormality bound's operation depth
/// (`rest_frame_scope`).
pub const REST_DEPTH: f64 = 32.;

pub(crate) fn refuse<T>(cause: BodyModel) -> Result<T, AnimError> {
    Err(AnimError::UnsupportedHumanoidBodyModel(cause))
}

/// The coefficient of edge `(parent, child)` in `table`.
#[must_use]
pub fn coefficient(table: &[(usize, usize, f64)], parent: usize, child: usize) -> Option<f64> {
    table
        .iter()
        .find(|&&(i, j, _)| (i, j) == (parent, child))
        .map(|&(_, _, a)| a)
}

impl BodyAvatar<'_> {
    /// Decision 1, on the Avatar's masses and mapped slots (`weights`).
    #[must_use]
    pub fn weights(&self) -> [f64; BODY_BONES] {
        weights(&self.masses, &self.nodes.map(|node| node.is_some()))
    }

    /// Each mapped slot's nearest mapped body-slot ancestor in the Avatar's
    /// skeleton, the reference's `recorded_edges`. An edge freeze v8 has no
    /// coefficient for refuses (`unfitted_edge`).
    pub fn edges(&self, meter: &Meter) -> Result<Edges, AnimError> {
        let skeleton = self.human().skeleton.nodes.as_slice();
        let slot_at = |node: usize| self.nodes.iter().position(|&n| n == Some(node));
        let mut out = [None; BODY_BONES];
        for (child, (entry, start)) in out.iter_mut().zip(&self.nodes).enumerate() {
            let Some(mut at) = *start else {
                continue;
            };
            // Each step moves to a lower node, so this ends.
            let parent = loop {
                meter.count_decode(BODY_BONES as u64)?;
                let Some(up) = skeleton
                    .get(at)
                    .and_then(|node| usize::try_from(node.parent).ok())
                    .filter(|&up| up < at)
                else {
                    break None;
                };
                at = up;
                if let Some(slot) = slot_at(at) {
                    break Some(slot);
                }
            };
            if let Some(parent) = parent {
                if coefficient(&COEFFICIENTS, parent, child).is_none() {
                    return refuse(BodyModel::UnfittedEdge);
                }
                *entry = Some(parent);
            }
        }
        Ok(out)
    }

    /// The mapped slots' rest positions in the Animator node's frame:
    /// `m_SkeletonPose` walked from the root node at the identity (the
    /// reference's `skeleton_points`), each link `p + s_p R_p t` with
    /// rotation `R_p q` and uniform scale `s_p s`. A nonuniform,
    /// nonpositive or non-finite scale refuses as the reference's
    /// `uniform` does (`unsupported_animator_root`).
    pub fn rest_points(&self, meter: &Arc<Meter>) -> Result<Points, AnimError> {
        let human = self.human();
        let skeleton = human.skeleton.nodes.as_slice();
        let pose = human.pose.as_slice();
        meter.count_decode(skeleton.len() as u64)?;
        let mut states: MeteredVec<(Point, Quaternion, f64)> =
            MeteredVec::with_capacity(meter, skeleton.len())?;
        for (k, node) in skeleton.iter().enumerate() {
            let state = if k == 0 {
                ([0.; 3], [0., 0., 0., 1.], 1.)
            } else {
                let parent = usize::try_from(node.parent)
                    .ok()
                    .and_then(|parent| states.as_slice().get(parent))
                    .ok_or(AnimError::InvalidAvatarIndex("m_Skeleton"))?;
                let x = pose
                    .get(k)
                    .ok_or(AnimError::InvalidAvatarIndex("m_SkeletonPose"))?;
                finite_state(link(
                    *parent,
                    x.t.map(f64::from),
                    x.q.map(f64::from),
                    uniform(x.s)?,
                ))?
            };
            states.push(state)?;
        }
        let mut out = [None; BODY_BONES];
        for (point, node) in out.iter_mut().zip(&self.nodes) {
            if let Some(node) = node {
                *point = Some(
                    states
                        .as_slice()
                        .get(*node)
                        .ok_or(AnimError::InvalidAvatarIndex("m_HumanBoneIndex"))?
                        .0,
                );
            }
        }
        Ok(out)
    }
}

/// Decision 1: each mapped slot's stored mass over the mapped masses' sum,
/// taken in `SUM_ORDER` (the reference's `folded_masses`, where every
/// absent slot's mass is zero); 0 for an unmapped slot.
#[must_use]
pub fn weights(masses: &[f32; BODY_BONES], mapped: &[bool; BODY_BONES]) -> [f64; BODY_BONES] {
    let mut total = 0.;
    for &slot in &SUM_ORDER {
        if let (Some(true), Some(&mass)) = (mapped.get(slot), masses.get(slot)) {
            total += f64::from(mass);
        }
    }
    let mut out = [0.; BODY_BONES];
    for ((weight, &on), &mass) in out.iter_mut().zip(mapped).zip(masses) {
        if on {
            *weight = f64::from(mass) / total;
        }
    }
    out
}

/// A linked state whose position, rotation and accumulated scale are all
/// finite, or `non_finite_body`: a chain of large scales can overflow the
/// product, and an infinite scale times a zero translation is NaN.
pub(crate) fn finite_state(
    state: (Point, Quaternion, f64),
) -> Result<(Point, Quaternion, f64), AnimError> {
    let (position, rotation, scale) = state;
    if position.iter().chain(&rotation).all(|v| v.is_finite()) && scale.is_finite() {
        Ok(state)
    } else {
        refuse(BodyModel::NonFiniteBody)
    }
}

pub(crate) fn finite(values: &[f64]) -> Result<(), AnimError> {
    if values.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        refuse(BodyModel::NonFiniteBody)
    }
}

/// The reference's `uniform`: one scale for all three axes, positive.
pub(crate) fn uniform(s: [f32; 3]) -> Result<f64, AnimError> {
    let [x, y, z] = s.map(f64::from);
    let (max, min) = (x.max(y).max(z), x.min(y).min(z));
    let magnitude = x.abs().max(y.abs()).max(z.abs());
    let finite = x.is_finite() && y.is_finite() && z.is_finite();
    if !finite || max - min > UNIFORM_TOLERANCE * magnitude || min <= 0. {
        return Err(AnimError::UnsupportedAnimatorRoot);
    }
    Ok(x)
}

/// `f` of `a` and `b` per component.
pub(crate) fn zip3(a: Point, b: Point, f: impl Fn(f64, f64) -> f64) -> Point {
    let ([a0, a1, a2], [b0, b1, b2]) = (a, b);
    [f(a0, b0), f(a1, b1), f(a2, b2)]
}

pub(crate) fn add(a: Point, b: Point) -> Point {
    zip3(a, b, |a, b| a + b)
}

pub(crate) fn sub(a: Point, b: Point) -> Point {
    zip3(a, b, |a, b| a - b)
}

/// The reference's `link` without its error terms.
pub(crate) fn link(
    (position, rotation, scale): (Point, Quaternion, f64),
    t: Point,
    q: Quaternion,
    s: f64,
) -> (Point, Quaternion, f64) {
    let offset = unity_rotate(rotation, t).map(|c| scale * c);
    (add(position, offset), multiply(rotation, q), scale * s)
}

#[must_use]
pub fn cross(a: Point, b: Point) -> Point {
    let ([a0, a1, a2], [b0, b1, b2]) = (a, b);
    [a1 * b2 - a2 * b1, a2 * b0 - a0 * b2, a0 * b1 - a1 * b0]
}

/// The reference's `sum(a * b ...)`: from 0, in order.
pub(crate) fn dot(a: Point, b: Point) -> f64 {
    let ([a0, a1, a2], [b0, b1, b2]) = (a, b);
    0. + a0 * b0 + a1 * b1 + a2 * b2
}

#[must_use]
pub fn norm(v: Point) -> f64 {
    dot(v, v).sqrt()
}

/// The Hamilton product `q r`, the reference's `multiply`.
#[must_use]
#[allow(clippy::many_single_char_names, reason = "the reference's names")]
pub fn multiply(q: Quaternion, r: Quaternion) -> Quaternion {
    let [x, y, z, w] = q;
    let [a, b, c, d] = r;
    [
        w * a + x * d + y * c - z * b,
        w * b - x * c + y * d + z * a,
        w * c + x * b - y * a + z * d,
        w * d - x * a - y * b - z * c,
    ]
}

/// Unity's quaternion-vector product, `v + w t + u x t` with
/// `t = 2 (u x v)`, the reference's `unity_rotate`.
#[must_use]
pub fn unity_rotate(q: Quaternion, v: Point) -> Point {
    let [qx, qy, qz, qw] = q;
    let axis = [qx, qy, qz];
    let twice = cross(axis, v).map(|c| 2. * c);
    add(zip3(v, twice, |v, t| v + qw * t), cross(axis, twice))
}

/// 2.1's centre of mass in the frame of `points`, with the coefficients
/// of `table` (`COEFFICIENTS` for the frozen model; an edge it lacks takes
/// 0, as in the reference). A non-finite point, weight or centre refuses
/// (`non_finite_body`), and an edge whose end has no point is an index
/// fault.
pub fn centre(
    points: &Points,
    weights: &[f64; BODY_BONES],
    edges: &Edges,
    table: &[(usize, usize, f64)],
) -> Result<Point, AnimError> {
    finite(weights)?;
    for point in points.iter().flatten() {
        finite(point)?;
    }
    let missing = AnimError::InvalidAvatarIndex("m_HumanBoneIndex");
    let point = |slot: usize| points.get(slot).copied().flatten();
    let weight = |slot: usize| weights.get(slot).copied().unwrap_or(0.);
    let mut c = [0.; 3];
    for &slot in &SUM_ORDER {
        if let Some(p) = point(slot) {
            c = zip3(c, p, |c, p| c + weight(slot) * p);
        }
    }
    for &child in &SUM_ORDER {
        if let Some(&Some(parent)) = edges.get(child) {
            let a = coefficient(table, parent, child).unwrap_or(0.);
            let (Some(to), Some(from)) = (point(child), point(parent)) else {
                return Err(missing);
            };
            c = zip3(c, sub(to, from), |c, d| c + a * weight(parent) * d);
        }
    }
    finite(&c)?;
    Ok(c)
}

/// The four frame bones' points, each finite (`non_finite_body`); a
/// missing one leaves the twist undefined.
fn frame_points(points: &Points) -> Result<[Point; 4], AnimError> {
    let mut out = [[0.; 3]; 4];
    for (point, slot) in out.iter_mut().zip(FRAME_BONES) {
        *point = match points.get(slot).copied().flatten() {
            Some(p) => p,
            None => return refuse(BodyModel::UndefinedTwist),
        };
        finite(point)?;
    }
    Ok(out)
}

/// `P`, the sum of the frame points' norms: the scale every rounding in
/// the frame is relative to.
fn magnitude(points: &[Point; 4]) -> f64 {
    points.iter().map(|&p| norm(p)).sum()
}

/// `up` (any positive multiple) unless it is within its own rounding of
/// zero: each component sums four points with three roundings, so the
/// computed `up` is off by at most `gamma(3) P`. `gamma(FRAME_DEPTH) P`
/// dominates that; at or below it `up` has no direction (`undefined_twist`,
/// the reference's "no up").
fn checked_up(up: Point, points: &[Point; 4]) -> Result<Point, AnimError> {
    let length = norm(up);
    if !(length > gamma(FRAME_DEPTH) * magnitude(points) && length.is_finite()) {
        return refuse(BodyModel::UndefinedTwist);
    }
    Ok(up.map(|c| c / length))
}

/// The degenerate-frame threshold on `|lateral x up_hat|`. With `P` the
/// sum of the frame points' norms, the computed lateral is off by at most
/// `gamma(3) P`, the unit `up` by `2 gamma(3) P / |up|` from its inputs
/// and `gamma(6)` from its normalization, and the cross product's own
/// roundings by `gamma(8) |lateral|`, so a lateral exactly along `up` can
/// show a cross product of at most `gamma(3) P + |lateral| (gamma(14) + 2
/// gamma(3) P / |up|)`. `gamma(FRAME_DEPTH) (P + |lateral| (1 + P / |up|))`
/// dominates it term by term.
#[must_use]
pub fn frame_threshold(points: &[Point; 4], lateral: Point, up: Point) -> f64 {
    let p = magnitude(points);
    gamma(FRAME_DEPTH) * (p + norm(lateral) * (1. + p / norm(up)))
}

/// 2.2's frame `F` by rows: the matrix with the columns `(right, up,
/// forward)`. An `up` within its rounding of zero refuses as an undefined
/// twist; a lateral whose cross product with `up` is within
/// `frame_threshold` refuses as `degenerate_frame`: collinear points whose
/// rounding residual would otherwise become the forward axis.
pub fn body_frame(points: &Points) -> Result<Matrix, AnimError> {
    let frame = frame_points(points)?;
    let [ul, ur, ll, lr] = frame;
    let up = sub(sub(add(ul, ur), ll), lr).map(|c| c / 2.);
    let lateral = add(sub(ur, ul), sub(lr, ll));
    let up_hat = checked_up(up, &frame)?;
    let forward = cross(lateral, up_hat);
    let length = norm(forward);
    if !(length > frame_threshold(&frame, lateral, up) && length.is_finite()) {
        return refuse(BodyModel::DegenerateFrame);
    }
    let forward = forward.map(|c| c / length);
    let [r0, r1, r2] = cross(up_hat, forward);
    let ([u0, u1, u2], [f0, f1, f2]) = (up_hat, forward);
    let out = [[r0, u0, f0], [r1, u1, f1], [r2, u2, f2]];
    finite(out.as_flattened())?;
    Ok(out)
}

/// The angle about 2.2's `up` between the upper-arm and the upper-leg
/// laterals, in radians, unsigned.
pub fn shoulder_hip_twist(points: &Points) -> Result<f64, AnimError> {
    let frame = frame_points(points)?;
    let [ul, ur, ll, lr] = frame;
    let up = checked_up(sub(add(ul, ur), add(ll, lr)), &frame)?;
    let flat = |v: Point| {
        let along = dot(v, up);
        let moved = zip3(v, up, |v, u| v - along * u);
        // A lateral along up has no direction about it.
        if norm(moved) > ALONG_UP * U64 * norm(v) {
            Ok(moved)
        } else {
            refuse(BodyModel::UndefinedTwist)
        }
    };
    let (top, bottom) = (flat(sub(ur, ul))?, flat(sub(lr, ll))?);
    let twist = dot(cross(bottom, top), up).atan2(dot(bottom, top)).abs();
    finite(&[twist])?;
    Ok(twist)
}

/// Refuses a pose outside 2.2's frozen twist scope.
pub fn frame_scope(points: &Points) -> Result<(), AnimError> {
    twist_scope(shoulder_hip_twist(points)?)
}

/// Refuses a twist over `TWIST_LIMIT` (`shoulder_hip_twist`); NaN refuses.
pub fn twist_scope(twist: f64) -> Result<(), AnimError> {
    if twist <= TWIST_LIMIT {
        Ok(())
    } else {
        refuse(BodyModel::ShoulderHipTwist)
    }
}

/// The largest `|value|` of the spine Left-Right muscles in `muscles`
/// (`HumanTrait` muscle order; a muscle past its end counts 0), NaN if any
/// is NaN.
#[must_use]
pub fn spine_lateral(muscles: &[f64]) -> f64 {
    LATERAL_MUSCLES
        .iter()
        .map(|&m| muscles.get(m).map_or(0., |v| v.abs()))
        .fold(0., |max: f64, v| {
            if v.is_nan() || max.is_nan() {
                f64::NAN
            } else {
                max.max(v)
            }
        })
}

/// Refuses a pose outside 2.2's validated lateral domain.
pub fn lateral_scope(muscles: &[f64]) -> Result<(), AnimError> {
    if spine_lateral(muscles) <= LATERAL_LIMIT {
        Ok(())
    } else {
        refuse(BodyModel::SpineLateralRange)
    }
}

/// `a b^T`, the reference's `product(a, b, transpose=True)`.
#[must_use]
pub fn product_transposed(a: &Matrix, b: &Matrix) -> Matrix {
    a.map(|row| b.map(|column| dot(row, column)))
}

/// A rotation matrix's unit quaternion, `w >= 0`, by Shepperd's method
/// (the reference's `quaternion`): the largest of the four components
/// from the diagonal, the first on a tie, the rest from off-diagonal sums
/// and differences.
#[must_use]
#[allow(clippy::many_single_char_names, reason = "the reference's names")]
pub fn quaternion(m: &Matrix) -> Quaternion {
    let [[m00, m01, m02], [m10, m11, m12], [m20, m21, m22]] = *m;
    let trace = m00 + m11 + m22;
    let mut pick = (0, m00);
    for (k, key) in [(1, m11), (2, m22), (3, trace)] {
        if key > pick.1 {
            pick = (k, key);
        }
    }
    let root = |v: f64| 2. * v.max(0.).sqrt();
    let q = match pick.0 {
        3 => {
            let s = root(1. + trace);
            [(m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, s / 4.]
        }
        0 => {
            let s = root(1. + m00 - m11 - m22);
            [s / 4., (m10 + m01) / s, (m20 + m02) / s, (m21 - m12) / s]
        }
        1 => {
            let s = root(1. + m11 - m22 - m00);
            [(m01 + m10) / s, s / 4., (m21 + m12) / s, (m02 - m20) / s]
        }
        _ => {
            let s = root(1. + m22 - m00 - m11);
            [(m02 + m20) / s, (m12 + m21) / s, s / 4., (m10 - m01) / s]
        }
    };
    let [x, y, z, w] = q;
    let length = (0. + x * x + y * y + z * z + w * w).sqrt();
    let q = q.map(|v| v / length);
    if q[3] < 0. { q.map(|v| -v) } else { q }
}

/// Refuses a rest frame `F(T)` that is not finite or not orthonormal
/// within `gamma(REST_DEPTH)` per entry of `F^T F - I` (`invalid_rest_frame`).
/// `body_frame`'s columns are unit within `gamma(6)` and pairwise
/// orthogonal within `gamma(16)` (the forward from a rounded cross product
/// of unit vectors, the right from another), and each entry of `F^T F` adds
/// a three-term dot product's `gamma(3)`: `gamma(32)` dominates.
pub fn rest_frame_scope(rest: &Matrix) -> Result<(), AnimError> {
    let bound = gamma(REST_DEPTH);
    let [[a0, b0, c0], [a1, b1, c1], [a2, b2, c2]] = *rest;
    let columns = [[a0, a1, a2], [b0, b1, b2], [c0, c1, c2]];
    let finite = rest.as_flattened().iter().all(|v| v.is_finite());
    let orthonormal = columns.iter().enumerate().all(|(i, &x)| {
        columns.iter().enumerate().all(|(j, &y)| {
            let want = if i == j { 1. } else { 0. };
            (dot(x, y) - want).abs() <= bound
        })
    });
    if finite && orthonormal {
        Ok(())
    } else {
        refuse(BodyModel::InvalidRestFrame)
    }
}

/// 2.2: `F(pose) F(T)^-1` as a quaternion, `F(T)` given as its matrix,
/// after the twist scope and `rest_frame_scope`.
pub fn body_rotation(points: &Points, rest_frame: &Matrix) -> Result<Quaternion, AnimError> {
    rest_frame_scope(rest_frame)?;
    frame_scope(points)?;
    let q = quaternion(&product_transposed(&body_frame(points)?, rest_frame));
    finite(&q)?;
    Ok(q)
}

const _: () = crate::send_sync::<Point>();
const _: () = crate::send_sync::<Points>();
const _: () = crate::send_sync::<Edges>();
const _: () = crate::send_sync::<Matrix>();
const _: () = crate::send_sync::<Quaternion>();
