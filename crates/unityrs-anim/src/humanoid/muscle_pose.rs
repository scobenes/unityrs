//! B5a's muscle pose (`docs/plans/humanoid-body.md`, B5a): muscle values to
//! a bone's local rotation: `anim_humanoid_qualify.muscle_rotation`'s
//! equation with reference 2's angle rule, its floating-point order fixed
//! (`tools/parity/anim_muscle_pose.py`).
//!
//! For one bone, each animated muscle sets its axis's angle, in radians:
//! the limit by the value's sign, then the axis sign,
//! `m_Sgn[axis] * (value * m_Max[axis])` when `value >= 0` and
//! `m_Sgn[axis] * (value * -m_Min[axis])` otherwise (reference 2, the plan's
//! pre-registered correction). `anim_humanoid_qualify`'s hypothesis applied
//! the sign first, `d = m_Sgn * value`, then the limit by `d`'s sign; the
//! two agree bit for bit wherever `m_Sgn` is +1 or `m_Max = -m_Min`
//! exactly, and on the round 10 recordings the hypothesis disagreed on the
//! other axes alone, by `-|value| * (m_Max + m_Min)`. The twist (x) and swing
//! (y, z) combine from half-angle tangents,
//! `joint = normalize((0, tan(y/2), tan(z/2), 1) * (tan(x/2), 0, 0, 1))`,
//! between the axes' pre rotation and conjugate post rotation:
//! `normalize(pre * joint * conj(post))`. The bone's local rotation is
//! `normalize(rest * that)`, from the bone's rest local rotation.
//!
//! **The arithmetic.** Every step is f64 from the stored f32 inputs (the
//! Avatar's axes, the rest rotation, the muscle values), in this order and
//! no other, with no fused multiply-add:
//!
//! - `f64(value) * limit`, exact (two f32 operands), then times `f64(sign)`,
//!   exact (it is +1 or -1);
//! - `tan(angle / 2)` by the platform's `tan`, as Python's `math.tan` is:
//!   the one step whose bits are the C library's, so the goldens record
//!   each tangent (glibc's, where CI runs), and everything after it is
//!   pinned given the tangents;
//! - a quaternion product's components each summed left to right, as
//!   `multiply` writes them (`aw*bx + ax*bw + ay*bz - az*by` and so on);
//! - a norm as `sqrt(((x*x + y*y) + z*z) + w*w)`, left to right, then each
//!   component divided by it. Python's `sum` adds this way up to 3.11
//!   only: from 3.12 it compensates, so `anim_humanoid_qualify.normalize`
//!   can differ in its last bit under a newer Python. The reference in
//!   `anim_muscle_pose.py` writes the order out, and is the one both
//!   follow.
//!
//! Each later muscle on the same axis replaces the earlier one's angle.
//!
//! **The domain of "bit for bit".** The ports agree bit for bit on the
//! same finite f32 inputs given the same `tan` bits: one C library, or the
//! tangents a row records. Across C libraries only the angles are pinned.
//! A NaN or infinite muscle value has a NaN tangent, which gives `None`
//! here. The Python reference also returns `None` for it, as it does for
//! an axis past z or a zero quaternion.
//!
//! Nothing here plays a muscle yet: every humanoid clip still refuses, and
//! the release gate's table (`gate.rs`) is empty. Which bones the equation
//! holds for is measured by the agreement check against Unity's recordings
//! (B5a's acceptance), never assumed.
use super::{Axes, MUSCLE_COUNT, muscle_bone};

/// The axis (0 x twist, 1 y, 2 z) each `HumanTrait` muscle turns its bone
/// about, in `MuscleName` order: per bone its z muscle, then y, then x,
/// those it has (`HumanTrait.MuscleFromBone`).
pub const MUSCLE_AXES: [u8; MUSCLE_COUNT] = [
    2, 1, 0, 2, 1, 0, 2, 1, 0, 2, 1, 0, 2, 1, 0, // Spine to Head
    2, 1, 2, 1, 2, 1, // Eyes and Jaw
    2, 1, 0, 2, 0, 2, 1, 2, // left leg
    2, 1, 0, 2, 0, 2, 1, 2, // right leg
    2, 1, 2, 1, 0, 2, 0, 2, 1, // left arm
    2, 1, 2, 1, 0, 2, 0, 2, 1, // right arm
    2, 1, 2, 2, 2, 1, 2, 2, 2, 1, 2, 2, 2, 1, 2, 2, 2, 1, 2, 2, // left fingers
    2, 1, 2, 2, 2, 1, 2, 2, 2, 1, 2, 2, 2, 1, 2, 2, 2, 1, 2, 2, // right fingers
];

/// A `HumanTrait` muscle's bone and axis.
#[must_use]
pub fn muscle_axis(muscle: usize) -> Option<(usize, usize)> {
    muscle_bone(muscle)
        .zip(MUSCLE_AXES.get(muscle))
        .map(|(bone, &axis)| (bone, usize::from(axis)))
}

/// The (value, axis) pairs of one bone's muscles, in muscle order, from a
/// full muscle vector.
#[must_use]
pub fn bone_muscles(values: &[f32; MUSCLE_COUNT], bone: usize) -> Vec<(f32, usize)> {
    values
        .iter()
        .enumerate()
        .filter_map(|(muscle, &value)| {
            muscle_axis(muscle)
                .filter(|&(b, _)| b == bone)
                .map(|(_, axis)| (value, axis))
        })
        .collect()
}

/// A quaternion product, `a * b`, XYZW, each component summed left to right.
#[must_use]
pub fn multiply(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// `q` over its norm, the squares summed left to right; `None` for a norm
/// that is not positive (zero or NaN).
#[must_use]
pub fn normalize(q: [f64; 4]) -> Option<[f64; 4]> {
    let norm = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    (norm > 0.).then(|| q.map(|x| x / norm))
}

/// Each axis's angle in radians from (value, axis) pairs; `None` for an
/// axis past z.
#[must_use]
pub fn muscle_angles(axes: &Axes, values: &[(f32, usize)]) -> Option<[f64; 3]> {
    let mut angles = [0.; 3];
    for &(value, axis) in values {
        let value = f64::from(value);
        let limit = if value >= 0. {
            f64::from(*axes.max.get(axis)?)
        } else {
            -f64::from(*axes.min.get(axis)?)
        };
        *angles.get_mut(axis)? = f64::from(*axes.sign.get(axis)?) * (value * limit);
    }
    Some(angles)
}

fn wide(q: [f32; 4]) -> [f64; 4] {
    q.map(f64::from)
}

/// Each angle's half-angle tangent, `tan(angle / 2)`, by the platform's
/// `tan`. That is the one step whose bits are the C library's: glibc's (CI)
/// and Apple's differ in the last bit for about one angle in ten.
#[must_use]
pub fn tangents(angles: [f64; 3]) -> [f64; 3] {
    angles.map(|angle| (angle / 2.).tan())
}

/// The rotation between the pre and post rotations from the three
/// half-angle tangents, `normalize(pre * joint * conj(post))`; `None` for
/// a zero quaternion.
#[must_use]
pub fn joint_rotation(axes: &Axes, [tx, ty, tz]: [f64; 3]) -> Option<[f64; 4]> {
    let joint = normalize(multiply([0., ty, tz, 1.], [tx, 0., 0., 1.]))?;
    let [px, py, pz, pw] = wide(axes.post);
    normalize(multiply(
        multiply(wide(axes.pre), joint),
        [-px, -py, -pz, pw],
    ))
}

/// The rotation one bone's muscles give it between its pre and post
/// rotations; `None` for an axis past z or a zero quaternion.
#[must_use]
pub fn muscle_rotation(axes: &Axes, values: &[(f32, usize)]) -> Option<[f64; 4]> {
    joint_rotation(axes, tangents(muscle_angles(axes, values)?))
}

/// `normalize(rest * rotation)`: a bone's local rotation from its rest local
/// rotation and the rotation its muscles give it.
#[must_use]
pub fn local_rotation(rest: [f32; 4], rotation: [f64; 4]) -> Option<[f64; 4]> {
    normalize(multiply(wide(rest), rotation))
}

/// A bone's local rotation from its rest local rotation and its muscles.
#[must_use]
pub fn bone_rotation(rest: [f32; 4], axes: &Axes, values: &[(f32, usize)]) -> Option<[f64; 4]> {
    local_rotation(rest, muscle_rotation(axes, values)?)
}
