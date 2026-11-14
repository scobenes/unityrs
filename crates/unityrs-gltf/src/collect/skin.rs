//! Mesh-node transforms are identity; the original joint hierarchy supplies
//! world transforms. Unity row-major inverse binds are mirrored and transposed
//! into glTF's column-major MAT4 payload, without multiplying by renderer TRS.
use super::{
    Arc, Component, Graph, Meter, MeteredVec, PPtrTarget, Result, Scene, SkinInfo, geometry,
    invalid, same, selection,
};
use crate::{Data, Kind, Skin};
mod palette;
pub(super) struct Built {
    pub id: usize,
    pub attributes: Option<[usize; 2]>,
    pub duplicates: usize,
    pub root_fallback: bool,
    pub influences: u64,
    pub bind_rows: u64,
}
fn joint(graph: &Graph, target: Option<&PPtrTarget>, meter: &Arc<Meter>) -> Result<usize> {
    let Some(PPtrTarget::Object(object)) = target else {
        return Err(invalid("unresolved skin joint"));
    };
    meter.count_decode(graph.nodes().len() as u64)?;
    graph
        .nodes()
        .iter()
        .position(|n| same(&n.transform, object))
        .ok_or_else(|| invalid("skin joint outside graph"))
}
pub(super) fn available(graph: &Graph, skin: &SkinInfo, meter: &Arc<Meter>) -> Result<bool> {
    if skin.bones.is_empty() {
        return Ok(true);
    }
    for target in skin.bones.as_slice() {
        meter.count_decode(graph.nodes().len() as u64 + 1)?;
        let Some(PPtrTarget::Object(object)) = target else {
            return Ok(false);
        };
        if !graph
            .nodes()
            .iter()
            .any(|node| same(&node.transform, object))
        {
            return Ok(false);
        }
    }
    Ok(true)
}
fn optional_root(graph: &Graph, skin: &SkinInfo, meter: &Arc<Meter>) -> Result<Option<usize>> {
    let Some(target) = skin.root_bone.as_ref() else {
        return Ok(None);
    };
    match joint(graph, Some(target), meter) {
        Ok(id) => Ok(Some(id)),
        Err(crate::GltfError::Invalid(_)) => Ok(None),
        Err(error) => Err(error),
    }
}
pub(super) fn retain_joints(
    graph: &Graph,
    selection: &mut MeteredVec<selection::Selected>,
    profile: &super::Profile,
    meter: &Arc<Meter>,
) -> Result<()> {
    for (i, n) in graph.nodes().iter().enumerate() {
        if !selection.as_slice().get(i).is_some_and(|s| s.render) {
            continue;
        }
        for c in n.components.as_slice() {
            meter.count_decode(1)?;
            let Component::SkinnedMeshRenderer(s) = c else {
                continue;
            };
            if !s.renderer.enabled
                || !selection::renderer(graph, &s.renderer.object, profile, meter)?
            {
                continue;
            }
            if s.bones.is_empty() || !available(graph, s, meter)? {
                continue;
            }
            let root = optional_root(graph, s, meter)?;
            for bone in s
                .bones
                .as_slice()
                .iter()
                .map(|bone| joint(graph, bone.as_ref(), meter))
                .chain(root.map(Ok))
            {
                let mut id = Some(bone?);
                while let Some(i) = id {
                    meter.count_decode(1)?;
                    selection
                        .as_mut_slice()
                        .get_mut(i)
                        .ok_or_else(|| invalid("joint selection"))?
                        .keep = true;
                    id = graph.nodes().get(i).and_then(|n| n.parent);
                }
            }
        }
    }
    Ok(())
}
pub(super) fn build(
    scene: &mut Scene,
    g: &geometry::Geometry,
    s: &SkinInfo,
    graph: &Graph,
    ids: &[Option<usize>],
    normalise: bool,
    meter: &Arc<Meter>,
) -> Result<Built> {
    if s.bones.is_empty() || g.source.bind_poses.len() != s.bones.len() * 16 {
        return Err(invalid("skin bind pose count"));
    }
    let palette = palette::build(scene, g, s, graph, ids, normalise, meter)?;
    let mut candidate = optional_root(graph, s, meter)?;
    if let Some(root) = candidate {
        for bone in s.bones.as_slice() {
            if !ancestor(graph, root, joint(graph, bone.as_ref(), meter)?, meter)? {
                candidate = None;
                break;
            }
        }
    }
    let root_fallback = s.root_bone.is_some() && candidate.is_none();
    if candidate.is_none() {
        candidate = Some(joint(
            graph,
            s.bones.as_slice().first().and_then(Option::as_ref),
            meter,
        )?);
        while let Some(root) = candidate {
            let mut common = true;
            for bone in s.bones.as_slice() {
                common &= ancestor(graph, root, joint(graph, bone.as_ref(), meter)?, meter)?;
            }
            if common {
                break;
            }
            candidate = graph.nodes().get(root).and_then(|n| n.parent);
        }
    }
    let skeleton = Some(if let Some(root) = candidate {
        ids.get(root)
            .copied()
            .flatten()
            .ok_or_else(|| invalid("skin root omitted"))?
    } else {
        meter.count_decode(ids.len() as u64)?;
        wrap_roots(scene, ids.iter().flatten().count(), meter)?
    });
    let id = scene.skins.len();
    scene.skins.push(Skin {
        name: None,
        joints: palette.joints,
        inverse_bind_matrices: Some(palette.inverse_bind),
        skeleton,
    })?;
    Ok(Built {
        id,
        attributes: palette.attributes,
        duplicates: palette.duplicates,
        root_fallback,
        influences: palette.influences,
        bind_rows: palette.bind_rows,
    })
}

pub(super) fn inverse_bind(
    scene: &mut Scene,
    mesh: &unityrs_mesh::Mesh,
    meter: &Arc<Meter>,
) -> Result<Option<usize>> {
    if mesh.bind_poses.is_empty() || !mesh.bind_poses.len().is_multiple_of(16) {
        return Ok(None);
    }
    meter.count_decode(mesh.bind_poses.len() as u64 * 8)?;
    let mut values = MeteredVec::with_capacity(meter, mesh.bind_poses.len())?;
    for matrix in mesh.bind_poses.as_slice().as_chunks::<16>().0 {
        for col in 0..4 {
            for row in 0..4 {
                let mut v = f64::from(
                    *matrix
                        .get(row * 4 + col)
                        .ok_or_else(|| invalid("bind pose"))?,
                );
                if (row == 0) != (col == 0) {
                    v = -v;
                }
                values.push(v)?;
            }
        }
    }
    let accessor = geometry::accessor(
        scene,
        Kind::InverseBind,
        mesh.bind_poses.len() / 16,
        Data::Float(values),
        None,
    )?;
    Ok(Some(accessor))
}

fn ancestor(graph: &Graph, root: usize, node: usize, meter: &Arc<Meter>) -> Result<bool> {
    let mut current = Some(node);
    while let Some(id) = current {
        meter.count_decode(1)?;
        if id == root {
            return Ok(true);
        }
        current = graph.nodes().get(id).and_then(|n| n.parent);
    }
    Ok(false)
}
fn wrap_roots(scene: &mut Scene, original_nodes: usize, meter: &Arc<Meter>) -> Result<usize> {
    meter.count_decode(scene.nodes.len() as u64)?;
    if let Some(id) = scene.nodes.as_slice().iter().position(|n| {
        n.extras
            .as_slice()
            .iter()
            .any(|(k, _)| k.as_bytes() == b"unityrs_synthetic_root")
    }) {
        return Ok(id);
    }
    let id = scene.nodes.len();
    let mut node = crate::Node::new(meter);
    super::extra(
        &mut node.extras,
        meter,
        "unityrs_synthetic_root",
        crate::text(meter, "true")?,
    )?;
    let mut roots = MeteredVec::with_capacity(meter, scene.roots.len() + 1)?;
    roots.push(id)?;
    for &root in scene.roots.as_slice() {
        meter.count_decode(1)?;
        if root < original_nodes {
            node.children.push(root)?;
        } else {
            roots.push(root)?;
        }
    }
    scene.roots = roots;
    scene.nodes.push(node)?;
    Ok(id)
}
