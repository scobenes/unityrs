use crate::{
    Component, Graph, GraphOptions, Issue, Node, NodeId, SceneError, TransformKind, Trs,
    components::Reader, read::Read, visited::Visited,
};
use std::sync::Arc;
use unityrs_core::ObjectRef;
use unityrs_core::env::{Environment, PPtr};
use unityrs_core::files::SerializedFile;
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_core::value::Value;

/// A prefab graph, rooted at a `GameObject`'s Transform (or `RectTransform`).
/// A failed root is an error. Broken descendants become issues; resource
/// refusals always fail the call, releasing partial output.
pub fn graph_from_game_object(
    env: &Environment,
    root: &ObjectRef,
    opts: &GraphOptions,
    meter: &Arc<Meter>,
) -> Result<Graph, SceneError> {
    if root.class_id() != 1 {
        return Err(SceneError::WrongClass(root.class_id()));
    }
    let mut walk = Walk::new(env, opts, meter);
    let p = root.parse_as_dict(false)?;
    let pointers = crate::read::components(&p)?;
    meter.count_decode(pointers.len() as u64)?;
    let mut found = None;
    for ptr in pointers.as_slice() {
        match env.deref(root.file(), *ptr) {
            Ok(t) if matches!(t.class_id(), 4 | 224) => {
                found = Some(t);
                break;
            }
            Ok(_) => (),
            Err(e) => {
                let e = SceneError::Deref(e);
                if e.resource_refusal() {
                    return Err(e);
                }
            }
        }
    }
    let transform = found.ok_or(SceneError::NoTransform)?;
    walk.walk(transform, Some(root))?;
    Ok(walk.graph)
}

/// Roots are Transforms/RectTransforms with null fathers, visited in
/// `SerializedFile::objects()` order, never sorted by path id. An
/// unreachable non-root is not promoted to a root. Editor confirmation
/// of this root-order policy remains pending (`PARITY.md` section 24).
pub fn graph_from_scene_file(
    env: &Environment,
    file: &Arc<SerializedFile>,
    opts: &GraphOptions,
    meter: &Arc<Meter>,
) -> Result<Graph, SceneError> {
    let mut walk = Walk::new(env, opts, meter);
    meter.count_decode(file.object_infos().len() as u64)?;
    for obj in file.objects().filter(|o| matches!(o.class_id(), 4 | 224)) {
        let father = (|| {
            let p = obj.parse_as_dict(false)?;
            Read { p: &p, meter }.pointer("m_Father")
        })();
        match father {
            Ok(p) if !p.is_set() => walk.walk(obj, None)?,
            Ok(_) => (),
            Err(e) => walk.malformed(obj, e)?,
        }
    }
    Ok(walk.graph)
}

struct Pending {
    from: ObjectRef,
    pointer: PPtr,
    parent: NodeId,
    depth: u32,
}
struct Walk<'a> {
    env: &'a Environment,
    opts: &'a GraphOptions,
    meter: &'a Arc<Meter>,
    graph: Graph,
    visited: Visited,
}
impl<'a> Walk<'a> {
    fn new(env: &'a Environment, opts: &'a GraphOptions, meter: &'a Arc<Meter>) -> Self {
        Self {
            env,
            opts,
            meter,
            graph: Graph {
                nodes: MeteredVec::new(meter),
                roots: MeteredVec::new(meter),
                issues: MeteredVec::new(meter),
            },
            visited: Visited::new(meter),
        }
    }
    fn malformed(&mut self, object: ObjectRef, error: SceneError) -> Result<(), SceneError> {
        if error.resource_refusal() {
            return Err(error);
        }
        self.graph.issues.push(Issue::Malformed { object, error })?;
        Ok(())
    }
    fn walk(&mut self, root: ObjectRef, expected_go: Option<&ObjectRef>) -> Result<(), SceneError> {
        let mut pending: MeteredVec<Option<Pending>> = MeteredVec::new(self.meter);
        let limit = self.limit(&root);
        if limit == 0 {
            return Err(SceneError::Malformed("root exceeds depth limit"));
        }
        if !self.visited.insert(&root)? {
            self.graph.issues.push(Issue::Cycle {
                transform: root,
                parent: None,
            })?;
            return Ok(());
        }
        let (node, children) = self.node(root, None)?;
        if expected_go.is_some_and(|g| !same(g, &node.game_object)) {
            return Err(SceneError::Malformed("root m_GameObject"));
        }
        let id = self.graph.nodes.len();
        self.graph.nodes.push(node)?;
        self.graph.roots.push(id)?;
        self.enqueue(&mut pending, id, 1, &children)?;
        while let Some(item) = pending.as_mut_slice().last_mut().and_then(Option::take) {
            pending.truncate(pending.len().saturating_sub(1));
            self.meter.count_decode(1)?;
            let transform = match self.env.deref(item.from.file(), item.pointer) {
                Ok(t) => t,
                Err(e) => {
                    let error = SceneError::Deref(e);
                    if error.resource_refusal() {
                        return Err(error);
                    }
                    let SceneError::Deref(reason) = error else {
                        return Err(error);
                    };
                    self.graph.issues.push(Issue::Unresolved {
                        from: item.from,
                        pointer: item.pointer,
                        reason,
                    })?;
                    continue;
                }
            };
            let limit = self.limit(&transform).min(limit);
            if item.depth > limit {
                self.graph.issues.push(Issue::Depth {
                    transform,
                    depth: item.depth,
                    limit,
                })?;
                continue;
            }
            if !self.visited.insert(&transform)? {
                self.graph.issues.push(Issue::Cycle {
                    transform,
                    parent: Some(item.parent),
                })?;
                continue;
            }
            let (node, children) = match self.node(transform.clone(), Some(item.parent)) {
                Ok(n) => n,
                Err(e) => {
                    self.malformed(transform, e)?;
                    continue;
                }
            };
            let id = self.graph.nodes.len();
            self.graph.nodes.push(node)?;
            self.graph
                .nodes
                .as_mut_slice()
                .get_mut(item.parent)
                .ok_or(SceneError::Malformed("parent id"))?
                .children
                .push(id)?;
            self.enqueue(&mut pending, id, item.depth, &children)?;
        }
        Ok(())
    }
    fn limit(&self, obj: &ObjectRef) -> u32 {
        let file = obj.file().context().limits.max_tree_depth;
        self.opts.max_depth.unwrap_or(file).min(file)
    }
    fn enqueue(
        &self,
        pending: &mut MeteredVec<Option<Pending>>,
        id: NodeId,
        depth: u32,
        children: &MeteredVec<PPtr>,
    ) -> Result<(), SceneError> {
        pending.reserve(children.len())?;
        self.meter.count_decode(children.len() as u64)?;
        let from = &self
            .graph
            .nodes
            .as_slice()
            .get(id)
            .ok_or(SceneError::Malformed("node id"))?
            .transform;
        for pointer in children.as_slice().iter().rev() {
            pending.push(Some(Pending {
                from: from.clone(),
                pointer: *pointer,
                parent: id,
                depth: depth.saturating_add(1),
            }))?;
        }
        Ok(())
    }
    fn check_parent(
        &mut self,
        transform: &ObjectRef,
        r: &Read<'_>,
        id: NodeId,
    ) -> Result<(), SceneError> {
        let father = r.pointer("m_Father")?;
        let expected = &self
            .graph
            .nodes
            .as_slice()
            .get(id)
            .ok_or(SceneError::Malformed("parent id"))?
            .transform;
        match self.env.deref(transform.file(), father) {
            Ok(actual) if same(&actual, expected) => return Ok(()),
            Err(e) => {
                if SceneError::Deref(e.clone()).resource_refusal() {
                    return Err(e.into());
                }
                if father.is_set() {
                    self.graph.issues.push(Issue::Unresolved {
                        from: transform.clone(),
                        pointer: father,
                        reason: e,
                    })?;
                }
            }
            Ok(_) => (),
        }
        self.graph.issues.push(Issue::ParentMismatch {
            transform: transform.clone(),
            expected: id,
            actual: father,
        })?;
        Ok(())
    }
    fn node(
        &mut self,
        transform: ObjectRef,
        parent: Option<NodeId>,
    ) -> Result<(Node, MeteredVec<PPtr>), SceneError> {
        let kind = match transform.class_id() {
            4 => TransformKind::Transform,
            224 => TransformKind::RectTransform,
            id => return Err(SceneError::WrongClass(id)),
        };
        let p = transform.parse_as_dict(false)?;
        let r = Read {
            p: &p,
            meter: self.meter,
        };
        if let Some(id) = parent {
            self.check_parent(&transform, &r, id)?;
        }
        let go = self
            .env
            .deref(transform.file(), r.pointer("m_GameObject")?)?;
        if go.class_id() != 1 {
            return Err(SceneError::WrongClass(go.class_id()));
        }
        let children = r.pointers(r.root("m_Children")?)?;
        let local = Trs {
            position: r.xyz(r.root("m_LocalPosition")?)?,
            rotation: r.xyz(r.root("m_LocalRotation")?)?,
            scale: r.xyz(r.root("m_LocalScale")?)?,
        };
        let g = go.parse_as_dict(false)?;
        let gr = Read {
            p: &g,
            meter: self.meter,
        };
        let name = match gr.root("m_Name")? {
            Value::Str(s) => s.clone(),
            _ => return Err(SceneError::Malformed("m_Name")),
        };
        let layer = u32::try_from(gr.uint(g.value(), "m_Layer")?)
            .map_err(|_| SceneError::Malformed("m_Layer"))?;
        let tag = u16::try_from(gr.uint(g.value(), "m_Tag")?)
            .map_err(|_| SceneError::Malformed("m_Tag"))?;
        let active = gr.boolean("m_IsActive", true)?;
        let ptrs = crate::read::components(&g)?;
        let mut components = MeteredVec::with_capacity(self.meter, ptrs.len())?;
        self.meter.count_decode(ptrs.len() as u64)?;
        for ptr in ptrs.as_slice() {
            let mut reader = Reader {
                env: self.env,
                meter: self.meter,
                issues: &mut self.graph.issues,
            };
            match self.env.deref(go.file(), *ptr) {
                Ok(obj) => match reader.component(obj.clone()) {
                    Ok(c) => components.push(c)?,
                    Err(e) => {
                        self.malformed(obj.clone(), e)?;
                        components.push(Component::Other(obj.class_id(), obj))?;
                    }
                },
                Err(e) => {
                    if SceneError::Deref(e.clone()).resource_refusal() {
                        return Err(e.into());
                    }
                    self.graph.issues.push(Issue::Unresolved {
                        from: go.clone(),
                        pointer: *ptr,
                        reason: e.clone(),
                    })?;
                    components.push(Component::Unresolved {
                        pointer: *ptr,
                        reason: e,
                    })?;
                }
            }
        }
        Ok((
            Node {
                game_object: go,
                transform,
                kind,
                name,
                active,
                layer,
                tag,
                local,
                parent,
                children: MeteredVec::new(self.meter),
                components,
            },
            children,
        ))
    }
}
fn same(a: &ObjectRef, b: &ObjectRef) -> bool {
    Arc::ptr_eq(a.file(), b.file()) && a.path_id() == b.path_id()
}
