//! Metered dict-mode metadata reads. Diagnostics only name static field paths.
use crate::{Aabb, MeshError, RawString};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_core::{Map, Parsed, PrimArray, Value};

#[derive(Clone, Copy)]
pub(crate) struct Fields<'a> {
    pub parsed: &'a Parsed,
    pub map: &'a Map,
    pub meter: &'a Arc<Meter>,
}
impl<'a> Fields<'a> {
    pub fn new(
        parsed: &'a Parsed,
        value: &'a Value,
        meter: &'a Arc<Meter>,
    ) -> Result<Self, MeshError> {
        let Value::Dict(map) = value else {
            return Err(MeshError::Field("class"));
        };
        Ok(Self { parsed, map, meter })
    }
    pub fn get(&self, name: &'static str) -> Result<Option<&'a Value>, MeshError> {
        // A lookup compares each raw name at most once; include name bytes.
        let list = self
            .parsed
            .lists()
            .get(usize::try_from(self.map.list()).unwrap_or(usize::MAX))
            .ok_or(MeshError::Field(name))?;
        let entries = self.map.entries();
        // Node-name lengths are cached by RawStr; this first pass only reads lengths.
        self.meter
            .count_decode(u64::try_from(entries.len()).unwrap_or(u64::MAX))?;
        let work = entries.iter().fold(0u64, |n, (key, _)| {
            n.saturating_add(
                u64::try_from(list.name(*key).len())
                    .unwrap_or(u64::MAX)
                    .min(u64::try_from(name.len()).unwrap_or(u64::MAX)),
            )
        });
        self.meter.count_decode(work)?;
        Ok(self.parsed.field(self.map, name.as_bytes()))
    }
    pub fn required(&self, name: &'static str) -> Result<&'a Value, MeshError> {
        self.get(name)?.ok_or(MeshError::Field(name))
    }
    pub fn class(&self, name: &'static str) -> Result<Option<Self>, MeshError> {
        self.get(name)?
            .map(|v| Self::new(self.parsed, v, self.meter))
            .transpose()
    }
    pub fn child(&self, value: &'a Value) -> Result<Self, MeshError> {
        Self::new(self.parsed, value, self.meter)
    }
    pub fn uint(&self, name: &'static str) -> Result<u32, MeshError> {
        uint(self.required(name)?, name)
    }
    pub fn int(&self, name: &'static str) -> Result<i32, MeshError> {
        int(self.required(name)?, name)
    }
    pub fn float(&self, name: &'static str) -> Result<f32, MeshError> {
        float(self.required(name)?, name)
    }
    pub fn boolean(&self, name: &'static str) -> Result<bool, MeshError> {
        match self.required(name)? {
            Value::Bool(v) => Ok(*v),
            _ => Err(MeshError::Field(name)),
        }
    }
    pub fn string(&self, name: &'static str) -> Result<RawString, MeshError> {
        let Value::Str(s) = self.required(name)? else {
            return Err(MeshError::Field(name));
        };
        let mut out = MeteredVec::with_capacity(self.meter, s.len())?;
        work(self.meter, s.len(), 1)?;
        out.extend_from_slice(s.as_bytes())?;
        Ok(RawString(out))
    }
    pub fn vec3(&self) -> Result<[f32; 3], MeshError> {
        Ok([self.float("x")?, self.float("y")?, self.float("z")?])
    }
    pub fn optional_vec3(&self, name: &'static str) -> Result<Option<[f32; 3]>, MeshError> {
        self.class(name)?.map(|f| f.vec3()).transpose()
    }
    pub fn bounds(&self, name: &'static str) -> Result<Option<Aabb>, MeshError> {
        self.class(name)?
            .map(|f| {
                Ok(Aabb {
                    center: f
                        .class("m_Center")?
                        .ok_or(MeshError::Field("m_Center"))?
                        .vec3()?,
                    extent: f
                        .class("m_Extent")?
                        .ok_or(MeshError::Field("m_Extent"))?
                        .vec3()?,
                })
            })
            .transpose()
    }
    pub fn items(&self, name: &'static str) -> Result<&'a [Value], MeshError> {
        match self.get(name)? {
            None => Ok(&[]),
            Some(Value::List(v)) => Ok(v),
            Some(Value::Tuple(v)) => Ok(v),
            Some(Value::Prims(p)) if p.is_empty() => Ok(&[]),
            _ => Err(MeshError::Field(name)),
        }
    }
    pub fn collect<T>(
        &self,
        name: &'static str,
        mut read: impl FnMut(Self) -> Result<T, MeshError>,
    ) -> Result<MeteredVec<T>, MeshError> {
        let items = self.items(name)?;
        let mut out = MeteredVec::with_capacity(self.meter, items.len())?;
        work(self.meter, items.len(), std::mem::size_of::<T>())?;
        for value in items {
            out.push(read(self.child(value)?)?)?;
        }
        Ok(out)
    }
    pub fn numbers<T>(
        &self,
        name: &'static str,
        read: fn(&Value, &'static str) -> Result<T, MeshError>,
    ) -> Result<MeteredVec<T>, MeshError> {
        let value = self.get(name)?;
        let n = match value {
            None => 0,
            Some(Value::Prims(p)) => p.len(),
            Some(Value::List(v)) => v.len(),
            Some(Value::Tuple(v)) => v.len(),
            _ => return Err(MeshError::Field(name)),
        };
        let mut out = MeteredVec::with_capacity(self.meter, n)?;
        work(self.meter, n, std::mem::size_of::<T>())?;
        for i in 0..n {
            let v = if let Some(Value::Prims(p)) = value {
                scalar(p, i).ok_or(MeshError::Field(name))?
            } else {
                let item = match value {
                    Some(Value::List(v)) => v.get(i),
                    Some(Value::Tuple(v)) => v.get(i),
                    _ => None,
                };
                out.push(read(item.ok_or(MeshError::Field(name))?, name)?)?;
                continue;
            };
            out.push(read(&v, name)?)?;
        }
        Ok(out)
    }
}

fn scalar(p: &PrimArray, i: usize) -> Option<Value> {
    macro_rules! unsigned {
        ($v:expr) => {
            $v.get(i).map(|v| Value::UInt(u64::from(*v)))
        };
    }
    macro_rules! signed {
        ($v:expr) => {
            $v.get(i).map(|v| Value::Int(i64::from(*v)))
        };
    }
    match p {
        PrimArray::U8(v) => unsigned!(v),
        PrimArray::U16(v) => unsigned!(v),
        PrimArray::U32(v) => unsigned!(v),
        PrimArray::I8(v) => signed!(v),
        PrimArray::I16(v) => signed!(v),
        PrimArray::I32(v) => signed!(v),
        PrimArray::U64(v) => v.get(i).copied().map(Value::UInt),
        PrimArray::I64(v) => v.get(i).copied().map(Value::Int),
        PrimArray::F32(v) => v.get(i).copied().map(Value::F32),
        PrimArray::F64(v) => v.get(i).copied().map(Value::F64),
        PrimArray::Bool(v) => v.get(i).copied().map(Value::Bool),
    }
}
pub(crate) fn uint(v: &Value, name: &'static str) -> Result<u32, MeshError> {
    match v {
        Value::UInt(v) => u32::try_from(*v).map_err(|_| MeshError::Field(name)),
        Value::Int(v) => u32::try_from(*v).map_err(|_| MeshError::Field(name)),
        _ => Err(MeshError::Field(name)),
    }
}
pub(crate) fn int(v: &Value, name: &'static str) -> Result<i32, MeshError> {
    match v {
        Value::Int(v) => i32::try_from(*v).map_err(|_| MeshError::Field(name)),
        _ => Err(MeshError::Field(name)),
    }
}
pub(crate) fn float(v: &Value, name: &'static str) -> Result<f32, MeshError> {
    match v {
        Value::F32(v) => Ok(f32::from_bits(*v)),
        _ => Err(MeshError::Field(name)),
    }
}
pub(crate) fn work(meter: &Meter, n: usize, width: usize) -> Result<(), MeshError> {
    meter.count_decode(
        u64::try_from(n)
            .unwrap_or(u64::MAX)
            .saturating_mul(u64::try_from(width.max(1)).unwrap_or(u64::MAX)),
    )?;
    Ok(())
}
