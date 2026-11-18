//! Borrow the collection's instance and Transform maps during animation.
use super::{Arc, Cached, Meter, MeteredVec, ObjectRef, Result, invalid, morph};
use unityrs_scene::NodeId;

/// A last-frame morph target in emitted mesh order.
#[derive(Clone, Copy, Debug)]
pub struct MorphChannel<'a> {
    pub name: &'a [u8],
    pub hash: u32,
    pub target_slot: usize,
    pub frame: usize,
    pub full_weight: f32,
    pub frame_count: i32,
}
const _: () = crate::send_sync::<MorphChannel<'static>>();

/// The actual mesh instance may be separate from its source Transform.
#[derive(Debug)]
pub struct RendererAttachment<'a> {
    pub renderer: &'a ObjectRef,
    pub transform: NodeId,
    pub attachment: usize,
    pub detached_static_batch: bool,
    pub channels: MeteredVec<MorphChannel<'a>>,
}
const _: () = crate::send_sync::<RendererAttachment<'static>>();

#[derive(Clone, Copy, Debug)]
pub struct CollectionTargets<'a> {
    pub transforms: &'a [Option<usize>],
    pub renderers: &'a [RendererAttachment<'a>],
    pub filtered_renderers: &'a [&'a ObjectRef],
}
const _: () = crate::send_sync::<CollectionTargets<'static>>();

pub(super) struct Instance<'a> {
    pub renderer: &'a ObjectRef,
    pub transform: NodeId,
    pub node: usize,
    pub source: Option<usize>,
    pub batched: bool,
}

pub(super) fn attachments<'a>(
    instances: &[Instance<'a>],
    sources: &'a [Cached],
    meter: &Arc<Meter>,
) -> Result<MeteredVec<RendererAttachment<'a>>> {
    let mut out = MeteredVec::with_capacity(meter, instances.len())?;
    for instance in instances {
        meter.count_decode(1)?;
        let mut channels = MeteredVec::new(meter);
        if let Some(index) = instance.source {
            let mesh = &sources
                .get(index)
                .ok_or_else(|| invalid("morph source"))?
                .geometry
                .source;
            let shapes = &mesh.blend_shapes;
            channels.reserve_exact(shapes.channels.len())?;
            for (target_slot, channel) in shapes.channels.as_slice().iter().enumerate() {
                meter.count_decode(1)?;
                let (frame, full_weight) = morph::endpoint(mesh, channel)?;
                channels.push(MorphChannel {
                    name: channel.name.as_bytes(),
                    hash: channel.name_hash,
                    target_slot,
                    frame,
                    full_weight,
                    frame_count: channel.frame_count,
                })?;
            }
        }
        out.push(RendererAttachment {
            renderer: instance.renderer,
            transform: instance.transform,
            attachment: instance.node,
            detached_static_batch: instance.batched,
            channels,
        })?;
    }
    Ok(out)
}
