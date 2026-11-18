use super::{floats, rich};
use std::sync::Arc;
use unityrs_core::limits::Meter;
use unityrs_gltf::*;

// Extend the ordinary writer fixture with animated joints and two morph
// instances of exactly the same mesh. All keys are in glTF coordinates.
pub fn animated(meter: &Arc<Meter>) -> Result<Scene, GltfError> {
    let mut scene = rich(meter)?;
    scene.meshes.truncate(1);
    scene.nodes.as_mut_slice()[1].mesh = Some(0);
    let second_target = scene.accessors.len();
    scene.accessors.push(Accessor {
        kind: Kind::MorphPosition,
        count: 3,
        data: Data::Float(array(meter, &[0.25, 0., 0.])?),
        sparse_indices: Some(array(meter, &[2])?),
    })?;
    scene.meshes.as_mut_slice()[0].primitives.as_mut_slice()[0]
        .targets
        .push([Some(second_target), None, None])?;
    scene.meshes.as_mut_slice()[0]
        .target_names
        .push(text(meter, "shift")?)?;
    scene.nodes.as_mut_slice()[0].weights.push(0.5)?;
    scene.nodes.as_mut_slice()[1].weights.push(0.25)?;

    let mut animation = Animation::new(meter);
    animation.name = Some(text(meter, "sweep\"é")?);
    animation
        .extras
        .push((text(meter, "source")?, text(meter, "synthetic")?))?;
    // One unreferenced sampler tests compaction and channel remapping.
    animation.samplers.push(AnimationSampler {
        input: usize::MAX,
        output: usize::MAX,
        interpolation: Interpolation::Step,
    })?;
    let times = scene.accessors.len();
    scene
        .accessors
        .push(floats(meter, Kind::AnimationTime, &[0.25, 1.000_000_15])?)?;
    let s = std::f64::consts::FRAC_1_SQRT_2;
    for (kind, values, node, target, interpolation) in [
        (
            Kind::AnimationVec3,
            &[-2., 0., 4., 6., 4., 0.][..],
            2,
            AnimationTarget::Translation,
            Interpolation::Linear,
        ),
        (
            Kind::AnimationRotation,
            &[0., 0., 0., 1., 0., 0., s, s],
            3,
            AnimationTarget::Rotation,
            Interpolation::Linear,
        ),
        (
            Kind::AnimationWeights,
            &[0.25, 0.5, 0.75, -0.25],
            0,
            AnimationTarget::Weights,
            Interpolation::Linear,
        ),
        (
            Kind::AnimationWeights,
            &[0.75, 0.25, 0.5, 1.25],
            1,
            AnimationTarget::Weights,
            Interpolation::Step,
        ),
    ] {
        let output = scene.accessors.len();
        let sampler = animation.samplers.len();
        scene.accessors.push(floats(meter, kind, values)?)?;
        animation.samplers.push(AnimationSampler {
            input: times,
            output,
            interpolation,
        })?;
        animation.channels.push(AnimationChannel {
            sampler,
            node,
            target,
        })?;
    }

    // Channel order differs from sampler order. A translation and scale
    // channel may share a VEC3 sampler on different paths of the same node.
    animation.channels.push(AnimationChannel {
        sampler: 1,
        node: 2,
        target: AnimationTarget::Scale,
    })?;
    animation.channels.as_mut_slice().swap(0, 3);

    let mut empty = Animation::new(meter);
    empty.name = Some(text(meter, "metadata only")?);
    empty
        .extras
        .push((text(meter, "binding")?, text(meter, "event")?))?;
    scene.animations.push(empty)?;
    scene.animations.push(animation)?;
    let single = single_key(meter, &mut scene)?;
    scene.animations.push(single)?;
    Ok(scene)
}

fn single_key(meter: &Arc<Meter>, scene: &mut Scene) -> Result<Animation, GltfError> {
    let mut single = Animation::new(meter);
    single.name = Some(text(meter, "sweep\"é")?);
    single.samplers.push(AnimationSampler {
        input: scene.accessors.len(),
        output: scene.accessors.len() + 1,
        interpolation: Interpolation::Step,
    })?;
    scene
        .accessors
        .push(floats(meter, Kind::AnimationTime, &[0.])?)?;
    scene
        .accessors
        .push(floats(meter, Kind::AnimationVec3, &[1., 2., 3.])?)?;
    single.channels.push(AnimationChannel {
        sampler: 0,
        node: 2,
        target: AnimationTarget::Scale,
    })?;
    Ok(single)
}
