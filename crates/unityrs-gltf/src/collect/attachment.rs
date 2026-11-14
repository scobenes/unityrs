//! Local-space instances share their Transform via identity children when needed.
use super::{Arc, Meter, Node, Result, Scene, extra, invalid, number, raw_text};

pub(super) fn node(
    scene: &mut Scene,
    id: usize,
    source: &unityrs_scene::Node,
    world_space: bool,
    meter: &Arc<Meter>,
) -> Result<usize> {
    let original = scene
        .nodes
        .as_slice()
        .get(id)
        .ok_or_else(|| invalid("attachment source"))?;
    if !world_space && original.mesh.is_none() {
        return Ok(id);
    }
    let mut node = Node::new(meter);
    node.name = Some(raw_text(meter, source.name.as_bytes())?);
    meter.count_decode(original.extras.len() as u64)?;
    if original
        .extras
        .as_slice()
        .iter()
        .any(|(key, _)| key.as_bytes() == b"hidden")
    {
        extra(
            &mut node.extras,
            meter,
            "hidden",
            crate::text(meter, "true")?,
        )?;
    }
    extra(
        &mut node.extras,
        meter,
        "source_path_id",
        number(meter, source.game_object.path_id())?,
    )?;
    let attachment = scene.nodes.len();
    scene.nodes.push(node)?;
    if world_space {
        scene.roots.push(attachment)?;
    } else {
        scene
            .nodes
            .as_mut_slice()
            .get_mut(id)
            .ok_or_else(|| invalid("attachment parent"))?
            .children
            .push(attachment)?;
    }
    Ok(attachment)
}
