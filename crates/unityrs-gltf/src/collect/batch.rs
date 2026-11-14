//! Keep-combined draws the union of selected renderer ranges, once per source.
use super::{
    Arc, Component, Graph, Meter, MeteredVec, ObjectRef, PPtrTarget, Profile, Result, builtin,
    diagnostics, same, selection,
};
pub(super) fn selected(
    env: &super::Environment,
    graph: &Graph,
    selected: &[selection::Selected],
    object: &ObjectRef,
    count: usize,
    profile: &Profile,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<usize>> {
    let mut included = MeteredVec::with_capacity(meter, count)?;
    for _ in 0..count {
        meter.count_decode(1)?;
        included.push(false)?;
    }
    for (index, node) in graph.nodes().iter().enumerate() {
        meter.count_decode(1)?;
        if !selected.get(index).is_some_and(|s| s.render) {
            continue;
        }
        for component in node.components.as_slice() {
            meter.count_decode(node.components.len() as u64 + 1)?;
            let Component::MeshRenderer(renderer) = component else {
                continue;
            };
            if !renderer.enabled
                || !selection::batched(renderer)
                || !selection::renderer(graph, &renderer.object, profile, meter)?
            {
                continue;
            }
            let source = node.components.as_slice().iter().find_map(|c| {
                if let Component::MeshFilter { mesh, .. } = c {
                    mesh.as_ref()
                } else {
                    None
                }
            });
            let resolved;
            let source = match source {
                Some(PPtrTarget::Object(object)) => object,
                Some(PPtrTarget::Builtin(b)) if profile.builtin == crate::Builtin::Resolve => {
                    resolved = match builtin::resolve(env, b, meter) {
                        Ok(object) => object,
                        // Its own collection visit reports malformed/missing sources.
                        Err(super::GltfError::Invalid(_)) => continue,
                        Err(super::GltfError::Mesh(error))
                            if diagnostics::mesh_reason(&error).is_some() =>
                        {
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    &resolved
                }
                _ => continue,
            };
            if !same(source, object) {
                continue;
            }
            let ranges = match selection::submeshes(count, renderer, meter) {
                Ok(ranges) => ranges,
                // Its own collection visit reports this renderer's bad range.
                Err(super::GltfError::Invalid(_)) => continue,
                Err(error) => return Err(error),
            };
            for &id in ranges.as_slice() {
                meter.count_decode(1)?;
                if let Some(slot) = included.as_mut_slice().get_mut(id) {
                    *slot = true;
                }
            }
        }
    }
    let mut out = MeteredVec::new(meter);
    for (index, &include) in included.as_slice().iter().enumerate() {
        meter.count_decode(1)?;
        if include {
            out.push(index)?;
        }
    }
    Ok(out)
}
