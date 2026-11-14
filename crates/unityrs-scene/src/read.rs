//! Dict-only field reads. Every field scan and array iteration is metered.
use crate::SceneError;
use std::sync::Arc;
use unityrs_core::env::PPtr;
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_core::value::{Parsed, PrimArray, Value};

pub(crate) struct Read<'a> {
    pub p: &'a Parsed,
    pub meter: &'a Arc<Meter>,
}

/// Graph facts retain raw component-field names, including legacy pairs.
pub(crate) fn components(go: &Parsed) -> Result<MeteredVec<PPtr>, SceneError> {
    let r = Read {
        p: go,
        meter: go.charge().meter(),
    };
    let mut out = MeteredVec::new(r.meter);
    let Some(v) = r.field(go.value(), "m_Component")? else {
        return Ok(out);
    };
    let items = list(v)?;
    out.reserve_exact(items.len())?;
    r.meter.count_decode(items.len() as u64)?;
    for item in items {
        let ptr = match item {
            Value::Tuple(pair) => pair.get(1).ok_or(SceneError::Malformed("component pair"))?,
            _ => r.req(item, "component")?,
        };
        out.push(r.ptr(ptr)?)?;
    }
    Ok(out)
}
impl<'p> Read<'p> {
    pub fn field<'a>(
        &self,
        v: &'a Value,
        name: &'static str,
    ) -> Result<Option<&'a Value>, SceneError>
    where
        'p: 'a,
    {
        let map = v.as_dict().ok_or(SceneError::Malformed(name))?;
        self.meter
            .count_decode(u64::try_from(map.len()).unwrap_or(u64::MAX))?;
        Ok(self.p.field(map, name.as_bytes()))
    }
    pub fn req<'a>(&self, v: &'a Value, name: &'static str) -> Result<&'a Value, SceneError>
    where
        'p: 'a,
    {
        self.field(v, name)?.ok_or(SceneError::Malformed(name))
    }
    pub fn root(&self, name: &'static str) -> Result<&Value, SceneError> {
        self.req(self.p.value(), name)
    }
    pub fn ptr(&self, v: &Value) -> Result<PPtr, SceneError> {
        let file_id = self
            .req(v, "m_FileID")?
            .as_i64()
            .and_then(|x| i32::try_from(x).ok())
            .ok_or(SceneError::Malformed("m_FileID"))?;
        let path_id = self
            .req(v, "m_PathID")?
            .as_i64()
            .ok_or(SceneError::Malformed("m_PathID"))?;
        Ok(PPtr { file_id, path_id })
    }
    pub fn pointer(&self, name: &'static str) -> Result<PPtr, SceneError> {
        self.ptr(self.root(name)?)
    }
    pub fn optional_ptr(&self, name: &'static str) -> Result<Option<PPtr>, SceneError> {
        self.field(self.p.value(), name)?
            .map(|v| self.ptr(v))
            .transpose()
    }
    pub fn uint(&self, v: &Value, name: &'static str) -> Result<u64, SceneError> {
        self.req(v, name)?
            .as_u64()
            .ok_or(SceneError::Malformed(name))
    }
    pub fn float(&self, v: &Value, name: &'static str) -> Result<f32, SceneError> {
        f32_value(self.req(v, name)?, name)
    }
    pub fn boolean(&self, name: &'static str, default: bool) -> Result<bool, SceneError> {
        self.field(self.p.value(), name)?.map_or(Ok(default), |v| {
            v.as_bool()
                .or_else(|| v.as_i64().map(|x| x != 0))
                .ok_or(SceneError::Malformed(name))
        })
    }
    pub fn xyz<const N: usize>(&self, v: &Value) -> Result<[f32; N], SceneError> {
        let mut out = [0.; N];
        for (n, x) in ["x", "y", "z", "w"].iter().zip(&mut out) {
            *x = self.float(v, n)?;
        }
        Ok(out)
    }
    pub fn pointers(&self, v: &Value) -> Result<MeteredVec<PPtr>, SceneError> {
        let list = list(v)?;
        let mut out = MeteredVec::with_capacity(self.meter, list.len())?;
        self.meter.count_decode(list.len() as u64)?;
        for item in list {
            out.push(self.ptr(item)?)?;
        }
        Ok(out)
    }
}
pub(crate) fn list(v: &Value) -> Result<&[Value], SceneError> {
    match v {
        Value::List(v) => Ok(v),
        Value::Tuple(v) => Ok(v),
        _ => Err(SceneError::Malformed("array")),
    }
}
#[allow(
    clippy::cast_possible_truncation,
    reason = "Unity scene TRS and weights are f32"
)]
pub(crate) fn f32_value(v: &Value, name: &'static str) -> Result<f32, SceneError> {
    v.as_f64()
        .map(|f| f as f32)
        .ok_or(SceneError::Malformed(name))
}
pub(crate) fn floats(v: &Value, meter: &Arc<Meter>) -> Result<MeteredVec<f32>, SceneError> {
    if let Value::Prims(PrimArray::F32(bits)) = v {
        let mut out = MeteredVec::with_capacity(meter, bits.len())?;
        meter.count_decode(bits.len() as u64)?;
        for b in bits {
            out.push(f32::from_bits(*b))?;
        }
        return Ok(out);
    }
    let v = list(v)?;
    let mut out = MeteredVec::with_capacity(meter, v.len())?;
    meter.count_decode(v.len() as u64)?;
    for x in v {
        out.push(f32_value(x, "float array")?)?;
    }
    Ok(out)
}
pub(crate) fn uints(v: &Value, meter: &Arc<Meter>) -> Result<MeteredVec<u32>, SceneError> {
    let mut out = MeteredVec::new(meter);
    if let Value::Prims(PrimArray::U32(v)) = v {
        out.reserve_exact(v.len())?;
        meter.count_decode(v.len() as u64)?;
        for x in v {
            out.push(*x)?;
        }
    } else {
        let v = list(v)?;
        out.reserve_exact(v.len())?;
        meter.count_decode(v.len() as u64)?;
        for x in v {
            out.push(
                x.as_u64()
                    .and_then(|x| u32::try_from(x).ok())
                    .ok_or(SceneError::Malformed("uint array"))?,
            )?;
        }
    }
    Ok(out)
}
