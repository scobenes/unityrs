//! Iterative Python inspection and a flat, metered snapshot for native workers.
use super::{Inspect, State, next_depth, next_item, none_for_shape, scalar_for_shape};
use crate::errors;
use pyo3::prelude::*;
use std::sync::Arc;
use unityrs_core::{
    Built, Parsed, RawStr, Value, ValueBuilder,
    limits::{Meter, MeteredVec},
    typetree::{NodeId, NodeList, NodeRow},
};

enum Staged {
    Scalar(Value),
    List(MeteredVec<usize>),
    Tuple(MeteredVec<usize>),
    Map(MeteredVec<(NodeId, usize)>),
    Built(Built),
}

pub(super) struct Arena {
    values: MeteredVec<Option<Staged>>,
}

impl Arena {
    pub(super) fn new(meter: &Arc<Meter>) -> Self {
        Self {
            values: MeteredVec::new(meter),
        }
    }
    fn push(&mut self, value: Staged) -> PyResult<usize> {
        let id = self.values.len();
        self.values.push(Some(value)).map_err(errors::charge)?;
        Ok(id)
    }
    fn take(&mut self, id: usize) -> PyResult<Staged> {
        self.values
            .as_mut_slice()
            .get_mut(id)
            .and_then(Option::take)
            .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("invalid snapshot index"))
    }
    fn take_built(&mut self, id: usize) -> PyResult<Built> {
        match self.take(id)? {
            Staged::Built(value) => Ok(value),
            _ => Err(pyo3::exceptions::PyValueError::new_err(
                "snapshot child was not built",
            )),
        }
    }
    fn build(mut self, root: usize, builder: &ValueBuilder) -> PyResult<Built> {
        // Children precede parents. Only this worker-side pass constructs
        // nested native values; caller-side refusal drops a flat arena.
        for id in 0..self.values.len() {
            let built = match self.take(id)? {
                Staged::Scalar(value) => builder.scalar(value).map_err(|_| {
                    pyo3::exceptions::PyTypeError::new_err("invalid scalar snapshot")
                })?,
                Staged::List(items) => {
                    let mut out = builder.vec();
                    let (items, _charge) = items.into_parts();
                    for child in items {
                        out.push_built(self.take_built(child)?)
                            .map_err(errors::charge)?;
                    }
                    builder.list(out)
                }
                Staged::Tuple(items) => {
                    let mut out = builder.vec();
                    let (items, _charge) = items.into_parts();
                    for child in items {
                        out.push_built(self.take_built(child)?)
                            .map_err(errors::charge)?;
                    }
                    builder.tuple(out).map_err(errors::charge)?
                }
                Staged::Map(items) => {
                    let mut out = builder.vec();
                    let (items, _charge) = items.into_parts();
                    for (key, child) in items {
                        out.push_entry(key, self.take_built(child)?)
                            .map_err(errors::charge)?;
                    }
                    builder.map(0, out)
                }
                Staged::Built(_) => {
                    return Err(pyo3::exceptions::PyValueError::new_err(
                        "snapshot already built",
                    ));
                }
            };
            *self.values.as_mut_slice().get_mut(id).ok_or_else(|| {
                pyo3::exceptions::PyValueError::new_err("invalid snapshot index")
            })? = Some(Staged::Built(built));
        }
        self.take_built(root)
    }
    fn remap_rows(&mut self, mapping: &[NodeId]) -> PyResult<()> {
        for value in self.values.as_mut_slice() {
            if let Some(Staged::Map(items)) = value {
                for (node, _) in items.as_mut_slice() {
                    *node = *mapping.get(node.index()).ok_or_else(|| {
                        pyo3::exceptions::PyValueError::new_err("invalid snapshot row")
                    })?;
                }
            }
        }
        Ok(())
    }
}

/// Move only inferred rows, never Python evaluations or completed values.
pub(super) struct Rotation {
    start: usize,
    first_end: usize,
    end: usize,
}

/// No Python references and no nested native values; safe on the caller stack.
pub(crate) struct Snapshot {
    rows: MeteredVec<NodeRow>,
    values: Arena,
    root: usize,
    rotations: MeteredVec<Rotation>,
}
impl Snapshot {
    fn reorder_rows(&mut self) -> PyResult<()> {
        if self.rotations.is_empty() {
            return Ok(());
        }
        let meter = self.rows.charge().meter().clone();
        let count = self.rows.len();
        let mut order = MeteredVec::with_capacity(&meter, count).map_err(errors::charge)?;
        let mut mapping = MeteredVec::with_capacity(&meter, count).map_err(errors::charge)?;
        meter
            .count_decode(count as u64 * 3 + (self.values.values.len() as u64).saturating_mul(2))
            .map_err(errors::charge)?;
        for index in 0..count {
            order
                .push(NodeId(u32::try_from(index)?))
                .map_err(errors::charge)?;
            mapping.push(NodeId(0)).map_err(errors::charge)?;
        }
        // Child rotations precede parent rotations and keep subtree lengths.
        // Tails remain under the synthetic size node; only the first element
        // supplies the vector's shared schema, just as before staging.
        for rotation in self.rotations.as_slice() {
            let range = rotation.start..rotation.end;
            let pivot = rotation.first_end - rotation.start;
            meter
                .count_decode((range.len() as u64).saturating_mul(2))
                .map_err(errors::charge)?;
            self.rows
                .as_mut_slice()
                .get_mut(range.clone())
                .ok_or_else(|| {
                    pyo3::exceptions::PyValueError::new_err("invalid snapshot rotation")
                })?
                .rotate_left(pivot);
            order
                .as_mut_slice()
                .get_mut(range)
                .ok_or_else(|| {
                    pyo3::exceptions::PyValueError::new_err("invalid snapshot rotation")
                })?
                .rotate_left(pivot);
        }
        for (index, old) in order.as_slice().iter().enumerate() {
            *mapping
                .as_mut_slice()
                .get_mut(old.index())
                .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("invalid snapshot row"))? =
                NodeId(u32::try_from(index)?);
        }
        self.values.remap_rows(mapping.as_slice())
    }

    pub(crate) fn build(mut self) -> PyResult<Parsed> {
        self.reorder_rows()?;
        let meter = self.rows.charge().meter().clone();
        let list = NodeList::from_rows(self.rows)
            .and_then(NodeList::into_shared)
            .map_err(|error| pyo3::exceptions::PyValueError::new_err(errors::message(&error)))?;
        let builder = ValueBuilder::new(&meter, list).map_err(errors::charge)?;
        let value = self.values.build(self.root, &builder)?;
        Ok(builder.finish(value, 0))
    }
}

struct Visit {
    input: Py<PyAny>,
    name: RawStr,
    depth: u32,
    shape: Option<NodeId>,
}

enum Container {
    Map(super::Fields, MeteredVec<(NodeId, usize)>),
    Sequence {
        iter: Py<pyo3::types::PyIterator>,
        tuple: bool,
        skip_null: bool,
        first_rows: Option<(usize, usize)>,
        items: MeteredVec<usize>,
    },
}
struct Frame {
    node: NodeId,
    depth: u32,
    shape: Option<NodeId>,
    container: Container,
}

impl Frame {
    fn accept(&mut self, node: NodeId, value: usize, rows_end: usize) -> PyResult<()> {
        match &mut self.container {
            Container::Map(_, items) => items.push((node, value)),
            Container::Sequence {
                items,
                tuple,
                first_rows,
                ..
            } => {
                if !*tuple && items.is_empty() {
                    *first_rows = Some((node.index(), rows_end));
                }
                items.push(value)
            }
        }
        .map_err(errors::charge)
    }
    fn next(&mut self, state: &mut State<'_>) -> PyResult<Option<Visit>> {
        match &mut self.container {
            Container::Map(fields, items) => {
                Self::map_next(fields, items, state, self.depth, self.shape)
            }
            Container::Sequence {
                iter,
                tuple,
                skip_null,
                items,
                ..
            } => {
                items.reserve(1).map_err(errors::charge)?;
                let input = loop {
                    let Some(input) = next_item(iter)? else {
                        if !*tuple && items.is_empty() {
                            state.row(
                                next_depth(self.depth, 2)?,
                                RawStr::from_static(b"Object"),
                                RawStr::from_static(b"data"),
                            )?;
                        }
                        return Ok(None);
                    };
                    state
                        .staging
                        .meter()
                        .count_decode(1)
                        .map_err(errors::charge)?;
                    if !*skip_null
                        || !Python::attach(|py| input.bind(py).get_item(1).map(|v| v.is_none()))?
                    {
                        break input;
                    }
                };
                let first_item = items.is_empty();
                let shape = state.tree.zip(self.shape).and_then(|(tree, node)| {
                    match tree.info(node)?.shape {
                        unityrs_core::typetree::Shape::Vector { elem, .. } => Some(elem),
                        unityrs_core::typetree::Shape::Pair => {
                            tree.list().children(node).get(items.len()).copied()
                        }
                        _ => None,
                    }
                });
                let key = if *tuple {
                    if items.is_empty() {
                        b"first".as_slice()
                    } else {
                        b"second".as_slice()
                    }
                } else {
                    b"data".as_slice()
                };
                Ok(Some(Visit {
                    input,
                    name: RawStr::from_static(key),
                    shape,
                    depth: next_depth(
                        self.depth,
                        if *tuple {
                            1
                        } else if first_item {
                            2
                        } else {
                            3
                        },
                    )?,
                }))
            }
        }
    }
    fn map_next(
        fields: &mut super::Fields,
        items: &mut MeteredVec<(NodeId, usize)>,
        state: &mut State<'_>,
        depth: u32,
        parent: Option<NodeId>,
    ) -> PyResult<Option<Visit>> {
        let Some((key, input)) = Python::attach(|py| fields.next(py, &mut state.staging))? else {
            return Ok(None);
        };
        items.reserve(1).map_err(errors::charge)?;
        let shape = if let Some((tree, node)) = state.tree.zip(parent) {
            state.lookups.find(
                tree,
                node,
                fields.dict || fields.raw_names,
                key.as_bytes(),
                state.staging.meter(),
            )?
        } else {
            None
        };
        let name = if fields.dict || fields.raw_names {
            key
        } else if let Some((tree, node)) = state.tree.zip(shape) {
            RawStr::charged(tree.list().name(node), state.staging.meter())
                .map_err(errors::charge)?
        } else {
            key
        };
        Ok(Some(Visit {
            input,
            name,
            depth: next_depth(depth, 1)?,
            shape,
        }))
    }
    fn finish(self) -> Staged {
        match self.container {
            Container::Map(_, items) => Staged::Map(items),
            Container::Sequence {
                tuple: true, items, ..
            } => Staged::Tuple(items),
            Container::Sequence { items, .. } => Staged::List(items),
        }
    }
}

enum Opened {
    Scalar(NodeId, Value),
    Container(Frame),
}
impl State<'_> {
    pub(super) fn into_snapshot(self, root: usize) -> Snapshot {
        Snapshot {
            rows: self.rows,
            values: self.values,
            root,
            rotations: self.rotations,
        }
    }
    pub(super) fn stage(
        &mut self,
        input: &Py<PyAny>,
        name: RawStr,
        depth: u32,
        shape: Option<NodeId>,
    ) -> PyResult<(NodeId, usize)> {
        let mut frames = MeteredVec::new(self.staging.meter());
        let mut next = Some(Visit {
            input: Python::attach(|py| input.clone_ref(py)),
            name,
            depth,
            shape,
        });
        let mut completed = None;
        loop {
            if let Some(visit) = next.take() {
                match self.open(visit)? {
                    Opened::Scalar(node, value) => {
                        completed = Some((node, self.values.push(Staged::Scalar(value))?));
                    }
                    Opened::Container(frame) => frames.push(frame).map_err(errors::charge)?,
                }
            }
            if let Some((node, value)) = completed.take() {
                if let Some(frame) = frames.as_mut_slice().last_mut() {
                    frame.accept(node, value, self.rows.len())?;
                } else {
                    return Ok((node, value));
                }
            }
            let frame = frames
                .as_mut_slice()
                .last_mut()
                .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("missing snapshot frame"))?;
            if let Some(visit) = frame.next(self)? {
                next = Some(visit);
                continue;
            }
            let frame = frames
                .pop()
                .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("missing snapshot frame"))?;
            self.active.pop();
            let node = frame.node;
            if let Container::Sequence {
                first_rows: Some((start, first_end)),
                ..
            } = &frame.container
                && *first_end < self.rows.len()
            {
                self.rotations
                    .push(Rotation {
                        start: *start,
                        first_end: *first_end,
                        end: self.rows.len(),
                    })
                    .map_err(errors::charge)?;
            }
            completed = Some((node, self.values.push(frame.finish())?));
        }
    }
    fn sequence_rows(&mut self, depth: u32, tuple: bool) -> PyResult<()> {
        if !tuple {
            self.row(
                next_depth(depth, 1)?,
                RawStr::from_static(b"Array"),
                RawStr::from_static(b"Array"),
            )?;
            self.row(
                next_depth(depth, 2)?,
                RawStr::from_static(b"int"),
                RawStr::from_static(b"size"),
            )?;
        }
        Ok(())
    }

    fn inspect_target(
        &mut self,
        input: &Py<PyAny>,
        shape: Option<NodeId>,
    ) -> PyResult<(RawStr, Inspect)> {
        let scalar = self
            .tree
            .zip(shape)
            .map(|(tree, shape)| {
                scalar_for_shape(tree, shape, input, &mut self.staging, self.limits)
            })
            .transpose()?
            .flatten();
        let result = if let Some(scalar) = scalar {
            scalar
        } else if let Some((tree, shape)) = self.tree.zip(shape)
            && Python::attach(|py| input.bind(py).is_none())
        {
            none_for_shape(tree, shape, input, self.staging.meter())?
        } else if let Some(info) = self
            .tree
            .zip(shape)
            .and_then(|(tree, node)| tree.info(node))
            && matches!(
                info.shape,
                unityrs_core::typetree::Shape::Vector { .. } | unityrs_core::typetree::Shape::Pair
            )
        {
            // A typed buffer is a sequence of Python elements for vectors.
            // Only scalar byte schemas use the raw buffer representation.
            let tuple = matches!(info.shape, unityrs_core::typetree::Shape::Pair);
            let iter = Python::attach(|py| input.bind(py).try_iter().map(Bound::unbind))?;
            (
                RawStr::from_static(if tuple { b"pair" } else { b"vector" }),
                Inspect::Sequence(iter, tuple),
            )
        } else {
            self.inspect(input)?
        };
        Ok(result)
    }

    fn open(&mut self, visit: Visit) -> PyResult<Opened> {
        let Visit {
            input,
            name,
            depth,
            shape,
        } = visit;
        if depth > self.limits.max_value_depth {
            return Err(errors::LimitError::new_err(
                "Python value depth exceeds max_value_depth",
            ));
        }
        self.count = self.count.saturating_add(1);
        if self.count > self.limits.max_values_per_object {
            return Err(errors::LimitError::new_err(
                "Python snapshot exceeds max_values_per_object",
            ));
        }
        self.staging
            .meter()
            .count_decode(1)
            .map_err(errors::charge)?;
        let (ty, inspection) = self.inspect_target(&input, shape)?;
        let skip_null = self.filter_null_properties
            && matches!(name.as_bytes(), b"m_Colors" | b"m_Floats" | b"m_TexEnvs");
        let node = self.row(depth, ty, name)?;
        if let Inspect::Scalar(value) = inspection {
            return Ok(Opened::Scalar(node, value));
        }
        let identity = input.as_ptr() as usize;
        self.staging
            .meter()
            .count_decode(self.active.len() as u64)
            .map_err(errors::charge)?;
        if self.active.as_slice().contains(&identity) {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "cyclic Python value",
            ));
        }
        self.active.push(identity).map_err(errors::charge)?;
        let container = match inspection {
            Inspect::Map(mut fields) => {
                if let Some((tree, node)) = self.tree.zip(shape)
                    && tree.info(node).is_some_and(|info| {
                        matches!(
                            info.shape,
                            unityrs_core::typetree::Shape::Class
                                | unityrs_core::typetree::Shape::RefObject
                        )
                    })
                {
                    fields.restrict(tree, node, &mut self.staging)?;
                }
                Container::Map(fields, MeteredVec::new(self.staging.meter()))
            }
            Inspect::Sequence(iter, tuple) => {
                self.sequence_rows(depth, tuple)?;
                Container::Sequence {
                    iter,
                    tuple,
                    skip_null,
                    first_rows: None,
                    items: MeteredVec::new(self.staging.meter()),
                }
            }
            Inspect::Scalar(_) => {
                return Err(pyo3::exceptions::PyTypeError::new_err(
                    "invalid snapshot state",
                ));
            }
        };
        Ok(Opened::Container(Frame {
            node,
            depth,
            shape,
            container,
        }))
    }
}
