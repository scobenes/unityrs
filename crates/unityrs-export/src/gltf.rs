//! Shared standalone and batch-facing composition of Unity playback and glTF.
#![allow(
    clippy::result_large_err,
    reason = "keep source slot provenance inline and fallible allocation-free"
)]
mod bake;
mod select;

use std::sync::Arc;
use unityrs_anim::{
    AnimError, Attachment, BindingTarget, Clip, Discovery, RendererChannel, RendererChannels,
    RigClips, RigPaths, SlotError, bind_clip, discover, read_clip,
};
use unityrs_core::{
    ObjectRef,
    env::{Environment, FileOccurrence},
    files::File,
    limits::{ChargeError, Meter, MeteredVec},
    value::Value,
};
use unityrs_gltf::{CollectionTargets, GltfError, MaterialMapper, Profile, Scene};
use unityrs_scene::Graph;

pub use select::walk_files;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error(transparent)]
    Gltf(#[from] GltfError),
    #[error(transparent)]
    Animation(#[from] AnimError),
    #[error(transparent)]
    Charge(#[from] ChargeError),
    #[error(transparent)]
    Input(#[from] unityrs_core::Error),
    #[error("missing_clip_selection")]
    MissingClipSelection,
    #[error("ambiguous_clip_selection")]
    AmbiguousClipSelection,
    #[error("clip source {}[{}]: {}", .0.source.field, .0.source.ordinal, .0.cause)]
    Unresolved(SlotError),
    #[error("animated_static_batch")]
    AnimatedStaticBatch,
    #[error("animation_bake_error: {0}")]
    Bake(&'static str),
}

/// Discovery includes unresolved slots. Decoded clips retain their serialized
/// bindings, authoring curves and inert events, including metadata-only clips.
#[derive(Debug)]
pub struct Collected {
    pub scene: Scene,
    pub discovery: Option<Discovery>,
    pub clips: MeteredVec<Clip>,
}

/// Collect with the caller's ordered input roots. File addresses use their
/// concatenated depth-first inspection order, including root and raw entries.
pub fn collect(
    env: &Environment,
    roots: &[File],
    graph: &Graph,
    profile: &Profile,
    mapper: &dyn MaterialMapper,
    meter: &Arc<Meter>,
) -> Result<Collected, ExportError> {
    let mut clips = MeteredVec::new(meter);
    if !profile.clips.requested() {
        return Ok(Collected {
            scene: unityrs_gltf::collect_with_mapper(env, graph, profile, mapper, meter)?,
            discovery: None,
            clips,
        });
    }
    if profile
        .sample_rate
        .is_some_and(|rate| !rate.is_finite() || rate <= 0. || rate > unityrs_gltf::MAX_SAMPLE_RATE)
    {
        return Err(AnimError::InvalidSampleRate.into());
    }
    let discovery = discover(env, graph, meter)?;
    let selected = select::clips(&discovery, roots, &profile.clips, meter)?;
    let scene = unityrs_gltf::collect_with_animation(
        env,
        graph,
        profile,
        mapper,
        meter,
        |scene, targets| {
            animate(
                scene,
                targets,
                graph,
                profile,
                selected.as_slice(),
                &mut clips,
                meter,
            )
        },
    )?;
    Ok(Collected {
        scene,
        discovery: Some(discovery),
        clips,
    })
}

fn animate(
    scene: &mut Scene,
    targets: CollectionTargets<'_>,
    graph: &Graph,
    profile: &Profile,
    selected: &[(&RigClips, &ObjectRef)],
    clips: &mut MeteredVec<Clip>,
    meter: &Arc<Meter>,
) -> Result<(), ExportError> {
    let mut filtered = MeteredVec::new(meter);
    for (node, target) in targets.transforms.iter().enumerate() {
        meter.count_decode(1)?;
        if target.is_none() {
            filtered.push(node)?;
        }
    }
    let mut facts = MeteredVec::with_capacity(meter, targets.renderers.len())?;
    for row in targets.renderers {
        let mut channels = MeteredVec::with_capacity(meter, row.channels.len())?;
        meter.count_decode(1 + row.channels.len() as u64)?;
        for channel in row.channels.as_slice() {
            channels.push(RendererChannel {
                name: channel.name,
                hash: channel.hash,
                target_slot: channel.target_slot,
                frame: channel.frame,
                full_weight: channel.full_weight,
            })?;
        }
        facts.push(channels)?;
    }
    let mut renderers = MeteredVec::new(meter);
    for (row, channels) in targets.renderers.iter().zip(facts.as_slice()) {
        renderers.push(RendererChannels {
            renderer: row.renderer,
            attachment: Attachment::Present(channels.as_slice()),
        })?;
    }
    for &renderer in targets.filtered_renderers {
        meter.count_decode(1)?;
        renderers.push(RendererChannels {
            renderer,
            attachment: Attachment::Filtered,
        })?;
    }
    let detached = detached_ancestors(graph, targets, meter)?;
    for &(rig, object) in selected {
        meter.count_decode(1)?;
        let mut index = None;
        for (i, clip) in clips.as_slice().iter().enumerate() {
            if same(&clip.object, object, meter)? {
                index = Some(i);
                break;
            }
        }
        let index = if let Some(index) = index {
            index
        } else {
            let index = clips.len();
            clips.push(read_clip(object, meter)?)?;
            index
        };
        let clip = clips
            .as_slice()
            .get(index)
            .ok_or(ExportError::MissingClipSelection)?;
        let paths = RigPaths::new(graph, rig.node, meter)?;
        let bound = bind_clip(
            clip,
            &paths,
            filtered.as_slice(),
            renderers.as_slice(),
            meter,
        )?;
        for track in bound.tracks() {
            meter.count_decode(1)?;
            if let BindingTarget::Transform { node, .. } = track.target
                && detached.as_slice().get(node) == Some(&true)
            {
                return Err(ExportError::AnimatedStaticBatch);
            }
        }
        bake::animation(scene, targets, graph, &bound, profile.sample_rate, meter)?;
    }
    Ok(())
}

fn detached_ancestors(
    graph: &Graph,
    targets: CollectionTargets<'_>,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<bool>, ExportError> {
    let mut marked = MeteredVec::with_capacity(meter, graph.nodes().len())?;
    meter.count_decode(graph.nodes().len() as u64)?;
    for _ in graph.nodes() {
        marked.push(false)?;
    }
    for renderer in targets.renderers {
        if !renderer.detached_static_batch {
            continue;
        }
        let mut current = Some(renderer.transform);
        while let Some(node) = current {
            meter.count_decode(1)?;
            let flag = marked
                .as_mut_slice()
                .get_mut(node)
                .ok_or(AnimError::MissingPath)?;
            if *flag {
                break;
            }
            *flag = true;
            current = graph
                .nodes()
                .get(node)
                .ok_or(AnimError::MissingPath)?
                .parent;
        }
    }
    Ok(marked)
}

fn same(a: &ObjectRef, b: &ObjectRef, meter: &Meter) -> Result<bool, ChargeError> {
    let a_file = FileOccurrence::of(a.file());
    let b_file = FileOccurrence::of(b.file());
    meter.count_decode(1 + a_file.path_len().min(b_file.path_len()) as u64)?;
    Ok(a.path_id() == b.path_id() && a_file == b_file)
}

fn clip_field<'a>(
    clip: &'a Clip,
    field: &[u8],
    meter: &Meter,
) -> Result<Option<&'a Value>, ChargeError> {
    if let Value::Dict(dict) = clip.serialized().value() {
        meter.count_decode((dict.entries().len() as u64).saturating_mul(field.len() as u64))?;
        Ok(clip.serialized().field(dict, field))
    } else {
        Ok(None)
    }
}

const fn send_sync<T: Send + Sync>() {}
const _: () = send_sync::<Collected>();
const _: () = send_sync::<ExportError>();
