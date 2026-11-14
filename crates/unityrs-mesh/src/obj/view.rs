// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/MeshRendererExporter.py's property access. See THIRD_PARTY_LICENSES.
//! Nested pair properties use `Parsed::attr`, core's object-mode clean-name
//! view. Each scan counts work before comparing names. Required fields have
//! no defaults; optional renderer fields are handled by `objmode::Obj`.
use crate::MeshError;
use unityrs_core::{Parsed, RawStr, Value, env::PPtr, limits::Meter};

pub(super) fn attr<'a>(
    p: &'a Parsed,
    v: &'a Value,
    name: &'static str,
    meter: &Meter,
) -> Result<&'a Value, MeshError> {
    optional_attr(p, v, name, meter)?.ok_or(MeshError::Field(name))
}
pub(super) fn optional_attr<'a>(
    p: &'a Parsed,
    v: &'a Value,
    name: &'static str,
    meter: &Meter,
) -> Result<Option<&'a Value>, MeshError> {
    let Value::Dict(map) = v else {
        return Err(MeshError::Field(name));
    };
    let children = map
        .entries()
        .first()
        .and_then(|(n, _)| n.0.checked_sub(1))
        .and_then(|n| {
            p.lists()
                .get(map.list() as usize)
                .map(|l| l.children(unityrs_core::typetree::NodeId(n)).len())
        })
        .unwrap_or(0);
    meter.count_decode((map.entries().len().saturating_add(children) as u64).saturating_mul(2))?;
    Ok(p.attr(map, name.as_bytes()))
}
pub(super) fn list(v: &Value) -> Result<&[Value], MeshError> {
    match v {
        Value::List(v) => Ok(v),
        Value::Tuple(v) => Ok(v),
        Value::Prims(v) if v.is_empty() => Ok(&[]),
        _ => Err(MeshError::Field("list")),
    }
}
pub(super) fn string(v: &Value) -> Result<&RawStr, MeshError> {
    match v {
        Value::Str(s) => Ok(s),
        _ => Err(MeshError::Field("string")),
    }
}
pub(super) fn number(v: &Value) -> Result<f64, MeshError> {
    match v {
        Value::F32(b) => Ok(f64::from(f32::from_bits(*b))),
        Value::F64(b) => Ok(f64::from_bits(*b)),
        _ => Err(MeshError::Field("float")),
    }
}
pub(super) fn pointer(p: &Parsed, v: &Value, meter: &Meter) -> Result<PPtr, MeshError> {
    let file_id = attr(p, v, "m_FileID", meter)?
        .as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or(MeshError::Field("m_FileID"))?;
    let path_id = attr(p, v, "m_PathID", meter)?
        .as_i64()
        .ok_or(MeshError::Field("m_PathID"))?;
    Ok(PPtr { file_id, path_id })
}
pub(super) fn key<'a>(p: &'a Parsed, v: &'a Value, meter: &Meter) -> Result<&'a RawStr, MeshError> {
    match v {
        Value::Str(s) => Ok(s),
        _ => string(attr(p, v, "name", meter)?),
    }
}
pub(super) fn pair(v: &Value) -> Result<(&Value, &Value), MeshError> {
    match list(v)? {
        [a, b] => Ok((a, b)),
        _ => Err(MeshError::Field("property pair")),
    }
}
pub(super) fn lookup<'a>(
    p: &'a Parsed,
    items: &'a Value,
    name: &[u8],
    meter: &Meter,
) -> Result<Option<&'a Value>, MeshError> {
    let items = list(items)?;
    meter.count_decode((items.len() as u64).saturating_mul(name.len() as u64 + 1))?;
    for item in items.iter().rev() {
        let (k, v) = pair(item)?;
        if key(p, k, meter)?.as_bytes() == name {
            return Ok(Some(v));
        }
    }
    Ok(None)
}

pub(super) fn min_index(v: &Value, meter: &Meter) -> Result<Option<u64>, MeshError> {
    use unityrs_core::PrimArray;
    if let Value::Prims(p) = v {
        meter.count_decode(p.len() as u64)?;
        return Ok(match p {
            PrimArray::U8(v) => v.iter().copied().map(u64::from).min(),
            PrimArray::U16(v) => v.iter().copied().map(u64::from).min(),
            PrimArray::U32(v) => v.iter().copied().map(u64::from).min(),
            PrimArray::U64(v) => v.iter().copied().min(),
            _ if p.is_empty() => None,
            _ => return Err(MeshError::Field("m_SubsetIndices")),
        });
    }
    let items = list(v)?;
    meter.count_decode(items.len() as u64)?;
    let mut min = None;
    for v in items {
        let n = v.as_u64().ok_or(MeshError::Field("m_SubsetIndices"))?;
        min = Some(min.map_or(n, |m: u64| m.min(n)));
    }
    Ok(min)
}
