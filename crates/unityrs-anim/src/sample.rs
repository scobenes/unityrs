//! Rig-scoped property ownership and Unity-space sample output.
use crate::{
    AnimError, BindingTarget, Clip, ClipField, ClipSource, ClipTiming, CompiledClip, MorphProperty,
    PathQuery, RendererChannels, RigPaths, StorageKind, TransformProperty,
    clip::{
        Playback,
        explicit::{Curve, Data, Property, visit_runtime},
    },
    curve::{EulerOrder, normalize, work_for_search},
    read::Dict,
};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_scene::{NodeId, Trs};

#[derive(Debug)]
enum Evaluator<'a> {
    Explicit(&'a Curve),
    Compiled(&'a CompiledClip, usize, u8),
}

/// A chosen writer for a resolved property or scalar TRS component.
#[derive(Debug)]
pub struct BoundTrack<'a, 'r> {
    pub target: BindingTarget<'r>,
    pub source: ClipSource,
    /// Component index for scalar translation or scale curves.
    pub component: Option<usize>,
    pub storage: Option<StorageKind>,
    evaluator: Evaluator<'a>,
}

#[derive(Debug)]
pub struct BoundClip<'a, 'r> {
    clip: &'a Clip,
    tracks: MeteredVec<BoundTrack<'a, 'r>>,
    timing: Option<ClipTiming>,
    transforms: usize,
    morphs: usize,
}

impl<'a, 'r> BoundClip<'a, 'r> {
    #[must_use]
    pub fn clip(&self) -> &'a Clip {
        self.clip
    }

    #[must_use]
    pub fn timing(&self) -> Option<ClipTiming> {
        self.timing
    }

    #[must_use]
    pub fn tracks(&self) -> &[BoundTrack<'a, 'r>] {
        self.tracks.as_slice()
    }
}

fn property_key(target: BindingTarget<'_>) -> (NodeId, u8, usize) {
    use TransformProperty as T;
    match target {
        BindingTarget::Transform { node, property } => {
            let family = match property {
                T::Translation => 0,
                T::Scale => 1,
                T::Quaternion | T::Euler => 2,
            };
            (node, family, 0)
        }
        BindingTarget::Morph { node, channel, .. } => (node, 3, channel.target_slot),
        BindingTarget::Metadata => (0, 4, 0),
    }
}

fn bind_explicit<'r>(
    property: &Property,
    path: &[u8],
    paths: &RigPaths<'_>,
    filtered: &[NodeId],
    renderers: &'r [RendererChannels<'r>],
    meter: &Arc<Meter>,
) -> Result<(BindingTarget<'r>, Option<usize>), AnimError> {
    let path = PathQuery::Name(path);
    Ok(match property {
        Property::Transform(property) => (
            paths.bind_transform(path, *property, filtered, meter)?,
            None,
        ),
        Property::Component(property, component) => (
            paths.bind_transform(path, *property, filtered, meter)?,
            Some(*component),
        ),
        Property::RotationComponent => (
            paths.bind_transform(path, TransformProperty::Quaternion, filtered, meter)?,
            None,
        ),
        Property::Morph(attribute) => (
            paths.bind_morph(
                path,
                MorphProperty::Attribute(attribute.as_bytes()),
                filtered,
                renderers,
                meter,
            )?,
            None,
        ),
    })
}

/// Bind to one component's rig using the PR 3 path and renderer tables.
/// Selected playback failures retain the original cause. There is no
/// fallback to a shadowed source when a chosen writer fails.
#[allow(
    clippy::too_many_lines,
    reason = "keep writer selection and uniqueness checks together"
)]
pub fn bind_clip<'a, 'r>(
    clip: &'a Clip,
    paths: &RigPaths<'_>,
    filtered: &[NodeId],
    renderers: &'r [RendererChannels<'r>],
    meter: &Arc<Meter>,
) -> Result<BoundClip<'a, 'r>, AnimError> {
    let timing = clip.timing()?;
    let mut tracks = MeteredVec::new(meter);
    let playback = clip.playback.as_ref().map_err(Clone::clone)?;
    match playback {
        Playback::Compiled(compiled) => {
            for (ordinal, slot) in compiled.slot_bindings().iter().enumerate() {
                meter.count_decode(1)?;
                let target = paths.bind_generic(slot, filtered, renderers, meter)?;
                if matches!(target, BindingTarget::Metadata) {
                    continue;
                }
                let first = compiled.storage(slot.first);
                let last = compiled.storage(slot.first + usize::from(slot.width) - 1);
                if first != last {
                    return Err(AnimError::UnsupportedMixedTransformStorage(slot.first));
                }
                if slot.binding.script.path_id != 0 {
                    return Err(AnimError::UnsupportedBindingTarget);
                }
                tracks.push(BoundTrack {
                    target,
                    source: ClipSource {
                        field: ClipField::Compiled,
                        ordinal,
                    },
                    component: None,
                    storage: Some(first),
                    evaluator: Evaluator::Compiled(compiled, slot.first, slot.width),
                })?;
            }
        }
        Playback::Explicit(explicit) => {
            for track in explicit.as_slice() {
                meter.count_decode(1)?;
                let (target, component) = bind_explicit(
                    &track.property,
                    track.path.as_bytes(),
                    paths,
                    filtered,
                    renderers,
                    meter,
                )?;
                let Data::Curve(curve) = &track.data else {
                    return Err(AnimError::UnsupportedPackedRotationSlope);
                };
                tracks.push(BoundTrack {
                    target,
                    source: track.source,
                    component,
                    storage: None,
                    evaluator: Evaluator::Explicit(curve),
                })?;
            }
        }
    }
    meter.count_decode((tracks.len() as u64).saturating_mul(work_for_search(tracks.len()) * 4))?;
    tracks
        .as_mut_slice()
        .sort_unstable_by_key(|track| (property_key(track.target), track.component));
    for pair in tracks.as_slice().windows(2) {
        if let [left, right] = pair
            && property_key(left.target) == property_key(right.target)
            && (left.component.is_none() || left.component == right.component)
        {
            return Err(AnimError::AmbiguousPlaybackSource);
        }
    }
    if matches!(playback, Playback::Compiled(_)) {
        let dict = Dict {
            parsed: clip.serialized(),
            meter,
        };
        visit_runtime(&dict, None, |candidate| {
            let (target, _) = bind_explicit(
                &candidate.property,
                candidate.path.as_bytes(),
                paths,
                filtered,
                renderers,
                meter,
            )?;
            let key = property_key(target);
            meter.count_decode(work_for_search(tracks.len()))?;
            tracks
                .as_slice()
                .binary_search_by_key(&key, |track| property_key(track.target))
                .map(|_| ())
                .map_err(|_| match candidate.property {
                    Property::RotationComponent => AnimError::UnsupportedBindingTarget,
                    _ => AnimError::UnsupportedSupplementalCurve,
                })
        })?;
    }
    if tracks.is_empty() && !clip.has_metadata_tracks() {
        return Err(AnimError::NoPlayableChannels);
    }
    let mut transforms = 0;
    let mut morphs = 0;
    let mut previous = None;
    for track in tracks.as_slice() {
        match track.target {
            BindingTarget::Transform { node, .. } if previous != Some(node) => {
                transforms += 1;
                previous = Some(node);
            }
            BindingTarget::Morph { .. } => morphs += 1,
            _ => {}
        }
    }
    Ok(BoundClip {
        clip,
        tracks,
        timing,
        transforms,
        morphs,
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SampleTime {
    pub source: f64,
    pub local: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalTrs {
    pub position: [f64; 3],
    pub rotation: [f64; 4],
    pub scale: [f64; 3],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransformSample {
    pub node: NodeId,
    pub pose: LocalTrs,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MorphSample {
    pub node: NodeId,
    pub target_slot: usize,
    /// Unity blend-shape weight, before any glTF conversion.
    pub weight: f64,
}

/// Samples in schedule order. Each time has one row per animated
/// Transform and one row per animated renderer/channel.
#[derive(Debug)]
pub struct ClipSamples<'b, 'a, 'r> {
    times: MeteredVec<SampleTime>,
    transforms: MeteredVec<TransformSample>,
    morphs: MeteredVec<MorphSample>,
    bound: &'b BoundClip<'a, 'r>,
}

impl<'b, 'a, 'r> ClipSamples<'b, 'a, 'r> {
    #[must_use]
    pub fn times(&self) -> &[SampleTime] {
        self.times.as_slice()
    }

    /// Frame-major, with the same node order at every sample.
    #[must_use]
    pub fn transforms(&self) -> &[TransformSample] {
        self.transforms.as_slice()
    }

    /// Frame-major, in node and attachment target-slot order.
    #[must_use]
    pub fn morphs(&self) -> &[MorphSample] {
        self.morphs.as_slice()
    }

    #[must_use]
    pub fn bound(&self) -> &'b BoundClip<'a, 'r> {
        self.bound
    }
}

/// Sample strictly increasing source times within the closed clip interval.
/// Rest is indexed by graph node. Only nodes with a chosen writer appear in
/// the result, and their unwritten components come from rest.
pub fn sample_clip<'b, 'a, 'r>(
    clip: &'b BoundClip<'a, 'r>,
    rest: &[Trs],
    source_times: &[f64],
    meter: &Arc<Meter>,
) -> Result<ClipSamples<'b, 'a, 'r>, AnimError> {
    let count = source_times.len();
    let capacity = |width| count.checked_mul(width).ok_or(AnimError::SlotOverflow);
    if clip.tracks.is_empty() && source_times.is_empty() {
        return Ok(ClipSamples {
            times: MeteredVec::new(meter),
            transforms: MeteredVec::new(meter),
            morphs: MeteredVec::new(meter),
            bound: clip,
        });
    }
    let timing = clip.timing.ok_or(AnimError::InvalidSchedule)?;
    meter.count_decode(count as u64)?;
    if source_times.is_empty()
        || source_times.iter().any(|&time| !timing.contains(time))
        || source_times
            .windows(2)
            .any(|pair| matches!(pair, [a, b] if a >= b))
    {
        return Err(AnimError::InvalidSchedule);
    }
    // Check the maximum number of scalar evaluations plus output sizes
    // before any pose allocation.
    let tracks = capacity(clip.tracks.len())?
        .checked_mul(4)
        .ok_or(AnimError::SlotOverflow)?;
    let transforms = capacity(clip.transforms)?;
    let morphs = capacity(clip.morphs)?;
    let bytes = count
        .checked_mul(size_of::<SampleTime>())
        .and_then(|bytes| bytes.checked_add(transforms.checked_mul(size_of::<TransformSample>())?))
        .and_then(|bytes| bytes.checked_add(morphs.checked_mul(size_of::<MorphSample>())?))
        .ok_or(AnimError::SlotOverflow)?;
    meter.count_decode((bytes as u64).saturating_add(tracks as u64))?;
    let mut output = ClipSamples {
        times: MeteredVec::with_capacity(meter, count)?,
        transforms: MeteredVec::with_capacity(meter, transforms)?,
        morphs: MeteredVec::with_capacity(meter, morphs)?,
        bound: clip,
    };
    for &source in source_times {
        output.times.push(SampleTime {
            source,
            local: source - timing.start(),
        })?;
        let mut pose = None;
        for track in clip.tracks() {
            let values = track.evaluate(source, meter)?;
            let [x, y, z, _] = values;
            match track.target {
                BindingTarget::Transform { node, property } => {
                    if pose
                        .as_ref()
                        .is_none_or(|p: &TransformSample| p.node != node)
                    {
                        if let Some(previous) = pose.take() {
                            output.transforms.push(previous)?;
                        }
                        pose = Some(TransformSample {
                            node,
                            pose: rest_pose(rest, node)?,
                        });
                    }
                    let pose = &mut pose.as_mut().ok_or(AnimError::InvalidRestPose(node))?.pose;
                    match property {
                        TransformProperty::Translation => {
                            write_vector(&mut pose.position, track.component, [x, y, z])?;
                        }
                        TransformProperty::Scale => {
                            write_vector(&mut pose.scale, track.component, [x, y, z])?;
                        }
                        TransformProperty::Quaternion | TransformProperty::Euler => {
                            pose.rotation = track.rotation(values, meter)?;
                        }
                    }
                }
                BindingTarget::Morph { node, channel, .. } => {
                    output.morphs.push(MorphSample {
                        node,
                        target_slot: channel.target_slot,
                        weight: x,
                    })?;
                }
                BindingTarget::Metadata => {}
            }
        }
        if let Some(previous) = pose {
            output.transforms.push(previous)?;
        }
    }
    Ok(output)
}

impl BoundTrack<'_, '_> {
    #[must_use]
    pub fn width(&self) -> usize {
        match self.evaluator {
            Evaluator::Explicit(curve) => curve.width(),
            Evaluator::Compiled(_, _, width) => usize::from(width),
        }
    }

    pub fn visit_key_times(
        &self,
        mut visit: impl FnMut(f64) -> Result<(), AnimError>,
    ) -> Result<(), AnimError> {
        match self.evaluator {
            Evaluator::Explicit(curve) => {
                for time in curve.key_times() {
                    visit(time)?;
                }
                Ok(())
            }
            Evaluator::Compiled(clip, first, width) => {
                // Dense slots share one grid. Streamed slots can be sparse.
                let count = if self.storage == Some(StorageKind::Streamed) {
                    usize::from(width)
                } else {
                    1
                };
                for slot in first..first + count {
                    clip.visit_times(slot, &mut visit)?;
                }
                Ok(())
            }
        }
    }

    pub fn segment(
        &self,
        axis: usize,
        source: f64,
        left_side: bool,
        meter: &Meter,
    ) -> Result<crate::ScalarSegment, AnimError> {
        if axis >= self.width() {
            return Err(AnimError::InvalidSampleValue);
        }
        match self.evaluator {
            Evaluator::Explicit(curve) => curve.segment(axis, source, left_side, meter),
            Evaluator::Compiled(clip, first, _) => {
                clip.segment(first + axis, source, left_side, meter)
            }
        }
    }

    /// Evaluate a track's raw components (before quaternion normalization or
    /// Euler conversion), optionally taking the exact left limit at a key.
    pub fn raw_at(
        &self,
        source: f64,
        left_side: bool,
        meter: &Meter,
    ) -> Result<[f64; 4], AnimError> {
        if !left_side {
            return self.evaluate(source, meter);
        }
        let mut values = [0.; 4];
        for (axis, value) in values.iter_mut().enumerate().take(self.width()) {
            *value = self.segment(axis, source, true, meter)?.at(source, meter)?;
        }
        if values.iter().any(|value| !value.is_finite()) {
            return Err(AnimError::InvalidSampleValue);
        }
        Ok(values)
    }

    /// Convert a raw quaternion or the chosen Euler source to local XYZW.
    pub fn rotation(&self, raw: [f64; 4], meter: &Meter) -> Result<[f64; 4], AnimError> {
        if let Evaluator::Explicit(curve) = self.evaluator
            && let Some(order) = curve.order
        {
            meter.count_decode(256)?;
            return order.rotation(raw);
        }
        if let Evaluator::Compiled(..) = self.evaluator
            && let BindingTarget::Transform {
                property: TransformProperty::Euler,
                ..
            } = self.target
        {
            // The slot table has no order field. Built 2022.3 generic replays
            // match Unity's default ZXY order and no other.
            meter.count_decode(256)?;
            return EulerOrder::ZXY.rotation(raw);
        }
        meter.count_decode(32)?;
        normalize(raw)
    }

    fn evaluate(&self, source: f64, meter: &Meter) -> Result<[f64; 4], AnimError> {
        let values = match self.evaluator {
            Evaluator::Explicit(curve) => curve.at(source, meter)?,
            Evaluator::Compiled(compiled, first, width) => {
                let mut values = [0.; 4];
                for (i, value) in values.iter_mut().enumerate().take(usize::from(width)) {
                    *value = compiled
                        .slot_value(first + i, source, meter)?
                        .continuous()?;
                }
                values
            }
        };
        if values.iter().any(|value| !value.is_finite()) {
            return Err(AnimError::InvalidSampleValue);
        }
        Ok(values)
    }
}

fn write_vector(
    target: &mut [f64; 3],
    component: Option<usize>,
    values: [f64; 3],
) -> Result<(), AnimError> {
    if let Some(axis) = component {
        let [value, ..] = values;
        *target.get_mut(axis).ok_or(AnimError::InvalidSampleValue)? = value;
    } else {
        *target = values;
    }
    Ok(())
}

fn rest_pose(rest: &[Trs], node: NodeId) -> Result<LocalTrs, AnimError> {
    let rest = rest.get(node).ok_or(AnimError::InvalidRestPose(node))?;
    if rest
        .position
        .iter()
        .chain(&rest.rotation)
        .chain(&rest.scale)
        .any(|x| !x.is_finite())
        || rest.rotation.iter().all(|&x| x == 0.)
    {
        return Err(AnimError::InvalidRestPose(node));
    }
    Ok(LocalTrs {
        position: rest.position.map(f64::from),
        rotation: rest.rotation.map(f64::from),
        scale: rest.scale.map(f64::from),
    })
}

const _: () = crate::send_sync::<BoundTrack<'static, 'static>>();
const _: () = crate::send_sync::<BoundClip<'static, 'static>>();
const _: () = crate::send_sync::<SampleTime>();
const _: () = crate::send_sync::<LocalTrs>();
const _: () = crate::send_sync::<TransformSample>();
const _: () = crate::send_sync::<MorphSample>();
const _: () = crate::send_sync::<ClipSamples<'static, 'static, 'static>>();
