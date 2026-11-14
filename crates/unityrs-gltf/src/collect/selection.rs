//! Profile selection preserves transform ancestry and graph traversal order.
use super::{
    Arc, Component, Graph, Inactive, Lod, Meter, MeteredVec, ObjectRef, PPtrTarget, Profile,
    RendererInfo, Result, Text, invalid, same,
};
#[derive(Clone, Copy)]
pub(super) struct Selected {
    pub keep: bool,
    pub render: bool,
    pub active: bool,
}
pub(super) fn nodes(
    graph: &Graph,
    p: &Profile,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<Selected>> {
    let mut out: MeteredVec<Selected> = MeteredVec::with_capacity(meter, graph.nodes().len())?;
    let mut paths: MeteredVec<Text> = MeteredVec::with_capacity(meter, graph.nodes().len())?;
    for n in graph.nodes() {
        meter.count_decode(p.layer_skips.len() as u64 + 1)?;
        let mut path = Text(MeteredVec::new(meter));
        if let Some(parent) = n.parent.and_then(|i| paths.as_slice().get(i)) {
            meter.count_decode(parent.as_bytes().len() as u64)?;
            path.0.extend_from_slice(parent.as_bytes())?;
            path.0.push(b'/')?;
        }
        meter.count_decode(n.name.len() as u64)?;
        path.0.extend_from_slice(n.name.as_bytes())?;
        let parent = n.parent.and_then(|i| out.as_slice().get(i));
        let active = n.active && parent.is_none_or(|p| p.active);
        let root_selected = n.parent.is_some()
            || p.scene_roots.is_empty()
            || matches(&p.scene_roots, n.name.as_bytes(), path.as_bytes(), meter)?;
        let keep = parent.is_none_or(|p| p.keep)
            && root_selected
            && !matches(&p.exclusions, n.name.as_bytes(), path.as_bytes(), meter)?
            && !p.layer_skips.as_slice().contains(&n.layer)
            && (active || p.inactive != Inactive::Skip);
        out.push(Selected {
            keep,
            render: keep,
            active,
        })?;
        paths.push(path)?;
    }
    Ok(out)
}
fn matches(
    patterns: &MeteredVec<Text>,
    name: &[u8],
    path: &[u8],
    meter: &Arc<Meter>,
) -> Result<bool> {
    for p in patterns.as_slice() {
        meter.count_decode((name.len() + path.len() + p.as_bytes().len()) as u64)?;
        if p.as_bytes() == name || p.as_bytes() == path {
            return Ok(true);
        }
    }
    Ok(false)
}
pub(super) fn renderer(
    graph: &Graph,
    object: &ObjectRef,
    p: &Profile,
    meter: &Arc<Meter>,
) -> Result<bool> {
    if p.lod == Lod::All {
        return Ok(true);
    }
    let mut lower = false;
    let mut first = false;
    for n in graph.nodes() {
        meter.count_decode(1)?;
        for c in n.components.as_slice() {
            meter.count_decode(1)?;
            if let Component::LodGroup(g) = c {
                if !g.enabled {
                    continue;
                }
                for (level, lod) in g.lods.as_slice().iter().enumerate() {
                    meter.count_decode(lod.renderers.len() as u64 + 1)?;
                    if lod
                        .renderers
                        .as_slice()
                        .iter()
                        .any(|r| matches!(r,Some(PPtrTarget::Object(o)) if same(o,object)))
                    {
                        if level == 0 {
                            first = true;
                        } else {
                            lower = true;
                        }
                    }
                }
            }
        }
    }
    Ok(!lower || first)
}
pub(super) fn submeshes(
    count: usize,
    r: &RendererInfo,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<usize>> {
    let mut selected = MeteredVec::new(meter);
    if let Some((first, n)) = r.static_batch.filter(|(_, n)| *n > 0) {
        let (first, end) = (usize::from(first), usize::from(first) + usize::from(n));
        if end > count {
            return Err(invalid("static batch submesh range"));
        }
        selected.reserve_exact(end - first)?;
        meter.count_decode((end - first) as u64)?;
        for i in first..end {
            selected.push(i)?;
        }
    } else if !r.subset_indices.is_empty() {
        selected.reserve_exact(r.subset_indices.len())?;
        meter.count_decode(r.subset_indices.len() as u64)?;
        for &i in r.subset_indices.as_slice() {
            if i as usize >= count {
                return Err(invalid("subset index"));
            }
            selected.push(i as usize)?;
        }
    } else {
        selected.reserve_exact(count)?;
        meter.count_decode(count as u64)?;
        for i in 0..count {
            selected.push(i)?;
        }
    }
    Ok(selected)
}

pub(super) fn batched(r: &RendererInfo) -> bool {
    r.static_batch.is_some_and(|(_, count)| count > 0) || !r.subset_indices.is_empty()
}
