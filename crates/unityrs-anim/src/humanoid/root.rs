//! Freeze v8's root projection (`docs/plans/humanoid-body.md` 2.4): the
//! target body pose `(T_A, B_A)` in the Animator node's frame from a built
//! humanoid clip, transcribed from the f64 reference's `cells`, `project`
//! and `animator_body` (`tools/parity/anim_body_model.py`) and E4's
//! `built_pose` and `clip_sample` (`tools/parity/anim_body_e4.py`), in
//! their operation order, on `root_sample`'s runtime slots.
//!
//! - **Inputs.** The body and motion slots at the clip time
//!   (`root_sample`); the serialized start `m_StartX` (the bake cells'
//!   start value, as E4 validated it, never a sampled `RootT`); `m_Level`
//!   and `m_OrientationOffsetY`.
//! - **The root.** Per cell (yaw, height, XZ): bake takes the start,
//!   keep-original the motion slots, otherwise the body. The yaw is the
//!   twist about up, `2 atan2(q_y, q_w)`, plus the orientation offset unless
//!   baked; the height adds the level unless baked.
//! - **The body in `A`.** `T_A = h y^-1 (RootT - r)`, `B_A = y^-1 RootQ`:
//!   an `AnimatorBodyPose`, in metres. This is the one place `m_Scale`
//!   applies; `body_solve::solve_hips` takes the metres.
//! - **No body slots.** The plan's `T_A = 0` and identity `B_A` refuses
//!   (`unsupported_humanoid_root_basis(m_IndexArray)`): E4 compares only
//!   clips with body slots.
//!
//! Only E4's passing configurations are admitted (`ADMITTED`: the
//! variants of freeze v8's `e4.passing`, each with the settings E4's
//! fixture built it with), field by field: all six bake and keep-original
//! flags, and `m_Level` and `m_OrientationOffsetY` to the bit. Every other
//! refuses before any sample (`root_settings`):
//! - `m_StartAtOrigin` off, or Based Upon Feet on
//!   (`unsupported_humanoid_root_basis`): the lowest-foot rule is
//!   undecided;
//! - flags no admitted variant has (`unsupported_humanoid_root_basis`, the
//!   first keep-original flag set, which a bake flag never masks);
//! - a level or orientation offset other than the variant's, a mirror, a
//!   cycle offset or loop flags (`unsupported_humanoid_clip_settings`);
//! - motion slots (`MotionT`, `MotionQ`) other than the variant's: all
//!   bound for the all-Original variant, none for the others
//!   (`unsupported_humanoid_root_basis(m_IndexArray)`);
//! - an additive reference pose (`unsupported_additive_pose`), which the
//!   fixture requires off.
//!
//! A yaw whose error E4 leaves unbounded, at f32 with its source's error or at f64, refuses
//! (`undefined_yaw`), and so does a non-finite or zero `m_StartX`
//! rotation (`non_finite_body`, `degenerate_root_rotation`).
//!
//! Departures, each refusing where it would answer: a quaternion is
//! normalized by the square root of its sum of squares (the reference's
//! `math.hypot`, within an ulp), and the yaw's bound compares `f64::hypot`
//! with a margin of four ulps, so that an ulp of `math.hypot` cannot admit
//! a yaw E4 refuses. A non-finite input or result refuses
//! (`non_finite_body`).
pub use super::root_sample::{RootSample, SampledPose, U32, root_sample, root_timing};
use super::{
    body_model::{Point, Quaternion, U64, multiply, refuse},
    body_solve::{AnimatorBodyPose, conjugate, rotate},
    clip::SettingValue,
    root_sample::{humanoid, normalize},
};
use crate::{AnimError, BodyModel, clip::Clip, read::Dict};
use std::sync::Arc;
use unityrs_core::limits::Meter;

/// Where a root component comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    /// Bake Into Pose: the start value, `m_StartX`.
    Bake,
    /// Based Upon Original: the motion slots.
    Original,
    /// Based Upon the body: the body slots.
    Body,
}

/// Each root component's rule, the reference's `cells`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RootRules {
    pub yaw: Cell,
    pub height: Cell,
    pub xz: Cell,
}

/// One root configuration freeze v8's E4 passed: its variant in
/// `e4.passing`, and the settings E4's fixture built it with
/// (`tools/parity/anim_fixture_body.py`'s `CLIP_VARIANTS`): the bake and
/// keep-original flags in (orientation, Y, XZ) order, `m_Level`,
/// `m_OrientationOffsetY` and whether it plays motion curves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdmittedRoot {
    pub variant: &'static str,
    pub bake: [bool; 3],
    pub keep: [bool; 3],
    pub level: f32,
    pub offset_degrees: f32,
    pub motion: bool,
}

const fn admitted(variant: &'static str, bake: [bool; 3], keep: [bool; 3]) -> AdmittedRoot {
    AdmittedRoot {
        variant,
        bake,
        keep,
        level: 0.,
        offset_degrees: 0.,
        motion: false,
    }
}

const NONE: [bool; 3] = [false; 3];
/// `MotionT` and `MotionQ`'s DOFs.
const MOTION_DOFS: std::ops::Range<usize> = 0..7;
const ALL: [bool; 3] = [true; 3];

/// Freeze v8's E4-passing root configurations, which a test binds to
/// `e4.passing` and to the fixture's settings.
pub const ADMITTED: [AdmittedRoot; 11] = [
    admitted("bake-o0-y0-xz1", [false, false, true], NONE),
    admitted("bake-o0-y1-xz0", [false, true, false], NONE),
    admitted("bake-o0-y1-xz1", [false, true, true], NONE),
    admitted("bake-o1-y0-xz0", [true, false, false], NONE),
    admitted("bake-o1-y0-xz1", [true, false, true], NONE),
    admitted("bake-o1-y1-xz0", [true, true, false], NONE),
    admitted("bake-o1-y1-xz1", ALL, NONE),
    admitted("keep-o0-y0-xz0", NONE, NONE),
    AdmittedRoot {
        motion: true,
        ..admitted("keep-o1-y1-xz1-motion", NONE, ALL)
    },
    AdmittedRoot {
        level: 0.125,
        ..admitted("level", NONE, NONE)
    },
    AdmittedRoot {
        offset_degrees: 30.,
        ..admitted("orientation-offset", NONE, NONE)
    },
];

const BAKE: [&str; 3] = [
    "m_LoopBlendOrientation",
    "m_LoopBlendPositionY",
    "m_LoopBlendPositionXZ",
];
const KEEP: [&str; 3] = [
    "m_KeepOriginalOrientation",
    "m_KeepOriginalPositionY",
    "m_KeepOriginalPositionXZ",
];

/// A clip's admitted root settings: its `ADMITTED` row, rules and start
/// `m_StartX`. Only `root_settings` makes one, from the clip it reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RootSettings {
    admitted: &'static AdmittedRoot,
    rules: RootRules,
    start: (Point, Quaternion),
}

impl RootSettings {
    /// The admitted configuration the clip's settings are.
    #[must_use]
    pub const fn admitted(&self) -> &'static AdmittedRoot {
        self.admitted
    }

    #[must_use]
    pub const fn rules(&self) -> RootRules {
        self.rules
    }

    /// `m_StartX`: its translation and normalized rotation.
    #[must_use]
    pub const fn start(&self) -> (Point, Quaternion) {
        self.start
    }

    #[must_use]
    pub fn level(&self) -> f64 {
        f64::from(self.admitted.level)
    }

    #[must_use]
    pub fn offset_degrees(&self) -> f64 {
        f64::from(self.admitted.offset_degrees)
    }
}

/// The clip's root settings if they are one of `ADMITTED`, field by field,
/// else the typed refusal (the module documents which).
pub fn root_settings(clip: &Clip, meter: &Arc<Meter>) -> Result<RootSettings, AnimError> {
    let human = humanoid(clip)?;
    let settings = human.settings();
    let value = |field: &'static str| {
        settings
            .iter()
            .find(|setting| setting.field == field)
            .and_then(|setting| setting.value)
    };
    let flag = |field: &'static str| match value(field) {
        Some(SettingValue::Bool(on)) => Ok(on),
        _ => Err(AnimError::UnsupportedHumanoidClipSettings(field)),
    };
    let float = |field: &'static str| match value(field) {
        Some(SettingValue::Float(x)) => Ok(x),
        _ => Err(AnimError::UnsupportedHumanoidClipSettings(field)),
    };
    for field in ["m_Mirror", "m_LoopTime", "m_LoopBlend"] {
        if flag(field)? {
            return Err(AnimError::UnsupportedHumanoidClipSettings(field));
        }
    }
    if float("m_CycleOffset")?.to_bits() != 0_f32.to_bits() {
        return Err(AnimError::UnsupportedHumanoidClipSettings("m_CycleOffset"));
    }
    if !flag("m_StartAtOrigin")? {
        return Err(AnimError::UnsupportedHumanoidRootBasis("m_StartAtOrigin"));
    }
    if flag("m_HeightFromFeet")? {
        return Err(AnimError::UnsupportedHumanoidRootBasis("m_HeightFromFeet"));
    }
    let bake = [flag(BAKE[0])?, flag(BAKE[1])?, flag(BAKE[2])?];
    let keep = [flag(KEEP[0])?, flag(KEEP[1])?, flag(KEEP[2])?];
    let (level, offset) = (float("m_Level")?, float("m_OrientationOffsetY")?);
    let rows = || {
        ADMITTED
            .iter()
            .filter(move |row| row.bake == bake && row.keep == keep)
    };
    if rows().next().is_none() {
        let set = keep
            .iter()
            .zip(KEEP)
            .find_map(|(&on, field)| on.then_some(field));
        return Err(AnimError::UnsupportedHumanoidRootBasis(
            set.unwrap_or(KEEP[0]),
        ));
    }
    let Some(row) = rows().find(|row| {
        row.level.to_bits() == level.to_bits() && row.offset_degrees.to_bits() == offset.to_bits()
    }) else {
        let field = if rows().any(|row| row.level.to_bits() == level.to_bits()) {
            "m_OrientationOffsetY"
        } else {
            "m_Level"
        };
        return Err(AnimError::UnsupportedHumanoidClipSettings(field));
    };
    // E4 played each variant with its motion slots all bound or all
    // unbound, as its fixture marks it, and never an additive clip.
    let bound = MOTION_DOFS
        .filter(|&dof| {
            human
                .slots()
                .any(|slot| slot.dof == dof && slot.storage.is_some())
        })
        .count();
    if !matches!((row.motion, bound), (true, 7) | (false, 0)) {
        return Err(AnimError::UnsupportedHumanoidRootBasis("m_IndexArray"));
    }
    if human.has_reference_pose() {
        return Err(AnimError::UnsupportedAdditivePose);
    }
    let [yaw, height, xz] = [0, 1, 2].map(|k| match (bake.get(k), keep.get(k)) {
        (Some(true), _) => Cell::Bake,
        (_, Some(true)) => Cell::Original,
        _ => Cell::Body,
    });
    Ok(RootSettings {
        admitted: row,
        rules: RootRules { yaw, height, xz },
        start: start(clip, meter)?,
    })
}

/// `m_StartX`: its translation and normalized rotation, E4's `start`.
fn start(clip: &Clip, meter: &Arc<Meter>) -> Result<(Point, Quaternion), AnimError> {
    let dict = Dict {
        parsed: clip.serialized(),
        meter,
    };
    let muscle = dict
        .root("m_MuscleClip")?
        .ok_or(AnimError::UnsupportedHumanoidLayout)?;
    let start = dict.required(muscle, "m_StartX")?;
    let (t, q) = (dict.required(start, "t")?, dict.required(start, "q")?);
    let position = [
        dict.float(t, "x")?,
        dict.float(t, "y")?,
        dict.float(t, "z")?,
    ];
    let rotation = [
        dict.float(q, "x")?,
        dict.float(q, "y")?,
        dict.float(q, "z")?,
        dict.float(q, "w")?,
    ];
    if !position.iter().chain(&rotation).all(|v| v.is_finite()) {
        return refuse(BodyModel::NonFiniteBody);
    }
    Ok((position, normalize(rotation)?))
}

/// E4's `yaw_error` is unbounded for `q` erring by `dq` at `unit`: `(q_y,
/// q_w)` not above twice its movement. `f64::hypot` stands in for
/// `math.hypot` with a margin of four ulps, so it refuses whenever E4 would.
fn unbounded_yaw(q: Quaternion, dq: f64, unit: f64) -> bool {
    let across = q[1].hypot(q[3]);
    let margin = 4. * f64::EPSILON;
    let moved = dq + unit * across * (1. + margin);
    (across * (1. - margin)).partial_cmp(&(2. * moved)) != Some(std::cmp::Ordering::Greater)
}

/// `(0, sin(a / 2), 0, cos(a / 2))`, the reference's `about_up`.
fn about_up(radians: f64) -> Quaternion {
    [0., (radians / 2.).sin(), 0., (radians / 2.).cos()]
}

/// The root `(r, y)` at one sample, the reference's `project`: `body` and
/// `motion` are the sampled body and motion poses.
pub fn project(
    settings: &RootSettings,
    body: (Point, Quaternion),
    motion: Option<(Point, Quaternion)>,
) -> Result<(Point, Quaternion), AnimError> {
    let source = |cell: Cell| match cell {
        Cell::Bake => Ok(settings.start),
        Cell::Original => motion.ok_or(AnimError::UnsupportedHumanoidRootBasis("m_IndexArray")),
        Cell::Body => Ok(body),
    };
    let RootRules { yaw, height, xz } = settings.rules;
    let offset = if yaw == Cell::Bake {
        0.
    } else {
        settings.offset_degrees().to_radians()
    };
    let lift = if height == Cell::Bake {
        0.
    } else {
        settings.level()
    };
    let q = source(yaw)?.1;
    let angle = 2. * q[1].atan2(q[3]) + offset;
    let (xz, height) = (source(xz)?.0, source(height)?.0);
    Ok(([xz[0], height[1] + lift, xz[2]], about_up(angle)))
}

/// `X_A = X_r^-1 o X_b`, the reference's `animator_body`: `T_A` in
/// metres and `B_A`.
pub fn animator_body(
    body: (Point, Quaternion),
    root: (Point, Quaternion),
    human_scale: f64,
) -> Result<AnimatorBodyPose, AnimError> {
    let ((position, rotation), (r, y)) = (body, root);
    let inverse = conjugate(y);
    let d = [position[0] - r[0], position[1] - r[1], position[2] - r[2]];
    let target = rotate(inverse, d).map(|v| human_scale * v);
    let turned = normalize(multiply(inverse, rotation))?;
    if !target.iter().chain(&turned).all(|v| v.is_finite()) {
        return refuse(BodyModel::NonFiniteBody);
    }
    Ok(AnimatorBodyPose {
        translation: target,
        rotation: turned,
    })
}

/// The target body pose `(T_A, B_A)` of 2.4 at the clip time `time`, for
/// the Avatar's `m_Scale`, with the clip's own admitted settings
/// (`root_settings`), E4's `clip_sample` without its bounds: an unbounded
/// yaw refuses.
pub fn target_body_pose(
    clip: &Clip,
    time: f64,
    human_scale: f64,
    meter: &Arc<Meter>,
) -> Result<AnimatorBodyPose, AnimError> {
    let settings = root_settings(clip, meter)?;
    let sampled = root_sample(clip, time, meter)?;
    let Some(body) = sampled.body else {
        return Err(AnimError::UnsupportedHumanoidRootBasis("m_IndexArray"));
    };
    let pair = |pose: SampledPose| (pose.position, pose.rotation);
    let motion = sampled.motion.map(pair);
    let (source, dq) = match settings.rules.yaw {
        Cell::Bake => (settings.start.1, 0.),
        Cell::Original => {
            let motion = sampled
                .motion
                .ok_or(AnimError::UnsupportedHumanoidRootBasis("m_IndexArray"))?;
            (motion.rotation, motion.rotation_error)
        }
        Cell::Body => (body.rotation, body.rotation_error),
    };
    if unbounded_yaw(source, dq, U32) || unbounded_yaw(source, 0., U64) {
        return refuse(BodyModel::UndefinedYaw);
    }
    let root = project(&settings, pair(body), motion)?;
    animator_body(pair(body), root, human_scale)
}

const _: () = crate::send_sync::<Cell>();
const _: () = crate::send_sync::<RootRules>();
const _: () = crate::send_sync::<AdmittedRoot>();
const _: () = crate::send_sync::<RootSettings>();
