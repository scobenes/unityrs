//! Iterative Python inspection and a flat, metered snapshot for native workers.
use super::schema::{Schemas, Shape};
use super::{Inspect, State, next_depth, next_item, none_for_shape, scalar_for_shape};
use crate::errors;
use crate::worker_error::Failure;
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
    fn field(&self, items: &[(NodeId, usize)], rows: &[NodeRow], key: &[u8]) -> Option<&Staged> {
        let (_, id) = items.iter().find(|(node, _)| {
            rows.get(node.index())
                .is_some_and(|row| row.name.as_bytes() == key)
        })?;
        self.values.as_slice().get(*id)?.as_ref()
    }
    fn reference_names(&self, items: &[(NodeId, usize)], rows: &[NodeRow]) -> Option<[RawStr; 3]> {
        let Staged::Map(fields) = self.field(items, rows, b"type")? else {
            return None;
        };
        let get = |key| match self.field(fields.as_slice(), rows, key)? {
            Staged::Scalar(Value::Str(name)) => Some(name.clone()),
            _ => None,
        };
        Some([get(b"class")?, get(b"ns")?, get(b"asm")?])
    }
    pub(super) fn new(meter: &Arc<Meter>) -> Self {
        Self {
            values: MeteredVec::new(meter),
        }
    }
    fn push(&mut self, value: Staged) -> crate::worker_error::Result<usize> {
        let id = self.values.len();
        self.values.push(Some(value)).map_err(Failure::charge)?;
        Ok(id)
    }
    fn take(&mut self, id: usize) -> crate::worker_error::Result<Staged> {
        self.values
            .as_mut_slice()
            .get_mut(id)
            .and_then(Option::take)
            .ok_or_else(|| Failure::Static("ValueError", "invalid snapshot index"))
    }
    fn take_built(&mut self, id: usize) -> crate::worker_error::Result<Built> {
        match self.take(id)? {
            Staged::Built(value) => Ok(value),
            _ => Err(Failure::Static(
                "ValueError",
                "snapshot child was not built",
            )),
        }
    }
    fn build(mut self, root: usize, builder: &ValueBuilder) -> crate::worker_error::Result<Built> {
        // Children precede parents. Only this worker-side pass constructs
        // nested native values; caller-side refusal drops a flat arena.
        for id in 0..self.values.len() {
            let built = match self.take(id)? {
                Staged::Scalar(value) => builder
                    .scalar(value)
                    .map_err(|_| Failure::Static("TypeError", "invalid scalar snapshot"))?,
                Staged::List(items) => {
                    let mut out = builder.vec();
                    let (items, _charge) = items.into_parts();
                    for child in items {
                        out.push_built(self.take_built(child)?)
                            .map_err(Failure::charge)?;
                    }
                    builder.list(out)
                }
                Staged::Tuple(items) => {
                    let mut out = builder.vec();
                    let (items, _charge) = items.into_parts();
                    for child in items {
                        out.push_built(self.take_built(child)?)
                            .map_err(Failure::charge)?;
                    }
                    builder.tuple(out).map_err(Failure::charge)?
                }
                Staged::Map(items) => {
                    let mut out = builder.vec();
                    let (items, _charge) = items.into_parts();
                    for (key, child) in items {
                        out.push_entry(key, self.take_built(child)?)
                            .map_err(Failure::charge)?;
                    }
                    builder.map(0, out)
                }
                Staged::Built(_) => {
                    return Err(Failure::Static("ValueError", "snapshot already built"));
                }
            };
            *self
                .values
                .as_mut_slice()
                .get_mut(id)
                .ok_or_else(|| Failure::Static("ValueError", "invalid snapshot index"))? =
                Some(Staged::Built(built));
        }
        self.take_built(root)
    }
    fn remap_rows(&mut self, mapping: &[NodeId]) -> crate::worker_error::Result<()> {
        for value in self.values.as_mut_slice() {
            if let Some(Staged::Map(items)) = value {
                for (node, _) in items.as_mut_slice() {
                    *node = *mapping
                        .get(node.index())
                        .ok_or_else(|| Failure::Static("ValueError", "invalid snapshot row"))?;
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
    logical_lengths: MeteredVec<(u64, usize)>,
}
impl Snapshot {
    fn reorder_rows(&mut self) -> crate::worker_error::Result<()> {
        if self.rotations.is_empty() {
            return Ok(());
        }
        let meter = self.rows.charge().meter().clone();
        let count = self.rows.len();
        let mut order = MeteredVec::with_capacity(&meter, count).map_err(Failure::charge)?;
        let mut mapping = MeteredVec::with_capacity(&meter, count).map_err(Failure::charge)?;
        meter
            .count_decode(count as u64 * 3 + (self.values.values.len() as u64).saturating_mul(2))
            .map_err(Failure::charge)?;
        for index in 0..count {
            order
                .push(NodeId(u32::try_from(index)?))
                .map_err(Failure::charge)?;
            mapping.push(NodeId(0)).map_err(Failure::charge)?;
        }
        // Child rotations precede parent rotations and keep subtree lengths.
        // Tails remain under the synthetic size node; only the first element
        // supplies the vector's shared schema, just as before staging.
        for rotation in self.rotations.as_slice() {
            let range = rotation.start..rotation.end;
            let pivot = rotation.first_end - rotation.start;
            meter
                .count_decode((range.len() as u64).saturating_mul(2))
                .map_err(Failure::charge)?;
            self.rows
                .as_mut_slice()
                .get_mut(range.clone())
                .ok_or_else(|| Failure::Static("ValueError", "invalid snapshot rotation"))?
                .rotate_left(pivot);
            order
                .as_mut_slice()
                .get_mut(range)
                .ok_or_else(|| Failure::Static("ValueError", "invalid snapshot rotation"))?
                .rotate_left(pivot);
        }
        for (index, old) in order.as_slice().iter().enumerate() {
            *mapping
                .as_mut_slice()
                .get_mut(old.index())
                .ok_or_else(|| Failure::Static("ValueError", "invalid snapshot row"))? =
                NodeId(u32::try_from(index)?);
        }
        self.values.remap_rows(mapping.as_slice())
    }

    pub(crate) fn build(self) -> crate::worker_error::Result<Parsed> {
        self.build_writer().map(|(parsed, _)| parsed)
    }

    pub(crate) fn build_writer(
        mut self,
    ) -> crate::worker_error::Result<(Parsed, super::lengths::Lengths)> {
        self.reorder_rows()?;
        let meter = self.rows.charge().meter().clone();
        let list = NodeList::from_rows(self.rows)
            .and_then(NodeList::into_shared)
            .map_err(|error| Failure::Message("ValueError", errors::message(&error)))?;
        let builder = ValueBuilder::new(&meter, list).map_err(Failure::charge)?;
        let value = self.values.build(self.root, &builder)?;
        let parsed = builder.finish(value, 0);
        let lengths = super::lengths::Lengths::resolve(&parsed, &self.logical_lengths)?;
        Ok((parsed, lengths))
    }
}

struct Visit {
    input: Py<PyAny>,
    name: RawStr,
    depth: u32,
    shape: Option<Shape>,
}

enum Container {
    Map(super::Fields, MeteredVec<(NodeId, usize)>),
    Sequence {
        iter: super::Elements,
        tuple: bool,
        skip_null: bool,
        first_rows: Option<(usize, usize)>,
        items: MeteredVec<usize>,
    },
}
struct Frame {
    node: NodeId,
    depth: u32,
    shape: Option<Shape>,
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
                    let Some(input) = next_item(iter, items.len())? else {
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
                let shape = state.schemas.target(self.shape).and_then(|(tree, node)| {
                    let child = match tree.info(node)?.shape {
                        unityrs_core::typetree::Shape::Vector { elem, .. } => Some(elem),
                        unityrs_core::typetree::Shape::Pair => {
                            tree.list().children(node).get(items.len()).copied()
                        }
                        _ => None,
                    }?;
                    self.shape.map(|shape| Schemas::child(shape, child))
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
        parent: Option<Shape>,
    ) -> PyResult<Option<Visit>> {
        let Some((key, input)) = Python::attach(|py| fields.next(py, &mut state.staging))? else {
            return Ok(None);
        };
        items.reserve(1).map_err(errors::charge)?;
        let mut shape = if let Some(parent) = parent {
            state.schemas.find(
                parent,
                fields.dict || fields.raw_names,
                key.as_bytes(),
                state.staging.meter(),
            )?
        } else {
            None
        };
        let name = if fields.dict || fields.raw_names {
            key
        } else if let Some((tree, node)) = state.schemas.target(shape) {
            RawStr::charged(tree.list().name(node), state.staging.meter())
                .map_err(errors::charge)?
        } else {
            key
        };
        if state.schemas.target(parent).is_some_and(|(tree, node)| {
            tree.info(node)
                .is_some_and(|i| matches!(i.shape, unityrs_core::typetree::Shape::RefObject))
        }) && state
            .schemas
            .target(shape)
            .is_some_and(|(tree, node)| tree.list().type_name(node) == b"ReferencedObjectData")
            && let Some(names) = state
                .values
                .reference_names(items.as_slice(), state.rows.as_slice())
            && let Some(target) =
                state
                    .schemas
                    .referenced(state.refs, &names, state.limits, state.staging.meter())?
        {
            shape = Some(target);
        }
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
            logical_lengths: self.logical_lengths,
        }
    }
    pub(super) fn stage(
        &mut self,
        input: &Py<PyAny>,
        name: RawStr,
        depth: u32,
        shape: Option<Shape>,
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
        shape: Option<Shape>,
    ) -> PyResult<(RawStr, Inspect)> {
        let scalar = self
            .schemas
            .target(shape)
            .map(|(tree, shape)| {
                scalar_for_shape(tree, shape, input, &mut self.staging, self.limits)
            })
            .transpose()?
            .flatten();
        let result = if let Some(scalar) = scalar {
            scalar
        } else if let Some((tree, shape)) = self.schemas.target(shape)
            && Python::attach(|py| input.bind(py).is_none())
        {
            none_for_shape(tree, shape, input, self.staging.meter())?
        } else if let Some(info) = self
            .schemas
            .target(shape)
            .and_then(|(tree, node)| tree.info(node))
            && matches!(
                info.shape,
                unityrs_core::typetree::Shape::Vector { .. } | unityrs_core::typetree::Shape::Pair
            )
        {
            // A typed buffer is a sequence of Python elements for vectors.
            // Only scalar byte schemas use the raw buffer representation.
            let tuple = matches!(info.shape, unityrs_core::typetree::Shape::Pair);
            let logical_len = if tuple {
                None
            } else {
                Some(Python::attach(|py| input.bind(py).len())?)
            };
            let iter = if tuple {
                super::Elements::Pair(Python::attach(|py| input.clone_ref(py)))
            } else {
                super::Elements::Iter(Python::attach(|py| {
                    input.bind(py).try_iter().map(Bound::unbind)
                })?)
            };
            (
                RawStr::from_static(if tuple { b"pair" } else { b"vector" }),
                Inspect::Sequence(iter, tuple, logical_len),
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
        #[cfg(test)]
        crate::row_copy_test::before_value()?;
        let skip_null = self.filter_null_properties
            && matches!(name.as_bytes(), b"m_Colors" | b"m_Floats" | b"m_TexEnvs");
        let node = self.row(depth, ty, name)?;
        let inspection = match inspection {
            Inspect::Sized(value, len) => {
                self.logical_lengths
                    .push((self.count, len))
                    .map_err(errors::charge)?;
                return Ok(Opened::Scalar(node, value));
            }
            Inspect::Scalar(value) => return Ok(Opened::Scalar(node, value)),
            other => other,
        };
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
                if let Some((tree, node)) = self.schemas.target(shape)
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
            Inspect::Sequence(iter, tuple, logical_len) => {
                if let Some(len) = logical_len {
                    self.logical_lengths
                        .push((self.count, len))
                        .map_err(errors::charge)?;
                }
                self.sequence_rows(depth, tuple)?;
                Container::Sequence {
                    iter,
                    tuple,
                    skip_null,
                    first_rows: None,
                    items: MeteredVec::new(self.staging.meter()),
                }
            }
            Inspect::Scalar(_) | Inspect::Sized(_, _) => {
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
