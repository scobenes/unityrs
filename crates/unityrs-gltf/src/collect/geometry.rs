//! One accessor set per source mesh; binding/range variants reuse those ids.
use super::{Arc, Meter, MeteredVec, Result, Scene, Text, append_fmt, extra, invalid, raw_text};
use crate::{Accessor, Attribute, Data, Kind, Mesh, Primitive};
use unityrs_meshdecode::VertexArray;
pub(super) struct Geometry {
    pub source: unityrs_mesh::Mesh,
    pub padded_rows: u64,
    pub attributes: MeteredVec<(Attribute, usize)>,
    pub targets: MeteredVec<[Option<usize>; 3]>,
    pub indices: MeteredVec<Option<usize>>,
    pub inverse_bind: Option<usize>,
}
pub(super) fn accessor(
    scene: &mut Scene,
    kind: Kind,
    count: usize,
    data: Data,
    sparse: Option<MeteredVec<u32>>,
) -> Result<usize> {
    let id = scene.accessors.len();
    scene.accessors.push(Accessor {
        kind,
        count,
        data,
        sparse_indices: sparse,
    })?;
    Ok(id)
}
fn attribute(
    scene: &mut Scene,
    a: &VertexArray,
    kind: Kind,
    count: usize,
    meter: &Arc<Meter>,
) -> Result<usize> {
    let width = kind.width();
    let padded = matches!(kind, Kind::Joints | Kind::Weights) && matches!(a.dimension(), 1 | 2);
    if (a.dimension() < width && !padded) || a.is_empty() {
        return Err(invalid("vertex attribute width/count"));
    }
    if a.len() != count {
        return Err(invalid("attribute count"));
    }
    meter.count_decode(a.len() as u64 * width as u64 * 8)?;
    let mut floats = MeteredVec::new(meter);
    let mut joints = MeteredVec::new(meter);
    if kind == Kind::Joints {
        joints.reserve_exact(a.len() * width)?;
    } else {
        floats.reserve_exact(a.len() * width)?;
    }
    for row in 0..a.len() {
        for col in 0..width {
            let mut v = if padded && col >= a.dimension() {
                0.
            } else {
                a.component(row, col)
                    .ok_or_else(|| invalid("partial vertex row"))?
                    .as_f64()
            };
            if kind == Kind::Joints {
                if !v.is_finite() || v < 0. || v > f64::from(u16::MAX) || v.fract() != 0. {
                    return Err(invalid("joint index"));
                }
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                joints.push(v as u16)?;
            } else {
                if (col == 0 && matches!(kind, Kind::Position | Kind::Normal | Kind::Tangent))
                    || (col == 3 && kind == Kind::Tangent)
                {
                    v = -v;
                }
                if col == 1 && kind == Kind::TexCoord {
                    v = 1. - v;
                }
                floats.push(v)?;
            }
        }
    }
    accessor(
        scene,
        kind,
        a.len(),
        if kind == Kind::Joints {
            Data::U16(joints)
        } else {
            Data::Float(floats)
        },
        None,
    )
}
#[allow(clippy::too_many_lines)]
pub(super) fn build(
    scene: &mut Scene,
    source: unityrs_mesh::Mesh,
    normalise: bool,
    meter: &Arc<Meter>,
) -> Result<Geometry> {
    let p = &source.processed;
    let count = usize::try_from(p.vertex_count()).map_err(|_| invalid("vertex count"))?;
    let positions = p.vertices().ok_or_else(|| invalid("missing vertices"))?;
    if positions.len() != count {
        return Err(invalid("vertex count"));
    }
    let mut attrs = MeteredVec::new(meter);
    for (semantic, kind, a) in [
        (Attribute::Position, Kind::Position, Some(positions)),
        (Attribute::Normal, Kind::Normal, p.normals()),
        (Attribute::Tangent, Kind::Tangent, p.tangents()),
    ] {
        if kind == Kind::Tangent && p.normals().is_none() {
            continue;
        }
        if let Some(a) = a {
            attrs.push((semantic, attribute(scene, a, kind, count, meter)?))?;
        }
    }
    // glTF demands contiguous UV sets. Missing sets before a present one are
    // explicit zero UVs. Track them so a repair uses only real surviving sets.
    let last = (0..8).rfind(|&i| p.uv(i).is_some());
    for i in 0..last.map_or(0, |i| i + 1) {
        let id = if let Some(a) = p.uv(i) {
            attribute(scene, a, Kind::TexCoord, count, meter)?
        } else {
            meter.count_decode(count as u64 * 16)?;
            let mut v = MeteredVec::with_capacity(meter, count * 2)?;
            for _ in 0..count {
                v.extend_from_slice(&[0., 0.])?;
            }
            let id = accessor(scene, Kind::TexCoord, count, Data::Float(v), None)?;
            scene.collected_uv_fillers.push(id)?;
            id
        };
        attrs.push((
            Attribute::TexCoord(u8::try_from(i).map_err(|_| invalid("UV set"))?),
            id,
        ))?;
    }
    if p.bone_weights().is_some_and(|w| {
        matches!(w.dimension(), 1 | 2)
            && p.bone_indices()
                .is_none_or(|j| j.dimension() != w.dimension())
    }) {
        return Err(invalid("skin attribute width mismatch"));
    }
    let padded_rows = match p.bone_indices() {
        Some(joints) if matches!(joints.dimension(), 1 | 2) => {
            if p.bone_weights()
                .is_some_and(|w| w.dimension() != joints.dimension())
                || (joints.dimension() == 2 && p.bone_weights().is_none())
            {
                return Err(invalid("skin attribute width mismatch"));
            }
            count as u64
        }
        _ => 0,
    };
    for (semantic, kind, a) in [
        (Attribute::Color, Kind::Color, p.colors()),
        (Attribute::Joints, Kind::Joints, p.bone_indices()),
        (Attribute::Weights, Kind::Weights, p.bone_weights()),
    ] {
        if let Some(a) = a {
            attrs.push((semantic, attribute(scene, a, kind, count, meter)?))?;
        }
    }
    if p.bone_indices().is_some_and(|a| a.dimension() == 1) && p.bone_weights().is_none() {
        meter.count_decode(count as u64 * 4)?;
        let mut weights = MeteredVec::with_capacity(meter, count * 4)?;
        for _ in 0..count {
            weights.extend_from_slice(&[1., 0., 0., 0.])?;
        }
        let id = accessor(scene, Kind::Weights, count, Data::Float(weights), None)?;
        attrs.push((Attribute::Weights, id))?;
    }
    let targets = super::morph::build(
        scene,
        &source,
        count,
        p.normals().is_some(),
        p.tangents().is_some() && p.normals().is_some(),
        normalise,
        meter,
    )?;
    let mut indices = MeteredVec::with_capacity(meter, source.submeshes.len())?;
    for (sub, tri) in source
        .submeshes
        .as_slice()
        .iter()
        .zip(source.triangles.submeshes())
    {
        meter.count_decode(tri.len().saturating_mul(16))?;
        if !tri.cut().is_empty() {
            return Err(invalid("partial triangle"));
        }
        if tri.triangles().is_empty() {
            indices.push(None)?;
            continue;
        }
        let mut data = MeteredVec::with_capacity(meter, tri.triangles().len() * 3)?;
        let mut max = 0;
        for &[a, b, c] in tri.triangles() {
            for v in [c, b, a] {
                let v = v
                    .checked_add(sub.base_vertex.unwrap_or(0))
                    .ok_or_else(|| invalid("base vertex overflow"))?;
                if v as usize >= count {
                    return Err(invalid("vertex index"));
                }
                max = max.max(v);
                data.push(v)?;
            }
        }
        let len = data.len();
        let data = if max < u32::from(u16::MAX) {
            let mut small = MeteredVec::with_capacity(meter, len)?;
            meter.count_decode(len as u64 * 2)?;
            for &v in data.as_slice() {
                small.push(u16::try_from(v).map_err(|_| invalid("index width"))?)?;
            }
            Data::U16(small)
        } else {
            Data::U32(data)
        };
        indices.push(Some(accessor(scene, Kind::Indices, len, data, None)?))?;
    }
    if indices.len() != source.submeshes.len() {
        return Err(invalid("submesh triangles"));
    }
    let inverse_bind = super::skin::inverse_bind(scene, &source, meter)?;
    Ok(Geometry {
        source,
        padded_rows,
        inverse_bind,
        attributes: attrs,
        targets,
        indices,
    })
}
pub(super) fn mesh(
    scene: &mut Scene,
    g: &Geometry,
    selected: &[usize],
    skin: bool,
    skin_attributes: Option<[usize; 2]>,
    materials: &[Option<usize>],
    meter: &Arc<Meter>,
) -> Result<Option<usize>> {
    // Both per-renderer ranges and keep-combined unions were checked against
    // this immutable geometry. A range failure here is an internal invariant.
    let mut primitives = MeteredVec::new(meter);
    meter.count_decode(selected.len() as u64)?;
    for (slot, &i) in selected.iter().enumerate() {
        let Some(indices) = g
            .indices
            .as_slice()
            .get(i)
            .copied()
            .ok_or_else(|| invalid("submesh range"))?
        else {
            continue;
        };
        let mut attributes = MeteredVec::new(meter);
        meter.count_decode(g.attributes.len() as u64)?;
        let material = materials.get(slot).copied().flatten();
        let ignore_colors = material
            .and_then(|id| scene.materials.as_slice().get(id))
            .is_some_and(|m| !m.vertex_colors);
        for &(kind, id) in g.attributes.as_slice() {
            if !skin && matches!(kind, Attribute::Joints | Attribute::Weights) {
                continue;
            }
            let id = match (kind, skin_attributes) {
                (Attribute::Joints, Some([j, _])) => j,
                (Attribute::Weights, Some([_, w])) => w,
                _ => id,
            };
            let kind = if kind == Attribute::Color && ignore_colors {
                Attribute::CustomColor
            } else {
                kind
            };
            attributes.push((kind, id))?;
        }
        primitives.push(Primitive {
            attributes,
            indices: Some(indices),
            material,
            targets: crate::array(meter, g.targets.as_slice())?,
            extras: MeteredVec::new(meter),
        })?;
    }
    if primitives.is_empty() {
        return Ok(None);
    }
    let mut names = MeteredVec::new(meter);
    for channel in g.source.blend_shapes.channels.as_slice() {
        names.push(raw_text(meter, channel.name.as_bytes())?)?;
    }
    let mut extras = MeteredVec::new(meter);
    if !g.source.blend_shapes.channels.is_empty() {
        let mut frames = Text(MeteredVec::new(meter));
        for channel in g.source.blend_shapes.channels.as_slice() {
            append_fmt(
                &mut frames,
                meter,
                format_args!("{}:{};", channel.frame_index, channel.frame_count),
            )?;
        }
        extra(&mut extras, meter, "blend_shape_frames", frames)?;
        let mut weights = Text(MeteredVec::new(meter));
        for &w in g.source.blend_shapes.full_weights.as_slice() {
            append_fmt(&mut weights, meter, format_args!("{w};"))?;
        }
        extra(&mut extras, meter, "blend_shape_full_weights", weights)?;
    }
    let id = scene.meshes.len();
    scene.meshes.push(Mesh {
        name: Some(raw_text(meter, g.source.name.as_bytes())?),
        primitives,
        target_names: names,
        extras,
    })?;
    Ok(Some(id))
}
