//! Resolve binding targets using the scene graph and caller-owned mesh facts.
use crate::{AnimError, BindingKind, SlotBinding, TransformProperty, read::same_object};
use std::{ops::Range, sync::Arc};
use unityrs_core::{
    ObjectRef,
    limits::{Meter, MeteredVec},
};
use unityrs_scene::{Component, Graph, NodeId};

#[derive(Clone, Copy, Debug)]
pub enum PathQuery<'a> {
    Name(&'a [u8]),
    Hash(u32),
    /// A path from the matching TOS domain, to be checked against the graph.
    HashWithEvidence(u32, &'a [u8]),
}

#[derive(Clone, Copy, Debug)]
pub enum MorphProperty<'a> {
    Attribute(&'a [u8]),
    Hash(u32),
}

/// The channel facts supplied for one resolved mesh instance.
#[derive(Clone, Copy, Debug)]
pub struct RendererChannel<'a> {
    pub name: &'a [u8],
    pub hash: u32,
    pub target_slot: usize,
    pub frame: usize,
    pub full_weight: f32,
}

#[derive(Clone, Copy, Debug)]
pub enum Attachment<'a> {
    Present(&'a [RendererChannel<'a>]),
    Filtered,
    Missing,
}

#[derive(Clone, Copy, Debug)]
pub struct RendererChannels<'a> {
    pub renderer: &'a ObjectRef,
    pub attachment: Attachment<'a>,
}

#[derive(Clone, Copy, Debug)]
pub enum BindingTarget<'a> {
    Transform {
        node: NodeId,
        property: TransformProperty,
    },
    Morph {
        node: NodeId,
        renderer: &'a ObjectRef,
        channel: &'a RendererChannel<'a>,
    },
    Metadata,
}

#[derive(Debug)]
struct Entry {
    hash: u32,
    node: NodeId,
    bytes: Range<usize>,
    segmented: bool,
}

/// A charged relative-path index for one component's graph subtree.
#[derive(Debug)]
pub struct RigPaths<'a> {
    graph: &'a Graph,
    root: NodeId,
    entries: MeteredVec<Entry>,
    names: MeteredVec<u8>,
}

impl<'a> RigPaths<'a> {
    pub fn new(graph: &'a Graph, root: NodeId, meter: &Arc<Meter>) -> Result<Self, AnimError> {
        let nodes = graph
            .nodes()
            .get(root..)
            .filter(|nodes| !nodes.is_empty())
            .ok_or(AnimError::MissingPath)?;
        let mut entries = MeteredVec::new(meter);
        let mut names = MeteredVec::new(meter);
        let mut ancestry = MeteredVec::new(meter);
        let mut path = MeteredVec::new(meter);
        entries.push(Entry {
            hash: 0,
            node: root,
            bytes: 0..0,
            segmented: true,
        })?;
        ancestry.push((root, 0, !0u32, true))?;

        // Scene nodes are in depth-first first-reach order. Once a parent
        // is outside this ancestry, the component's subtree has ended.
        for (node, object) in (root..).zip(nodes).skip(1) {
            meter.count_decode(1)?;
            while ancestry
                .as_slice()
                .last()
                .is_some_and(|&(id, ..)| Some(id) != object.parent)
            {
                meter.count_decode(1)?;
                ancestry.pop();
            }
            let Some(&(parent, length, mut crc, segmented)) = ancestry.as_slice().last() else {
                break;
            };
            let name = object.name.as_bytes();
            path.truncate(length);
            if parent != root {
                path.push(b'/')?;
                crc = update_crc(crc, b"/");
            }
            meter.count_decode((name.len() as u64).saturating_mul(2).saturating_add(1))?;
            path.extend_from_slice(name)?;
            crc = update_crc(crc, name);
            // A literal slash in a stored name is part of the raw path hash.
            // It cannot stand for a hierarchy edge in an explicit path.
            let segmented = segmented && !name.contains(&b'/');
            ancestry.push((node, path.len(), crc, segmented))?;
            meter.count_decode(path.len() as u64)?;
            let start = names.len();
            names.extend_from_slice(path.as_slice())?;
            entries.push(Entry {
                hash: !crc,
                node,
                bytes: start..names.len(),
                segmented,
            })?;
        }
        meter.count_decode((entries.len() as u64).saturating_mul(search_work(entries.len())))?;
        entries
            .as_mut_slice()
            .sort_unstable_by_key(|entry| (entry.hash, entry.node));
        Ok(Self {
            graph,
            root,
            entries,
            names,
        })
    }

    /// All graph candidates in stored order, including the owner for hash zero.
    pub fn hash_candidates(
        &self,
        hash: u32,
        meter: &Arc<Meter>,
    ) -> Result<MeteredVec<NodeId>, AnimError> {
        let entries = self.entries_for(hash, meter)?;
        meter.count_decode(entries.len() as u64)?;
        let mut out = MeteredVec::with_capacity(meter, entries.len())?;
        for entry in entries {
            out.push(entry.node)?;
        }
        Ok(out)
    }

    pub fn resolve(&self, path: PathQuery<'_>, meter: &Arc<Meter>) -> Result<NodeId, AnimError> {
        match path {
            PathQuery::Name(b"") => Ok(self.root),
            PathQuery::Name(name) => {
                meter.count_decode(name.len() as u64)?;
                let mut found = None;
                for entry in self.entries_for(!update_crc(!0, name), meter)? {
                    meter.count_decode(1)?;
                    if entry.segmented
                        && self.path_equals(entry, name, meter)?
                        && found.replace(entry.node).is_some()
                    {
                        return Err(AnimError::AmbiguousPath);
                    }
                }
                found.ok_or(AnimError::MissingPath)
            }
            PathQuery::Hash(hash) => self.resolve_hash(hash, None, meter),
            PathQuery::HashWithEvidence(hash, evidence) => {
                self.resolve_hash(hash, Some(evidence), meter)
            }
        }
    }

    fn resolve_hash(
        &self,
        hash: u32,
        evidence: Option<&[u8]>,
        meter: &Arc<Meter>,
    ) -> Result<NodeId, AnimError> {
        let Some((first, others)) = self.entries_for(hash, meter)?.split_first() else {
            return Err(AnimError::MissingPath);
        };
        let name = self.bytes(first)?;
        for other in others {
            if !self.path_equals(other, name, meter)? {
                return Err(AnimError::CrcCollision(hash));
            }
        }
        if !others.is_empty() {
            return Err(AnimError::AmbiguousPath);
        }
        if let Some(evidence) = evidence
            && !self.path_equals(first, evidence, meter)?
        {
            return Err(AnimError::PathEvidenceMismatch);
        }
        Ok(first.node)
    }

    fn entries_for(&self, hash: u32, meter: &Arc<Meter>) -> Result<&[Entry], AnimError> {
        let entries = self.entries.as_slice();
        meter.count_decode(search_work(entries.len()) * 2)?;
        let start = entries.partition_point(|entry| entry.hash < hash);
        let end = entries.partition_point(|entry| entry.hash <= hash);
        entries.get(start..end).ok_or(AnimError::MissingPath)
    }

    fn bytes(&self, entry: &Entry) -> Result<&[u8], AnimError> {
        self.names
            .as_slice()
            .get(entry.bytes.clone())
            .ok_or(AnimError::MissingPath)
    }

    fn path_equals(
        &self,
        entry: &Entry,
        name: &[u8],
        meter: &Arc<Meter>,
    ) -> Result<bool, AnimError> {
        let other = self.bytes(entry)?;
        meter.count_decode((other.len().min(name.len()) as u64).saturating_add(1))?;
        Ok(other == name)
    }

    /// Resolve a compiled row. Metadata-only rows require no graph target.
    pub fn bind_generic<'r>(
        &self,
        slot: &SlotBinding,
        filtered: &[NodeId],
        table: &'r [RendererChannels<'r>],
        meter: &Arc<Meter>,
    ) -> Result<BindingTarget<'r>, AnimError> {
        let path = PathQuery::Hash(slot.binding.path);
        match slot.kind {
            BindingKind::Transform(property) => {
                self.bind_transform(path, property, filtered, meter)
            }
            BindingKind::Morph => {
                let node = self.available(path, filtered, meter)?;
                self.morph_on(
                    node,
                    MorphProperty::Hash(slot.binding.attribute),
                    table,
                    meter,
                )
            }
            BindingKind::UnsupportedScalar => Err(AnimError::UnsupportedBindingTarget),
            _ => Ok(BindingTarget::Metadata),
        }
    }

    pub fn bind_transform(
        &self,
        path: PathQuery<'_>,
        property: TransformProperty,
        filtered: &[NodeId],
        meter: &Arc<Meter>,
    ) -> Result<BindingTarget<'static>, AnimError> {
        Ok(BindingTarget::Transform {
            node: self.available(path, filtered, meter)?,
            property,
        })
    }

    pub fn bind_morph<'r>(
        &self,
        path: PathQuery<'_>,
        property: MorphProperty<'_>,
        table: &'r [RendererChannels<'r>],
        meter: &Arc<Meter>,
    ) -> Result<BindingTarget<'r>, AnimError> {
        self.morph_on(self.resolve(path, meter)?, property, table, meter)
    }

    fn available(
        &self,
        path: PathQuery<'_>,
        filtered: &[NodeId],
        meter: &Arc<Meter>,
    ) -> Result<NodeId, AnimError> {
        let node = self.resolve(path, meter)?;
        meter.count_decode(filtered.len() as u64)?;
        if filtered.contains(&node) {
            return Err(AnimError::FilteredTarget(node));
        }
        Ok(node)
    }

    fn morph_on<'r>(
        &self,
        node: NodeId,
        property: MorphProperty<'_>,
        table: &'r [RendererChannels<'r>],
        meter: &Arc<Meter>,
    ) -> Result<BindingTarget<'r>, AnimError> {
        let components = self
            .graph
            .nodes()
            .get(node)
            .ok_or(AnimError::MissingPath)?
            .components
            .as_slice();
        meter.count_decode(components.len() as u64)?;
        let mut renderers = components.iter().filter_map(|component| match component {
            Component::SkinnedMeshRenderer(skin) => Some(&skin.renderer.object),
            _ => None,
        });
        let renderer = renderers.next().ok_or(AnimError::MissingRenderer(node))?;
        if renderers.next().is_some() {
            return Err(AnimError::AmbiguousRenderer(node));
        }
        meter.count_decode(table.len() as u64)?;
        let mut rows = table
            .iter()
            .filter(|row| same_object(row.renderer, renderer));
        let row = rows.next().ok_or(AnimError::MissingAttachment(node))?;
        if rows.next().is_some() {
            return Err(AnimError::AmbiguousRenderer(node));
        }
        let channels = match row.attachment {
            Attachment::Present(channels) => channels,
            Attachment::Filtered => return Err(AnimError::FilteredTarget(node)),
            Attachment::Missing => return Err(AnimError::MissingAttachment(node)),
        };
        let name = match property {
            MorphProperty::Attribute(attribute) => Some(
                attribute
                    .strip_prefix(b"blendShape.")
                    .ok_or(AnimError::UnsupportedMorphProperty)?,
            ),
            MorphProperty::Hash(_) => None,
        };
        let mut found = None;
        for channel in channels {
            meter.count_decode(1)?;
            let matches = match property {
                MorphProperty::Attribute(_) => {
                    meter.count_decode(channel.name.len() as u64)?;
                    name == Some(channel.name)
                }
                MorphProperty::Hash(hash) => hash == channel.hash,
            };
            if matches && found.replace(channel).is_some() {
                return Err(AnimError::AmbiguousChannel(node));
            }
        }
        let channel = found.ok_or(AnimError::MissingChannel(node))?;
        Ok(BindingTarget::Morph {
            node,
            renderer: row.renderer,
            channel,
        })
    }
}

fn search_work(len: usize) -> u64 {
    u64::from(len.bit_width()) + 1
}

/// Reflected IEEE CRC32 over serialized bytes. Final complement is applied
/// by the caller so a child's path can reuse its parent's accumulator.
fn update_crc(mut crc: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let polynomial = if crc & 1 == 0 { 0 } else { 0xedb8_8320 };
            crc = (crc >> 1) ^ polynomial;
        }
    }
    crc
}

const _: () = crate::send_sync::<PathQuery<'static>>();
const _: () = crate::send_sync::<MorphProperty<'static>>();
const _: () = crate::send_sync::<RendererChannel<'static>>();
const _: () = crate::send_sync::<Attachment<'static>>();
const _: () = crate::send_sync::<RendererChannels<'static>>();
const _: () = crate::send_sync::<BindingTarget<'static>>();
const _: () = crate::send_sync::<RigPaths<'static>>();

#[cfg(test)]
mod tests {
    use super::update_crc;

    #[test]
    fn ieee_crc32_over_raw_bytes() {
        assert_eq!(!update_crc(!0, b""), 0);
        assert_eq!(!update_crc(!0, b"123456789"), 0xcbf4_3926);
        assert_eq!(!update_crc(update_crc(!0, b"1234"), b"56789"), 0xcbf4_3926);
    }
}
