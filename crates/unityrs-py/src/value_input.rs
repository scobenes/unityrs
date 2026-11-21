// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp (clean_name). See THIRD_PARTY_LICENSES.
//! Charged snapshots of current Python values. No generated schema is retained.
//! The synthetic node names describe only the values supplied by this caller.
use crate::{direct_keys::name, errors};
use pyo3::{
    ffi,
    prelude::*,
    types::{
        PyBool, PyBytes, PyDict, PyFloat, PyInt, PyIterator, PyList, PyMemoryView, PySet, PyString,
        PyTuple,
    },
};
use std::sync::Arc;
use unityrs_core::{
    RawStr, Value,
    budget::DecodeBuf,
    limits::{Charge, Limits, Meter, MeteredVec},
    typetree::{NodeId, NodeRow, Tree},
};
#[path = "value_input_lengths.rs"]
mod lengths;
#[path = "value_input_lookup.rs"]
mod lookup;
#[path = "value_input_schema.rs"]
mod schema;
#[path = "value_input_stage.rs"]
mod staging;
pub(crate) use staging::Snapshot;

struct State<'a> {
    rows: MeteredVec<NodeRow>,
    active: MeteredVec<usize>,
    staging: Charge,
    limits: &'a Limits,
    count: u64,
    tree: Option<&'a Tree>,
    schemas: schema::Schemas<'a>,
    refs: &'a [unityrs_core::files::SerializedType],
    values: staging::Arena,
    filter_null_properties: bool,
    keep_none: bool,
    rotations: MeteredVec<staging::Rotation>,
    logical_lengths: MeteredVec<(u64, usize)>,
}

#[cfg(test)]
pub(crate) fn parsed(
    input: &Py<PyAny>,
    meter: &Arc<Meter>,
    limits: &Limits,
) -> PyResult<unityrs_core::Parsed> {
    snapshot(input, meter, limits, None)
}

#[cfg(test)]
pub(crate) fn for_tree(
    input: &Py<PyAny>,
    meter: &Arc<Meter>,
    limits: &Limits,
    tree: &Tree,
) -> PyResult<unityrs_core::Parsed> {
    snapshot(input, meter, limits, Some(tree))
}

pub(crate) fn for_tree_snapshot(
    input: &Py<PyAny>,
    meter: &Arc<Meter>,
    limits: &Limits,
    tree: &Tree,
    refs: &[unityrs_core::files::SerializedType],
) -> PyResult<Snapshot> {
    snapshot_input(input, meter, limits, Some(tree), false, refs, false)
}

#[cfg(test)]
fn snapshot(
    input: &Py<PyAny>,
    meter: &Arc<Meter>,
    limits: &Limits,
    tree: Option<&Tree>,
) -> PyResult<unityrs_core::Parsed> {
    snapshot_input(input, meter, limits, tree, false, &[], false)?
        .build()
        .map_err(Into::into)
}

pub(crate) fn material_snapshot(
    input: &Py<PyAny>,
    meter: &Arc<Meter>,
    limits: &Limits,
) -> PyResult<Snapshot> {
    snapshot_input(input, meter, limits, None, true, &[], false)
}

pub(crate) fn export_snapshot(
    input: &Py<PyAny>,
    meter: &Arc<Meter>,
    limits: &Limits,
) -> PyResult<Snapshot> {
    snapshot_input(input, meter, limits, None, false, &[], false)
}

pub(crate) fn shader_snapshot(
    input: &Py<PyAny>,
    meter: &Arc<Meter>,
    limits: &Limits,
) -> PyResult<Snapshot> {
    snapshot_input(input, meter, limits, None, false, &[], true)
}

fn snapshot_input(
    input: &Py<PyAny>,
    meter: &Arc<Meter>,
    limits: &Limits,
    tree: Option<&Tree>,
    filter_null_properties: bool,
    refs: &[unityrs_core::files::SerializedType],
    keep_none: bool,
) -> PyResult<Snapshot> {
    crate::python::relay::attach(|py| {
        py.detach(|| {
            snapshot_input_native(
                input,
                meter,
                limits,
                tree,
                filter_null_properties,
                refs,
                keep_none,
            )
        })
    })
}

fn snapshot_input_native(
    input: &Py<PyAny>,
    meter: &Arc<Meter>,
    limits: &Limits,
    tree: Option<&Tree>,
    filter_null_properties: bool,
    refs: &[unityrs_core::files::SerializedType],
    keep_none: bool,
) -> PyResult<Snapshot> {
    let mut state = State {
        rows: MeteredVec::new(meter),
        active: MeteredVec::new(meter),
        staging: Charge::zero(meter),
        limits,
        count: 0,
        tree,
        schemas: schema::Schemas::new(tree, meter)?,
        refs,
        values: staging::Arena::new(meter),
        filter_null_properties,
        keep_none,
        rotations: MeteredVec::new(meter),
        logical_lengths: MeteredVec::new(meter),
    };
    let (_, value) = state.stage(input, RawStr::from_static(b"Base"), 0, state.schemas.root())?;
    Ok(state.into_snapshot(value))
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
    let raw = bytes.as_bytes();
    value
        .py()
        .detach(|| RawStr::charged(raw, staging.meter()).map_err(errors::charge))
}

enum Inspect {
    Scalar(Value),
    Sized(Value, usize),
    Sequence(Elements, bool, Option<usize>),
    Map(Fields),
}
enum Elements {
    Iter(Py<PyIterator>),
    Pair(Py<PyAny>),
}
enum NullFields {
    Omit,
    Preserve,
    ShaderClip,
}
struct Fields {
    input: Py<PyAny>,
    attrs: Option<Py<PyIterator>>,
    keys: Option<Py<PyIterator>>,
    seen: Py<PySet>,
    dict: bool,
    nulls: NullFields,
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
        self.schema = Some(crate::python::relay::attach(|py| {
            rows.bind(py).try_iter().map(Bound::unbind)
        })?);
        self.raw_names = true;
        self.nulls = NullFields::Preserve;
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
                attr?.getattr(name!(py, c"name")?)?
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
            let keep_none = self.dict
                || match self.nulls {
                    NullFields::Omit => false,
                    NullFields::Preserve => true,
                    NullFields::ShaderClip => name.as_bytes() == b"zClip",
                };
            if !keep_none && child.is_none() {
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
        crate::python::relay::attach(|py| {
            let value = input.bind(py);
            if self.keep_none && value.is_none() {
                // Parsed has no null variant. Preserve explicit Python None
                // with a synthetic NoneType row and an allocation-free scalar.
                return Ok((
                    RawStr::from_static(b"NoneType"),
                    Inspect::Scalar(Value::Bool(false)),
                ));
            }
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
            } else {
                buffer_value(value, &self.staging, self.limits)?
                    .map(|buffer| (b"TypelessData".as_slice(), buffer))
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
                    Inspect::Sequence(Elements::Iter(value.try_iter()?.unbind()), tuple, None),
                ));
            }
            self.staging.grow(512).map_err(errors::charge)?;
            let dict = value.is_instance_of::<PyDict>();
            let attrs = if dict {
                None
            } else {
                optional_attr(
                    value.get_type().as_any(),
                    name!(value.py(), c"__attrs_attrs__")?,
                )?
                .map(|fields| fields.try_iter().map(Bound::unbind))
                .transpose()?
            };
            let keys = if dict {
                Some(value.try_iter()?.unbind())
            } else {
                optional_attr(value, name!(value.py(), c"__dict__")?)?
                    .map(|d| d.try_iter().map(Bound::unbind))
                    .transpose()?
            };
            if !dict && attrs.is_none() && keys.is_none() {
                return Err(crate::errors::new::<pyo3::exceptions::PyTypeError>(
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
                    nulls: if self.tree.is_some() {
                        NullFields::Preserve
                    } else if self.keep_none {
                        NullFields::ShaderClip
                    } else {
                        NullFields::Omit
                    },
                    schema: None,
                    raw_names: false,
                }),
            ))
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

fn buffer_value(
    value: &Bound<'_, PyAny>,
    staging: &Charge,
    limits: &Limits,
) -> PyResult<Option<Value>> {
    let charge = staging.meter().charge(256).map_err(errors::charge)?;
    let view = match PyMemoryView::from(value) {
        Ok(view) => view,
        Err(error) if error.is_instance_of::<pyo3::exceptions::PyTypeError>(value.py()) => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let bytes = crate::buffer_snapshot::BufferSnapshot::from_view(value, &view, charge)?;
    value
        .py()
        .detach(|| copy_bytes(bytes.as_ref(), staging, limits))
        .map(Some)
}

fn next_item(iter: &Elements, index: usize) -> PyResult<Option<Py<PyAny>>> {
    crate::python::relay::attach(|py| {
        let Elements::Iter(iter) = iter else {
            let Elements::Pair(input) = iter else {
                return Ok(None);
            };
            return if index == 2 {
                Ok(None)
            } else {
                input.bind(py).get_item(index).map(|v| Some(v.unbind()))
            };
        };
        iter.bind(py)
            .clone()
            .next()
            .map(|v| v.map(Bound::unbind))
            .transpose()
    })
}

/// `value.name`, `None` where it is absent; `name` interned (`name!`).
fn optional_attr<'py>(
    value: &Bound<'py, PyAny>,
    name: &Bound<'py, PyString>,
) -> PyResult<Option<Bound<'py, PyAny>>> {
    match value.getattr(name) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.is_instance_of::<pyo3::exceptions::PyAttributeError>(value.py()) => {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn writer_buffer(
    input: &Bound<'_, PyAny>,
    meter: &Arc<Meter>,
) -> PyResult<(crate::buffer_snapshot::BufferSnapshot, usize)> {
    // UnityPy measures the value before asking its writer for a buffer.
    // Preserve both operations' Python errors without copying invalid input.
    let len = input.len()?;
    Ok((
        crate::buffer_snapshot::BufferSnapshot::writer(input, meter)?,
        len,
    ))
}

fn scalar_for_shape(
    tree: &Tree,
    node: NodeId,
    input: &Py<PyAny>,
    staging: &mut Charge,
    limits: &Limits,
) -> PyResult<Option<(RawStr, Inspect)>> {
    use unityrs_core::typetree::{Kind, tree::py_kind_of};
    let ty = tree.list().type_name(node);
    let kind = py_kind_of(ty);
    if matches!(kind, Kind::Str | Kind::Bytes) {
        let value = crate::python::relay::attach(|py| {
            let input = input.bind(py);
            if kind == Kind::Str {
                // Charge the encoder's bound before invoking Python. Calling
                // encode preserves AttributeError and subclass protocols.
                let mut encoding = staging.meter().charge(256).map_err(errors::charge)?;
                if input.is_instance_of::<PyString>() {
                    let len = unsafe { ffi::PyUnicode_GetLength(input.as_ptr()) };
                    encoding
                        .grow(u64::try_from(len)?.saturating_mul(4))
                        .map_err(errors::charge)?;
                }
                let encoded = crate::python::held::call(
                    &input.getattr(name!(py, c"encode")?)?,
                    &[
                        name!(py, c"utf8")?.clone().into_any(),
                        name!(py, c"surrogateescape")?.clone().into_any(),
                    ],
                )?;
                let (bytes, logical_len) = writer_buffer(&encoded, staging.meter())?;
                staging
                    .meter()
                    .count_decode(bytes.as_ref().len() as u64)
                    .map_err(errors::charge)?;
                return py.detach(|| {
                    RawStr::charged(bytes.as_ref(), staging.meter())
                        .map(Value::Str)
                        .map(|value| Inspect::Sized(value, logical_len))
                        .map_err(errors::charge)
                });
            }
            let (bytes, logical_len) = writer_buffer(input, staging.meter())?;
            py.detach(|| copy_bytes(bytes.as_ref(), staging, limits))
                .map(|value| Inspect::Sized(value, logical_len))
        })?;
        return Ok(Some((
            RawStr::charged(ty, staging.meter()).map_err(errors::charge)?,
            value,
        )));
    }
    let format = match kind {
        Kind::Bool => "<?",
        Kind::S8 => "<b",
        Kind::U8 => "<B",
        Kind::S16 => "<h",
        Kind::U16 => "<H",
        Kind::S32 => "<i",
        Kind::U32 => "<I",
        Kind::S64 => "<q",
        Kind::U64 => "<Q",
        Kind::F32 => "<f",
        Kind::F64 => "<d",
        _ => return Ok(None),
    };
    staging.grow(256).map_err(errors::charge)?;
    staging.meter().count_decode(8).map_err(errors::charge)?;
    let value = crate::python::relay::attach(|py| {
        // Match the writer's Python protocols and struct.error before any
        // narrowing into Rust integers. This also handles arbitrary __bool__,
        // __index__, and __float__ implementations on the caller's thread.
        let packed = struct_pack(py, format, input.bind(py))?;
        let bytes = packed.cast::<PyBytes>()?.as_bytes();
        let mut raw = [0_u8; 8];
        raw.get_mut(..bytes.len())
            .ok_or_else(|| {
                crate::errors::new::<pyo3::exceptions::PyValueError>(
                    "invalid numeric snapshot width",
                )
            })?
            .copy_from_slice(bytes);
        Ok::<_, PyErr>(match kind {
            Kind::Bool => Value::Bool(bytes.first().copied().unwrap_or_default() != 0),
            Kind::F32 => {
                let bits = u32::try_from(u64::from_le_bytes(raw))?;
                Value::F32(bits)
            }
            Kind::F64 => Value::F64(u64::from_le_bytes(raw)),
            Kind::S8 | Kind::S16 | Kind::S32 | Kind::S64 => {
                if bytes.last().is_some_and(|byte| byte & 0x80 != 0) {
                    raw.get_mut(bytes.len()..).unwrap_or_default().fill(0xff);
                }
                Value::Int(i64::from_le_bytes(raw))
            }
            _ => Value::UInt(u64::from_le_bytes(raw)),
        })
    })?;
    Ok(Some((
        RawStr::charged(ty, staging.meter()).map_err(errors::charge)?,
        Inspect::Scalar(value),
    )))
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
        | Kind::F64 => crate::python::relay::attach(|py| {
            let format = if matches!(py_kind_of(ty), Kind::F32 | Kind::F64) {
                "f"
            } else {
                "i"
            };
            struct_pack(py, format, input.bind(py))?;
            Err(crate::errors::new::<pyo3::exceptions::PyTypeError>(
                "None is not a numeric writer value",
            ))
        }),
        Kind::Str => crate::python::relay::attach(|py| {
            input.bind(py).getattr(name!(py, c"encode")?)?;
            Err(crate::errors::new::<pyo3::exceptions::PyTypeError>(
                "None is not a string writer value",
            ))
        }),
        _ if tree.list().children(node).is_empty() => {
            Ok((RawStr::from_static(b"int"), Inspect::Scalar(Value::Int(0))))
        }
        _ => Err(crate::errors::new::<pyo3::exceptions::PyTypeError>(
            "unsupported None in structured writer value",
        )),
    }
}

/// `struct.pack(format, value)`, every object made fallibly.
fn struct_pack<'py>(
    py: Python<'py>,
    format: &str,
    value: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    let pack = crate::direct_keys::import_attr(py, name!(py, c"struct")?, name!(py, c"pack")?)?;
    crate::python::held::call(&pack, &[errors::text_of(py, format)?, value.clone()])
}

fn next_depth(depth: u32, step: u32) -> PyResult<u32> {
    depth.checked_add(step).ok_or_else(|| {
        crate::errors::new::<crate::errors::LimitError>("Python value depth overflow")
    })
}
