// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeHelper.py. See THIRD_PARTY_LICENSES.
//! Exact Python types (helpers/TypeTreeHelper.py:123-158,
//! helpers/TypeTreeHelper.py:182-263). An explicit, metered stack also handles
//! deep values on small Python thread stacks. Only `CPython` constructors attach.
use crate::{errors, python as p};
use pyo3::prelude::*;
use std::sync::Arc;
use unityrs_core::{
    Parsed, PrimArray, Value,
    limits::{Charge, Meter, MeteredVec},
};

// A tuple is completed after its children because tuples are immutable.
enum Target {
    Root,
    List(Py<PyAny>),
    Dict(Py<PyAny>, Py<PyAny>),
}
enum Task<'a> {
    Value(&'a Value, Target),
    Tuple(Py<PyAny>, Target),
}

fn assign(target: Target, item: Py<PyAny>, result: &mut Option<Py<PyAny>>) -> PyResult<()> {
    match target {
        Target::Root => *result = Some(item),
        Target::List(parent) => p::push(&parent, &item)?,
        Target::Dict(parent, key) => p::set(&parent, &key, &item)?,
    }
    Ok(())
}
fn cloned(object: &Py<PyAny>) -> Py<PyAny> {
    Python::attach(|py| object.clone_ref(py))
}

pub(crate) fn convert(parsed: &Parsed, meter: &Arc<Meter>) -> PyResult<Py<PyAny>> {
    let mut charge = Charge::zero(meter);
    convert_charged(parsed, meter, &mut charge)
}

pub(crate) fn convert_charged(
    parsed: &Parsed,
    meter: &Arc<Meter>,
    charge: &mut Charge,
) -> PyResult<Py<PyAny>> {
    convert_mode(parsed, meter, charge, false)
}

pub(crate) fn convert_fields(
    parsed: &Parsed,
    meter: &Arc<Meter>,
    charge: &mut Charge,
) -> PyResult<Py<PyAny>> {
    convert_mode(parsed, meter, charge, true)
}

fn convert_mode(
    parsed: &Parsed,
    meter: &Arc<Meter>,
    charge: &mut Charge,
    fields: bool,
) -> PyResult<Py<PyAny>> {
    let mut tasks = MeteredVec::new(meter);
    tasks
        .push(Task::Value(parsed.value(), Target::Root))
        .map_err(errors::charge)?;
    let mut result = None;
    while let Some(task) = tasks.pop() {
        let (v, target) = match task {
            Task::Value(v, target) => (v, target),
            Task::Tuple(items, target) => {
                assign(target, p::tuple(&items)?, &mut result)?;
                continue;
            }
        };
        // Conservative CPython staging costs; returned Python objects belong
        // to Python's allocator after handoff, outside the Rust memory budget.
        charge.grow(256).map_err(errors::charge)?;
        let item = match v {
            Value::Bool(x) => p::boolean(*x)?,
            Value::Int(x) => p::int(*x)?,
            Value::UInt(x) => p::uint(*x)?,
            Value::F32(x) => p::float(f64::from(f32::from_bits(*x)))?,
            Value::F64(x) => p::float(f64::from_bits(*x))?,
            Value::Str(x) => {
                charge
                    .grow((x.as_bytes().len() as u64).saturating_mul(4))
                    .map_err(errors::charge)?;
                meter
                    .count_decode((x.as_bytes().len() as u64).saturating_mul(4))
                    .map_err(errors::charge)?;
                p::string(x.as_bytes())?
            }
            Value::Bytes(x) => {
                charge.grow(x.len() as u64).map_err(errors::charge)?;
                meter.count_decode(x.len() as u64).map_err(errors::charge)?;
                p::bytes(x)?
            }
            Value::Dict(map) => {
                charge
                    .grow((map.len() as u64).saturating_mul(256))
                    .map_err(errors::charge)?;
                let out = p::dict()?;
                // Insert in forward order; scheduling reverse traversal alone
                // would put a later sibling before its earlier descendants.
                for (key, _) in map.entries() {
                    let name = parsed.key_name(map, *key);
                    charge
                        .grow((name.len() as u64).saturating_mul(8))
                        .map_err(errors::charge)?;
                    let key = if fields {
                        p::uint(u64::from(key.0))?
                    } else {
                        p::string(name)?
                    };
                    p::set(&out, &key, &p::none())?;
                }
                for (key, v) in map.entries().iter().rev() {
                    let key = if fields {
                        p::uint(u64::from(key.0))?
                    } else {
                        p::string(parsed.key_name(map, *key))?
                    };
                    tasks
                        .push(Task::Value(v, Target::Dict(cloned(&out), key)))
                        .map_err(errors::charge)?;
                }
                out
            }
            Value::List(xs) => {
                charge
                    .grow((xs.len() as u64).saturating_mul(16))
                    .map_err(errors::charge)?;
                let out = p::list()?;
                for v in xs.iter().rev() {
                    tasks
                        .push(Task::Value(v, Target::List(cloned(&out))))
                        .map_err(errors::charge)?;
                }
                out
            }
            Value::Tuple(xs) => {
                charge
                    .grow((xs.len() as u64).saturating_mul(32))
                    .map_err(errors::charge)?;
                let out = p::list()?;
                tasks
                    .push(Task::Tuple(cloned(&out), target))
                    .map_err(errors::charge)?;
                for v in xs.iter().rev() {
                    tasks
                        .push(Task::Value(v, Target::List(cloned(&out))))
                        .map_err(errors::charge)?;
                }
                continue;
            }
            Value::Prims(array) => primitives(array, charge)?,
        };
        assign(target, item, &mut result)?;
    }
    result.ok_or_else(|| pyo3::exceptions::PyValueError::new_err("empty conversion"))
}

fn primitives(array: &PrimArray, charge: &mut Charge) -> PyResult<Py<PyAny>> {
    charge
        .grow((array.len() as u64).saturating_mul(64))
        .map_err(errors::charge)?;
    let out = p::list()?;
    macro_rules! run {
        ($xs:expr, $f:expr) => {
            for x in $xs {
                p::push(&out, &$f(*x)?)?;
            }
        };
    }
    match array {
        PrimArray::Bool(xs) => run!(xs, p::boolean),
        PrimArray::U8(xs) => run!(xs, |x| p::uint(u64::from(x))),
        PrimArray::I8(xs) => run!(xs, |x| p::int(i64::from(x))),
        PrimArray::U16(xs) => run!(xs, |x| p::uint(u64::from(x))),
        PrimArray::I16(xs) => run!(xs, |x| p::int(i64::from(x))),
        PrimArray::U32(xs) => run!(xs, |x| p::uint(u64::from(x))),
        PrimArray::I32(xs) => run!(xs, |x| p::int(i64::from(x))),
        PrimArray::U64(xs) => run!(xs, p::uint),
        PrimArray::I64(xs) => run!(xs, p::int),
        PrimArray::F32(xs) => run!(xs, |x| p::float(f64::from(f32::from_bits(x)))),
        PrimArray::F64(xs) => run!(xs, |x| p::float(f64::from_bits(x))),
    }
    Ok(out)
}
