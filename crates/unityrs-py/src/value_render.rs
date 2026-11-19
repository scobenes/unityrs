// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeHelper.py. See THIRD_PARTY_LICENSES.
//! Parsed values to Python objects in one attached pass, without recursion.
//! Each value's staging is charged before it is built, in traversal order.
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

// A tuple is completed after its children because tuples are immutable.
enum Target<'py> {
    Root,
    /// Appended to a list a tuple is made from.
    List(Bound<'py, PyAny>),
    /// Slot `n` of a list made at its final length.
    Slot(Bound<'py, PyAny>, usize),
    Dict(Bound<'py, PyAny>, Bound<'py, PyAny>),
}
enum Task<'a, 'py> {
    Value(&'a Value, Target<'py>),
    Tuple(Bound<'py, PyAny>, Target<'py>),
}

fn charged(error: unityrs_core::limits::ChargeError) -> PyErr {
    Failure::charge(error).into()
}

fn assign<'py>(
    target: Target<'py>,
    item: Bound<'py, PyAny>,
    result: &mut Option<Bound<'py, PyAny>>,
) -> PyResult<()> {
    match target {
        Target::Root => *result = Some(item),
        Target::List(parent) => h::push(&parent, &item)?,
        Target::Slot(parent, at) => h::put(&parent, at, item)?,
        Target::Dict(parent, key) => h::set(&parent, &key, &item)?,
    }
    Ok(())
}

/// The value of `parsed` (dict keys by name, or by field index when
/// `fields`). Conservative `CPython` staging costs go to `charge`; returned
/// Python objects belong to Python's allocator after the call, outside the
/// Rust memory budget.
pub(crate) fn render<'py>(
    py: Python<'py>,
    parsed: &Parsed,
    meter: &Arc<Meter>,
    charge: &mut Charge,
    fields: bool,
) -> PyResult<Bound<'py, PyAny>> {
    let mut tasks = MeteredVec::new(meter);
    tasks
        .push(Task::Value(parsed.value(), Target::Root))
        .map_err(charged)?;
    let mut result = None;
    let mut keys = Keys::new(parsed, meter)?;
    charge
        .grow(VALUE.saturating_add(payload(parsed.value())))
        .map_err(charged)?;
    while let Some(task) = tasks.pop() {
        let (v, target) = match task {
            Task::Value(v, target) => (v, target),
            Task::Tuple(items, target) => {
                assign(target, h::tuple(&items)?, &mut result)?;
                continue;
            }
        };
        let item = match v {
            Value::Bool(x) => h::boolean(py, *x)?,
            Value::Int(x) => h::int(py, *x)?,
            Value::UInt(x) => h::uint(py, *x)?,
            Value::F32(x) => h::float(py, f64::from(f32::from_bits(*x)))?,
            Value::F64(x) => h::float(py, f64::from_bits(*x))?,
            Value::Str(x) => {
                meter.count_decode(payload(v)).map_err(charged)?;
                h::string(py, x.as_bytes())?
            }
            Value::Bytes(x) => {
                meter.count_decode(payload(v)).map_err(charged)?;
                h::bytes(py, x)?
            }
            Value::Dict(map) => {
                let names = map.entries().iter().fold(0_u64, |sum, (key, _)| {
                    let name = parsed.key_name(map, *key);
                    sum.saturating_add((name.len() as u64).saturating_mul(8))
                });
                charge
                    .grow(
                        (map.len() as u64)
                            .saturating_mul(256 + VALUE)
                            .saturating_add(names)
                            .saturating_add(payloads(map.entries().iter().map(|(_, v)| v))),
                    )
                    .map_err(charged)?;
                let out = h::dict(py)?;
                // Children are set in forward order as the traversal reaches
                // them, so a repeated name keeps its first place and last value.
                for (key, v) in map.entries().iter().rev() {
                    let key = if fields {
                        h::uint(py, u64::from(key.0))?
                    } else {
                        keys.name(py, map, *key)?
                    };
                    tasks
                        .push(Task::Value(v, Target::Dict(out.clone(), key)))
                        .map_err(charged)?;
                }
                out
            }
            Value::List(xs) => {
                charge
                    .grow(
                        (xs.len() as u64)
                            .saturating_mul(16 + VALUE)
                            .saturating_add(payloads(xs.iter())),
                    )
                    .map_err(charged)?;
                let out = h::list_of(py, xs.len())?;
                for (at, v) in xs.iter().enumerate().rev() {
                    tasks
                        .push(Task::Value(v, Target::Slot(out.clone(), at)))
                        .map_err(charged)?;
                }
                out
            }
            Value::Tuple(xs) => {
                charge
                    .grow(
                        (xs.len() as u64)
                            .saturating_mul(32 + VALUE)
                            .saturating_add(payloads(xs.iter())),
                    )
                    .map_err(charged)?;
                let out = h::list(py)?;
                tasks
                    .push(Task::Tuple(out.clone(), target))
                    .map_err(charged)?;
                for v in xs.iter().rev() {
                    tasks
                        .push(Task::Value(v, Target::List(out.clone())))
                        .map_err(charged)?;
                }
                continue;
            }
            Value::Prims(array) => primitives(py, array, charge)?,
        };
        assign(target, item, &mut result)?;
    }
    result.ok_or_else(|| Failure::Static("ValueError", "empty conversion").into())
}

fn primitives<'py>(
    py: Python<'py>,
    array: &PrimArray,
    charge: &mut Charge,
) -> PyResult<Bound<'py, PyAny>> {
    charge
        .grow((array.len() as u64).saturating_mul(64))
        .map_err(charged)?;
    macro_rules! run {
        ($xs:expr, $method:ident, $convert:expr) => {{
            let out = h::list_of(py, $xs.len())?;
            for (at, value) in $xs.iter().enumerate() {
                h::put(&out, at, h::$method(py, $convert(*value))?)?;
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

/// Each node name's Python string, made once per call: lists of dicts repeat
/// their fields' names. Unknown positions are made each time.
struct Keys<'a, 'py> {
    parsed: &'a Parsed,
    lists: MeteredVec<MeteredVec<Option<Bound<'py, PyAny>>>>,
}

impl<'a, 'py> Keys<'a, 'py> {
    fn new(parsed: &'a Parsed, meter: &'a Arc<Meter>) -> PyResult<Self> {
        let mut lists = MeteredVec::with_capacity(meter, parsed.lists().len()).map_err(charged)?;
        for _ in parsed.lists() {
            lists.push(MeteredVec::new(meter)).map_err(charged)?;
        }
        Ok(Self { parsed, lists })
    }

    fn name(&mut self, py: Python<'py>, map: &Map, key: NodeId) -> PyResult<Bound<'py, PyAny>> {
        let list = usize::try_from(map.list()).ok();
        let rows = list
            .and_then(|i| self.parsed.lists().get(i))
            .map_or(0, |nodes| nodes.rows().len());
        let at = usize::try_from(key.0).unwrap_or(usize::MAX);
        let Some(slots) = list.and_then(|i| self.lists.as_mut_slice().get_mut(i)) else {
            return h::string(py, self.parsed.key_name(map, key));
        };
        if at >= rows {
            return h::string(py, self.parsed.key_name(map, key));
        }
        if slots.len() < rows {
            slots.reserve_exact(rows - slots.len()).map_err(charged)?;
            while slots.len() < rows {
                slots.push(None).map_err(charged)?;
            }
        }
        if let Some(Some(name)) = slots.as_slice().get(at) {
            return Ok(name.clone());
        }
        let name = h::string(py, self.parsed.key_name(map, key))?;
        if let Some(slot) = slots.as_mut_slice().get_mut(at) {
            *slot = Some(name.clone());
        }
        Ok(name)
    }
}
