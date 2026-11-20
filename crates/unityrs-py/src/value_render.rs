// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeHelper.py. See THIRD_PARTY_LICENSES.
//! Parsed values to Python objects, without recursion. [`stage`] runs
//! detached: it charges every value's staging and sizes the key cache.
//! [`Staged::render`] then only builds objects, attached, and lets other
//! Python threads run every [`CHUNK`] objects.
use crate::{python::held as h, worker_error::Failure};
use pyo3::prelude::*;
use std::sync::Arc;
use unityrs_core::{
    Parsed, PrimArray, Value,
    limits::{Charge, Meter, MeteredVec},
    typetree::NodeId,
    value::Map,
};

/// Each value's own staging. A container charges its children's (their text
/// and bytes included) with its own: one meter reservation per container.
const VALUE: u64 = 256;

/// Objects built between releases of the GIL while rendering.
pub(crate) const CHUNK: u32 = 8192;

/// Counts built objects; every [`CHUNK`], detaches briefly so a large
/// render does not keep other Python threads waiting for its whole length.
/// Whatever another thread can see meanwhile is complete: lists hold `None`
/// until filled, and tuples are made from ready parts.
pub(crate) struct Pace(u32);

impl Pace {
    pub(crate) fn new() -> Self {
        Self(0)
    }
    pub(crate) fn tick(&mut self, py: Python<'_>) {
        self.0 += 1;
        if self.0 >= CHUNK {
            self.0 = 0;
            py.detach(|| ());
        }
    }
}

/// A string's or bytes' payload staging, charged with its container.
fn payload(v: &Value) -> u64 {
    match v {
        Value::Str(x) => (x.as_bytes().len() as u64).saturating_mul(4),
        Value::Bytes(x) => x.len() as u64,
        _ => 0,
    }
}

fn payloads<'a>(values: impl Iterator<Item = &'a Value>) -> u64 {
    values.fold(0, |sum, v| sum.saturating_add(payload(v)))
}

fn charged(error: unityrs_core::limits::ChargeError) -> PyErr {
    Failure::charge(error).into()
}

/// The value of `parsed` (dict keys by name, or by field index when
/// `fields`), staged detached and built with one attachment. Conservative
/// `CPython` staging costs go to `charge`; returned Python objects belong to
/// Python's allocator after the call, outside the Rust memory budget.
pub(crate) fn convert(
    parsed: &Parsed,
    meter: &Arc<Meter>,
    charge: &mut Charge,
    fields: bool,
) -> PyResult<Py<PyAny>> {
    let staged = stage(parsed, meter, charge, fields)?;
    crate::python::relay::attach(move |py| staged.render(py).map(Bound::unbind))
}

/// A value whose staging is charged, ready to build.
pub(crate) struct Staged<'a> {
    parsed: &'a Parsed,
    meter: &'a Arc<Meter>,
    fields: bool,
    /// Each node name's Python string, made once per call (lists of dicts
    /// repeat their fields' names): one slot per row of each node list.
    keys: MeteredVec<MeteredVec<Option<Py<PyAny>>>>,
}

/// Charge every value's staging and decode work, in the order rendering
/// reaches the values, and size the key cache. No Python is touched.
pub(crate) fn stage<'a>(
    parsed: &'a Parsed,
    meter: &'a Arc<Meter>,
    charge: &mut Charge,
    fields: bool,
) -> PyResult<Staged<'a>> {
    let mut keys = MeteredVec::with_capacity(meter, parsed.lists().len()).map_err(charged)?;
    for nodes in parsed.lists() {
        let rows = if fields { 0 } else { nodes.rows().len() };
        let mut slots = MeteredVec::with_capacity(meter, rows).map_err(charged)?;
        for _ in 0..rows {
            slots.push(None).map_err(charged)?;
        }
        keys.push(slots).map_err(charged)?;
    }
    // Room for a typical object's pending values: one reservation, not one per doubling.
    let mut pending = MeteredVec::with_capacity(meter, 64).map_err(charged)?;
    pending.push(parsed.value()).map_err(charged)?;
    charge
        .grow(VALUE.saturating_add(payload(parsed.value())))
        .map_err(charged)?;
    while let Some(v) = pending.pop() {
        let (each, names, n, bytes) = match v {
            Value::Str(_) | Value::Bytes(_) => {
                meter.count_decode(payload(v)).map_err(charged)?;
                continue;
            }
            Value::Dict(map) => {
                let names = map.entries().iter().fold(0_u64, |sum, (key, _)| {
                    let name = parsed.key_name(map, *key);
                    sum.saturating_add((name.len() as u64).saturating_mul(8))
                });
                let values = map.entries().iter().map(|(_, v)| v);
                (256 + VALUE, names, map.len(), payloads(values))
            }
            Value::List(xs) => (16 + VALUE, 0, xs.len(), payloads(xs.iter())),
            Value::Tuple(xs) => (32 + VALUE, 0, xs.len(), payloads(xs.iter())),
            Value::Prims(array) => {
                charge
                    .grow((array.len() as u64).saturating_mul(64))
                    .map_err(charged)?;
                continue;
            }
            _ => continue,
        };
        charge
            .grow(
                (n as u64)
                    .saturating_mul(each)
                    .saturating_add(names)
                    .saturating_add(bytes),
            )
            .map_err(charged)?;
        match v {
            Value::Dict(map) => schedule(&mut pending, map.entries().iter().map(|(_, v)| v))?,
            Value::List(xs) => schedule(&mut pending, xs.iter())?,
            Value::Tuple(xs) => schedule(&mut pending, xs.iter())?,
            _ => {}
        }
    }
    Ok(Staged {
        parsed,
        meter,
        fields,
        keys,
    })
}

/// Queue `children` to be visited first to last, as rendering reaches them.
fn schedule<'a>(
    pending: &mut MeteredVec<&'a Value>,
    children: impl DoubleEndedIterator<Item = &'a Value>,
) -> PyResult<()> {
    for child in children.rev() {
        pending.push(child).map_err(charged)?;
    }
    Ok(())
}

// A tuple is completed after its children because tuples are immutable: they
// wait on a stack of parts, the tuple's own from where it started.
enum Target<'py> {
    Root,
    /// Pushed on the parts a tuple is made from.
    Part,
    /// Slot `n` of a list made at its final length (holding `None` till then).
    Slot(Bound<'py, PyAny>, usize),
    Dict(Bound<'py, PyAny>, Bound<'py, PyAny>),
}
enum Task<'a, 'py> {
    Value(&'a Value, Target<'py>),
    /// A tuple of the parts from this index on.
    Tuple(usize, Target<'py>),
}

fn assign<'py>(
    target: Target<'py>,
    item: Bound<'py, PyAny>,
    result: &mut Option<Bound<'py, PyAny>>,
    parts: &mut MeteredVec<Bound<'py, PyAny>>,
) -> PyResult<()> {
    match target {
        Target::Root => *result = Some(item),
        Target::Part => parts.push(item).map_err(charged)?,
        Target::Slot(parent, at) => h::put(&parent, at, item)?,
        Target::Dict(parent, key) => h::set(&parent, &key, &item)?,
    }
    Ok(())
}

impl Staged<'_> {
    /// Build the staged value; the caller is attached.
    pub(crate) fn render(mut self, py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
        let parsed = self.parsed;
        let mut tasks = MeteredVec::with_capacity(self.meter, 64).map_err(charged)?;
        tasks
            .push(Task::Value(parsed.value(), Target::Root))
            .map_err(charged)?;
        let mut parts = MeteredVec::new(self.meter);
        let mut result = None;
        // Paced per value built, per child scheduled, and per element.
        let mut pace = Pace::new();
        while let Some(task) = tasks.pop() {
            pace.tick(py);
            let (v, target) = match task {
                Task::Value(v, target) => (v, target),
                Task::Tuple(start, target) => {
                    let tuple = h::tuple_of(py, parts.as_slice().get(start..).unwrap_or_default())?;
                    parts.truncate(start);
                    assign(target, tuple, &mut result, &mut parts)?;
                    continue;
                }
            };
            let item = match v {
                Value::Bool(x) => h::boolean(py, *x)?,
                Value::Int(x) => h::int(py, *x)?,
                Value::UInt(x) => h::uint(py, *x)?,
                Value::F32(x) => h::float(py, f64::from(f32::from_bits(*x)))?,
                Value::F64(x) => h::float(py, f64::from_bits(*x))?,
                Value::Str(x) => h::string(py, x.as_bytes())?,
                Value::Bytes(x) => h::bytes(py, x)?,
                Value::Dict(map) => {
                    let out = h::dict(py)?;
                    // Children are set in forward order as the traversal reaches
                    // them, so a repeated name keeps its first place and last value.
                    for (key, v) in map.entries().iter().rev() {
                        pace.tick(py);
                        let key = if self.fields {
                            h::uint(py, u64::from(key.0))?
                        } else {
                            self.key(py, map, *key)?
                        };
                        tasks
                            .push(Task::Value(v, Target::Dict(out.clone(), key)))
                            .map_err(charged)?;
                    }
                    out
                }
                Value::List(xs) => {
                    let out = h::list_of(py, xs.len())?;
                    for (at, v) in xs.iter().enumerate().rev() {
                        pace.tick(py);
                        tasks
                            .push(Task::Value(v, Target::Slot(out.clone(), at)))
                            .map_err(charged)?;
                    }
                    out
                }
                Value::Tuple(xs) => {
                    tasks
                        .push(Task::Tuple(parts.len(), target))
                        .map_err(charged)?;
                    for v in xs.iter().rev() {
                        pace.tick(py);
                        tasks.push(Task::Value(v, Target::Part)).map_err(charged)?;
                    }
                    continue;
                }
                Value::Prims(array) => primitives(py, array, &mut pace)?,
            };
            assign(target, item, &mut result, &mut parts)?;
        }
        result.ok_or_else(|| Failure::Static("ValueError", "empty conversion").into())
    }

    fn key<'py>(&mut self, py: Python<'py>, map: &Map, key: NodeId) -> PyResult<Bound<'py, PyAny>> {
        let slot = usize::try_from(map.list())
            .ok()
            .and_then(|list| self.keys.as_mut_slice().get_mut(list))
            .and_then(|slots| slots.as_mut_slice().get_mut(usize::try_from(key.0).ok()?));
        // Positions outside the list are made each time.
        let Some(slot) = slot else {
            return h::string(py, self.parsed.key_name(map, key));
        };
        if let Some(name) = slot {
            return Ok(name.bind(py).clone());
        }
        let name = h::string(py, self.parsed.key_name(map, key))?;
        *slot = Some(name.clone().unbind());
        Ok(name)
    }
}

fn primitives<'py>(
    py: Python<'py>,
    array: &PrimArray,
    pace: &mut Pace,
) -> PyResult<Bound<'py, PyAny>> {
    macro_rules! run {
        ($xs:expr, $method:ident, $convert:expr) => {{
            let out = h::list_of(py, $xs.len())?;
            for (at, value) in $xs.iter().enumerate() {
                h::put(&out, at, h::$method(py, $convert(*value))?)?;
                pace.tick(py);
            }
            out
        }};
    }
    Ok(match array {
        PrimArray::Bool(xs) => run!(xs, boolean, |x| x),
        PrimArray::U8(xs) => run!(xs, uint, u64::from),
        PrimArray::I8(xs) => run!(xs, int, i64::from),
        PrimArray::U16(xs) => run!(xs, uint, u64::from),
        PrimArray::I16(xs) => run!(xs, int, i64::from),
        PrimArray::U32(xs) => run!(xs, uint, u64::from),
        PrimArray::I32(xs) => run!(xs, int, i64::from),
        PrimArray::U64(xs) => run!(xs, uint, |x| x),
        PrimArray::I64(xs) => run!(xs, int, |x| x),
        PrimArray::F32(xs) => run!(xs, float, |x| f64::from(f32::from_bits(x))),
        PrimArray::F64(xs) => run!(xs, float, f64::from_bits),
    })
}

#[cfg(test)]
#[path = "value_render_tests.rs"]
mod tests;
