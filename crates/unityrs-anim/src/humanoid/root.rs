//! Freeze v8's root projection (`docs/plans/humanoid-body.md` 2.4): the
//! target body pose `(T_A, B_A)` in the Animator node's frame from a built
//! humanoid clip, transcribed from the f64 reference's `cells`, `project`
//! and `animator_body` (`tools/parity/anim_body_model.py`) and E4's
//! sampling (`tools/parity/anim_body_e4.py`: `sample`, `pose_at`,
//! `built_pose` and `clip_sample`), in their operation order.
//!
//! - **Inputs.** The runtime body slots (`RootT`, `RootQ`: DOFs 7-13
//!   through `m_IndexArray`) and motion slots (`MotionT`, `MotionQ`: DOFs
//!   0-6) at the clip time, each with E4's bound on its f32 sample's error;
//!   the serialized start `m_StartX` (the bake cells' start value, as E4
//!   validated it, never a sampled `RootT`); `m_Level` and
//!   `m_OrientationOffsetY`.
//! - **The root.** Per cell (yaw, height, XZ): bake takes the start,
//!   keep-original the motion slots, otherwise the body. The yaw is the
//!   twist about up, `2 atan2(q_y, q_w)`, plus the orientation offset unless
//!   baked; the height adds the level unless baked.
//! - **The body in `A`.** `T_A = h y^-1 (RootT - r)`, `B_A = y^-1 RootQ`.
//! - **No body slots.** The plan's `T_A = 0` and identity `B_A` refuses
//!   (`unsupported_humanoid_root_basis(m_IndexArray)`): E4 compares only
//!   clips with body slots.
//!
//! Only E4's passing configurations (freeze v8's `e4.passing`) are
//! admitted; every other refuses before any sample (`root_settings`):
//! - `m_StartAtOrigin` off, or Based Upon Feet on
//!   (`unsupported_humanoid_root_basis`): the lowest-foot rule is
//!   undecided;
//! - an Original cell beside a bake or body cell
//!   (`unsupported_humanoid_root_basis`, the first Original flag): only all
//!   three Original, with motion slots, was recorded;
//! - a level or orientation offset other than alone on all-body cells, a
//!   mirror, a cycle offset or loop flags (`unsupported_humanoid_clip_settings`).
//!
//! An all-Original clip without motion slots refuses
//! (`unsupported_humanoid_root_basis(m_IndexArray)`), as do partly bound
//! body or motion slots. A rotation E4 cannot normalize (a norm not above
//! four times its sample's error) refuses (`degenerate_root_rotation`), and
//! so does a yaw whose error E4 leaves unbounded, at f32 with its source's
//! error or at f64 (`undefined_yaw`).
//!
//! Departures, each refusing where it would answer: a quaternion is
//! normalized by the square root of its sum of squares (the reference's
//! `math.hypot`, within an ulp), and the yaw's bound compares `f64::hypot`
//! with a margin of four ulps, so that an ulp of `math.hypot` cannot admit
//! a yaw E4 refuses. A non-finite input or result refuses
//! (`non_finite_body`).
use super::{
    body_model::{Point, Quaternion, multiply, refuse},
    body_solve::{conjugate, rotate},
    clip::{HumanoidClip, SettingValue, stream_kinds},
};
use crate::clip::StreamSlot;
use crate::{
    AnimError, BodyModel, ClipTiming, StorageKind,
    clip::{Clip, decode_stream},
    read::Dict,
};
use std::sync::Arc;
use unityrs_core::{
    limits::{Meter, MeteredVec},
    value::Value,
};
use unityrs_pysum::python_sum;

/// f32's unit roundoff, E4's `U32`.
const U32: f64 = 1. / 16_777_216.;
/// f64's unit roundoff, E4's `U64`.
const U64: f64 = f64::EPSILON / 2.;

/// `n u / (1 - n u)`, the reference's `gamma`.
fn gamma(n: f64, unit: f64) -> f64 {
    n * unit / (1. - n * unit)
}

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

/// A clip's admitted root settings: its rules, the start `m_StartX`, the
/// level and the orientation offset in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RootSettings {
    pub rules: RootRules,
    pub start: (Point, Quaternion),
    pub level: f64,
    pub offset_degrees: f64,
}

/// A body or motion pose sampled from the runtime slots, with E4's bounds
/// on the errors of its f32 samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampledPose {
    pub position: Point,
    pub position_error: f64,
    pub rotation: Quaternion,
    pub rotation_error: f64,
}

/// The root slots at one clip time: the body (`RootT`, `RootQ`) and the
/// motion (`MotionT`, `MotionQ`), each `None` when unbound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RootSample {
    pub body: Option<SampledPose>,
    pub motion: Option<SampledPose>,
}

const BODY_T: [usize; 3] = [7, 8, 9];
const BODY_Q: [usize; 4] = [10, 11, 12, 13];
const MOTION_T: [usize; 3] = [0, 1, 2];
const MOTION_Q: [usize; 4] = [3, 4, 5, 6];

fn humanoid(clip: &Clip) -> Result<&HumanoidClip, AnimError> {
    clip.humanoid().ok_or(AnimError::UnsupportedHumanoidLayout)
}

/// The clip's root settings if freeze v8's E4 passed their configuration,
/// else the typed refusal (the module documents which).
pub fn root_settings(clip: &Clip, meter: &Arc<Meter>) -> Result<RootSettings, AnimError> {
    let settings = humanoid(clip)?.settings();
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
        Some(SettingValue::Float(x)) if x.is_finite() => Ok(f64::from(x)),
        _ => Err(AnimError::UnsupportedHumanoidClipSettings(field)),
    };
    for field in ["m_Mirror", "m_LoopTime", "m_LoopBlend"] {
        if flag(field)? {
            return Err(AnimError::UnsupportedHumanoidClipSettings(field));
        }
    }
    if float("m_CycleOffset")? != 0. {
        return Err(AnimError::UnsupportedHumanoidClipSettings("m_CycleOffset"));
    }
    if !flag("m_StartAtOrigin")? {
        return Err(AnimError::UnsupportedHumanoidRootBasis("m_StartAtOrigin"));
    }
    if flag("m_HeightFromFeet")? {
        return Err(AnimError::UnsupportedHumanoidRootBasis("m_HeightFromFeet"));
    }
    let cell = |bake: &'static str, keep: &'static str| -> Result<Cell, AnimError> {
        Ok(if flag(bake)? {
            Cell::Bake
        } else if flag(keep)? {
            Cell::Original
        } else {
            Cell::Body
        })
    };
    let rules = RootRules {
        yaw: cell("m_LoopBlendOrientation", "m_KeepOriginalOrientation")?,
        height: cell("m_LoopBlendPositionY", "m_KeepOriginalPositionY")?,
        xz: cell("m_LoopBlendPositionXZ", "m_KeepOriginalPositionXZ")?,
    };
    let cells = [rules.yaw, rules.height, rules.xz];
    let originals = [
        "m_KeepOriginalOrientation",
        "m_KeepOriginalPositionY",
        "m_KeepOriginalPositionXZ",
    ];
    if cells.contains(&Cell::Original) && cells != [Cell::Original; 3] {
        let first = cells
            .iter()
            .zip(originals)
            .find_map(|(&cell, field)| (cell == Cell::Original).then_some(field));
        return Err(AnimError::UnsupportedHumanoidRootBasis(
            first.unwrap_or(originals[0]),
        ));
    }
    let (level, offset_degrees) = (float("m_Level")?, float("m_OrientationOffsetY")?);
    let all_body = cells == [Cell::Body; 3];
    if level != 0. && !(all_body && offset_degrees == 0.) {
        return Err(AnimError::UnsupportedHumanoidClipSettings("m_Level"));
    }
    if offset_degrees != 0. && !(all_body && level == 0.) {
        return Err(AnimError::UnsupportedHumanoidClipSettings(
            "m_OrientationOffsetY",
        ));
    }
    Ok(RootSettings {
        rules,
        start: start(clip, meter)?,
        level,
        offset_degrees,
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

/// A quaternion over the square root of its sum of squares (the
/// reference's `math.hypot`, within an ulp); a zero or non-finite length
/// refuses.
fn normalize(q: Quaternion) -> Result<Quaternion, AnimError> {
    let length = norm(&q);
    if !(length.is_finite() && length > 0.) {
        return refuse(BodyModel::DegenerateRootRotation);
    }
    Ok(q.map(|v| v / length))
}

/// `sqrt(sum(c * c))`, the reference's `norm`.
fn norm(values: &[f64]) -> f64 {
    python_sum(values.iter().map(|c| c * c)).sqrt()
}

/// The decoded storage a humanoid clip's root slots are sampled from:
/// the muscle clip's clock, its streamed slots (when a root DOF is
/// streamed), its dense grid (curves, frames, begin, rate and the frame-
/// major words) and its constant words.
struct Storage<'a> {
    timing: ClipTiming,
    streams: MeteredVec<StreamSlot>,
    dense: Dense<'a>,
    constants: &'a [u32],
}

struct Dense<'a> {
    curves: usize,
    frames: usize,
    begin: f64,
    rate: f64,
    words: &'a [u32],
}

/// The muscle clip's interval, `m_StartTime` to `m_StopTime`, at the
/// clip's `m_SampleRate`: the times `root_sample` takes.
pub fn root_timing(clip: &Clip, meter: &Arc<Meter>) -> Result<ClipTiming, AnimError> {
    let dict = Dict {
        parsed: clip.serialized(),
        meter,
    };
    let muscle = dict
        .root("m_MuscleClip")?
        .ok_or(AnimError::UnsupportedHumanoidLayout)?;
    timing(&dict, muscle)
}

fn timing(dict: &Dict<'_, '_>, muscle: &Value) -> Result<ClipTiming, AnimError> {
    let rate = dict.float(dict.parsed.value(), "m_SampleRate")?;
    ClipTiming::new(
        dict.float(muscle, "m_StartTime")?,
        dict.float(muscle, "m_StopTime")?,
        rate,
    )
}

fn storage<'a>(
    clip: &'a Clip,
    human: &HumanoidClip,
    meter: &'a Arc<Meter>,
) -> Result<Storage<'a>, AnimError> {
    let dict = Dict {
        parsed: clip.serialized(),
        meter,
    };
    let muscle = dict
        .root("m_MuscleClip")?
        .ok_or(AnimError::UnsupportedHumanoidLayout)?;
    let timing = timing(&dict, muscle)?;
    let data = dict.required(muscle, "m_Clip")?;
    let data = dict.field(data, "data")?.unwrap_or(data);
    let counts = human.counts();
    let streamed = ROOT_DOFS.iter().any(|&dof| {
        human
            .slots()
            .any(|slot| slot.dof == dof && matches!(slot.storage, Some((StorageKind::Streamed, _))))
    });
    let streams = if streamed {
        let words = dict.words(dict.required(data, "m_StreamedClip")?, "data", false)?;
        let kinds = stream_kinds(&dict, human.index(), counts)?;
        decode_stream(&dict, counts.streamed, words, kinds.as_slice(), &[])?
    } else {
        MeteredVec::new(meter)
    };
    let grid = dict.required(data, "m_DenseClip")?;
    let dense = Dense {
        curves: counts.dense,
        frames: dict.count(grid, "m_FrameCount")?,
        begin: dict.float(grid, "m_BeginTime")?,
        rate: dict.float(grid, "m_SampleRate")?,
        words: dict.words(grid, "m_SampleArray", true)?,
    };
    let constants = match dict.field(data, "m_ConstantClip")? {
        None => &[][..],
        Some(constant) => dict.words(constant, "data", true)?,
    };
    Ok(Storage {
        timing,
        streams,
        dense,
        constants,
    })
}

const ROOT_DOFS: [usize; 14] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];

impl Storage<'_> {
    /// One slot at `time` as playback samples it, and E4's bound on the
    /// error of Unity's f32 sample (`sample`).
    fn sample(
        &self,
        storage: (StorageKind, usize),
        time: f64,
        meter: &Meter,
    ) -> Result<(f64, f64), AnimError> {
        match storage {
            (StorageKind::Streamed, slot) => {
                let stream = self
                    .streams
                    .as_slice()
                    .get(slot)
                    .ok_or(AnimError::SlotOverflow)?;
                let value = stream.at(slot, time, meter)?.continuous()?;
                let keys = stream.keys();
                let next = keys.partition_point(|key| key.time <= time);
                let Some(key) = next
                    .checked_sub(1)
                    .filter(|&i| i + 1 < keys.len())
                    .and_then(|i| keys.get(i))
                else {
                    return Ok((value, 0.));
                };
                let [c3, c2, c1, c0] = key.coefficients.map(|x| f64::from(f32::from_bits(x)));
                let dt = time - key.time;
                let magnitude = ((c3.abs() * dt + c2.abs()) * dt + c1.abs()) * dt + c0.abs();
                let slope = ((3. * c3 * dt + 2. * c2) * dt + c1).abs();
                Ok((
                    value,
                    gamma(6., U32) * magnitude + slope * U32 * (time.abs() + key.time.abs()),
                ))
            }
            (StorageKind::Dense, slot) => self.dense.sample(slot, time, meter),
            (StorageKind::Constant, slot) => {
                meter.count_decode(1)?;
                let word = self.constants.get(slot).ok_or(AnimError::SlotOverflow)?;
                Ok((f64::from(f32::from_bits(*word)), 0.))
            }
        }
    }
}

impl Dense<'_> {
    /// The reference's `dense` for one slot, and E4's error bound.
    fn sample(&self, slot: usize, time: f64, meter: &Meter) -> Result<(f64, f64), AnimError> {
        meter.count_decode(self.frames as u64)?;
        if !(slot < self.curves
            && self.frames > 0
            && self.curves.checked_mul(self.frames) == Some(self.words.len())
            && self.begin.is_finite()
            && self.rate.is_finite()
            && self.rate > 0.)
        {
            return Err(AnimError::InvalidDenseGrid);
        }
        let frames = u32::try_from(self.frames).map_err(|_| AnimError::InvalidDenseGrid)?;
        let value = |frame: u32| {
            usize::try_from(frame)
                .ok()
                .and_then(|frame| frame.checked_mul(self.curves)?.checked_add(slot))
                .and_then(|index| self.words.get(index))
                .map(|word| f64::from(f32::from_bits(*word)))
                .ok_or(AnimError::InvalidDenseGrid)
        };
        if frames == 1 {
            return Ok((value(0)?, 0.));
        }
        // The decoded grid's times, as `Dense::at` takes them.
        let frame_time = |frame: u32| self.begin + f64::from(frame) / self.rate;
        let next = (0..frames)
            .take_while(|&frame| frame_time(frame) <= time)
            .count();
        let i = u32::try_from(next.saturating_sub(1)).map_err(|_| AnimError::InvalidDenseGrid)?;
        let first = value(i)?;
        let sampled = if i == frames - 1 || time <= frame_time(i) {
            first
        } else {
            let second = value(i + 1)?;
            first + (second - first) * ((time - frame_time(i)) * self.rate).min(1.)
        };
        // E4's frame for the bound: floor((time - begin) rate), clamped.
        let floor = ((time - self.begin) * self.rate).floor();
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped to the grid's frames"
        )]
        let k = floor.clamp(0., f64::from(frames - 2)) as u32;
        let (a, b) = (value(k)?, value(k + 1)?);
        let error = gamma(4., U32) * (a.abs() + b.abs())
            + (b - a).abs() * gamma(4., U32) * (1. + (time - self.begin).abs() * self.rate);
        Ok((sampled, error))
    }
}

/// The root slots at the clip time `time`, E4's `pose_at` for the body and
/// the motion: a translation with the norm of its slots' errors, a
/// rotation normalized with E4's bound `2 e / (|q| - e)`. Partly bound
/// slots refuse, as does a rotation not above four times its error.
pub fn root_sample(clip: &Clip, time: f64, meter: &Arc<Meter>) -> Result<RootSample, AnimError> {
    let human = humanoid(clip)?;
    let storage = storage(clip, human, meter)?;
    if !storage.timing.contains(time) {
        return Err(AnimError::InvalidSchedule);
    }
    let slot_of = |dof: usize| {
        human
            .slots()
            .find(|slot| slot.dof == dof)
            .and_then(|slot| slot.storage)
    };
    let group = |dofs: &[usize]| -> Result<Option<([f64; 4], f64)>, AnimError> {
        if dofs.iter().all(|&dof| slot_of(dof).is_none()) {
            return Ok(None);
        }
        let mut values = [0.; 4];
        let mut errors = [0.; 4];
        for ((&dof, value), error) in dofs.iter().zip(&mut values).zip(&mut errors) {
            let slot =
                slot_of(dof).ok_or(AnimError::UnsupportedHumanoidRootBasis("m_IndexArray"))?;
            (*value, *error) = storage.sample(slot, time, meter)?;
        }
        let errors = errors.get(..dofs.len()).unwrap_or(&errors);
        Ok(Some((values, norm(errors))))
    };
    let pose = |t: &[usize], q: &[usize]| -> Result<Option<SampledPose>, AnimError> {
        match (group(t)?, group(q)?) {
            (None, None) => Ok(None),
            (Some((t, position_error)), Some((q, error))) => {
                let size = norm(&q);
                // Not above four times its error, or NaN: E4 cannot normalize it.
                if size.partial_cmp(&(4. * error)) != Some(std::cmp::Ordering::Greater) {
                    return refuse(BodyModel::DegenerateRootRotation);
                }
                Ok(Some(SampledPose {
                    position: [t[0], t[1], t[2]],
                    position_error,
                    rotation: normalize(q)?,
                    rotation_error: 2. * error / (size - error),
                }))
            }
            _ => Err(AnimError::UnsupportedHumanoidRootBasis("m_IndexArray")),
        }
    };
    Ok(RootSample {
        body: pose(&BODY_T, &BODY_Q)?,
        motion: pose(&MOTION_T, &MOTION_Q)?,
    })
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
        settings.offset_degrees.to_radians()
    };
    let lift = if height == Cell::Bake {
        0.
    } else {
        settings.level
    };
    let q = source(yaw)?.1;
    let angle = 2. * q[1].atan2(q[3]) + offset;
    let (xz, height) = (source(xz)?.0, source(height)?.0);
    Ok(([xz[0], height[1] + lift, xz[2]], about_up(angle)))
}

/// `X_A = X_r^-1 o X_b`, the reference's `animator_body`: `(T_A in metres,
/// B_A)`.
pub fn animator_body(
    body: (Point, Quaternion),
    root: (Point, Quaternion),
    human_scale: f64,
) -> Result<(Point, Quaternion), AnimError> {
    let ((position, rotation), (r, y)) = (body, root);
    let inverse = conjugate(y);
    let d = [position[0] - r[0], position[1] - r[1], position[2] - r[2]];
    let target = rotate(inverse, d).map(|v| human_scale * v);
    let turned = normalize(multiply(inverse, rotation))?;
    if !target.iter().chain(&turned).all(|v| v.is_finite()) {
        return refuse(BodyModel::NonFiniteBody);
    }
    Ok((target, turned))
}

/// The target body pose `(T_A, B_A)` of 2.4 at the clip time `time`, for
/// an admitted clip (`root_settings`) and the Avatar's `m_Scale`, E4's
/// `clip_sample` without its bounds: an unbounded yaw refuses.
pub fn target_body_pose(
    clip: &Clip,
    settings: &RootSettings,
    time: f64,
    human_scale: f64,
    meter: &Arc<Meter>,
) -> Result<(Point, Quaternion), AnimError> {
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
    let root = project(settings, pair(body), motion)?;
    animator_body(pair(body), root, human_scale)
}

const _: () = crate::send_sync::<Cell>();
const _: () = crate::send_sync::<RootRules>();
const _: () = crate::send_sync::<RootSettings>();
const _: () = crate::send_sync::<SampledPose>();
const _: () = crate::send_sync::<RootSample>();
