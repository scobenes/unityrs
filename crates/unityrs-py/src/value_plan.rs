// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeHelper.py. See THIRD_PARTY_LICENSES.
//! Metered flat construction instructions; no Python or recursive ownership.
use crate::{
    python::plan::{Id, Plan},
    worker_error::Failure,
};
use std::sync::Arc;
use unityrs_core::{
    Parsed, PrimArray, Value,
    limits::{Meter, MeteredVec},
};
// A tuple is completed after its children because tuples are immutable.
#[derive(Clone, Copy)]
enum Target {
    Root,
    List(Id),
    Dict(Id, Id),
}
enum Task<'a> {
    Value(&'a Value, Target),
    Tuple(Id, Target),
}

fn assign(
    target: Target,
    item: Id,
    result: &mut Option<Id>,
    plan: &mut Plan,
) -> crate::worker_error::Result<()> {
    match target {
        Target::Root => *result = Some(item),
        Target::List(parent) => plan.push(parent, item)?,
        Target::Dict(parent, key) => plan.set(parent, key, item)?,
    }
    Ok(())
}
pub(crate) fn convert(
    parsed: &Parsed,
    meter: &Arc<Meter>,
    plan: &mut Plan,
    fields: bool,
) -> crate::worker_error::Result<Id> {
    let mut tasks = MeteredVec::new(meter);
    tasks
        .push(Task::Value(parsed.value(), Target::Root))
        .map_err(Failure::charge)?;
    let mut result = None;
    while let Some(task) = tasks.pop() {
        let (v, target) = match task {
            Task::Value(v, target) => (v, target),
            Task::Tuple(items, target) => {
                let tuple = plan.tuple(items)?;
                assign(target, tuple, &mut result, plan)?;
                continue;
            }
        };
        // Conservative CPython staging costs; returned Python objects belong
        // to Python's allocator after handoff, outside the Rust memory budget.
        plan.charge.grow(256).map_err(Failure::charge)?;
        let item = match v {
            Value::Bool(x) => plan.boolean(*x)?,
            Value::Int(x) => plan.int(*x)?,
            Value::UInt(x) => plan.uint(*x)?,
            Value::F32(x) => plan.float(f64::from(f32::from_bits(*x)))?,
            Value::F64(x) => plan.float(f64::from_bits(*x))?,
            Value::Str(x) => {
                plan.charge
                    .grow((x.as_bytes().len() as u64).saturating_mul(4))
                    .map_err(Failure::charge)?;
                meter
                    .count_decode((x.as_bytes().len() as u64).saturating_mul(4))
                    .map_err(Failure::charge)?;
                plan.raw_string(x)?
            }
            Value::Bytes(x) => {
                plan.charge.grow(x.len() as u64).map_err(Failure::charge)?;
                meter
                    .count_decode(x.len() as u64)
                    .map_err(Failure::charge)?;
                plan.bytes(x)?
            }
            Value::Dict(map) => {
                plan.charge
                    .grow((map.len() as u64).saturating_mul(256))
                    .map_err(Failure::charge)?;
                let out = plan.dict()?;
                // Insert in forward order; scheduling reverse traversal alone
                // would put a later sibling before its earlier descendants.
                for (key, _) in map.entries() {
                    let name = parsed.key_name(map, *key);
                    plan.charge
                        .grow((name.len() as u64).saturating_mul(8))
                        .map_err(Failure::charge)?;
                    let key = if fields {
                        plan.uint(u64::from(key.0))?
                    } else {
                        plan.string(name)?
                    };
                    let none = plan.none()?;
                    plan.set(out, key, none)?;
                }
                for (key, v) in map.entries().iter().rev() {
                    let key = if fields {
                        plan.uint(u64::from(key.0))?
                    } else {
                        plan.string(parsed.key_name(map, *key))?
                    };
                    tasks
                        .push(Task::Value(v, Target::Dict(out, key)))
                        .map_err(Failure::charge)?;
                }
                out
            }
            Value::List(xs) => {
                plan.charge
                    .grow((xs.len() as u64).saturating_mul(16))
                    .map_err(Failure::charge)?;
                let out = plan.list()?;
                for v in xs.iter().rev() {
                    tasks
                        .push(Task::Value(v, Target::List(out)))
                        .map_err(Failure::charge)?;
                }
                out
            }
            Value::Tuple(xs) => {
                plan.charge
                    .grow((xs.len() as u64).saturating_mul(32))
                    .map_err(Failure::charge)?;
                let out = plan.list()?;
                tasks
                    .push(Task::Tuple(out, target))
                    .map_err(Failure::charge)?;
                for v in xs.iter().rev() {
                    tasks
                        .push(Task::Value(v, Target::List(out)))
                        .map_err(Failure::charge)?;
                }
                continue;
            }
            Value::Prims(array) => primitives(array, plan)?,
        };
        assign(target, item, &mut result, plan)?;
    }
    result.ok_or_else(|| Failure::Static("ValueError", "empty conversion"))
}

fn primitives(array: &PrimArray, plan: &mut Plan) -> crate::worker_error::Result<Id> {
    plan.charge
        .grow((array.len() as u64).saturating_mul(64))
        .map_err(Failure::charge)?;
    let out = plan.list()?;
    macro_rules! run {
        ($xs:expr, $method:ident, $convert:expr) => {
            for value in $xs {
                let item = plan.$method($convert(*value))?;
                plan.push(out, item)?;
            }
        };
    }
    match array {
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
    }
    Ok(out)
}
