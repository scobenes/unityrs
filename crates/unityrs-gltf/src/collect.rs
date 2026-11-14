//! Unity → glTF collection. Geometry is shared per source object; transforms
//! retain graph order. Skin and static-batch attachments are identity roots.
use crate::validate::invalid;
use crate::{
    Extras, ExtrasNaming, GltfError, Inactive, Lod, Node, Profile, RootNaming, Scene, StaticBatch,
    Text,
};
use std::{
    fmt::{self, Write as _},
    sync::Arc,
};
use unityrs_core::{
    ObjectRef,
    env::Environment,
    limits::{Meter, MeteredVec},
};
use unityrs_scene::{Component, Graph, PPtrTarget, RendererInfo, SkinInfo};
mod attachment;
mod batch;
mod builtin;
mod diagnostics;
mod geometry;
mod morph;
mod selection;
mod skin;
type Result<T> = std::result::Result<T, GltfError>;
struct Cached {
    object: ObjectRef,
    geometry: geometry::Geometry,
    combined_root: Option<usize>,
}
struct Binding {
    source: usize,
    skin: bool,
    skin_attributes: Option<[usize; 2]>,
    selected: MeteredVec<usize>,
    mesh: Option<usize>,
}
/// Collect one graph into the editable model. Materials are unbound until PR 7.
/// All allocation/work refusals propagate, including those of source decoding.
/// Missing skeleton references leave a reported empty node rather than
/// inventing transforms or changing palette order.
#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
pub fn collect(
    env: &Environment,
    graph: &Graph,
    profile: &Profile,
    meter: &Arc<Meter>,
) -> Result<Scene> {
    let mut scene = Scene::new(meter);
    let mut unresolved_skins = 0u64;
    let mut selection = selection::nodes(graph, profile, meter)?;
    skin::retain_joints(graph, &mut selection, profile, meter)?;
    let mut ids = MeteredVec::with_capacity(meter, graph.nodes().len())?;
    for (i, n) in graph.nodes().iter().enumerate() {
        meter.count_decode(n.components.len() as u64 + 32)?;
        if !selection.as_slice().get(i).is_some_and(|s| s.keep) {
            ids.push(None)?;
            continue;
        }
        let mut node = Node::new(meter);
        node.name = Some(
            if n.parent.is_none() && profile.root_naming == RootNaming::PathId {
                number(meter, n.game_object.path_id())?
            } else {
                raw_text(meter, n.name.as_bytes())?
            },
        );
        let [x, y, z] = n.local.position;
        node.translation = [-f64::from(x), f64::from(y), f64::from(z)];
        let [x, y, z, w] = n.local.rotation;
        node.rotation = [f64::from(x), -f64::from(y), -f64::from(z), f64::from(w)];
        node.scale = n.local.scale.map(f64::from);
        if profile.extras_naming == ExtrasNaming::Unity {
            extra(
                &mut node.extras,
                meter,
                "active",
                crate::text(
                    meter,
                    if selection.as_slice().get(i).is_some_and(|s| s.active) {
                        "true"
                    } else {
                        "false"
                    },
                )?,
            )?;
            extra(&mut node.extras, meter, "layer", number(meter, n.layer)?)?;
            extra(
                &mut node.extras,
                meter,
                "path_id",
                number(meter, n.game_object.path_id())?,
            )?;
            extra(
                &mut node.extras,
                meter,
                "unity_name",
                raw_text(meter, n.name.as_bytes())?,
            )?;
        }
        if profile.inactive == Inactive::IncludeHidden
            && !selection.as_slice().get(i).is_some_and(|s| s.active)
        {
            extra(
                &mut node.extras,
                meter,
                "hidden",
                crate::text(meter, "true")?,
            )?;
        }
        if n.components.as_slice().iter().any(|component| {
            matches!(component, Component::MeshRenderer(renderer)
                    if selection::batched(renderer))
        }) {
            extra(
                &mut node.extras,
                meter,
                "static_batched",
                crate::text(meter, "true")?,
            )?;
        }
        ids.push(Some(scene.nodes.len()))?;
        scene.nodes.push(node)?;
    }
    for (i, n) in graph.nodes().iter().enumerate() {
        let Some(id) = ids.as_slice().get(i).copied().flatten() else {
            continue;
        };
        meter.count_decode(n.children.len() as u64 + 1)?;
        if let Some(parent) = n
            .parent
            .and_then(|i| ids.as_slice().get(i).copied().flatten())
        {
            scene
                .nodes
                .as_mut_slice()
                .get_mut(parent)
                .ok_or_else(|| invalid("node parent"))?
                .children
                .push(id)?;
        } else {
            scene.roots.push(id)?;
        }
    }
    let mut sources: MeteredVec<Cached> = MeteredVec::new(meter);
    let mut bindings: MeteredVec<Binding> = MeteredVec::new(meter);
    for (i, n) in graph.nodes().iter().enumerate() {
        if !selection.as_slice().get(i).is_some_and(|s| s.render) {
            continue;
        }
        let Some(id) = ids.as_slice().get(i).copied().flatten() else {
            continue;
        };
        for c in n.components.as_slice() {
            meter.count_decode(1)?;
            let (r, skin) = match c {
                Component::MeshRenderer(r) => (r, None),
                Component::SkinnedMeshRenderer(s) => (&s.renderer, Some(s)),
                _ => continue,
            };
            if !r.enabled || !selection::renderer(graph, &r.object, profile, meter)? {
                continue;
            }
            let mut has_skin = skin.is_some_and(|s| !s.bones.is_empty());
            meter.count_decode(n.components.len() as u64)?;
            let target = if let Some(s) = skin {
                s.mesh.as_ref()
            } else {
                n.components.as_slice().iter().find_map(|c| {
                    if let Component::MeshFilter { mesh, .. } = c {
                        mesh.as_ref()
                    } else {
                        None
                    }
                })
            };
            let resolved;
            let object = match target {
                Some(PPtrTarget::Object(o)) => o,
                Some(PPtrTarget::Builtin(b)) => {
                    builtin::mark(&mut scene, id, b.path_id, meter)?;
                    match profile.builtin {
                        crate::Builtin::Placeholder => {
                            diagnostics::record(&mut scene, id, "builtin_placeholder", 1, meter)?;
                            continue;
                        }
                        crate::Builtin::Procedural => {
                            if skin.is_some() {
                                diagnostics::renderer(
                                    &mut scene,
                                    id,
                                    "procedural builtin skin",
                                    "unresolved_renderer",
                                    meter,
                                )?;
                                continue;
                            }
                            let mesh = match builtin::procedural(&mut scene, b.kind, meter) {
                                Ok(mesh) => mesh,
                                Err(GltfError::Invalid(reason)) => {
                                    diagnostics::renderer(
                                        &mut scene,
                                        id,
                                        reason,
                                        "unresolved_renderer",
                                        meter,
                                    )?;
                                    continue;
                                }
                                Err(error) => return Err(error),
                            };
                            let attachment =
                                attachment::node(&mut scene, id, n, selection::batched(r), meter)?;
                            let node = scene
                                .nodes
                                .as_mut_slice()
                                .get_mut(attachment)
                                .ok_or_else(|| invalid("builtin node"))?;
                            node.mesh = Some(mesh);
                            extra(
                                &mut node.extras,
                                meter,
                                "approximate",
                                crate::text(meter, "true")?,
                            )?;
                            continue;
                        }
                        crate::Builtin::Resolve => {
                            resolved = match builtin::resolve(env, b, meter) {
                                Ok(object) => object,
                                Err(error) => {
                                    let reason = match &error {
                                        GltfError::Invalid(reason) => Some(*reason),
                                        GltfError::Mesh(error) => diagnostics::mesh_reason(error),
                                        _ => None,
                                    };
                                    if let Some(reason) = reason {
                                        diagnostics::renderer(
                                            &mut scene,
                                            id,
                                            reason,
                                            "unresolved_renderer",
                                            meter,
                                        )?;
                                        continue;
                                    }
                                    return Err(error);
                                }
                            };
                            &resolved
                        }
                    }
                }
                Some(PPtrTarget::Unresolved(_)) => {
                    diagnostics::merge(
                        &mut scene
                            .nodes
                            .as_mut_slice()
                            .get_mut(id)
                            .ok_or_else(|| invalid("node"))?
                            .extras,
                        meter,
                        "unresolved_mesh",
                        b"true",
                    )?;
                    diagnostics::renderer(
                        &mut scene,
                        id,
                        "mesh_unresolved",
                        "unresolved_renderer",
                        meter,
                    )?;
                    continue;
                }
                None => {
                    diagnostics::renderer(
                        &mut scene,
                        id,
                        "missing_mesh",
                        "unresolved_renderer",
                        meter,
                    )?;
                    continue;
                }
            };
            meter.count_decode(sources.len() as u64)?;
            let source = if let Some(i) = sources
                .as_slice()
                .iter()
                .position(|s| same(&s.object, object))
            {
                i
            } else {
                let mesh = match unityrs_mesh::read_mesh(object, env, meter) {
                    Ok(mesh) => mesh,
                    Err(error) => {
                        if let Some(reason) = diagnostics::mesh_reason(&error) {
                            diagnostics::renderer(
                                &mut scene,
                                id,
                                reason,
                                "unresolved_renderer",
                                meter,
                            )?;
                            continue;
                        }
                        return Err(GltfError::Mesh(error));
                    }
                };
                let accessors = scene.accessors.len();
                let geometry = match geometry::build(&mut scene, mesh, profile.normalise, meter) {
                    Ok(geometry) => geometry,
                    Err(GltfError::Invalid(reason)) => {
                        scene.accessors.truncate(accessors);
                        diagnostics::renderer(
                            &mut scene,
                            id,
                            reason,
                            "unresolved_renderer",
                            meter,
                        )?;
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let i = sources.len();
                sources.push(Cached {
                    object: object.clone(),
                    geometry,
                    combined_root: None,
                })?;
                i
            };
            let combined_root = sources.as_slice().get(source).and_then(|c| c.combined_root);
            let g = &sources
                .as_slice()
                .get(source)
                .ok_or_else(|| invalid("source mesh"))?
                .geometry;
            let selected = match selection::submeshes(g.source.submeshes.len(), r, meter) {
                Ok(selected) => selected,
                Err(GltfError::Invalid(reason)) => {
                    diagnostics::renderer(&mut scene, id, reason, "unresolved_renderer", meter)?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let batched = selection::batched(r);
            let selected = if batched && profile.static_batch == StaticBatch::KeepCombined {
                batch::selected(
                    env,
                    graph,
                    selection.as_slice(),
                    object,
                    g.source.submeshes.len(),
                    profile,
                    meter,
                )?
            } else {
                selected
            };
            if let Some(s) = skin {
                meter.count_decode(g.targets.len() as u64)?;
                if s.blend_weights
                    .as_slice()
                    .iter()
                    .take(g.targets.len())
                    .any(|w| !w.is_finite())
                {
                    diagnostics::degrade(&mut scene, id, "invalid_blend_weights", meter)?;
                    unresolved_skins += 1;
                    continue;
                }
            }
            let mut built_skin = None;
            if has_skin {
                let s = skin.ok_or_else(|| invalid("skin renderer"))?;
                if g.source.processed.bone_weights().is_none()
                    || g.source.processed.bone_indices().is_none()
                {
                    has_skin = false;
                    diagnostics::record(&mut scene, id, "missing_skin_attributes", 1, meter)?;
                } else if !skin::available(graph, s, meter)? {
                    diagnostics::degrade(
                        &mut scene,
                        id,
                        "joint unresolved or outside graph",
                        meter,
                    )?;
                    unresolved_skins += 1;
                    continue;
                } else if g.source.bind_poses.len() != s.bones.len() * 16 {
                    diagnostics::degrade(&mut scene, id, "skin_bind_pose_count", meter)?;
                    unresolved_skins += 1;
                    continue;
                } else {
                    match skin::build(
                        &mut scene,
                        g,
                        s,
                        graph,
                        ids.as_slice(),
                        profile.normalise,
                        meter,
                    ) {
                        Ok(built) => {
                            for (reason, count) in [
                                ("bind_pose_rows_snapped", built.bind_rows),
                                ("duplicate_joints", built.duplicates as u64),
                                ("duplicate_skin_influence", built.influences),
                                ("skin_root_fallback", u64::from(built.root_fallback)),
                            ] {
                                if count > 0 {
                                    diagnostics::record(&mut scene, id, reason, count, meter)?;
                                }
                            }
                            built_skin = Some(built);
                        }
                        Err(GltfError::Invalid(reason)) => {
                            diagnostics::degrade(&mut scene, id, reason, meter)?;
                            unresolved_skins += 1;
                            continue;
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
            let skin_attributes = built_skin.as_ref().and_then(|s| s.attributes);
            meter
                .count_decode((bindings.len() as u64).saturating_mul(selected.len() as u64 + 2))?;
            let binding = bindings.as_slice().iter().position(|b| {
                b.source == source
                    && b.skin == has_skin
                    && b.skin_attributes == skin_attributes
                    && b.selected.as_slice() == selected.as_slice()
            });
            let mesh = if let Some(b) = binding {
                bindings
                    .as_slice()
                    .get(b)
                    .ok_or_else(|| invalid("mesh binding"))?
                    .mesh
            } else {
                let mesh = geometry::mesh(
                    &mut scene,
                    g,
                    selected.as_slice(),
                    has_skin,
                    skin_attributes,
                    meter,
                )?;
                bindings.push(Binding {
                    source,
                    skin: has_skin,
                    skin_attributes,
                    selected,
                    mesh,
                })?;
                mesh
            };
            let Some(mesh) = mesh else {
                continue;
            };
            if batched
                && profile.static_batch == StaticBatch::KeepCombined
                && let Some(root) = combined_root
            {
                if selection.as_slice().get(i).is_some_and(|s| s.active) {
                    let extras = &mut scene
                        .nodes
                        .as_mut_slice()
                        .get_mut(root)
                        .ok_or_else(|| invalid("combined root"))?
                        .extras;
                    meter.count_decode(extras.len() as u64 * 8)?;
                    if let Some(index) = extras
                        .as_slice()
                        .iter()
                        .position(|(key, _)| key.as_bytes() == b"hidden")
                    {
                        extras
                            .as_mut_slice()
                            .get_mut(index..)
                            .ok_or_else(|| invalid("extra index"))?
                            .rotate_left(1);
                        extras.truncate(extras.len().saturating_sub(1));
                    }
                }
                continue;
            }
            let attachment = attachment::node(&mut scene, id, n, has_skin || batched, meter)?;
            let skin_id = built_skin.map(|s| s.id);
            let node = scene
                .nodes
                .as_mut_slice()
                .get_mut(attachment)
                .ok_or_else(|| invalid("mesh node"))?;
            if node.mesh.is_some() {
                // attachment::node always returns a free slot, including for
                // several local-space renderers on the same Transform.
                return Err(invalid("multiple renderer attachments"));
            }
            node.mesh = Some(mesh);
            node.skin = skin_id;
            if let Some(s) = skin {
                meter.count_decode((s.blend_weights.len() + g.targets.len()) as u64 * 8)?;
                for (index, channel) in g.source.blend_shapes.channels.as_slice().iter().enumerate()
                {
                    // geometry::build checked every endpoint before caching
                    // this source, so frame_weight can only fail invariantly.
                    let weight = s.blend_weights.as_slice().get(index).copied().unwrap_or(0.);
                    node.weights
                        .push(f64::from(weight) / morph::frame_weight(&g.source, channel)?)?;
                }
                let actual = s.blend_weights.len();
                let expected = g.targets.len();
                if actual != expected {
                    diagnostics::record(
                        &mut scene,
                        id,
                        if actual < expected {
                            "blend_weights_padded"
                        } else {
                            "blend_weights_truncated"
                        },
                        actual.abs_diff(expected) as u64,
                        meter,
                    )?;
                }
            }
            if batched && profile.static_batch == StaticBatch::KeepCombined {
                sources
                    .as_mut_slice()
                    .get_mut(source)
                    .ok_or_else(|| invalid("source mesh"))?
                    .combined_root = Some(attachment);
            }
        }
    }
    extra(
        &mut scene.extras,
        meter,
        "graph_issues",
        number(meter, graph.issues().len())?,
    )?;
    extra(
        &mut scene.extras,
        meter,
        "unresolved_skins",
        number(meter, unresolved_skins)?,
    )?;
    for (key, value) in [
        ("generator", profile.export_metadata.generator.as_ref()),
        (
            "input_sha256",
            profile.export_metadata.input_sha256.as_ref(),
        ),
        ("source", profile.export_metadata.source.as_ref()),
    ] {
        if let Some(value) = value {
            extra(
                &mut scene.extras,
                meter,
                key,
                Text(crate::array(meter, value.as_bytes())?),
            )?;
        }
    }
    Ok(scene)
}
fn same(a: &ObjectRef, b: &ObjectRef) -> bool {
    Arc::ptr_eq(a.file(), b.file()) && a.path_id() == b.path_id()
}
fn extra(extras: &mut Extras, meter: &Arc<Meter>, key: &str, value: Text) -> Result<()> {
    meter.count_decode(extras.len() as u64 * key.len() as u64)?;
    if extras
        .as_slice()
        .iter()
        .any(|(k, _)| k.as_bytes() == key.as_bytes())
    {
        return Err(invalid("duplicate collect extra"));
    }
    extras.push((crate::text(meter, key)?, value))?;
    extras
        .as_mut_slice()
        .sort_unstable_by(|(a, _), (b, _)| a.as_bytes().cmp(b.as_bytes()));
    Ok(())
}
fn raw_text(meter: &Arc<Meter>, mut bytes: &[u8]) -> Result<Text> {
    meter.count_decode(bytes.len() as u64 * 3)?;
    let mut out = MeteredVec::new(meter);
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(_) => {
                out.extend_from_slice(bytes)?;
                break;
            }
            Err(e) => {
                let valid = e.valid_up_to();
                out.extend_from_slice(bytes.get(..valid).ok_or_else(|| invalid("text range"))?)?;
                out.extend_from_slice("�".as_bytes())?;
                bytes = bytes
                    .get(valid + e.error_len().unwrap_or(bytes.len() - valid)..)
                    .ok_or_else(|| invalid("text range"))?;
            }
        }
    }
    Ok(Text(out))
}
fn number(meter: &Arc<Meter>, n: impl fmt::Display) -> Result<Text> {
    let mut t = Text(MeteredVec::new(meter));
    append_fmt(&mut t, meter, format_args!("{n}"))?;
    Ok(t)
}
fn append_fmt(t: &mut Text, meter: &Arc<Meter>, args: fmt::Arguments<'_>) -> Result<()> {
    struct Sink<'a> {
        t: &'a mut Text,
        meter: &'a Meter,
        error: Option<GltfError>,
    }
    impl fmt::Write for Sink<'_> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            let result = (|| {
                self.meter.count_decode(s.len() as u64)?;
                self.t.0.extend_from_slice(s.as_bytes())?;
                Ok(())
            })();
            if let Err(e) = result {
                self.error = Some(e);
                return Err(fmt::Error);
            }
            Ok(())
        }
    }
    let mut sink = Sink {
        t,
        meter,
        error: None,
    };
    let result = sink.write_fmt(args);
    if result.is_err() {
        return Err(sink.error.unwrap_or_else(|| invalid("metadata formatting")));
    }
    Ok(())
}
