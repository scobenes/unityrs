//! Schema identities follow managed-reference payloads across compiled trees.
use super::lookup::Lookups;
use crate::errors;
use pyo3::prelude::*;
use std::sync::Arc;
use unityrs_core::{
    RawStr,
    files::SerializedType,
    limits::{Limits, Meter, MeteredVec},
    typetree::{NodeId, Tree},
};

#[derive(Clone, Copy)]
pub(super) struct Shape {
    schema: usize,
    pub(super) node: NodeId,
}
enum TreeOwner<'a> {
    Borrowed(&'a Tree),
    Shared(Arc<Tree>),
}
impl TreeOwner<'_> {
    fn tree(&self) -> &Tree {
        match self {
            Self::Borrowed(tree) => tree,
            Self::Shared(tree) => tree,
        }
    }
}
struct Schema<'a> {
    tree: TreeOwner<'a>,
    lookups: Lookups,
}
pub(super) struct Schemas<'a>(MeteredVec<Schema<'a>>);
impl<'a> Schemas<'a> {
    pub(super) fn new(tree: Option<&'a Tree>, meter: &Arc<Meter>) -> PyResult<Self> {
        let mut schemas = MeteredVec::new(meter);
        if let Some(tree) = tree {
            schemas
                .push(Schema {
                    tree: TreeOwner::Borrowed(tree),
                    lookups: Lookups::new(Some(tree), meter)?,
                })
                .map_err(errors::charge)?;
        }
        Ok(Self(schemas))
    }
    pub(super) fn root(&self) -> Option<Shape> {
        self.0.as_slice().first().map(|s| Shape {
            schema: 0,
            node: s.tree.tree().list().root(),
        })
    }
    pub(super) fn target(&self, shape: Option<Shape>) -> Option<(&Tree, NodeId)> {
        let shape = shape?;
        self.0
            .as_slice()
            .get(shape.schema)
            .map(|s| (s.tree.tree(), shape.node))
    }
    pub(super) fn child(shape: Shape, node: NodeId) -> Shape {
        Shape { node, ..shape }
    }
    pub(super) fn find(
        &mut self,
        shape: Shape,
        dict: bool,
        key: &[u8],
        meter: &Arc<Meter>,
    ) -> PyResult<Option<Shape>> {
        let schema = self.0.as_mut_slice().get_mut(shape.schema).ok_or_else(|| {
            crate::errors::new::<pyo3::exceptions::PyValueError>("snapshot schema index")
        })?;
        Ok(schema
            .lookups
            .find(schema.tree.tree(), shape.node, dict, key, meter)?
            .map(|node| Self::child(shape, node)))
    }
    pub(super) fn referenced(
        &mut self,
        refs: &[SerializedType],
        names: &[RawStr; 3],
        limits: &Limits,
        meter: &Arc<Meter>,
    ) -> PyResult<Option<Shape>> {
        for reference in refs {
            let fields = [
                &reference.class_name,
                &reference.namespace,
                &reference.assembly,
            ];
            meter
                .count_decode(
                    fields
                        .iter()
                        .filter_map(|n| n.as_ref())
                        .map(|n| n.len() as u64)
                        .sum::<u64>()
                        + names.iter().map(|n| n.len() as u64).sum::<u64>()
                        + 1,
                )
                .map_err(errors::charge)?;
            if fields.iter().zip(names).any(|(a, b)| a.as_ref() != Some(b)) {
                continue;
            }
            let Some(tree) = reference
                .tree(limits)
                .transpose()
                .map_err(crate::tree::tree_error)?
            else {
                return Ok(None);
            };
            for (schema, old) in self.0.as_slice().iter().enumerate() {
                if std::ptr::eq(old.tree.tree(), Arc::as_ptr(&tree)) {
                    return Ok(Some(Shape {
                        schema,
                        node: tree.list().root(),
                    }));
                }
            }
            let shape = Shape {
                schema: self.0.len(),
                node: tree.list().root(),
            };
            let lookups = Lookups::new(Some(&tree), meter)?;
            self.0
                .push(Schema {
                    tree: TreeOwner::Shared(tree),
                    lookups,
                })
                .map_err(errors::charge)?;
            return Ok(Some(shape));
        }
        Ok(None)
    }
}
