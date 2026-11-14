//! Fixed schema order, preserving array order and model references via layout IDs.
use crate::json::Json;
use crate::validate::invalid;
use crate::{Attribute, Extras, GltfError, Kind, Layout, Mime, Scene, Text};
type Result<T> = std::result::Result<T, GltfError>;
fn name(j: &mut Json<'_>, n: Option<&Text>) -> Result<()> {
    if let Some(n) = n {
        j.raw(",\"name\":")?;
        j.string(n.as_bytes())?;
    }
    Ok(())
}
fn extras(j: &mut Json<'_>, e: &Extras) -> Result<()> {
    if !e.is_empty() {
        j.raw(",\"extras\":{")?;
        for (i, (k, v)) in e.as_slice().iter().enumerate() {
            if i > 0 {
                j.raw(",")?;
            }
            j.string(k.as_bytes())?;
            j.raw(":")?;
            j.string(v.as_bytes())?;
        }
        j.raw("}")?;
    }
    Ok(())
}
fn aid(l: &Layout, id: usize) -> Result<usize> {
    l.accessors
        .as_slice()
        .get(id)
        .and_then(|a| a.id)
        .ok_or_else(|| invalid("omitted accessor reference"))
}
#[allow(clippy::too_many_lines)]
pub(crate) fn document(s: &Scene, l: &Layout, j: &mut Json<'_>) -> Result<()> {
    j.raw("{\"asset\":{\"version\":\"2.0\",\"generator\":\"unityrs-gltf\",\"extras\":{")?;
    let r = &l.report;
    for (i, (key, value)) in [
        ("normal_components", r.normal_components),
        ("joint_components", r.joint_components),
        ("tangent_components", r.tangent_components),
        ("weight_components", r.weight_components),
        ("zero_weight_rows", r.zero_weight_rows),
    ]
    .iter()
    .enumerate()
    {
        if i > 0 {
            j.raw(",")?;
        }
        j.string(key.as_bytes())?;
        j.raw(":")?;
        j.number(value)?;
    }
    if r.rotation_components != 0 {
        j.raw(",\"rotation_components\":")?;
        j.number(r.rotation_components)?;
    }
    j.raw(",\"issues\":[")?;
    for (i, issue) in r.issues.as_slice().iter().enumerate() {
        if i > 0 {
            j.raw(",")?;
        }
        j.raw("{\"mesh\":")?;
        if let Some(id) = issue.mesh {
            j.number(id)?;
        } else {
            j.raw("null")?;
        }
        j.raw(",\"primitive\":")?;
        if let Some(id) = issue.primitive {
            j.number(id)?;
        } else {
            j.raw("null")?;
        }
        j.raw(",\"source_mesh\":")?;
        j.number(issue.source_mesh)?;
        j.raw(",\"source_primitive\":")?;
        j.number(issue.source_primitive)?;
        j.raw(",\"reason\":")?;
        j.string(match issue.reason {
            crate::Reason::NonFinitePosition => b"non_finite_position",
            crate::Reason::InvalidNormal => b"invalid_normal",
            crate::Reason::InvalidTangent => b"invalid_tangent",
        })?;
        j.raw("}")?;
    }
    j.raw("]}}")?;
    if !s.roots.is_empty() {
        j.raw(",\"scene\":0,\"scenes\":[{\"nodes\":")?;
        j.indices(s.roots.as_slice())?;
        j.raw("}]")?;
    }
    nodes(s, l, j)?;
    meshes(s, l, j)?;
    accessors(s, l, j)?;
    if !l.views.is_empty() {
        j.raw(",\"bufferViews\":[")?;
        for (i, v) in l.views.as_slice().iter().enumerate() {
            if i > 0 {
                j.raw(",")?;
            }
            j.raw("{\"buffer\":0,\"byteOffset\":")?;
            j.number(v.byte_offset)?;
            j.raw(",\"byteLength\":")?;
            j.number(v.byte_length)?;
            if let Some(t) = v.target {
                j.raw(",\"target\":")?;
                j.number(t)?;
            }
            j.raw("}")?;
        }
        j.raw("],\"buffers\":[{\"byteLength\":")?;
        j.number(l.bin_length)?;
        j.raw("}]")?;
    }
    if !s.skins.is_empty() {
        j.raw(",\"skins\":[")?;
        for (i, skin) in s.skins.as_slice().iter().enumerate() {
            if i > 0 {
                j.raw(",")?;
            }
            j.raw("{\"joints\":")?;
            j.indices(skin.joints.as_slice())?;
            if let Some(id) = skin.inverse_bind_matrices {
                j.raw(",\"inverseBindMatrices\":")?;
                j.number(aid(l, id)?)?;
            }
            if let Some(id) = skin.skeleton {
                j.raw(",\"skeleton\":")?;
                j.number(id)?;
            }
            name(j, skin.name.as_ref())?;
            j.raw("}")?;
        }
        j.raw("]")?;
    }
    if !s.materials.is_empty() {
        j.raw(",\"materials\":[")?;
        for (i, m) in s.materials.as_slice().iter().enumerate() {
            if i > 0 {
                j.raw(",")?;
            }
            // A leading property lets the optional name/extras helpers keep
            // one implementation; these are core material defaults.
            j.raw("{\"doubleSided\":false")?;
            name(j, m.name.as_ref())?;
            extras(j, &m.extras)?;
            j.raw("}")?;
        }
        j.raw("]")?;
    }
    if !s.images.is_empty() {
        j.raw(",\"images\":[")?;
        for (i, image) in s.images.as_slice().iter().enumerate() {
            if i > 0 {
                j.raw(",")?;
            }
            j.raw("{\"bufferView\":")?;
            j.number(
                l.image_views
                    .as_slice()
                    .get(i)
                    .ok_or_else(|| invalid("image view"))?,
            )?;
            j.raw(",\"mimeType\":")?;
            j.string(match image.mime {
                Mime::Png => b"image/png",
                Mime::Jpeg => b"image/jpeg",
            })?;
            name(j, image.name.as_ref())?;
            j.raw("}")?;
        }
        j.raw("]")?;
    }
    if !s.textures.is_empty() {
        j.raw(",\"textures\":[")?;
        for (i, t) in s.textures.as_slice().iter().enumerate() {
            if i > 0 {
                j.raw(",")?;
            }
            j.raw("{\"source\":")?;
            j.number(t.image)?;
            j.raw("}")?;
        }
        j.raw("]")?;
    }
    extras(j, &s.extras)?;
    j.raw("}")
}
fn nodes(s: &Scene, l: &Layout, j: &mut Json<'_>) -> Result<()> {
    if s.nodes.is_empty() {
        return Ok(());
    }
    j.raw(",\"nodes\":[")?;
    for (i, n) in s.nodes.as_slice().iter().enumerate() {
        if i > 0 {
            j.raw(",")?;
        }
        j.raw("{\"translation\":")?;
        j.floats(&n.translation)?;
        j.raw(",\"rotation\":")?;
        j.floats(
            l.rotations
                .as_slice()
                .get(i)
                .ok_or_else(|| invalid("node rotation"))?,
        )?;
        j.raw(",\"scale\":")?;
        j.floats(&n.scale)?;
        let mesh = n
            .mesh
            .and_then(|id| l.meshes.as_slice().get(id))
            .and_then(|m| m.id);
        if let Some(id) = mesh {
            j.raw(",\"mesh\":")?;
            j.number(id)?;
            if let Some(id) = n.skin {
                j.raw(",\"skin\":")?;
                j.number(id)?;
            }
            if !n.weights.is_empty() {
                j.raw(",\"weights\":")?;
                j.floats(n.weights.as_slice())?;
            }
        }
        if !n.children.is_empty() {
            j.raw(",\"children\":")?;
            j.indices(n.children.as_slice())?;
        }
        name(j, n.name.as_ref())?;
        extras(j, &n.extras)?;
        j.raw("}")?;
    }
    j.raw("]")
}
fn semantic(j: &mut Json<'_>, k: Attribute) -> Result<()> {
    match k {
        Attribute::Position => j.raw("\"POSITION\""),
        Attribute::Normal => j.raw("\"NORMAL\""),
        Attribute::Tangent => j.raw("\"TANGENT\""),
        Attribute::Color => j.raw("\"COLOR_0\""),
        Attribute::Joints => j.raw("\"JOINTS_0\""),
        Attribute::Weights => j.raw("\"WEIGHTS_0\""),
        Attribute::TexCoord(n) => {
            j.raw("\"TEXCOORD_")?;
            j.number(n)?;
            j.raw("\"")
        }
    }
}
#[allow(clippy::too_many_lines)]
fn meshes(s: &Scene, l: &Layout, j: &mut Json<'_>) -> Result<()> {
    j.work(l.meshes.len())?;
    if l.meshes.as_slice().iter().all(|m| m.id.is_none()) {
        return Ok(());
    }
    j.raw(",\"meshes\":[")?;
    let mut comma = false;
    for (m, ml) in s.meshes.as_slice().iter().zip(l.meshes.as_slice()) {
        if ml.id.is_none() {
            continue;
        }
        if comma {
            j.raw(",")?;
        }
        comma = true;
        j.raw("{\"primitives\":[")?;
        let mut pc = false;
        for (p, pl) in m.primitives.as_slice().iter().zip(ml.primitives.as_slice()) {
            if !pl.keep {
                continue;
            }
            if pc {
                j.raw(",")?;
            }
            pc = true;
            j.raw("{\"attributes\":{")?;
            let mut ac = false;
            for &(kind, id) in p.attributes.as_slice() {
                if (kind == Attribute::Normal && !pl.normal)
                    || (kind == Attribute::Tangent && !pl.tangent)
                {
                    continue;
                }
                if ac {
                    j.raw(",")?;
                }
                ac = true;
                semantic(j, kind)?;
                j.raw(":")?;
                j.number(aid(l, id)?)?;
            }
            j.raw("}")?;
            if let Some(id) = p.indices {
                j.raw(",\"indices\":")?;
                j.number(aid(l, id)?)?;
            }
            if let Some(id) = p.material {
                j.raw(",\"material\":")?;
                j.number(id)?;
            }
            if !p.targets.is_empty() {
                j.raw(",\"targets\":[")?;
                for (i, t) in p.targets.as_slice().iter().enumerate() {
                    if i > 0 {
                        j.raw(",")?;
                    }
                    j.raw("{")?;
                    let mut tc = false;
                    for (slot, (id, key)) in t
                        .iter()
                        .zip([b"POSITION".as_slice(), b"NORMAL", b"TANGENT"])
                        .enumerate()
                    {
                        if (slot == 1 && !pl.normal) || (slot == 2 && !pl.tangent) {
                            continue;
                        }
                        if let Some(id) = id {
                            if tc {
                                j.raw(",")?;
                            }
                            tc = true;
                            j.string(key)?;
                            j.raw(":")?;
                            j.number(aid(l, *id)?)?;
                        }
                    }
                    j.raw("}")?;
                }
                j.raw("]")?;
            }
            extras(j, &p.extras)?;
            j.raw("}")?;
        }
        j.raw("]")?;
        name(j, m.name.as_ref())?;
        if m.target_names.is_empty() {
            extras(j, &m.extras)?;
        } else {
            j.raw(",\"extras\":{\"targetNames\":[")?;
            for (i, n) in m.target_names.as_slice().iter().enumerate() {
                if i > 0 {
                    j.raw(",")?;
                }
                j.string(n.as_bytes())?;
            }
            j.raw("]")?;
            for (k, v) in m.extras.as_slice() {
                j.raw(",")?;
                j.string(k.as_bytes())?;
                j.raw(":")?;
                j.string(v.as_bytes())?;
            }
            j.raw("}")?;
        }
        j.raw("}")?;
    }
    j.raw("]")
}
fn accessors(s: &Scene, l: &Layout, j: &mut Json<'_>) -> Result<()> {
    if l.accessor_order.is_empty() {
        return Ok(());
    }
    j.raw(",\"accessors\":[")?;
    let mut comma = false;
    for &id in l.accessor_order.as_slice() {
        let a = s
            .accessors
            .as_slice()
            .get(id)
            .ok_or_else(|| invalid("accessor index"))?;
        let al = l
            .accessors
            .as_slice()
            .get(id)
            .ok_or_else(|| invalid("accessor layout"))?;
        if al.id.is_none() {
            continue;
        }
        if comma {
            j.raw(",")?;
        }
        comma = true;
        j.raw("{\"componentType\":")?;
        j.number(a.data.component())?;
        j.raw(",\"count\":")?;
        j.number(a.count)?;
        j.raw(",\"type\":")?;
        j.string(a.kind.shape().as_bytes())?;
        if let Some(view) = al.view {
            j.raw(",\"bufferView\":")?;
            j.number(view)?;
        }
        if matches!(a.kind, Kind::Position | Kind::MorphPosition)
            || a.sparse_indices
                .as_ref()
                .is_some_and(unityrs_core::limits::MeteredVec::is_empty)
        {
            for (key, values) in [("min", &al.min), ("max", &al.max)] {
                j.raw(",")?;
                j.string(key.as_bytes())?;
                j.raw(":[")?;
                for (i, &v) in values.iter().take(a.kind.width()).enumerate() {
                    if i > 0 {
                        j.raw(",")?;
                    }
                    j.float(f64::from(v))?;
                }
                j.raw("]")?;
            }
        }
        if let Some((indices, values)) = al.sparse {
            j.raw(",\"sparse\":{\"count\":")?;
            j.number(
                a.sparse_indices
                    .as_ref()
                    .ok_or_else(|| invalid("sparse indices"))?
                    .len(),
            )?;
            j.raw(",\"indices\":{\"bufferView\":")?;
            j.number(indices)?;
            j.raw(",\"componentType\":5125},\"values\":{\"bufferView\":")?;
            j.number(values)?;
            j.raw("}}")?;
        }
        j.raw("}")?;
    }
    j.raw("]")
}
