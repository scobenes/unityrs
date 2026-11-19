// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeNode.py. See THIRD_PARTY_LICENSES.
// Ported from UnityPy git 417d595 (MIT): UnityPy/files/SerializedFile.py, UnityPy/helpers/TypeTreeNode.py. See THIRD_PARTY_LICENSES.
//! HEAD's node-info envelopes and shared children. The blob parser remains
//! the 1.25.2 parser: HEAD's blob layout is unchanged. Resolution uses an
//! explicit stack, charged tables and counted work, including expansion of
//! shared children; hostile cycles fail instead of recursing indefinitely.

use std::sync::Arc;

use crate::Error;
use crate::limits::{Meter, MeteredVec};
use crate::stream::ShortArray;
use crate::typetree::{NodeId, NodeList, NodeRow};

use super::parse::Parser;

type Hash = ShortArray<16>;

pub(super) struct Handler {
    pub(super) hash: Hash,
    nodes: Option<crate::typetree::SharedNodes>,
    pub(super) refs: MeteredVec<Hash>,
    pub(super) version: i32,
    pub(super) original_nodes: Option<crate::typetree::SharedNodes>,
    /// Whether the envelope carried HEAD's reference-hash list (false for
    /// the bare envelopes formats 23-25 may hold; see [`Parser::head_handler`]).
    pub(super) listed_refs: bool,
}

fn invalid(detail: &'static str) -> Error {
    Error::Malformed {
        what: "HEAD typetree",
        detail: detail.into(),
    }
}

impl Parser<'_> {
    pub(super) fn read_shared(
        &mut self,
        types: &mut MeteredVec<super::SerializedType>,
        refs: Option<&mut MeteredVec<super::SerializedType>>,
    ) -> Result<(), Error> {
        let roots = self.handlers.len();
        for _ in 0..self.r.read_i32()?.max(0) {
            let handler = self.head_handler()?;
            self.handlers.push(handler)?;
        }
        resolve(&mut self.handlers, roots, self.meter, self.limits)?;
        for (t, handler) in types
            .as_mut_slice()
            .iter_mut()
            .chain(refs.into_iter().flat_map(MeteredVec::as_mut_slice))
            .zip(self.handlers.as_slice())
        {
            t.nodes = handler.nodes();
        }
        Ok(())
    }

    /// HEAD helpers/TypeTreeNode.py:348-357, HEAD helpers/TypeTreeNode.py:375-383.
    /// The serialized size is a presence gate, not a cursor bound; a size
    /// below eight consumes no payload. The embedded version selects the blob layout.
    ///
    /// Tier C (PARITY.md section 19.1): at formats 23-25, which have no
    /// shared-subtree table to consume reference hashes, an envelope whose
    /// serialized size ends exactly at the blob's end has no reference-hash
    /// list, as Unity 6000.6 writes it. HEAD reads a count past it and
    /// misreads the rest of the metadata; conversely a hand-made listed
    /// envelope whose size leaves its list out reads here as bare. Any other
    /// size reads a list as HEAD does, bounded by the envelope or the input.
    pub(super) fn head_handler(&mut self) -> Result<Handler, Error> {
        self.meter.count_decode(20)?;
        let hash = self.r.read_short_array();
        let size = self.r.read_u32()?;
        let start = self.r.position();
        let mut refs = MeteredVec::new(self.meter);
        let mut embedded_version = 0;
        let mut listed_refs = true;
        let nodes = if size >= 8 {
            if self.r.read_bytes_short(4) != b"mhtt" {
                return Err(invalid("Invalid type tree blob magic"));
            }
            embedded_version = self.r.read_i32()?;
            let version = u32::try_from(embedded_version).unwrap_or(0);
            let mut rows = crate::typetree::parse::parse_blob(
                &mut self.r,
                self.data,
                version,
                self.meter,
                self.limits,
                self.common,
            )?;
            self.copy_tree_strings(&mut rows);
            let nodes = NodeList::from_rows(rows)?.into_shared()?;
            listed_refs = !((23..26).contains(&self.version)
                && self.r.position() - start == i128::from(size));
            if listed_refs {
                // Checked before any hash is read or charged, the list must
                // fit in what a size reaching past the blob declares, else
                // (HEAD's presence-only sizes, which its oracle reads) in the input.
                let blob_end = self.r.position();
                let declared = start + i128::from(size);
                let input = i128::try_from(self.data.len()).unwrap_or(i128::MAX);
                let left = if declared > blob_end { declared } else { input } - blob_end;
                let count = self.r.read_u32()?;
                if 4 + 16 * i128::from(count) > left {
                    return Err(invalid("reference hashes exceed the envelope"));
                }
                for _ in 0..count {
                    self.meter.count_decode(16)?;
                    let hash = self.r.read_short_array();
                    if hash.as_slice().len() != 16 {
                        return Err(invalid("short reference hash"));
                    }
                    refs.push(hash)?;
                }
            }
            Some(nodes)
        } else {
            None
        };
        Ok(Handler {
            hash,
            original_nodes: nodes.clone(),
            nodes,
            refs,
            version: embedded_version,
            listed_refs,
        })
    }
}

/// HEAD files/SerializedFile.py:356-394. Nodes retain identity when their
/// children are replaced. Each recursive call snapshots its descendants first,
/// including nodes later discarded by a parent's replacement.
struct Entry {
    handler: usize,
    row: NodeId,
    children: usize,
    marks: Marks,
    visiting: bool,
}

#[derive(Default)]
struct Marks {
    parents: Option<usize>,
    resolved: Option<usize>,
}

struct ChildList {
    edges: std::ops::Range<usize>,
    marks: Marks,
}

#[derive(Clone, Copy)]
enum Identity {
    Node(usize),
    List(usize),
}

struct Parent {
    owner: Identity,
    next: Option<usize>,
}

struct Arena {
    nodes: MeteredVec<Entry>,
    edges: MeteredVec<usize>,
    lists: MeteredVec<ChildList>,
    roots: MeteredVec<Option<usize>>,
    parents: MeteredVec<Parent>,
    epoch: u64,
    invalidations: u64,
}

impl Arena {
    fn entry_mut(&mut self, id: usize) -> Result<&mut Entry, Error> {
        self.nodes
            .as_mut_slice()
            .get_mut(id)
            .ok_or_else(|| invalid("node index"))
    }

    fn entry(&self, id: usize) -> Result<&Entry, Error> {
        self.nodes
            .as_slice()
            .get(id)
            .ok_or_else(|| invalid("node index"))
    }

    fn children(&self, id: usize) -> Result<std::ops::Range<usize>, Error> {
        Ok(self.list(self.entry(id)?.children)?.edges.clone())
    }

    fn list(&self, id: usize) -> Result<&ChildList, Error> {
        self.lists
            .as_slice()
            .get(id)
            .ok_or_else(|| invalid("child list index"))
    }

    fn replace(&mut self, id: usize, children: usize) -> Result<(), Error> {
        self.nodes
            .as_mut_slice()
            .get_mut(id)
            .ok_or_else(|| invalid("node index"))?
            .children = children;
        Ok(())
    }

    fn row<'a>(&self, handlers: &'a [Handler], id: usize) -> Result<&'a NodeRow, Error> {
        let entry = self.entry(id)?;
        handlers
            .get(entry.handler)
            .and_then(|h| h.nodes.as_ref())
            .and_then(|n| n.row(entry.row))
            .ok_or_else(|| invalid("node index"))
    }

    fn edge(&self, at: usize) -> Result<usize, Error> {
        self.edges
            .as_slice()
            .get(at)
            .copied()
            .ok_or_else(|| invalid("node index"))
    }

    fn root(&self, handler: usize) -> Option<usize> {
        self.roots.as_slice().get(handler).copied().flatten()
    }

    fn new(handlers: &[Handler], meter: &Arc<Meter>) -> Result<Self, Error> {
        let mut arena = Self {
            nodes: MeteredVec::new(meter),
            edges: MeteredVec::new(meter),
            lists: MeteredVec::new(meter),
            roots: MeteredVec::new(meter),
            parents: MeteredVec::new(meter),
            epoch: 0,
            invalidations: 0,
        };
        for (h, handler) in handlers.iter().enumerate() {
            meter.count_decode(1)?;
            let base = arena.nodes.len();
            arena.roots.push(handler.nodes.as_ref().map(|_| base))?;
            if let Some(nodes) = &handler.nodes {
                for (i, _) in nodes.rows().iter().enumerate() {
                    meter.count_decode(1)?;
                    let row = NodeId(u32::try_from(i).map_err(|_| invalid("node index"))?);
                    let start = arena.edges.len();
                    for child in nodes.children(row) {
                        meter.count_decode(1)?;
                        arena.edges.push(base + child.index())?;
                    }
                    let children = arena.lists.len();
                    arena.lists.push(ChildList {
                        edges: start..arena.edges.len(),
                        marks: Marks::default(),
                    })?;
                    arena.nodes.push(Entry {
                        handler: h,
                        row,
                        children,
                        marks: Marks::default(),
                        visiting: false,
                    })?;
                }
            }
        }
        for id in 0..arena.nodes.len() {
            meter.count_decode(1)?;
            let list = arena.entry(id)?.children;
            arena.link(Identity::List(list), Identity::Node(id), meter)?;
            for at in arena.children(id)? {
                arena.link(Identity::Node(arena.edge(at)?), Identity::List(list), meter)?;
            }
        }
        Ok(arena)
    }

    fn snapshot(
        &mut self,
        root: usize,
        snapshots: &mut MeteredVec<Visit>,
        meter: &Arc<Meter>,
        limits: &crate::limits::Limits,
    ) -> Result<std::ops::Range<usize>, Error> {
        let start = snapshots.len();
        let mut bounds = crate::limits::TreeTraversal::new(meter, limits.max_tree_depth);
        let mut stack = MeteredVec::new(meter);
        stack.push((root, 1u32, None::<usize>))?;
        while let Some((id, depth, finish)) = stack.as_slice().last().copied() {
            stack.truncate(stack.len() - 1);
            meter.count_decode(1)?;
            if let Some(at) = finish {
                self.entry_mut(id)?.visiting = false;
                let end = snapshots.len();
                snapshots
                    .as_mut_slice()
                    .get_mut(at)
                    .ok_or_else(|| invalid("snapshot index"))?
                    .end = end;
                continue;
            }
            bounds.step(u64::from(depth), 0, 0)?;
            if self.entry(id)?.visiting {
                return Err(invalid("shared subtree cycle"));
            }
            self.entry_mut(id)?.visiting = true;
            let at = snapshots.len();
            snapshots.push(Visit { node: id, end: 0 })?;
            stack.push((id, depth, Some(at)))?;
            for at in self.children(id)?.rev() {
                meter.count_decode(1)?;
                stack.push((self.edge(at)?, depth.saturating_add(1), None))?;
            }
        }
        Ok(start + 1..snapshots.len())
    }
}

fn check_depth(depth: u32, limits: &crate::limits::Limits) -> Result<(), Error> {
    if depth > limits.max_tree_depth {
        return Err(crate::error::LimitError::refused(
            crate::LimitKind::TreeDepth,
            u64::from(depth),
            u64::from(limits.max_tree_depth),
        )
        .into());
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Visit {
    node: usize,
    end: usize,
}

struct Frame {
    node: usize,
    hashes: usize,
    snapshot: std::ops::Range<usize>,
    epoch: u64,
    checkpoint: usize,
    resume: Option<usize>,
    invalidations: u64,
    replay_safe: bool,
    cache: Option<usize>,
}

struct HashIndex(MeteredVec<(Hash, usize)>);

impl HashIndex {
    fn new(handlers: &[Handler], roots: usize, meter: &Arc<Meter>) -> Result<Self, Error> {
        let mut hashes = MeteredVec::new(meter);
        for (i, h) in handlers.iter().enumerate().skip(roots) {
            meter.count_decode(16)?;
            hashes.push((h.hash, i))?;
        }
        // Pay the worst-case comparison bound before sorting the fixed hashes.
        let n = hashes.len() as u64;
        meter.count_decode(
            n.saturating_mul(u64::from(hashes.len().bit_width()) + 1)
                .saturating_mul(32),
        )?;
        hashes
            .as_mut_slice()
            .sort_unstable_by(|a, b| a.0.as_slice().cmp(b.0.as_slice()).then(a.1.cmp(&b.1)));
        Ok(Self(hashes))
    }

    fn target(
        &self,
        handlers: &[Handler],
        h: usize,
        row: &NodeRow,
        meter: &Arc<Meter>,
    ) -> Result<usize, Error> {
        meter.count_decode(16)?;
        let hash = row
            .ref_type_hash
            .and_then(|i| usize::try_from(i).ok())
            .and_then(|i| handlers.get(h)?.refs.as_slice().get(i))
            .ok_or_else(|| invalid("shared hash index out of range"))?;
        meter.count_decode(u64::from(self.0.len().bit_width() + 1) * 16)?;
        let at = self
            .0
            .as_slice()
            .partition_point(|(key, _)| key.as_slice() <= hash.as_slice());
        self.0
            .as_slice()
            .get(at.wrapping_sub(1))
            .filter(|(key, _)| key == hash)
            .map(|(_, i)| *i)
            .ok_or_else(|| invalid("shared subtree hash not found"))
    }
}

impl Arena {
    fn finish_frame(
        &mut self,
        frame: &Frame,
        handlers: &[Handler],
        meter: &Arc<Meter>,
    ) -> Result<bool, Error> {
        // A replacement may import placeholders resolved with the
        // dependency's hashes. Those must be revisited with this call's
        // hashes, just as HEAD does; they are not an idempotent result.
        let stable = self.stable_children(frame.node, frame.hashes, handlers, meter)?;
        if let Some(old) = self.entry(frame.node)?.marks.resolved
            && old != frame.hashes
        {
            self.invalidations = self.invalidations.saturating_add(1);
        }
        self.mark(Identity::Node(frame.node), stable.then_some(frame.hashes))?;
        // All identities in a shared snapshot subrange have just been
        // visited in this context. If none of their clean results was
        // invalidated, HEAD's repeated visits have no further effects.
        Ok(stable
            && frame.replay_safe
            && frame.invalidations == self.invalidations
            && self.invalidations < u64::MAX)
    }

    fn child_frame(
        &mut self,
        frame: &Frame,
        at: usize,
        visit: Visit,
        snapshots: &mut MeteredVec<Visit>,
        meter: &Arc<Meter>,
        limits: &crate::limits::Limits,
    ) -> Result<Frame, Error> {
        let checkpoint = snapshots.len();
        // Before any mutation, a nested call sees exactly this
        // immutable subrange of its caller's preorder snapshot.
        let shared = frame.epoch == self.epoch && self.epoch < u64::MAX;
        let snapshot = if shared {
            at + 1..visit.end
        } else {
            self.snapshot(visit.node, snapshots, meter, limits)?
        };
        Ok(Frame {
            node: visit.node,
            hashes: frame.hashes,
            snapshot,
            epoch: self.epoch,
            checkpoint,
            resume: shared.then_some(visit.end),
            invalidations: self.invalidations,
            replay_safe: true,
            cache: None,
        })
    }

    fn assign(&mut self, node: usize, dep: usize, meter: &Arc<Meter>) -> Result<(), Error> {
        let target = self.root(dep);
        // A missing dependency owns an empty list, just as a no-info handler.
        let children = target
            .map(|id| self.entry(id).map(|e| e.children))
            .transpose()?;
        let edges = children
            .map(|id| self.list(id).map(|l| l.edges.clone()))
            .transpose()?
            .unwrap_or(0..0);
        if self.children(node)? != edges {
            // Depend on the assigned children themselves: later assignments
            // to the dependency root leave this old child list in place.
            let children = if let Some(children) = children {
                children
            } else {
                meter.count_decode(1)?;
                let children = self.lists.len();
                self.lists.push(ChildList {
                    edges: 0..0,
                    marks: Marks::default(),
                })?;
                children
            };
            self.link(Identity::List(children), Identity::Node(node), meter)?;
            // Invalidate clean ancestors even while this node is
            // being resolved under a different hash context.
            self.invalidate(node, meter)?;
            self.replace(node, children)?;
            self.epoch = self.epoch.saturating_add(1);
        }
        Ok(())
    }

    fn resolve_roots(
        &mut self,
        handlers: &[Handler],
        roots: usize,
        index: &HashIndex,
        meter: &Arc<Meter>,
        limits: &crate::limits::Limits,
    ) -> Result<(), Error> {
        let mut state = MeteredVec::with_capacity(meter, handlers.len())?;
        for _ in 0..handlers.len() {
            meter.count_decode(1)?;
            state.push(0u8)?;
        }
        let mut stack = MeteredVec::new(meter);
        let mut snapshots = MeteredVec::new(meter);
        for root in 0..roots {
            meter.count_decode(1)?;
            let Some(node) = self.root(root) else {
                continue;
            };
            stack.push(Frame {
                node,
                hashes: root,
                snapshot: self.snapshot(node, &mut snapshots, meter, limits)?,
                epoch: self.epoch,
                checkpoint: 0,
                resume: None,
                invalidations: self.invalidations,
                replay_safe: true,
                cache: None,
            })?;
            while let Some(frame) = stack.as_mut_slice().last_mut() {
                meter.count_decode(1)?;
                if let Some(at) = frame.snapshot.next() {
                    let visit = *snapshots
                        .as_slice()
                        .get(at)
                        .ok_or_else(|| invalid("snapshot index"))?;
                    let child = visit.node;
                    if self.row(handlers, child)?.type_flags.unwrap_or(0) & 32 != 0 {
                        let hashes = frame.hashes;
                        if self.entry(child)?.marks.resolved == Some(hashes) {
                            continue;
                        }
                        let next =
                            self.child_frame(frame, at, visit, &mut snapshots, meter, limits)?;
                        stack.push(next)?;
                    }
                    continue;
                }
                let row = self.row(handlers, frame.node)?;
                if row.type_flags.unwrap_or(0) & 32 != 0 {
                    let dep = index.target(handlers, frame.hashes, row, meter)?;
                    match state.as_slice().get(dep).copied() {
                        Some(1) => return Err(invalid("cyclic shared subtree")),
                        Some(0) => {
                            if let Some(s) = state.as_mut_slice().get_mut(dep) {
                                *s = 1;
                            }
                            if let Some(node) = self.root(dep) {
                                let checkpoint = snapshots.len();
                                stack.push(Frame {
                                    node,
                                    hashes: dep,
                                    snapshot: self.snapshot(node, &mut snapshots, meter, limits)?,
                                    epoch: self.epoch,
                                    checkpoint,
                                    resume: None,
                                    invalidations: self.invalidations,
                                    replay_safe: true,
                                    cache: Some(dep),
                                })?;
                                continue;
                            }
                            if let Some(s) = state.as_mut_slice().get_mut(dep) {
                                *s = 2;
                            }
                        }
                        _ => {}
                    }
                    self.assign(frame.node, dep, meter)?;
                }
                let replay_safe = self.finish_frame(frame, handlers, meter)?;
                let resume = frame.resume.filter(|_| replay_safe);
                if let Some(cache) = frame.cache
                    && let Some(s) = state.as_mut_slice().get_mut(cache)
                {
                    *s = 2;
                }
                snapshots.truncate(frame.checkpoint);
                stack.truncate(stack.len() - 1);
                if let Some(parent) = stack.as_mut_slice().last_mut() {
                    parent.replay_safe &= replay_safe;
                    if let Some(end) = resume {
                        parent.snapshot.start = end;
                    }
                }
            }
        }
        Ok(())
    }
    fn check_heights(
        &self,
        roots: usize,
        meter: &Arc<Meter>,
        limits: &crate::limits::Limits,
    ) -> Result<(), Error> {
        // Check DAG heights before flattening, memoising each identity once. A long
        // shared chain costs linear space and work, including depth refusals.
        let mut heights = MeteredVec::with_capacity(meter, self.nodes.len())?;
        for _ in 0..self.nodes.len() {
            meter.count_decode(1)?;
            heights.push(0u32)?;
        }
        let mut walk = MeteredVec::new(meter);
        for root in 0..roots {
            let Some(node) = self.root(root) else {
                continue;
            };
            walk.push((node, false, 1u32))?;
            while let Some((id, visited, depth)) = walk.as_mut_slice().last().copied() {
                walk.truncate(walk.len() - 1);
                meter.count_decode(1)?;
                check_depth(depth, limits)?;
                let known = heights.as_slice().get(id).copied().unwrap_or(0);
                if !visited && known == u32::MAX {
                    return Err(invalid("shared subtree cycle"));
                }
                if known > 0 && known != u32::MAX {
                    continue;
                }
                if visited {
                    let mut height = 1u32;
                    for at in self.children(id)? {
                        meter.count_decode(1)?;
                        height = height.max(
                            heights
                                .as_slice()
                                .get(self.edge(at)?)
                                .copied()
                                .unwrap_or(0)
                                .saturating_add(1),
                        );
                    }
                    check_depth(height, limits)?;
                    if let Some(h) = heights.as_mut_slice().get_mut(id) {
                        *h = height;
                    }
                } else {
                    if let Some(h) = heights.as_mut_slice().get_mut(id) {
                        *h = u32::MAX;
                    }
                    walk.push((id, true, depth))?;
                    for at in self.children(id)?.rev() {
                        meter.count_decode(1)?;
                        walk.push((self.edge(at)?, false, depth.saturating_add(1)))?;
                    }
                }
            }
        }
        Ok(())
    }
    fn materialize(
        &self,
        handlers: &[Handler],
        roots: usize,
        meter: &Arc<Meter>,
        limits: &crate::limits::Limits,
    ) -> Result<MeteredVec<Option<crate::typetree::SharedNodes>>, Error> {
        let mut output = MeteredVec::new(meter);
        let mut bounds = crate::limits::TreeTraversal::new(meter, limits.max_tree_depth);
        for root in 0..roots {
            let Some(node) = self.root(root) else {
                output.push(None)?;
                continue;
            };
            let mut rows = MeteredVec::new(meter);
            let mut parents = MeteredVec::new(meter);
            let mut identities = MeteredVec::new(meter);
            // Keep stored levels separate from the resolved edges: HEAD does
            // not rewrite a shared child's m_Level when attaching it.
            let mut pending = MeteredVec::new(meter);
            pending.push((node, u32::MAX, 1_u64))?;
            while let Some((id, parent, depth)) = pending.as_mut_slice().last().copied() {
                pending.truncate(pending.len() - 1);
                bounds.step(depth, 0, 1)?;
                let index = u32::try_from(rows.len()).map_err(|_| invalid("node index"))?;
                rows.push(self.row(handlers, id)?.clone())?;
                identities.push((id, self.entry(id)?.children))?;
                parents.push(parent)?;
                for at in self.children(id)?.rev() {
                    meter.count_decode(1)?;
                    pending.push((self.edge(at)?, index, depth.saturating_add(1)))?;
                }
            }
            output.push(Some(
                NodeList::from_preorder(rows, parents)?
                    .with_graph_identities(identities)
                    .into_shared()?,
            ))?;
        }
        Ok(output)
    }
}

#[path = "head_cache.rs"]
mod cache;
pub(super) fn resolve(
    handlers: &mut MeteredVec<Handler>,
    roots: usize,
    meter: &Arc<Meter>,
    limits: &crate::limits::Limits,
) -> Result<(), Error> {
    let index = HashIndex::new(handlers.as_slice(), roots, meter)?;
    let mut arena = Arena::new(handlers.as_slice(), meter)?;
    arena.resolve_roots(handlers.as_slice(), roots, &index, meter, limits)?;
    arena.check_heights(roots, meter, limits)?;
    let mut output = arena.materialize(handlers.as_slice(), roots, meter, limits)?;
    for (handler, nodes) in handlers
        .as_mut_slice()
        .iter_mut()
        .take(roots)
        .zip(output.as_mut_slice())
    {
        handler.nodes = nodes.take();
    }
    Ok(())
}

impl Handler {
    pub(super) fn nodes(&self) -> Option<crate::typetree::SharedNodes> {
        self.nodes.clone()
    }
}

#[cfg(test)]
#[path = "head_tests.rs"]
mod tests;
