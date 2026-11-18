//! Bounded source-space subdivision followed by a single f32 quantization.
mod bounds;

use super::{
    AnimError, Arc, CollectionTargets, ExportError, Meter, MeteredVec, Scene, clip_field, same,
};
use unityrs_anim::{BindingTarget, BoundClip, BoundTrack, TransformProperty};
use unityrs_gltf::{
    Accessor, Animation, AnimationChannel, AnimationSampler, AnimationTarget, CollectionIssue,
    Data, Interpolation, Kind, Node,
};
use unityrs_scene::Graph;

type Result<T> = std::result::Result<T, ExportError>;
const MAX_KEYS: usize = 65_536;
const MAX_DEPTH: u32 = 20;
const MAX_TIME: f64 = 1024.;
const MAX_COMPONENT: f64 = 1_048_576.;

#[derive(Clone, Copy, Debug)]
struct Key {
    source: f64,
    output: f32,
    left: bool,
    sided: bool,
}

struct Channel<'a, 'c, 'r> {
    target: AnimationTarget,
    tracks: &'a [BoundTrack<'c, 'r>],
    rest: &'a Node,
    width: usize,
    start: f64,
    step: bool,
    meter: &'a Arc<Meter>,
}

struct Baked {
    times: MeteredVec<f64>,
    values: MeteredVec<f64>,
    sided: bool,
}

fn family(target: BindingTarget<'_>) -> (usize, AnimationTarget) {
    use TransformProperty as T;
    match target {
        BindingTarget::Transform { node, property } => (
            node,
            match property {
                T::Translation => AnimationTarget::Translation,
                T::Quaternion | T::Euler => AnimationTarget::Rotation,
                T::Scale => AnimationTarget::Scale,
            },
        ),
        BindingTarget::Morph { node, .. } => (node, AnimationTarget::Weights),
        BindingTarget::Metadata => (usize::MAX, AnimationTarget::Weights),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "construct an animation and its emitted tables together"
)]
pub(super) fn animation(
    scene: &mut Scene,
    targets: CollectionTargets<'_>,
    graph: &Graph,
    bound: &BoundClip<'_, '_>,
    rate: Option<f64>,
    meter: &Arc<Meter>,
) -> Result<()> {
    if bound.tracks().is_empty() {
        return Ok(());
    }
    let timing = bound.timing().ok_or(AnimError::InvalidClipInterval)?;
    let start = timing.start();
    let stop = timing.stop();
    let duration = stop - start;
    if duration > MAX_TIME || (duration > 0. && round(duration)? == 0.) {
        return Err(ExportError::Bake("clip duration"));
    }

    let mut animation = Animation::new(meter);
    if let Some(name) =
        clip_field(bound.clip(), b"m_Name", meter)?.and_then(unityrs_core::value::Value::as_str)
    {
        animation.name = Some(unityrs_gltf::text(meter, name)?);
    }
    if let Some(wrap) =
        clip_field(bound.clip(), b"m_WrapMode", meter)?.and_then(unityrs_core::value::Value::as_i64)
    {
        // The temporary decimal representation fits in 20 bytes.
        let formatted = wrap.to_string();
        animation.extras.push((
            unityrs_gltf::text(meter, "unity_wrap_mode")?,
            unityrs_gltf::text(meter, &formatted)?,
        ))?;
    }
    for tracks in bound
        .tracks()
        .chunk_by(|a, b| family(a.target) == family(b.target))
    {
        meter.count_decode(tracks.len() as u64)?;
        let first = tracks.first().ok_or(ExportError::Bake("empty target"))?;
        let (source_node, target) = family(first.target);
        let mut node = targets
            .transforms
            .get(source_node)
            .copied()
            .flatten()
            .ok_or(AnimError::FilteredTarget(source_node))?;
        let mut approximations = 0;
        if let BindingTarget::Morph { renderer, .. } = first.target {
            let mut attachment = None;
            for row in targets.renderers {
                if same(row.renderer, renderer, meter)? {
                    attachment = Some(row);
                    break;
                }
            }
            let row = attachment.ok_or(AnimError::MissingAttachment(source_node))?;
            node = row.attachment;
            for track in tracks {
                if let BindingTarget::Morph { channel, .. } = track.target
                    && row
                        .channels
                        .as_slice()
                        .get(channel.target_slot)
                        .is_some_and(|c| c.frame_count > 1)
                {
                    approximations += 1;
                }
            }
        }
        let rest = scene
            .nodes
            .as_slice()
            .get(node)
            .ok_or(AnimError::MissingPath)?;
        if target == AnimationTarget::Rotation {
            let local = &graph
                .nodes()
                .get(source_node)
                .ok_or(AnimError::MissingPath)?
                .local;
            if local.rotation.iter().any(|x| !x.is_finite())
                || local.rotation.iter().all(|&x| x == 0.)
            {
                return Err(AnimError::InvalidRestPose(source_node).into());
            }
        }
        let (kind, width) = match target {
            AnimationTarget::Rotation => (Kind::AnimationRotation, 4),
            AnimationTarget::Weights => (Kind::AnimationWeights, rest.weights.len()),
            _ => (Kind::AnimationVec3, 3),
        };
        let mut channel = Channel {
            target,
            tracks,
            rest,
            width,
            start,
            step: true,
            meter,
        };
        let baked = channel.bake(stop, rate.unwrap_or(timing.rate()))?;
        let interpolation = if channel.step {
            Interpolation::Step
        } else {
            Interpolation::Linear
        };
        let count = baked.times.len();
        let output_count = if target == AnimationTarget::Weights {
            baked.values.len()
        } else {
            count
        };
        let input = scene.accessors.len();
        scene.accessors.push(Accessor {
            kind: Kind::AnimationTime,
            count,
            data: Data::Float(baked.times),
            sparse_indices: None,
        })?;
        let output = scene.accessors.len();
        scene.accessors.push(Accessor {
            kind,
            count: output_count,
            data: Data::Float(baked.values),
            sparse_indices: None,
        })?;
        let sampler = animation.samplers.len();
        animation.samplers.push(AnimationSampler {
            input,
            output,
            interpolation,
        })?;
        animation.channels.push(AnimationChannel {
            sampler,
            node,
            target,
        })?;
        if approximations != 0 {
            scene.collection_issues.push(CollectionIssue {
                resource: None,
                node,
                reason: "animation_morph_last_frame_approximation",
                count: approximations,
            })?;
        }
    }
    scene.animations.push(animation)?;
    Ok(())
}

#[allow(clippy::float_cmp)]
impl Channel<'_, '_, '_> {
    #[allow(
        clippy::too_many_lines,
        reason = "process continuous spans using adjacent jump brackets"
    )]
    fn bake(&mut self, stop: f64, rate: f64) -> Result<Baked> {
        let mut times = MeteredVec::new(self.meter);
        let (start, duration) = (self.start, stop - self.start);
        times.push(start)?;
        if duration > 0. {
            times.push(stop)?;
            let steps = (duration * rate).ceil();
            if steps > f64::from(u16::MAX) {
                return Err(ExportError::Bake("schedule key cap"));
            }
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            for index in 1..steps as u32 {
                self.meter.count_decode(1)?;
                let time = start + f64::from(index) / rate;
                if start < time && time < stop {
                    times.push(time)?;
                }
            }
        }
        for track in self.tracks {
            track.visit_key_times(|time| {
                self.meter.count_decode(1)?;
                if start < time && time < stop {
                    times.push(time)?;
                }
                Ok(())
            })?;
        }
        self.meter.count_decode(
            (times.len() as u64).saturating_mul(u64::from(times.len().bit_width())),
        )?;
        times.as_mut_slice().sort_unstable_by(f64::total_cmp);
        // Compact in place before the cap, as components often share schedules.
        let mut retained = 0;
        let mut previous = None;
        for time in times.as_slice() {
            if previous != Some(*time) {
                previous = Some(*time);
                retained += 1;
            }
        }
        if retained > MAX_KEYS {
            return Err(ExportError::Bake("source key cap"));
        }
        let mut keys = MeteredVec::with_capacity(self.meter, retained)?;
        for &source in times.as_slice() {
            if keys
                .as_slice()
                .last()
                .is_none_or(|key: &Key| key.source != source)
            {
                keys.push(Key {
                    source,
                    output: round(source - start)?,
                    left: false,
                    sided: false,
                })?;
            }
        }
        drop(times);

        for pair in keys.as_slice().windows(2) {
            let [left, _] = pair else { continue };
            let mid = left.source;
            for track in self.tracks {
                for axis in 0..track.width() {
                    if !track.segment(axis, mid, false, self.meter)?.is_held() {
                        self.step = false;
                    }
                }
            }
        }
        let mut baked = Baked {
            times: MeteredVec::new(self.meter),
            values: MeteredVec::new(self.meter),
            sided: false,
        };
        let mut left = *keys
            .as_slice()
            .first()
            .ok_or(ExportError::Bake("missing endpoints"))?;
        self.append(&mut baked, left)?;
        let mut previous_window = None;
        for (index, &right) in keys.as_slice().iter().enumerate().skip(1) {
            let mut limit = Key {
                left: true,
                ..right
            };
            let jump = self.jump(limit, right)?;
            if jump {
                let b = f64::from(right.output);
                let j = right.source - start;
                let previous = f64::from(left.output);
                let next = keys.as_slice().get(index + 1);
                if previous >= b || next.is_some_and(|key| key.output <= right.output) {
                    return Err(ExportError::Bake("rounded jump bracket"));
                }
                let a = if self.step {
                    right.output
                } else {
                    right.output.next_down()
                };
                let gap = if j == b {
                    0.
                } else {
                    let toward = if j < b {
                        right.output.next_down()
                    } else {
                        right.output.next_up()
                    };
                    (f64::from(toward) - b).abs()
                };
                let a_time = f64::from(a);
                if (b - j).abs() > gap * 0.5
                    || (a_time - j).abs() > (b - a_time) + gap * 0.5
                    || !self.distinct_rounded_sides(limit, right)?
                {
                    return Err(ExportError::Bake("unrepresentable held sides"));
                }
                let span = (f64::from(a).min(j), b.max(j));
                if span.0 < 0.
                    || span.1 > duration.max(f64::from(round(duration)?))
                    || previous >= span.0
                    || previous_window.is_some_and(|end| end >= span.0)
                    || next.is_some_and(|key| f64::from(key.output) <= span.1)
                {
                    return Err(ExportError::Bake("overlapping jump window"));
                }
                previous_window = Some(span.1);
                limit.output = a;
                limit.sided = !self.step;
            } else {
                limit = right;
            }

            if self.step {
                if !self.same_value(left, limit)? {
                    return Err(ExportError::Bake("STEP changed between boundaries"));
                }
                self.append(
                    &mut baked,
                    Key {
                        sided: jump,
                        ..right
                    },
                )?;
            } else {
                self.refine(&mut baked, left, limit, 0)?;
                if jump {
                    self.append(
                        &mut baked,
                        Key {
                            sided: true,
                            ..right
                        },
                    )?;
                }
            }
            left = Key {
                sided: jump,
                ..right
            };
        }
        Ok(baked)
    }

    fn track(&self, axis: usize) -> Result<Option<(&BoundTrack<'_, '_>, usize, f64)>> {
        let axis_key = |track: &BoundTrack<'_, '_>| match track.target {
            BindingTarget::Morph { channel, .. } => channel.target_slot,
            _ => track.component.unwrap_or(0),
        };
        if let Some(track) = self.tracks.first()
            && matches!(track.target, BindingTarget::Transform { .. })
            && track.component.is_none()
        {
            return Ok(Some((
                track,
                axis,
                if self.target == AnimationTarget::Translation && axis == 0 {
                    -1.
                } else {
                    1.
                },
            )));
        }
        self.meter
            .count_decode(u64::from(self.tracks.len().bit_width()) + 1)?;
        let Some(track) = self
            .tracks
            .binary_search_by_key(&axis, axis_key)
            .ok()
            .and_then(|index| self.tracks.get(index))
        else {
            return Ok(None);
        };
        let gain = if let BindingTarget::Morph { channel, .. } = track.target {
            1. / f64::from(channel.full_weight)
        } else if self.target == AnimationTarget::Translation && axis == 0 {
            -1.
        } else {
            1.
        };
        Ok(Some((track, 0, gain)))
    }

    fn component(&self, axis: usize, key: Key) -> Result<f64> {
        if let Some((track, component, gain)) = self.track(axis)? {
            let values = track.raw_at(key.source, key.left, self.meter)?;
            Ok(values.get(component).ok_or(AnimError::InvalidSampleValue)? * gain)
        } else {
            let rest: &[f64] = match self.target {
                AnimationTarget::Translation => &self.rest.translation,
                AnimationTarget::Scale => &self.rest.scale,
                _ => self.rest.weights.as_slice(),
            };
            rest.get(axis)
                .copied()
                .ok_or(AnimError::InvalidSampleValue.into())
        }
    }

    fn rotation(&self, key: Key) -> Result<[f64; 4]> {
        let track = self.tracks.first().ok_or(AnimError::InvalidQuaternion)?;
        let [x, y, z, w] =
            track.rotation(track.raw_at(key.source, key.left, self.meter)?, self.meter)?;
        Ok([x, -y, -z, w])
    }

    fn jump(&self, left: Key, right: Key) -> Result<bool> {
        if self.target == AnimationTarget::Rotation {
            let distance = bounds::angle(self.rotation(left)?, self.rotation(right)?);
            if distance == 0. || distance > 1e-12 {
                return Ok(distance > 0.);
            }
            let track = self.tracks.first().ok_or(AnimError::InvalidQuaternion)?;
            let a = track.raw_at(left.source, true, self.meter)?;
            let b = track.raw_at(right.source, false, self.meter)?;
            for (axis, (a, b)) in a.into_iter().zip(b).enumerate().take(track.width()) {
                if a != b
                    && track
                        .segment(axis, left.source, true, self.meter)?
                        .is_held()
                {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        for axis in 0..self.width {
            let a = self.component(axis, left)?;
            let b = self.component(axis, right)?;
            if a == b {
                continue;
            }
            if self.step || (a - b).abs() > 1e-12 * a.abs().max(b.abs()).max(1.) {
                return Ok(true);
            }
            if let Some((track, component, _)) = self.track(axis)?
                && track
                    .segment(component, left.source, true, self.meter)?
                    .is_held()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn distinct_rounded_sides(&self, left: Key, right: Key) -> Result<bool> {
        if self.target == AnimationTarget::Rotation {
            let a = bounds::rounded(self.rotation(left)?)?;
            let b = bounds::rounded(self.rotation(right)?)?;
            return Ok(bounds::angle(a, b) > 0.);
        }
        for axis in 0..self.width {
            if round(self.component(axis, left)?)? != round(self.component(axis, right)?)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn same_value(&self, left: Key, right: Key) -> Result<bool> {
        if self.target == AnimationTarget::Rotation {
            return Ok(bounds::angle(self.rotation(left)?, self.rotation(right)?) <= 1e-12);
        }
        for axis in 0..self.width {
            if self.component(axis, left)? != self.component(axis, right)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn append(&self, baked: &mut Baked, key: Key) -> Result<()> {
        let previous = baked.times.as_slice().last().copied();
        let output = f64::from(key.output);
        let duplicate = previous == Some(output);
        if previous.is_some_and(|prev| prev > output) || (duplicate && (key.sided || baked.sided)) {
            return Err(ExportError::Bake("conflicting rounded timestamps"));
        }
        let count = baked.times.len();
        if !duplicate {
            if count >= MAX_KEYS {
                return Err(ExportError::Bake("subdivision key cap"));
            }
            count
                .checked_add(1)
                .and_then(|n| n.checked_mul(self.width))
                .ok_or(ExportError::Bake("output size"))?;
            baked.times.push(output)?;
        }
        let mut rotation = [0.; 4];
        if self.target == AnimationTarget::Rotation {
            let [x, y, z, w] = self.rotation(key)?;
            rotation = [round(x)?, round(y)?, round(z)?, round(w)?];
            let previous = count
                .checked_sub(1)
                .and_then(|n| baked.values.as_slice().get(n * self.width..));
            if let Some([x, y, z, w]) = previous {
                rotation = unityrs_gltf::align_rotation(
                    [round(*x)?, round(*y)?, round(*z)?, round(*w)?],
                    rotation,
                );
            }
        }
        for axis in 0..self.width {
            self.meter.count_decode(8)?;
            let rounded = if self.target == AnimationTarget::Rotation {
                f64::from(*rotation.get(axis).ok_or(AnimError::InvalidQuaternion)?)
            } else {
                let value = self.component(axis, key)?;
                let rounded = f64::from(round(value)?);
                if value.abs() > MAX_COMPONENT {
                    return Err(ExportError::Bake("component magnitude"));
                }
                rounded
            };
            if duplicate {
                if baked.values.as_slice().get((count - 1) * self.width + axis) != Some(&rounded) {
                    return Err(ExportError::Bake("collapsed key values"));
                }
            } else {
                baked.values.push(rounded)?;
            }
        }
        baked.sided = key.sided;
        Ok(())
    }

    fn refine(&self, baked: &mut Baked, left: Key, right: Key, depth: u32) -> Result<()> {
        let certified = if self.target == AnimationTarget::Rotation {
            bounds::rotation(self, left, right)?
        } else {
            bounds::components(self, left, right)?
        };
        if certified {
            return self.append(baked, right);
        }
        let source = midpoint(left.source, right.source);
        let output = round(source - self.start)?;
        if depth >= MAX_DEPTH
            || source <= left.source
            || source >= right.source
            || output <= left.output
            || output >= right.output
        {
            return Err(ExportError::Bake("cannot certify subdivision"));
        }
        let middle = Key {
            source,
            output,
            left: false,
            sided: false,
        };
        self.refine(baked, left, middle, depth + 1)?;
        self.refine(baked, middle, right, depth + 1)
    }

    fn derivative(&self, axis: usize, left: Key, right: Key) -> Result<(f64, f64)> {
        if let Some((track, component, gain)) = self.track(axis)? {
            let (lo, hi) = (left.source, right.source);
            let (rate, acceleration) = track
                .segment(component, lo, false, self.meter)?
                .derivatives(lo, hi);
            Ok((rate * gain.abs(), acceleration * gain.abs()))
        } else {
            Ok((0., 0.))
        }
    }
}

fn midpoint(left: f64, right: f64) -> f64 {
    left + (right - left) * 0.5
}

#[allow(clippy::cast_possible_truncation)]
fn round(value: f64) -> Result<f32> {
    let rounded = value as f32;
    if !rounded.is_finite() {
        return Err(AnimError::InvalidSampleValue.into());
    }
    Ok(rounded)
}
