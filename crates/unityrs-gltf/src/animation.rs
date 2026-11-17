//! Editable animation tables and their glTF layout.
use crate::json::Json;
use crate::validate::{accessor, invalid};
use crate::{Accessor, Data, Extras, GltfError, Kind, Layout, Options, Scene, Text};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

type Result<T> = std::result::Result<T, GltfError>;

/// One animation of already-baked local TRS or instance weights in glTF space.
/// Animation, channel and referenced sampler rows retain their table order.
/// An animation with no channels is omitted from the GLB.
#[derive(Debug)]
pub struct Animation {
    pub name: Option<Text>,
    pub samplers: MeteredVec<AnimationSampler>,
    pub channels: MeteredVec<AnimationChannel>,
    pub extras: Extras,
}
const _: () = crate::send_sync::<Animation>();

impl Animation {
    /// Empty animation. Tables and metadata are charged as they grow.
    #[must_use]
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            name: None,
            samplers: MeteredVec::new(meter),
            channels: MeteredVec::new(meter),
            extras: MeteredVec::new(meter),
        }
    }
}

/// Interpolation over the stored keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interpolation {
    Step,
    Linear,
}
const _: () = crate::send_sync::<Interpolation>();

/// One input/output pair, shared by compatible channels in an animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimationSampler {
    /// Index in `Scene::accessors`, with kind `AnimationTime`.
    pub input: usize,
    /// Index in `Scene::accessors`, with the kind required by the channels.
    pub output: usize,
    pub interpolation: Interpolation,
}
const _: () = crate::send_sync::<AnimationSampler>();

/// A node property driven by a sampler. Each (node, target) pair is unique
/// within one animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimationChannel {
    /// Index in the containing animation's sampler table.
    pub sampler: usize,
    /// Index in `Scene::nodes`. Weights target the mesh instance itself.
    pub node: usize,
    pub target: AnimationTarget,
}
const _: () = crate::send_sync::<AnimationChannel>();

/// Core glTF animation target paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationTarget {
    Translation,
    Rotation,
    Scale,
    Weights,
}
const _: () = crate::send_sync::<AnimationTarget>();

impl AnimationTarget {
    fn kind(self) -> Kind {
        match self {
            Self::Translation | Self::Scale => Kind::AnimationVec3,
            Self::Rotation => Kind::AnimationRotation,
            Self::Weights => Kind::AnimationWeights,
        }
    }

    fn path(self) -> &'static [u8] {
        match self {
            Self::Translation => b"translation",
            Self::Rotation => b"rotation",
            Self::Scale => b"scale",
            Self::Weights => b"weights",
        }
    }
}

#[derive(Debug)]
pub(crate) struct AnimationLayout {
    source: usize,
    sampler_ids: MeteredVec<Option<usize>>,
}

pub(crate) fn layout_animations(
    scene: &Scene,
    layout: &mut Layout,
    opts: Options,
    meter: &Arc<Meter>,
) -> Result<()> {
    let mut seen = MeteredVec::new(meter);
    for (source, animation) in scene.animations.as_slice().iter().enumerate() {
        meter.count_decode(1)?;
        if animation.channels.is_empty() {
            continue;
        }
        crate::validate::extras(&animation.extras, meter)?;
        if seen.is_empty() {
            seen = zeroed(meter, scene.nodes.len(), 0u8)?;
        }
        let mut sampler_ids = zeroed(meter, animation.samplers.len(), None)?;
        for channel in animation.channels.as_slice() {
            meter.count_decode(8)?;
            let targets = seen
                .as_mut_slice()
                .get_mut(channel.node)
                .ok_or_else(|| invalid("animation target node index"))?;
            let bit = 1 << channel.target as u8;
            if *targets & bit != 0 {
                return Err(invalid("duplicate animation target"));
            }
            *targets |= bit;
            let sampler = animation
                .samplers
                .as_slice()
                .get(channel.sampler)
                .ok_or_else(|| invalid("animation sampler index"))?;
            let keys = accessor(scene, sampler.input, Kind::AnimationTime)?.count;
            let output = accessor(scene, sampler.output, channel.target.kind())?;
            let arity = channel_arity(scene, layout, channel)?;
            if keys.checked_mul(arity) != Some(output.count) {
                return Err(invalid("animation output count"));
            }
            *sampler_ids
                .as_mut_slice()
                .get_mut(channel.sampler)
                .ok_or_else(|| invalid("animation sampler index"))? = Some(0);
        }
        for channel in animation.channels.as_slice() {
            if let Some(targets) = seen.as_mut_slice().get_mut(channel.node) {
                *targets = 0;
            }
        }
        let mut next = 0;
        for (id, sampler) in sampler_ids
            .as_mut_slice()
            .iter_mut()
            .zip(animation.samplers.as_slice())
        {
            meter.count_decode(1)?;
            if id.is_some() {
                *id = Some(next);
                next += 1;
                crate::layout::place(scene, layout, sampler.input, opts, meter)?;
                crate::layout::place(scene, layout, sampler.output, opts, meter)?;
            }
        }
        layout.animations.push(AnimationLayout {
            source,
            sampler_ids,
        })?;
    }
    Ok(())
}

fn zeroed<T: Copy>(meter: &Arc<Meter>, len: usize, value: T) -> Result<MeteredVec<T>> {
    meter.count_decode(len as u64)?;
    let mut out = MeteredVec::with_capacity(meter, len)?;
    for _ in 0..len {
        out.push(value)?;
    }
    Ok(out)
}

fn channel_arity(scene: &Scene, layout: &Layout, channel: &AnimationChannel) -> Result<usize> {
    let node = scene
        .nodes
        .as_slice()
        .get(channel.node)
        .ok_or_else(|| invalid("animation target node index"))?;
    if channel.target != AnimationTarget::Weights {
        if node.skin.is_some() {
            return Err(invalid("animation TRS targets a skinned mesh attachment"));
        }
        return Ok(1);
    }
    let mesh = node
        .mesh
        .ok_or_else(|| invalid("animation weights need a mesh"))?;
    if layout
        .meshes
        .as_slice()
        .get(mesh)
        .is_none_or(|m| m.id.is_none())
    {
        return Err(invalid("animation weights target an omitted mesh"));
    }
    scene
        .meshes
        .as_slice()
        .get(mesh)
        .and_then(|m| m.primitives.as_slice().first())
        .map(|p| p.targets.len())
        .filter(|&n| n != 0)
        .ok_or_else(|| invalid("animation weights need morph targets"))
}

/// Scan each placed animation accessor once. The streamer uses the same
/// rounding function and leaves all validated keys unchanged.
pub(crate) fn validate_keys(a: &Accessor, meter: &Meter) -> Result<Option<(f32, f32)>> {
    let Data::Float(values) = &a.data else {
        return Err(invalid("animation accessor component type"));
    };
    let samples = values.as_slice();
    meter.count_decode((samples.len() as u64).saturating_mul(16))?;
    match a.kind {
        Kind::AnimationTime => {
            let mut previous = None;
            for &key in samples {
                let t = crate::normalise::round(key);
                if !t.is_finite() || key < 0. || previous.is_some_and(|prev| t <= prev) {
                    return Err(invalid(
                        "animation times must be finite, nonnegative and strictly increasing in f32",
                    ));
                }
                previous = Some(t);
            }
            Ok(samples
                .first()
                .zip(previous)
                .map(|(&first, last)| (crate::normalise::round(first), last)))
        }
        Kind::AnimationRotation => {
            let mut previous = [0.; 4];
            for row in samples.as_chunks::<4>().0 {
                let rounded = row.map(crate::normalise::round);
                if rounded.iter().any(|x| !x.is_finite() || x.abs() > 1.) {
                    return Err(invalid("invalid animation rotation component"));
                }
                let q = rounded.map(f64::from);
                let norm2 = dot(q, q);
                // Squared-length allowance for f32 rounding of unit keys.
                if (norm2 - 1.).abs() > 5e-7 {
                    return Err(invalid("animation rotation must be unit length"));
                }
                if dot(previous, q) < 0. {
                    return Err(invalid("animation rotation signs must be continuous"));
                }
                previous = q;
            }
            Ok(None)
        }
        _ => {
            if samples
                .iter()
                .any(|&x| !crate::normalise::round(x).is_finite())
            {
                return Err(invalid("non-finite f32 animation output"));
            }
            Ok(None)
        }
    }
}

fn dot(a: [f64; 4], b: [f64; 4]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}

pub(crate) fn document(scene: &Scene, layout: &Layout, j: &mut Json<'_>) -> Result<()> {
    if layout.animations.is_empty() {
        return Ok(());
    }
    j.raw(",\"animations\":[")?;
    for (index, placed) in layout.animations.as_slice().iter().enumerate() {
        if index > 0 {
            j.raw(",")?;
        }
        let animation = scene
            .animations
            .as_slice()
            .get(placed.source)
            .ok_or_else(|| invalid("animation layout index"))?;
        j.raw("{\"channels\":[")?;
        for (i, channel) in animation.channels.as_slice().iter().enumerate() {
            if i > 0 {
                j.raw(",")?;
            }
            j.raw("{\"sampler\":")?;
            j.number(
                placed
                    .sampler_ids
                    .as_slice()
                    .get(channel.sampler)
                    .copied()
                    .flatten()
                    .ok_or_else(|| invalid("omitted animation sampler"))?,
            )?;
            j.raw(",\"target\":{\"node\":")?;
            j.number(channel.node)?;
            j.raw(",\"path\":")?;
            j.string(channel.target.path())?;
            j.raw("}}")?;
        }
        j.raw("],\"samplers\":[")?;
        for (id, sampler) in placed
            .sampler_ids
            .as_slice()
            .iter()
            .zip(animation.samplers.as_slice())
        {
            // Skipped rows still cost traversal in both JSON passes.
            j.work(1)?;
            let Some(id) = id else { continue };
            if *id > 0 {
                j.raw(",")?;
            }
            j.raw("{\"input\":")?;
            j.number(crate::schema::aid(layout, sampler.input)?)?;
            j.raw(",\"interpolation\":")?;
            j.string(match sampler.interpolation {
                Interpolation::Step => b"STEP",
                Interpolation::Linear => b"LINEAR",
            })?;
            j.raw(",\"output\":")?;
            j.number(crate::schema::aid(layout, sampler.output)?)?;
            j.raw("}")?;
        }
        j.raw("]")?;
        crate::schema::name(j, animation.name.as_ref())?;
        crate::schema::extras(j, &animation.extras)?;
        j.raw("}")?;
    }
    j.raw("]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_times_validate_after_rounding_to_even() {
        let meter = Meter::unlimited();
        let halfway = 1. + f64::from(f32::EPSILON) / 2.;
        let mut accessor = Accessor {
            kind: Kind::AnimationTime,
            count: 2,
            data: Data::Float(crate::array(&meter, &[0., halfway]).unwrap()),
            sparse_indices: None,
        };
        let (min, max) = validate_keys(&accessor, &meter).unwrap().unwrap();
        assert_eq!(min.to_bits(), 0.0f32.to_bits());
        assert_eq!(max.to_bits(), 1.0f32.to_bits());
        if let Data::Float(data) = &mut accessor.data {
            data.as_mut_slice()[0] = 1.;
        }
        assert!(matches!(
            validate_keys(&accessor, &meter),
            Err(GltfError::Invalid(_))
        ));
    }
}
