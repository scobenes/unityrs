//! Iterative Python inspection and a flat, metered snapshot for native workers.
use super::{Inspect, State, next_depth, next_item, none_for_shape, numeric_for_shape};
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
}

/// No Python references and no nested native values; safe on the caller stack.
pub(crate) struct Snapshot {
    rows: MeteredVec<NodeRow>,
    values: Arena,
    root: usize,
}
impl Snapshot {
    pub(crate) fn build(self) -> PyResult<Parsed> {
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
        first: Option<Py<PyAny>>,
        phase: SequencePhase,
        items: MeteredVec<usize>,
    },
}
enum SequencePhase {
    Tail,
    First,
    Done,
}
struct Frame {
    node: NodeId,
    depth: u32,
    shape: Option<NodeId>,
    container: Container,
}

impl Frame {
    fn accept(&mut self, node: NodeId, value: usize) -> PyResult<()> {
        match &mut self.container {
            Container::Map(_, items) => items.push((node, value)),
            Container::Sequence { items, .. } => items.push(value),
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
                first,
                phase,
                items,
            } => {
                match phase {
                    SequencePhase::Done => return Ok(None),
                    SequencePhase::First => {
                        state
                            .staging
                            .meter()
                            .count_decode(items.len() as u64)
                            .map_err(errors::charge)?;
                        items.as_mut_slice().rotate_right(1);
                        *phase = SequencePhase::Done;
                        return Ok(None);
                    }
                    SequencePhase::Tail => {}
                }
                items.reserve(1).map_err(errors::charge)?;
                let (input, first_item) = if let Some(input) = next_item(iter)? {
                    (input, false)
                } else if *tuple {
                    *phase = SequencePhase::Done;
                    return Ok(None);
                } else if let Some(input) = first.take() {
                    *phase = SequencePhase::First;
                    (input, true)
                } else {
                    state.row(
                        next_depth(self.depth, 2)?,
                        RawStr::from_static(b"Object"),
                        RawStr::from_static(b"data"),
                    )?;
                    *phase = SequencePhase::Done;
                    return Ok(None);
                };
                let shape = state.tree.zip(self.shape).and_then(|(tree, node)| {
                    let kids = tree.list().children(node);
                    if *tuple {
                        kids.get(items.len()).copied()
                    } else {
                        kids.first()
                            .and_then(|array| tree.list().children(*array).get(1))
                            .copied()
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
                    frame.accept(node, value)?;
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
            completed = Some((node, self.values.push(frame.finish())?));
        }
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
        let numeric = self
            .tree
            .zip(shape)
            .map(|(tree, shape)| numeric_for_shape(tree, shape, &input, &mut self.staging))
            .transpose()?
            .flatten();
        let (ty, inspection) = if let Some(numeric) = numeric {
            numeric
        } else if let Some((tree, shape)) = self.tree.zip(shape)
            && Python::attach(|py| input.bind(py).is_none())
        {
            none_for_shape(tree, shape, &input, self.staging.meter())?
        } else {
            self.inspect(&input)?
        };
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
                let first = if tuple { None } else { next_item(&iter)? };
                Container::Sequence {
                    iter,
                    tuple,
                    first,
                    phase: SequencePhase::Tail,
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
