//! Sparse last-frame targets, sorted without dense vertex staging.
use super::{Arc, Meter, MeteredVec, Result, Scene, invalid};
use crate::{Data, Kind};
pub(super) fn build(
    scene: &mut Scene,
    mesh: &unityrs_mesh::Mesh,
    count: usize,
    normals: bool,
    tangents: bool,
    normalise: bool,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<[Option<usize>; 3]>> {
    let shapes = &mesh.blend_shapes;
    let mut out = MeteredVec::new(meter);
    for channel in shapes.channels.as_slice() {
        meter.count_decode(1)?;
        frame_weight(mesh, channel)?;
        let start =
            usize::try_from(channel.frame_index).map_err(|_| invalid("morph frame range"))?;
        let length =
            usize::try_from(channel.frame_count).map_err(|_| invalid("morph frame range"))?;
        let end = start
            .checked_add(length)
            .ok_or_else(|| invalid("morph frame range"))?;
        let frame = shapes
            .frames
            .as_slice()
            .get(start..end)
            .and_then(|s| s.last())
            .ok_or_else(|| invalid("morph frame range"))?;
        let first = frame.first_vertex as usize;
        let end = first
            .checked_add(frame.vertex_count as usize)
            .ok_or_else(|| invalid("morph vertex range"))?;
        let vertices = shapes
            .vertices
            .as_slice()
            .get(first..end)
            .ok_or_else(|| invalid("morph vertex range"))?;
        meter.count_decode(
            (vertices.len() as u64).saturating_mul(16 + u64::from(vertices.len().bit_width())),
        )?;
        let mut sorted = crate::array(meter, vertices)?;
        sorted.as_mut_slice().sort_unstable_by_key(|v| v.index);
        if sorted.as_slice().iter().any(|v| v.index as usize >= count)
            || sorted
                .as_slice()
                .windows(2)
                .any(|w| w.first().map(|v| v.index) == w.get(1).map(|v| v.index))
        {
            return Err(invalid("morph sparse index"));
        }
        let mut target = [None; 3];
        for (slot, kind, enabled) in [
            (0, Kind::MorphPosition, true),
            (1, Kind::MorphNormal, frame.has_normals && normals),
            (2, Kind::MorphTangent, frame.has_tangents && tangents),
        ] {
            if !enabled {
                continue;
            }
            meter.count_decode(vertices.len() as u64 * 28)?;
            let mut indices = MeteredVec::with_capacity(meter, vertices.len())?;
            let mut values = MeteredVec::with_capacity(meter, vertices.len() * 3)?;
            for v in sorted.as_slice() {
                indices.push(v.index)?;
                let [x, y, z] = match slot {
                    1 => v.normal,
                    2 => v.tangent,
                    _ => v.vertex,
                };
                if normalise && [x, y, z].iter().any(|v| !v.is_finite()) {
                    return Err(invalid("non-finite morph delta"));
                }
                values.extend_from_slice(&[-f64::from(x), f64::from(y), f64::from(z)])?;
            }
            *target.get_mut(slot).ok_or_else(|| invalid("morph slot"))? = Some(
                super::geometry::accessor(scene, kind, count, Data::Float(values), Some(indices))?,
            );
        }
        out.push(target)?;
    }
    Ok(out)
}

pub(super) fn frame_weight(
    mesh: &unityrs_mesh::Mesh,
    channel: &unityrs_mesh::BlendShapeChannel,
) -> Result<f64> {
    let end = channel
        .frame_index
        .checked_add(channel.frame_count)
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| usize::try_from(i).ok())
        .ok_or_else(|| invalid("morph frame weight"))?;
    let value = *mesh
        .blend_shapes
        .full_weights
        .as_slice()
        .get(end)
        .ok_or_else(|| invalid("missing morph frame weight"))?;
    if !value.is_finite() || value == 0. {
        return Err(invalid("invalid morph frame weight"));
    }
    Ok(f64::from(value))
}
