//! E4's sampling of a built humanoid clip's root slots
//! (`tools/parity/anim_body_e4.py`: `runtime`, `sample` and `pose_at`):
//! the body (`RootT`, `RootQ`: DOFs 7-13) and the motion (`MotionT`,
//! `MotionQ`: DOFs 0-6) through `m_IndexArray`, streamed, dense or
//! constant, each with E4's bound on the error of Unity's f32 sample.
//!
//! A slot is sampled only through E4's exact binding key (path 0, class
//! 95, its DOF as `attribute`, `customType` 8, not a discrete curve);
//! another key refuses (`unsupported_humanoid_root_basis
//! (m_ClipBindingConstant)`), as do partly bound slots
//! (`(m_IndexArray)`). A non-finite sample or error refuses
//! (`non_finite_body`), and so does a streamed key or dense interpolation
//! whose f32 evaluation, as Unity runs it, overflows at any step, though
//! the f64 value is finite. A rotation E4 cannot normalize (a norm not
//! above four times its error) refuses (`degenerate_root_rotation`).
use super::{
    body_model::{Point, Quaternion, refuse},
    clip::{HumanoidClip, exact_bindings, stream_kinds},
};
use crate::{
    AnimError, BodyModel, ClipTiming, StorageKind,
    clip::{Clip, StreamSlot, decode_stream},
    read::Dict,
};
use std::sync::Arc;
use unityrs_core::{
    limits::{Meter, MeteredVec},
    value::Value,
};
use unityrs_pysum::python_sum;

/// Freeze v8's `bounds.unit_roundoff_f32`, E4's `U32`.
pub const U32: f64 = 1. / 16_777_216.;

/// `n u / (1 - n u)`, the reference's `gamma`.
pub(super) fn gamma(n: f64, unit: f64) -> f64 {
    n * unit / (1. - n * unit)
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
const ROOT_DOFS: [usize; 14] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];

pub(super) fn humanoid(clip: &Clip) -> Result<&HumanoidClip, AnimError> {
    clip.humanoid().ok_or(AnimError::UnsupportedHumanoidLayout)
}

/// A quaternion over the square root of its sum of squares (the
/// reference's `math.hypot`, within an ulp); a zero or non-finite length
/// refuses.
pub(super) fn normalize(q: Quaternion) -> Result<Quaternion, AnimError> {
    let length = norm(&q);
    if !(length.is_finite() && length > 0.) {
        return refuse(BodyModel::DegenerateRootRotation);
    }
    Ok(q.map(|v| v / length))
}

/// `sqrt(sum(c * c))`, the reference's `norm`.
pub(super) fn norm(values: &[f64]) -> f64 {
    python_sum(values.iter().map(|c| c * c)).sqrt()
}

fn finite(values: &[f64]) -> Result<(), AnimError> {
    if values.iter().all(|v| v.is_finite()) {
        Ok(())
    } else {
        refuse(BodyModel::NonFiniteBody)
    }
}

/// The decoded storage a humanoid clip's root slots are sampled from:
/// the muscle clip's clock, its streamed slots (when a root DOF is
/// streamed), its dense grid and its constant words.
struct Storage<'a> {
    timing: ClipTiming,
    streams: MeteredVec<StreamSlot>,
    dense: Dense<'a>,
    constants: &'a [u32],
}

/// The dense grid: curves, frames, begin, rate and the frame-major words.
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
    let counts = human.counts();
    if exact_bindings(&dict, human.index(), counts, ROOT_DOFS)?.contains(&false) {
        return Err(AnimError::UnsupportedHumanoidRootBasis(
            "m_ClipBindingConstant",
        ));
    }
    let data = dict.required(muscle, "m_Clip")?;
    let data = dict.field(data, "data")?.unwrap_or(data);
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

impl Storage<'_> {
    /// One slot at `time` as playback samples it, and E4's bound on the
    /// error of Unity's f32 sample (`sample`).
    fn sample(
        &self,
        storage: (StorageKind, usize),
        time: f64,
        meter: &Meter,
    ) -> Result<(f64, f64), AnimError> {
        let (value, error) = match storage {
            (StorageKind::Streamed, slot) => self.streamed(slot, time, meter)?,
            (StorageKind::Dense, slot) => self.dense.sample(slot, time, meter)?,
            (StorageKind::Constant, slot) => {
                meter.count_decode(1)?;
                let word = self.constants.get(slot).ok_or(AnimError::SlotOverflow)?;
                (f64::from(f32::from_bits(*word)), 0.)
            }
        };
        finite(&[value, error])?;
        Ok((value, error))
    }

    fn streamed(&self, slot: usize, time: f64, meter: &Meter) -> Result<(f64, f64), AnimError> {
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
        let [c3, c2, c1, c0] = key.coefficients.map(f32::from_bits);
        // Unity's f32 Horner, in E4's order: the clock rounds to f32, then
        // the key's time is subtracted in f32. Every step must stay finite.
        #[allow(clippy::cast_possible_truncation, reason = "Unity's f32 clock")]
        let (clock, start) = (time as f32, key.time as f32);
        let t = clock - start;
        if !t.is_finite() {
            return refuse(BodyModel::NonFiniteBody);
        }
        let mut horner = c3;
        for c in [c2, c1, c0] {
            let product = horner * t;
            horner = product + c;
            if !(product.is_finite() && horner.is_finite()) {
                return refuse(BodyModel::NonFiniteBody);
            }
        }
        let [c3, c2, c1, c0] = [c3, c2, c1, c0].map(f64::from);
        let dt = time - key.time;
        let magnitude = ((c3.abs() * dt + c2.abs()) * dt + c1.abs()) * dt + c0.abs();
        let slope = ((3. * c3 * dt + 2. * c2) * dt + c1).abs();
        Ok((
            value,
            gamma(6., U32) * magnitude + slope * U32 * (time.abs() + key.time.abs()),
        ))
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
                .map(|word| f32::from_bits(*word))
                .ok_or(AnimError::InvalidDenseGrid)
        };
        if frames == 1 {
            return Ok((f64::from(value(0)?), 0.));
        }
        // The decoded grid's times, as `Dense::at` takes them.
        let frame_time = |frame: u32| self.begin + f64::from(frame) / self.rate;
        let next = (0..frames)
            .take_while(|&frame| frame_time(frame) <= time)
            .count();
        let i = u32::try_from(next.saturating_sub(1)).map_err(|_| AnimError::InvalidDenseGrid)?;
        let first = value(i)?;
        let sampled = if i == frames - 1 || time <= frame_time(i) {
            f64::from(first)
        } else {
            let second = value(i + 1)?;
            let fraction = ((time - frame_time(i)) * self.rate).min(1.);
            // Unity's f32 interpolation: every step must stay finite.
            #[allow(clippy::cast_possible_truncation, reason = "Unity's f32 weight")]
            let weight = fraction as f32;
            let step = (second - first) * weight;
            if !((second - first).is_finite() && step.is_finite() && (first + step).is_finite()) {
                return refuse(BodyModel::NonFiniteBody);
            }
            let (first, second) = (f64::from(first), f64::from(second));
            first + (second - first) * fraction
        };
        // E4's frame for the bound: floor((time - begin) rate), clamped.
        let floor = ((time - self.begin) * self.rate).floor();
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped to the grid's frames"
        )]
        let k = floor.clamp(0., f64::from(frames - 2)) as u32;
        let (a, b) = (f64::from(value(k)?), f64::from(value(k + 1)?));
        let error = gamma(4., U32) * (a.abs() + b.abs())
            + (b - a).abs() * gamma(4., U32) * (1. + (time - self.begin).abs() * self.rate);
        Ok((sampled, error))
    }
}

/// The root slots at the clip time `time`, E4's `pose_at` for the body and
/// the motion: a translation with the norm of its slots' errors, a
/// rotation normalized with E4's bound `2 e / (|q| - e)`. The module
/// documents the refusals.
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
        let error = norm(errors);
        finite(&[error])?;
        Ok(Some((values, error)))
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
                let pose = SampledPose {
                    position: [t[0], t[1], t[2]],
                    position_error,
                    rotation: normalize(q)?,
                    rotation_error: 2. * error / (size - error),
                };
                finite(&pose.position)?;
                finite(&pose.rotation)?;
                finite(&[pose.rotation_error])?;
                Ok(Some(pose))
            }
            _ => Err(AnimError::UnsupportedHumanoidRootBasis("m_IndexArray")),
        }
    };
    Ok(RootSample {
        body: pose(&BODY_T, &BODY_Q)?,
        motion: pose(&MOTION_T, &MOTION_Q)?,
    })
}

const _: () = crate::send_sync::<SampledPose>();
const _: () = crate::send_sync::<RootSample>();
