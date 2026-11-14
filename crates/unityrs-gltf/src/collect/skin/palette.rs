//! A duplicate Transform needs one palette entry and matching vertex influences.
use super::{
    Arc, Data, Graph, Kind, Meter, MeteredVec, Result, Scene, SkinInfo, geometry, invalid, joint,
};
pub(super) struct Palette {
    pub joints: MeteredVec<usize>,
    pub inverse_bind: usize,
    pub attributes: Option<[usize; 2]>,
    pub duplicates: usize,
    pub influences: u64,
    pub bind_rows: u64,
}
#[allow(clippy::too_many_lines)]
pub(super) fn build(
    scene: &mut Scene,
    g: &geometry::Geometry,
    s: &SkinInfo,
    graph: &Graph,
    ids: &[Option<usize>],
    normalise: bool,
    meter: &Arc<Meter>,
) -> Result<Palette> {
    let mut joints = MeteredVec::with_capacity(meter, s.bones.len())?;
    let mut originals = MeteredVec::with_capacity(meter, s.bones.len())?;
    let mut remap = MeteredVec::with_capacity(meter, s.bones.len())?;
    for (index, bone) in s.bones.as_slice().iter().enumerate() {
        let graph_id = joint(graph, bone.as_ref(), meter)?;
        let id = ids
            .get(graph_id)
            .copied()
            .flatten()
            .ok_or_else(|| invalid("skin joint omitted"))?;
        meter.count_decode(joints.len() as u64 + 16)?;
        let palette = if let Some(existing) = joints.as_slice().iter().position(|&j| j == id) {
            let first = *originals
                .as_slice()
                .get(existing)
                .ok_or_else(|| invalid("palette entry"))?;
            let first_bind = g
                .source
                .bind_poses
                .as_slice()
                .get(first * 16..first * 16 + 16)
                .ok_or_else(|| invalid("inverse bind range"))?;
            let next_bind = g
                .source
                .bind_poses
                .as_slice()
                .get(index * 16..index * 16 + 16)
                .ok_or_else(|| invalid("inverse bind range"))?;
            if !first_bind
                .iter()
                .zip(next_bind)
                .enumerate()
                .all(|(slot, (&a, &b))| {
                    if normalise && slot >= 12 {
                        let want = if slot == 15 { 1. } else { 0. };
                        (a - want).abs() <= 1e-6 && (b - want).abs() <= 1e-6
                    } else {
                        // Outside the repaired affine row, palette entries
                        // must agree exactly (signed zero remains equivalent).
                        #[allow(clippy::float_cmp)]
                        {
                            a == b
                        }
                    }
                })
            {
                return Err(invalid("duplicate_joint_bind_conflict"));
            }
            existing
        } else {
            let next = joints.len();
            joints.push(id)?;
            originals.push(index)?;
            next
        };
        remap.push(u16::try_from(palette).map_err(|_| invalid("skin palette too large"))?)?;
    }
    let duplicates = s.bones.len() - joints.len();
    let inverse_bind = g
        .inverse_bind
        .ok_or_else(|| invalid("missing inverse bind matrices"))?;
    let Data::Float(matrices) = &scene
        .accessors
        .as_slice()
        .get(inverse_bind)
        .ok_or_else(|| invalid("inverse bind accessor"))?
        .data
    else {
        return Err(invalid("inverse bind data"));
    };
    let mut bind_rows = 0;
    for matrix in g.source.bind_poses.as_slice().as_chunks::<16>().0 {
        meter.count_decode(16)?;
        if normalise {
            if matrix.iter().any(|v| !v.is_finite())
                || matrix
                    .iter()
                    .skip(12)
                    .zip([0., 0., 0., 1.])
                    .any(|(value, want)| (*value - want).abs() > 1e-6)
            {
                return Err(invalid("invalid_bind_matrix"));
            }
            bind_rows += u64::from(matrix.get(12..16) != Some([0., 0., 0., 1.].as_slice()));
        }
    }
    let inverse_bind = if duplicates > 0 || bind_rows > 0 {
        let mut values = MeteredVec::with_capacity(meter, joints.len() * 16)?;
        for &index in originals.as_slice() {
            meter.count_decode(16)?;
            let matrix = matrices
                .as_slice()
                .get(index * 16..index * 16 + 16)
                .ok_or_else(|| invalid("inverse bind range"))?;
            for (slot, &value) in matrix.iter().enumerate() {
                values.push(if normalise {
                    match slot {
                        3 | 7 | 11 => 0.,
                        15 => 1.,
                        _ => value,
                    }
                } else {
                    value
                })?;
            }
        }
        geometry::accessor(
            scene,
            Kind::InverseBind,
            joints.len(),
            Data::Float(values),
            None,
        )?
    } else {
        inverse_bind
    };
    let (joint_id, weight_id) = (
        g.attributes
            .as_slice()
            .iter()
            .find(|(k, _)| *k == crate::Attribute::Joints)
            .map(|(_, id)| *id)
            .ok_or_else(|| invalid("joint attribute"))?,
        g.attributes
            .as_slice()
            .iter()
            .find(|(k, _)| *k == crate::Attribute::Weights)
            .map(|(_, id)| *id)
            .ok_or_else(|| invalid("weight attribute"))?,
    );
    let ja = scene
        .accessors
        .as_slice()
        .get(joint_id)
        .ok_or_else(|| invalid("joint accessor"))?;
    let wa = scene
        .accessors
        .as_slice()
        .get(weight_id)
        .ok_or_else(|| invalid("weight accessor"))?;
    let Data::Float(weights) = &wa.data else {
        return Err(invalid("weight data"));
    };
    let mut influences = 0;
    for row in 0..ja.count {
        meter.count_decode(64)?;
        if normalise && crate::normalise::zero_weights(wa, row)? {
            continue;
        }
        let effective = crate::normalise::row(wa, row, normalise)?;
        let mut seen = [u32::MAX; 4];
        for (slot, &weight) in effective.iter().take(4).enumerate() {
            let index = ja
                .data
                .integer(row * 4 + slot)
                .ok_or_else(|| invalid("joint data"))?;
            if (!normalise || weight > 0.) && index as usize >= s.bones.len() {
                return Err(invalid("skin_joint_palette_range"));
            }
            if normalise && weight > 0. && seen.contains(&index) {
                influences += 1;
            }
            if weight > 0. {
                *seen.get_mut(slot).ok_or_else(|| invalid("joint slot"))? = index;
            }
        }
    }
    if duplicates == 0 && influences == 0 {
        return Ok(Palette {
            joints,
            inverse_bind,
            attributes: None,
            duplicates,
            influences,
            bind_rows,
        });
    }
    let count = ja.count;
    let mut js = MeteredVec::with_capacity(meter, count * 4)?;
    let mut ws = MeteredVec::with_capacity(meter, count * 4)?;
    for row in 0..count {
        meter.count_decode(64)?;
        // Use the writer's palette-zero fallback before original f64 weights
        // can survive f32 underflow/overflow and be normalised by the merge.
        if normalise && crate::normalise::zero_weights(wa, row)? {
            js.extend_from_slice(&[0; 4])?;
            ws.extend_from_slice(&[1., 0., 0., 0.])?;
            continue;
        }
        let effective = crate::normalise::row(wa, row, normalise)?;
        let mut out_j = [0u16; 4];
        let mut out_w = [0.; 4];
        let mut used = 0;
        for slot in 0..4 {
            let offset = row * 4 + slot;
            let weight = *weights
                .as_slice()
                .get(offset)
                .ok_or_else(|| invalid("weight range"))?;
            if normalise
                && (!weight.is_finite()
                    || weight <= 0.
                    || effective.get(slot).is_some_and(|v| *v == 0.))
            {
                continue;
            }
            let original = ja
                .data
                .integer(offset)
                .ok_or_else(|| invalid("joint data"))? as usize;
            let mapped = *remap
                .as_slice()
                .get(original)
                .ok_or_else(|| invalid("skin_joint_palette_range"))?;
            let target = out_j
                .get(..used)
                .and_then(|j| j.iter().position(|&j| j == mapped))
                .unwrap_or(used);
            if target == used {
                used += 1;
            }
            *out_j.get_mut(target).ok_or_else(|| invalid("joint slot"))? = mapped;
            *out_w
                .get_mut(target)
                .ok_or_else(|| invalid("weight slot"))? += weight;
        }
        // Keep merged finite f32 weights representable before the writer rounds
        // them again. Otherwise two MAX weights become infinity and get clamped.
        if normalise {
            let total: f64 = out_w.iter().sum();
            if total > 0. {
                for weight in &mut out_w {
                    *weight /= total;
                }
            }
        }
        js.extend_from_slice(&out_j)?;
        ws.extend_from_slice(&out_w)?;
    }
    let j = geometry::accessor(scene, Kind::Joints, count, Data::U16(js), None)?;
    let w = geometry::accessor(scene, Kind::Weights, count, Data::Float(ws), None)?;
    if normalise {
        scene.collected_weight_sources.push((w, weight_id))?;
    }
    Ok(Palette {
        joints,
        inverse_bind,
        attributes: Some([j, w]),
        duplicates,
        influences,
        bind_rows,
    })
}
