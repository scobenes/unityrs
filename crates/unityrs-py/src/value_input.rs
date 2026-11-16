// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp (clean_name). See THIRD_PARTY_LICENSES.
//! Charged snapshots of current Python values. No generated schema is retained.
//! The synthetic node names describe only the values supplied by this caller.
use crate::errors;
use pyo3::{
    ffi,
    prelude::*,
    types::{
        PyBool, PyByteArray, PyBytes, PyDict, PyFloat, PyInt, PyIterator, PyList, PyMemoryView,
        PySet, PyString, PyTuple,
    },
};
use std::sync::Arc;
use unityrs_core::{
    Built, Parsed, RawStr, Value, ValueBuilder,
    budget::DecodeBuf,
    limits::{Charge, Limits, Meter, MeteredVec},
    typetree::{NodeId, NodeList, NodeRow, Tree},
};
#[path = "value_input_lookup.rs"]
mod lookup;

enum Staged {
    Scalar(Value),
    List(MeteredVec<Staged>),
    Tuple(MeteredVec<Staged>),
    Map(MeteredVec<(NodeId, Staged)>),
}
impl Staged {
    fn build(self, builder: &ValueBuilder) -> PyResult<Built> {
        match self {
            Self::Scalar(v) => builder
                .scalar(v)
                .map_err(|_| pyo3::exceptions::PyTypeError::new_err("invalid scalar snapshot")),
            Self::List(items) => {
                let mut out = builder.vec();
                let (items, _charge) = items.into_parts();
                for item in items {
                    out.push_built(item.build(builder)?)
                        .map_err(errors::charge)?;
                }
                Ok(builder.list(out))
            }
            Self::Tuple(items) => {
                let mut out = builder.vec();
                let (items, _charge) = items.into_parts();
                for item in items {
                    out.push_built(item.build(builder)?)
                        .map_err(errors::charge)?;
                }
                builder.tuple(out).map_err(errors::charge)
            }
            Self::Map(items) => {
                let mut out = builder.vec();
                let (items, _charge) = items.into_parts();
                for (key, item) in items {
                    out.push_entry(key, item.build(builder)?)
                        .map_err(errors::charge)?;
                }
                Ok(builder.map(0, out))
            }
        }
    }
}

struct State<'a> {
    rows: MeteredVec<NodeRow>,
    active: MeteredVec<usize>,
    staging: Charge,
    limits: &'a Limits,
    count: u64,
    tree: Option<&'a Tree>,
    lookups: lookup::Lookups,
}

/// Called on a scoped worker, outside the GIL. Python inspection alone attaches.
pub(crate) fn parsed(input: &Py<PyAny>, meter: &Arc<Meter>, limits: &Limits) -> PyResult<Parsed> {
    snapshot(input, meter, limits, None)
}

pub(crate) fn for_tree(
    input: &Py<PyAny>,
    meter: &Arc<Meter>,
    limits: &Limits,
    tree: &Tree,
) -> PyResult<Parsed> {
    snapshot(input, meter, limits, Some(tree))
}

fn snapshot(
    input: &Py<PyAny>,
    meter: &Arc<Meter>,
    limits: &Limits,
    tree: Option<&Tree>,
) -> PyResult<Parsed> {
    let mut state = State {
        rows: MeteredVec::new(meter),
        active: MeteredVec::new(meter),
        staging: Charge::zero(meter),
        limits,
        count: 0,
        tree,
        lookups: lookup::Lookups::new(tree, meter)?,
    };
    let (_, value) = state.stage(
        input,
        RawStr::from_static(b"Base"),
        0,
        tree.map(|t| t.list().root()),
    )?;
    let list = NodeList::from_rows(state.rows)
        .and_then(NodeList::into_shared)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(errors::message(&e)))?;
    let builder = ValueBuilder::new(meter, list).map_err(errors::charge)?;
    let value = value.build(&builder)?;
    Ok(builder.finish(value, 0))
}

fn text(value: &Bound<'_, PyAny>, staging: &mut Charge) -> PyResult<RawStr> {
    let text = value.cast::<PyString>()?;
    let len = usize::try_from(unsafe { ffi::PyUnicode_GetLength(text.as_ptr()) })?;
    let bound = (len as u64).saturating_mul(4);
    let _encoding = staging
        .meter()
        .charge(256_u64.saturating_add(bound))
        .map_err(errors::charge)?;
    let bytes = unsafe {
        Bound::from_owned_ptr_or_err(
            value.py(),
            ffi::PyUnicode_AsEncodedString(
                text.as_ptr(),
                c"utf-8".as_ptr(),
                c"surrogateescape".as_ptr(),
            ),
        )?
    };
    let bytes = bytes.cast::<PyBytes>()?;
    staging
        .meter()
        .count_decode(bytes.as_bytes().len() as u64)
        .map_err(errors::charge)?;
    RawStr::charged(bytes.as_bytes(), staging.meter()).map_err(errors::charge)
}

enum Inspect {
    Scalar(Value),
    Sequence(Py<PyIterator>, bool),
    Map(Fields),
}
struct Fields {
    input: Py<PyAny>,
    attrs: Option<Py<PyIterator>>,
    keys: Option<Py<PyIterator>>,
    seen: Py<PySet>,
    dict: bool,
    keep_none: bool,
    schema: Option<Py<PyIterator>>,
    raw_names: bool,
}
impl Fields {
    fn restrict(&mut self, tree: &Tree, node: NodeId, staging: &mut Charge) -> PyResult<()> {
        let children = tree.list().children(node);
        staging
            .grow(256 + (children.len() as u64).saturating_mul(128))
            .map_err(errors::charge)?;
        staging
            .meter()
            .count_decode(children.len() as u64)
            .map_err(errors::charge)?;
        let rows = crate::python::list()?;
        for &child in children {
            let raw = tree.list().name(child);
            staging
                .grow((raw.len() as u64).saturating_mul(8))
                .map_err(errors::charge)?;
            staging
                .meter()
                .count_decode(raw.len() as u64)
                .map_err(errors::charge)?;
            let key = if self.dict {
                RawStr::charged(raw, staging.meter()).map_err(errors::charge)?
            } else {
                lookup::clean(raw, staging.meter())?
            };
            let pair = crate::python::list()?;
            crate::python::push(&pair, &crate::python::string(raw)?)?;
            crate::python::push(&pair, &crate::python::string(key.as_bytes())?)?;
            crate::python::push(&rows, &pair)?;
        }
        self.schema = Some(Python::attach(|py| {
            rows.bind(py).try_iter().map(Bound::unbind)
        })?);
        self.raw_names = true;
        self.keep_none = true;
        Ok(())
    }
    fn next(
        &mut self,
        py: Python<'_>,
        staging: &mut Charge,
    ) -> PyResult<Option<(RawStr, Py<PyAny>)>> {
        loop {
            staging.grow(256).map_err(errors::charge)?;
            if let Some(schema) = &self.schema {
                let Some(pair) = schema.bind(py).clone().next() else {
                    return Ok(None);
                };
                let pair = pair?;
                let raw = pair.get_item(0)?;
                if self.seen.bind(py).contains(&raw)? {
                    continue;
                }
                self.seen.bind(py).add(&raw)?;
                let key = pair.get_item(1)?;
                let child = if self.dict {
                    self.input.bind(py).get_item(&key)?
                } else {
                    self.input.bind(py).getattr(key.cast::<PyString>()?)?
                };
                return Ok(Some((text(&raw, staging)?, child.unbind())));
            }
            let attr = self
                .attrs
                .as_ref()
                .and_then(|iter| iter.bind(py).clone().next());
            let key = if let Some(attr) = attr {
                attr?.getattr("name")?
            } else {
                self.attrs = None;
                let Some(key) = self
                    .keys
                    .as_ref()
                    .and_then(|iter| iter.bind(py).clone().next())
                else {
                    return Ok(None);
                };
                key?
            };
            if self.seen.bind(py).contains(&key)? {
                continue;
            }
            self.seen.bind(py).add(&key)?;
            let name = text(&key, staging)?;
            if !self.dict
                && (name.as_bytes().starts_with(b"__")
                    || matches!(name.as_bytes(), b"object_reader" | b"assetsfile"))
            {
                continue;
            }
            let child = if self.dict {
                self.input.bind(py).get_item(&key)?
            } else {
                self.input.bind(py).getattr(key.cast::<PyString>()?)?
            };
            if !self.dict && !self.keep_none && child.is_none() {
                continue;
            }
            return Ok(Some((name, child.unbind())));
        }
    }
}

impl State<'_> {
    fn row(&mut self, depth: u32, ty: RawStr, name: RawStr) -> PyResult<NodeId> {
        let id = NodeId(u32::try_from(self.rows.len())?);
        self.rows
            .push(NodeRow::new(i32::try_from(depth)?, ty, name))
            .map_err(errors::charge)?;
        Ok(id)
    }
    fn inspect(&mut self, input: &Py<PyAny>) -> PyResult<(RawStr, Inspect)> {
        Python::attach(|py| {
            let value = input.bind(py);
            let scalar = if value.is_instance_of::<PyBool>() {
                Some((b"bool".as_slice(), Value::Bool(value.extract()?)))
            } else if value.is_instance_of::<PyInt>() {
                Some((
                    b"int".as_slice(),
                    match value.extract::<i64>() {
                        Ok(n) => Value::Int(n),
                        Err(_) => Value::UInt(value.extract::<u64>()?),
                    },
                ))
            } else if value.is_instance_of::<PyFloat>() {
                Some((
                    b"double".as_slice(),
                    Value::F64(value.extract::<f64>()?.to_bits()),
                ))
            } else if value.is_instance_of::<PyString>() {
                Some((
                    b"string".as_slice(),
                    Value::Str(text(value, &mut self.staging)?),
                ))
            } else if value.is_instance_of::<PyBytes>()
                || value.is_instance_of::<PyByteArray>()
                || value.is_instance_of::<PyMemoryView>()
            {
                Some((
                    b"TypelessData".as_slice(),
                    buffer_value(value, &self.staging, self.limits)?,
                ))
            } else {
                None
            };
            if let Some((ty, value)) = scalar {
                return Ok((
                    RawStr::charged(ty, self.staging.meter()).map_err(errors::charge)?,
                    Inspect::Scalar(value),
                ));
            }
            if value.is_instance_of::<PyList>() || value.is_instance_of::<PyTuple>() {
                let tuple = value.is_instance_of::<PyTuple>();
                return Ok((
                    RawStr::from_static(if tuple { b"pair" } else { b"vector" }),
                    Inspect::Sequence(value.try_iter()?.unbind(), tuple),
                ));
            }
            self.staging.grow(512).map_err(errors::charge)?;
            let dict = value.is_instance_of::<PyDict>();
            let attrs = if dict {
                None
            } else {
                optional_attr(value.get_type().as_any(), "__attrs_attrs__")?
                    .map(|fields| fields.try_iter().map(Bound::unbind))
                    .transpose()?
            };
            let keys = if dict {
                Some(value.try_iter()?.unbind())
            } else {
                optional_attr(value, "__dict__")?
                    .map(|d| d.try_iter().map(Bound::unbind))
                    .transpose()?
            };
            if !dict && attrs.is_none() && keys.is_none() {
                return Err(pyo3::exceptions::PyTypeError::new_err(
                    "unsupported Python value in export or save",
                ));
            }
            let ty = text(value.get_type().name()?.as_any(), &mut self.staging)?;
            Ok((
                ty,
                Inspect::Map(Fields {
                    input: input.clone_ref(py),
                    attrs,
                    keys,
                    seen: PySet::empty(py)?.unbind(),
                    dict,
                    keep_none: self.tree.is_some(),
                    schema: None,
                    raw_names: false,
                }),
            ))
        })
    }
    fn stage(
        &mut self,
        input: &Py<PyAny>,
        name: RawStr,
        depth: u32,
        shape: Option<NodeId>,
    ) -> PyResult<(NodeId, Staged)> {
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
        let (ty, inspection) = if let Some((tree, shape)) = self.tree.zip(shape)
            && Python::attach(|py| input.bind(py).is_none())
        {
            none_for_shape(tree, shape, input, self.staging.meter())?
        } else {
            self.inspect(input)?
        };
        let id = self.row(depth, ty, name)?;
        if let Inspect::Scalar(value) = inspection {
            return Ok((id, Staged::Scalar(value)));
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
        let staged = match inspection {
            Inspect::Sequence(iter, tuple) => self.sequence(&iter, tuple, depth, shape)?,
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
                let mut items = MeteredVec::new(self.staging.meter());
                while let Some((key, child)) =
                    Python::attach(|py| fields.next(py, &mut self.staging))?
                {
                    items.reserve(1).map_err(errors::charge)?;
                    let child_shape = if let Some((tree, node)) = self.tree.zip(shape) {
                        self.lookups.find(
                            tree,
                            node,
                            fields.dict || fields.raw_names,
                            key.as_bytes(),
                            self.staging.meter(),
                        )?
                    } else {
                        None
                    };
                    let key = if fields.dict || fields.raw_names {
                        key
                    } else if let Some((tree, node)) = self.tree.zip(child_shape) {
                        RawStr::charged(tree.list().name(node), self.staging.meter())
                            .map_err(errors::charge)?
                    } else {
                        key
                    };
                    let (key, value) =
                        self.stage(&child, key, next_depth(depth, 1)?, child_shape)?;
                    items.push((key, value)).map_err(errors::charge)?;
                }
                Staged::Map(items)
            }
            Inspect::Scalar(_) => {
                return Err(pyo3::exceptions::PyTypeError::new_err(
                    "invalid snapshot state",
                ));
            }
        };
        self.active.pop();
        Ok((id, staged))
    }

    fn sequence(
        &mut self,
        iter: &Py<PyIterator>,
        tuple: bool,
        depth: u32,
        shape: Option<NodeId>,
    ) -> PyResult<Staged> {
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
        let mut items = MeteredVec::new(self.staging.meter());
        let first = if tuple { None } else { next_item(iter)? };
        loop {
            items.reserve(1).map_err(errors::charge)?;
            let Some(child) = next_item(iter)? else {
                break;
            };
            let key = if tuple {
                if items.is_empty() {
                    b"first".as_slice()
                } else {
                    b"second".as_slice()
                }
            } else {
                b"data".as_slice()
            };
            let child_shape = self.tree.zip(shape).and_then(|(tree, node)| {
                let kids = tree.list().children(node);
                if tuple {
                    kids.get(items.len()).copied()
                } else {
                    kids.first()
                        .and_then(|array| tree.list().children(*array).get(1))
                        .copied()
                }
            });
            // Later list schemas live below the unused size primitive. This
            // retains their field IDs while Array's public shape stays exactly
            // [size, element], as the geometry object-mode view requires.
            let child = self
                .stage(
                    &child,
                    RawStr::from_static(key),
                    next_depth(depth, if tuple { 1 } else { 3 })?,
                    child_shape,
                )?
                .1;
            items.push(child).map_err(errors::charge)?;
        }
        if !tuple {
            if let Some(first) = first {
                let child_shape = self.tree.zip(shape).and_then(|(tree, node)| {
                    tree.list()
                        .children(node)
                        .first()
                        .and_then(|array| tree.list().children(*array).get(1))
                        .copied()
                });
                let first = self
                    .stage(
                        &first,
                        RawStr::from_static(b"data"),
                        next_depth(depth, 2)?,
                        child_shape,
                    )?
                    .1;
                items.push(first).map_err(errors::charge)?;
                self.staging
                    .meter()
                    .count_decode(items.len() as u64)
                    .map_err(errors::charge)?;
                items.as_mut_slice().rotate_right(1);
            } else {
                self.row(
                    next_depth(depth, 2)?,
                    RawStr::from_static(b"Object"),
                    RawStr::from_static(b"data"),
                )?;
            }
        }
        Ok(if tuple {
            Staged::Tuple(items)
        } else {
            Staged::List(items)
        })
    }
}

fn copy_bytes(bytes: &[u8], staging: &Charge, limits: &Limits) -> PyResult<Value> {
    let mut buffer = DecodeBuf::new(
        staging.meter(),
        0,
        limits.max_input_memory,
        unityrs_core::LimitKind::InputMemory,
    )
    .map_err(errors::charge)?;
    staging
        .meter()
        .count_decode(bytes.len() as u64)
        .map_err(errors::charge)?;
    buffer.extend_from_slice(bytes).map_err(errors::charge)?;
    Ok(Value::Bytes(buffer.into_bytes().map_err(errors::charge)?))
}

fn buffer_value(value: &Bound<'_, PyAny>, staging: &Charge, limits: &Limits) -> PyResult<Value> {
    if let Ok(value) = value.cast::<PyByteArray>() {
        // The GIL stays held through this copy, so mutable bytearray storage
        // cannot resize or change while its borrowed bytes are read.
        return copy_bytes(unsafe { value.as_bytes() }, staging, limits);
    }
    if value.is_instance_of::<PyMemoryView>() {
        let count = value.getattr("nbytes")?.extract::<usize>()?;
        let _copy = staging
            .meter()
            .charge(256 + count as u64)
            .map_err(errors::charge)?;
        staging
            .meter()
            .count_decode(count as u64)
            .map_err(errors::charge)?;
        let encoded = value.call_method0("tobytes")?;
        return copy_bytes(encoded.cast::<PyBytes>()?.as_bytes(), staging, limits);
    }
    copy_bytes(value.cast::<PyBytes>()?.as_bytes(), staging, limits)
}

fn next_item(iter: &Py<PyIterator>) -> PyResult<Option<Py<PyAny>>> {
    Python::attach(|py| {
        iter.bind(py)
            .clone()
            .next()
            .map(|v| v.map(Bound::unbind))
            .transpose()
    })
}

fn optional_attr<'py>(
    value: &Bound<'py, PyAny>,
    name: &str,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    match value.getattr(name) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.is_instance_of::<pyo3::exceptions::PyAttributeError>(value.py()) => {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn none_for_shape(
    tree: &Tree,
    node: NodeId,
    input: &Py<PyAny>,
    meter: &Arc<Meter>,
) -> PyResult<(RawStr, Inspect)> {
    use unityrs_core::typetree::{Kind, tree::py_kind_of};
    let ty = tree.list().type_name(node);
    meter
        .count_decode(ty.len() as u64)
        .map_err(errors::charge)?;
    match py_kind_of(ty) {
        Kind::Bool => Ok((
            RawStr::from_static(b"bool"),
            Inspect::Scalar(Value::Bool(false)),
        )),
        Kind::S8
        | Kind::U8
        | Kind::S16
        | Kind::U16
        | Kind::S32
        | Kind::U32
        | Kind::S64
        | Kind::U64
        | Kind::F32
        | Kind::F64 => Python::attach(|py| {
            let format = if matches!(py_kind_of(ty), Kind::F32 | Kind::F64) {
                "f"
            } else {
                "i"
            };
            py.import("struct")?
                .call_method1("pack", (format, input.bind(py)))?;
            Err(pyo3::exceptions::PyTypeError::new_err(
                "None is not a numeric writer value",
            ))
        }),
        Kind::Str => Python::attach(|py| {
            input.bind(py).getattr("encode")?;
            Err(pyo3::exceptions::PyTypeError::new_err(
                "None is not a string writer value",
            ))
        }),
        _ if tree.list().children(node).is_empty() => {
            Ok((RawStr::from_static(b"int"), Inspect::Scalar(Value::Int(0))))
        }
        _ => Err(pyo3::exceptions::PyTypeError::new_err(
            "unsupported None in structured writer value",
        )),
    }
}

fn next_depth(depth: u32, step: u32) -> PyResult<u32> {
    depth
        .checked_add(step)
        .ok_or_else(|| errors::LimitError::new_err("Python value depth overflow"))
}
