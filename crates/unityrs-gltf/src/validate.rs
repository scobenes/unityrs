//! Structural checks run before writing a byte; walks are iterative and metered.
use crate::{Accessor, Attribute, Data, Extras, GltfError, Kind, Scene};
use std::sync::Arc;
use unityrs_core::limits::Meter;
type Result<T> = std::result::Result<T, GltfError>;
pub(crate) fn invalid(s: &'static str) -> GltfError {
    GltfError::Invalid(s)
}
pub(crate) fn accessor(scene: &Scene, id: usize, kind: Kind) -> Result<&Accessor> {
    let a = scene
        .accessors
        .as_slice()
        .get(id)
        .ok_or_else(|| invalid("accessor index"))?;
    if a.kind != kind {
        return Err(invalid("accessor semantic"));
    }
    Ok(a)
}
fn extras(e: &Extras, meter: &Meter) -> Result<()> {
    // Require sorted keys so uniqueness is linear, rather than quadratic.
    let mut previous: Option<&[u8]> = None;
    for (key, value) in e.as_slice() {
        meter.count_decode((key.as_bytes().len() + value.as_bytes().len() + 1) as u64)?;
        if previous.is_some_and(|p| p >= key.as_bytes()) {
            return Err(invalid("extras key order"));
        }
        previous = Some(key.as_bytes());
    }
    Ok(())
}
#[allow(clippy::too_many_lines)]
pub(crate) fn model(s: &Scene, opts: crate::Options, meter: &Arc<Meter>) -> Result<()> {
    hierarchy(s, opts, meter)?;
    extras(&s.extras, meter)?;
    for a in s.accessors.as_slice() {
        meter.count_decode(1)?;
        let rows = a
            .sparse_indices
            .as_ref()
            .map_or(a.count, unityrs_core::limits::MeteredVec::len);
        if a.count == 0 || rows.checked_mul(a.kind.width()) != Some(a.data.len()) {
            return Err(invalid("accessor length"));
        }
        let valid = match (&a.data, a.kind) {
            (Data::U8(_) | Data::U16(_), Kind::Joints | Kind::Indices)
            | (Data::U32(_), Kind::Indices) => true,
            (Data::Float(_), k) => !matches!(k, Kind::Joints | Kind::Indices),
            _ => false,
        };
        if !valid {
            return Err(invalid("accessor component type"));
        }
        if let Some(indices) = &a.sparse_indices {
            if !a.kind.morph() || indices.len() > a.count {
                return Err(invalid("sparse accessor"));
            }
            let mut prev = None;
            for &i in indices.as_slice() {
                meter.count_decode(4)?;
                if i as usize >= a.count || prev.is_some_and(|p| p >= i) {
                    return Err(invalid("sparse index order/range"));
                }
                prev = Some(i);
            }
        }
    }
    for mesh in s.meshes.as_slice() {
        meter.count_decode(1)?;
        extras(&mesh.extras, meter)?;
        for (key, _) in mesh.extras.as_slice() {
            meter.count_decode(key.as_bytes().len() as u64)?;
            if !mesh.target_names.is_empty() && key.as_bytes() == b"targetNames" {
                return Err(invalid("reserved targetNames extra"));
            }
        }
        if mesh.primitives.is_empty() {
            return Err(invalid("empty mesh"));
        }
        let targets = mesh
            .primitives
            .as_slice()
            .first()
            .map_or(0, |p| p.targets.len());
        if !mesh.target_names.is_empty() && mesh.target_names.len() != targets {
            return Err(invalid("target names length"));
        }
        for p in mesh.primitives.as_slice() {
            meter.count_decode(p.attributes.len() as u64 * 3 + 1)?;
            extras(&p.extras, meter)?;
            if p.material.is_some_and(|i| i >= s.materials.len()) {
                return Err(invalid("material index"));
            }
            if p.targets.len() != targets {
                return Err(invalid("target count mismatch"));
            }
            let position = p
                .attributes
                .as_slice()
                .first()
                .filter(|(k, _)| *k == Attribute::Position)
                .ok_or_else(|| invalid("POSITION required"))?
                .1;
            let count = accessor(s, position, Kind::Position)?.count;
            let mut prev = None;
            let mut next_texcoord = 0u16;
            let mut joints = false;
            let mut weights = false;
            for &(kind, id) in p.attributes.as_slice() {
                meter.count_decode(1)?;
                if prev.is_some_and(|p| p >= kind) {
                    return Err(invalid("attribute order"));
                }
                prev = Some(kind);
                if let Attribute::TexCoord(index) = kind {
                    if u16::from(index) != next_texcoord {
                        return Err(invalid(
                            "TEXCOORD sets must start at zero and be contiguous",
                        ));
                    }
                    next_texcoord += 1;
                }
                if accessor(s, id, kind.kind())?.count != count {
                    return Err(invalid("attribute count"));
                }
                joints |= kind == Attribute::Joints;
                weights |= kind == Attribute::Weights;
            }
            let has_normal = p
                .attributes
                .as_slice()
                .iter()
                .any(|(k, _)| *k == Attribute::Normal);
            let has_tangent = p
                .attributes
                .as_slice()
                .iter()
                .any(|(k, _)| *k == Attribute::Tangent);
            if has_tangent && !has_normal {
                return Err(invalid("tangent needs normal"));
            }
            if joints != weights {
                return Err(invalid("joints/weights pair"));
            }
            if let Some(i) = p.indices {
                let a = accessor(s, i, Kind::Indices)?;
                if a.count % 3 != 0 {
                    return Err(invalid("triangle index count"));
                }
                for j in 0..a.count {
                    meter.count_decode(4)?;
                    let v = a.data.integer(j).ok_or_else(|| invalid("index data"))?;
                    if usize::try_from(v).map_or(true, |v| v >= count)
                        || v == match a.data {
                            Data::U8(_) => 255,
                            Data::U16(_) => 65535,
                            _ => u32::MAX,
                        }
                    {
                        return Err(invalid("vertex index range"));
                    }
                }
            } else if count % 3 != 0 {
                return Err(invalid("triangle vertex count"));
            }
            for target in p.targets.as_slice() {
                meter.count_decode(3)?;
                if target.iter().all(Option::is_none) {
                    return Err(invalid("empty morph target"));
                }
                for (id, kind) in
                    target
                        .iter()
                        .zip([Kind::MorphPosition, Kind::MorphNormal, Kind::MorphTangent])
                {
                    if id.is_some()
                        && ((kind == Kind::MorphNormal && !has_normal)
                            || (kind == Kind::MorphTangent && !has_tangent))
                    {
                        return Err(invalid("morph needs base attribute"));
                    }
                    if let Some(id) = id
                        && accessor(s, *id, kind)?.count != count
                    {
                        return Err(invalid("morph count"));
                    }
                }
            }
        }
    }
    // One bitmap for all skins; reset only the joint slots touched by a skin.
    let mut seen = crate::array(meter, &[] as &[u8])?;
    if !s.skins.is_empty() {
        seen.reserve_exact(s.nodes.len())?;
        meter.count_decode(s.nodes.len() as u64)?;
        for _ in 0..s.nodes.len() {
            seen.push(0)?;
        }
    }
    for skin in s.skins.as_slice() {
        meter.count_decode(skin.joints.len() as u64 + 1)?;
        if skin.joints.is_empty()
            || skin.joints.as_slice().iter().any(|&i| i >= s.nodes.len())
            || skin.skeleton.is_some_and(|i| i >= s.nodes.len())
        {
            return Err(invalid("skin joints/skeleton"));
        }
        for &j in skin.joints.as_slice() {
            let slot = seen
                .as_mut_slice()
                .get_mut(j)
                .ok_or_else(|| invalid("joint index"))?;
            if *slot != 0 {
                return Err(invalid("duplicate joint"));
            }
            *slot = 1;
        }
        meter.count_decode(skin.joints.len() as u64)?;
        for &j in skin.joints.as_slice() {
            *seen
                .as_mut_slice()
                .get_mut(j)
                .ok_or_else(|| invalid("joint index"))? = 0;
        }
        if let Some(i) = skin.inverse_bind_matrices
            && accessor(s, i, Kind::InverseBind)?.count != skin.joints.len()
        {
            return Err(invalid("inverse bind count"));
        }
    }
    for n in s.nodes.as_slice() {
        meter.count_decode(32 + n.weights.len() as u64)?;
        extras(&n.extras, meter)?;
        if n.mesh.is_some_and(|i| i >= s.meshes.len()) || n.skin.is_some_and(|i| i >= s.skins.len())
        {
            return Err(invalid("node mesh/skin index"));
        }
        // Exact defaults are required for validator-clean skinned mesh nodes.
        #[allow(clippy::float_cmp)]
        let identity =
            n.translation == [0.; 3] && n.rotation == [0., 0., 0., 1.] && n.scale == [1.; 3];
        if opts.normalise && n.mesh.is_some() && n.skin.is_some() && !identity {
            return Err(invalid(
                "skinned mesh node must have an identity local transform in normalise mode",
            ));
        }
        if n.translation
            .iter()
            .chain(&n.rotation)
            .chain(&n.scale)
            .chain(n.weights.as_slice())
            .any(|x| !x.is_finite())
        {
            return Err(invalid("non-finite node transform/weight"));
        }
        if !opts.normalise && n.rotation.iter().any(|v| v.abs() > 1.) {
            return Err(invalid("node rotation component outside [-1,1]"));
        }
        if n.rotation.iter().all(|v| *v == 0.) {
            return Err(invalid("node rotation must be nonzero"));
        }
        if !opts.normalise {
            let norm = n.rotation.iter().map(|x| x * x).sum::<f64>();
            if (norm - 1.).abs() > 1e-6 {
                return Err(invalid("node rotation must be unit"));
            }
        }
        if let Some(mesh) = n.mesh.and_then(|i| s.meshes.as_slice().get(i)) {
            let targets = mesh
                .primitives
                .as_slice()
                .first()
                .map_or(0, |p| p.targets.len());
            if !n.weights.is_empty() && n.weights.len() != targets {
                return Err(invalid("node morph weight count"));
            }
            meter.count_decode(mesh.primitives.len() as u64)?;
            for p in mesh.primitives.as_slice() {
                meter.count_decode(p.attributes.len() as u64)?;
                if p.attributes
                    .as_slice()
                    .iter()
                    .any(|(k, _)| *k == Attribute::Joints)
                {
                    n.skin
                        .and_then(|i| s.skins.as_slice().get(i))
                        .ok_or_else(|| invalid("skinned attributes need skin"))?;
                } else if n.skin.is_some() {
                    return Err(invalid("skin needs joint attributes"));
                }
            }
        } else if n.skin.is_some() || !n.weights.is_empty() {
            return Err(invalid("skin/weights need mesh"));
        }
    }
    for m in s.materials.as_slice() {
        meter.count_decode(1)?;
        extras(&m.extras, meter)?;
    }
    for i in s.images.as_slice() {
        crate::image::validate(i, meter)?;
    }
    for t in s.textures.as_slice() {
        meter.count_decode(1)?;
        if t.image >= s.images.len() {
            return Err(invalid("texture image index"));
        }
    }
    Ok(())
}
fn hierarchy(s: &Scene, opts: crate::Options, meter: &Arc<Meter>) -> Result<()> {
    let mut parents = unityrs_core::limits::MeteredVec::with_capacity(meter, s.nodes.len())?;
    let mut states = unityrs_core::limits::MeteredVec::with_capacity(meter, s.nodes.len())?;
    meter.count_decode(s.nodes.len() as u64)?;
    for _ in 0..s.nodes.len() {
        parents.push(None)?;
        states.push(0u8)?;
    }
    for (i, n) in s.nodes.as_slice().iter().enumerate() {
        for &child in n.children.as_slice() {
            meter.count_decode(1)?;
            let slot = parents
                .as_mut_slice()
                .get_mut(child)
                .ok_or_else(|| invalid("child index"))?;
            if slot.is_some() {
                return Err(invalid("multiple parents"));
            }
            *slot = Some(i);
        }
    }
    for i in 0..s.nodes.len() {
        let mut current = Some(i);
        while let Some(j) = current {
            meter.count_decode(1)?;
            let state = states
                .as_mut_slice()
                .get_mut(j)
                .ok_or_else(|| invalid("node index"))?;
            if *state == 2 {
                break;
            }
            if *state == 1 {
                return Err(invalid("node cycle"));
            }
            *state = 1;
            current = parents.as_slice().get(j).copied().flatten();
        }
        current = Some(i);
        while let Some(j) = current {
            meter.count_decode(1)?;
            let state = states
                .as_mut_slice()
                .get_mut(j)
                .ok_or_else(|| invalid("node index"))?;
            if *state == 2 {
                break;
            }
            *state = 2;
            current = parents.as_slice().get(j).copied().flatten();
        }
    }
    for &root in s.roots.as_slice() {
        meter.count_decode(1)?;
        if parents.as_slice().get(root).is_none_or(Option::is_some) {
            return Err(invalid("scene root index/parent"));
        }
        let slot = states
            .as_mut_slice()
            .get_mut(root)
            .ok_or_else(|| invalid("root index"))?;
        if *slot == 3 {
            return Err(invalid("duplicate root"));
        }
        *slot = 3;
    }
    if opts.normalise {
        for (node, parent) in s.nodes.as_slice().iter().zip(parents.as_slice()) {
            meter.count_decode(1)?;
            if node.mesh.is_some() && node.skin.is_some() && parent.is_some() {
                return Err(invalid(
                    "skinned mesh node must be a root (no parent) in normalise mode",
                ));
            }
        }
    }
    skin_hierarchy(s, meter, parents.as_slice(), states.as_slice())
}
fn skin_hierarchy(
    s: &Scene,
    meter: &Arc<Meter>,
    parents: &[Option<usize>],
    states: &[u8],
) -> Result<()> {
    meter.count_decode(s.skins.len() as u64)?;
    let mut skin_in_scene = unityrs_core::limits::MeteredVec::with_capacity(meter, s.skins.len())?;
    for skin in s.skins.as_slice() {
        let mut common = None;
        for &joint in skin.joints.as_slice() {
            let mut root = joint;
            let mut has_skeleton = skin.skeleton.is_none();
            loop {
                meter.count_decode(1)?;
                has_skeleton |= skin.skeleton == Some(root);
                if let Some(parent) = parents.get(root).copied().flatten() {
                    root = parent;
                } else {
                    break;
                }
            }
            if common.is_some_and(|r| r != root) || !has_skeleton {
                return Err(invalid("skin joint hierarchy"));
            }
            common = Some(root);
        }
        skin_in_scene.push(common.and_then(|r| states.get(r)).is_some_and(|&v| v == 3))?;
    }
    for (id, node) in s.nodes.as_slice().iter().enumerate() {
        meter.count_decode(1)?;
        if let Some(&joints_in_scene) = node.skin.and_then(|i| skin_in_scene.as_slice().get(i)) {
            let mut root = id;
            while let Some(parent) = parents.get(root).copied().flatten() {
                meter.count_decode(1)?;
                root = parent;
            }
            if states.get(root) == Some(&3) && !joints_in_scene {
                return Err(invalid("skin joints absent from scene"));
            }
        }
    }
    Ok(())
}

/// Check palette bounds and positive influences after G8 omission decisions.
pub(crate) fn skin_attributes(
    s: &Scene,
    layout: &crate::Layout,
    opts: crate::Options,
    meter: &Arc<Meter>,
) -> Result<()> {
    let mut maxima = unityrs_core::limits::MeteredVec::with_capacity(meter, s.accessors.len())?;
    meter.count_decode(s.accessors.len() as u64)?;
    for _ in s.accessors.as_slice() {
        maxima.push(None)?;
    }
    for (m, ml) in s.meshes.as_slice().iter().zip(layout.meshes.as_slice()) {
        for (p, pl) in m.primitives.as_slice().iter().zip(ml.primitives.as_slice()) {
            meter.count_decode(1)?;
            if !pl.keep {
                continue;
            }
            meter.count_decode(p.attributes.len() as u64 * 2 + 1)?;
            let Some((_, jid)) = p
                .attributes
                .as_slice()
                .iter()
                .find(|(k, _)| *k == Attribute::Joints)
            else {
                continue;
            };
            if maxima.as_slice().get(*jid).is_some_and(Option::is_none) {
                effective_joint_maximum(s, p, maxima.as_mut_slice(), opts, meter)?;
            }
            if !opts.normalise {
                continue;
            }
            let wid = p
                .attributes
                .as_slice()
                .iter()
                .find(|(k, _)| *k == Attribute::Weights)
                .map(|(_, id)| *id)
                .ok_or_else(|| invalid("joint weights"))?;
            let joint = accessor(s, *jid, Kind::Joints)?;
            let weight = accessor(s, wid, Kind::Weights)?;
            meter.count_decode(joint.count as u64 * 32)?;
            for r in 0..joint.count {
                let w = crate::normalise::row(weight, r, true)?;
                let mut seen = [u32::MAX; 4];
                for (i, &v) in w.iter().take(4).enumerate() {
                    if v <= 0. {
                        continue;
                    }
                    let j = joint
                        .data
                        .integer(r * 4 + i)
                        .ok_or_else(|| invalid("joint data"))?;
                    if seen.contains(&j) {
                        return Err(invalid("duplicate positive joint influence"));
                    }
                    if let Some(slot) = seen.get_mut(i) {
                        *slot = j;
                    }
                }
            }
        }
    }
    // Palette checks see only emitted primitives and the effective shared rows.
    for n in s.nodes.as_slice() {
        let Some((mesh, ml)) = n.mesh.and_then(|id| {
            s.meshes
                .as_slice()
                .get(id)
                .zip(layout.meshes.as_slice().get(id))
        }) else {
            continue;
        };
        meter.count_decode(mesh.primitives.len() as u64 + 1)?;
        for (p, pl) in mesh
            .primitives
            .as_slice()
            .iter()
            .zip(ml.primitives.as_slice())
        {
            if !pl.keep {
                continue;
            }
            meter.count_decode(p.attributes.len() as u64)?;
            if let Some((_, id)) = p
                .attributes
                .as_slice()
                .iter()
                .find(|(k, _)| *k == Attribute::Joints)
            {
                let skin = n
                    .skin
                    .and_then(|id| s.skins.as_slice().get(id))
                    .ok_or_else(|| invalid("skinned attributes need skin"))?;
                let max = maxima
                    .as_slice()
                    .get(*id)
                    .copied()
                    .flatten()
                    .ok_or_else(|| invalid("joint data"))?;
                if usize::try_from(max).map_or(true, |v| v >= skin.joints.len()) {
                    return Err(invalid("joint palette index"));
                }
            }
        }
    }
    Ok(())
}

/// Cache bounds over emitted rows once per accessor, independent of instances.
fn effective_joint_maximum(
    s: &Scene,
    p: &crate::Primitive,
    maxima: &mut [Option<u32>],
    opts: crate::Options,
    meter: &Meter,
) -> Result<()> {
    meter.count_decode(p.attributes.len() as u64 * 2)?;
    let id = p
        .attributes
        .as_slice()
        .iter()
        .find(|(k, _)| *k == Attribute::Joints)
        .map(|(_, id)| *id)
        .ok_or_else(|| invalid("joint accessor"))?;
    let wid = p
        .attributes
        .as_slice()
        .iter()
        .find(|(k, _)| *k == Attribute::Weights)
        .map(|(_, id)| *id)
        .ok_or_else(|| invalid("joint weights"))?;
    let joint = accessor(s, id, Kind::Joints)?;
    let weight = accessor(s, wid, Kind::Weights)?;
    meter.count_decode(joint.count as u64 * 48)?;
    let slot = maxima
        .get_mut(id)
        .ok_or_else(|| invalid("joint accessor"))?;
    for row in 0..joint.count {
        let values = if opts.normalise {
            crate::normalise::joints(joint, weight, row)?
        } else {
            let mut values = [0; 4];
            for (col, v) in values.iter_mut().enumerate() {
                *v = joint
                    .data
                    .integer(row * 4 + col)
                    .ok_or_else(|| invalid("joint data"))?;
            }
            values
        };
        for value in values {
            *slot = Some(slot.map_or(value, |previous| previous.max(value)));
        }
    }
    Ok(())
}
