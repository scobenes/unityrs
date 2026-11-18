//! Checked dict reads shared by the discovery and binding tables.
use std::sync::Arc;
use unityrs_core::ObjectRef;
use unityrs_core::env::PPtr;
use unityrs_core::limits::Meter;
use unityrs_core::value::{Parsed, PrimArray, Value};

use crate::AnimError;

pub(crate) struct Dict<'a, 'm> {
    pub parsed: &'a Parsed,
    pub meter: &'m Arc<Meter>,
}

impl<'a> Dict<'a, '_> {
    pub fn field(
        &self,
        value: &'a Value,
        name: &'static str,
    ) -> Result<Option<&'a Value>, AnimError> {
        let map = value.as_dict().ok_or(AnimError::Malformed(name))?;
        self.meter.count_decode(map.len() as u64)?;
        Ok(self.parsed.field(map, name.as_bytes()))
    }

    pub fn root(&self, name: &'static str) -> Result<Option<&'a Value>, AnimError> {
        self.field(self.parsed.value(), name)
    }

    pub fn required(&self, value: &'a Value, name: &'static str) -> Result<&'a Value, AnimError> {
        self.field(value, name)?.ok_or(AnimError::Malformed(name))
    }

    pub fn list(&self, value: &'a Value, name: &'static str) -> Result<&'a [Value], AnimError> {
        let items: &[Value] = match value {
            Value::List(items) => items,
            Value::Tuple(items) => items,
            _ => return Err(AnimError::Malformed(name)),
        };
        self.meter.count_decode(items.len() as u64)?;
        Ok(items)
    }

    pub fn pointer(&self, value: &'a Value) -> Result<PPtr, AnimError> {
        Ok(PPtr {
            file_id: self
                .required(value, "m_FileID")?
                .as_i64()
                .and_then(|id| i32::try_from(id).ok())
                .ok_or(AnimError::Malformed("m_FileID"))?,
            path_id: self
                .required(value, "m_PathID")?
                .as_i64()
                .ok_or(AnimError::Malformed("m_PathID"))?,
        })
    }

    pub fn rows(&self, name: &'static str) -> Result<&'a [Value], AnimError> {
        self.root(name)?
            .map_or(Ok(&[]), |value| self.list(value, name))
    }

    pub fn float(&self, value: &'a Value, name: &'static str) -> Result<f64, AnimError> {
        match self.required(value, name)? {
            Value::F32(bits) => Ok(f64::from(f32::from_bits(*bits))),
            _ => Err(AnimError::Malformed(name)),
        }
    }

    pub fn int(&self, value: &'a Value, name: &'static str) -> Result<i32, AnimError> {
        int(self.required(value, name)?, name)
    }

    pub fn count(&self, value: &'a Value, name: &'static str) -> Result<usize, AnimError> {
        self.required(value, name)?
            .as_u64()
            .and_then(|count| u32::try_from(count).ok())
            .map(|count| count as usize)
            .ok_or(AnimError::Malformed(name))
    }

    pub fn words(
        &self,
        value: &'a Value,
        name: &'static str,
        float: bool,
    ) -> Result<&'a [u32], AnimError> {
        let words = match (float, self.required(value, name)?) {
            (true, Value::Prims(PrimArray::F32(words)))
            | (false, Value::Prims(PrimArray::U32(words))) => words.as_ref(),
            (_, Value::List(words)) if words.is_empty() => &[],
            _ => return Err(AnimError::Malformed(name)),
        };
        self.meter.count_decode(words.len() as u64)?;
        Ok(words)
    }
}

pub(crate) fn int(value: &Value, name: &'static str) -> Result<i32, AnimError> {
    value
        .as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or(AnimError::Malformed(name))
}

pub(crate) fn flag(value: &Value, name: &'static str) -> Result<bool, AnimError> {
    value.as_bool().ok_or(AnimError::Malformed(name))
}

pub(crate) fn version(object: &ObjectRef) -> Option<(u16, u16)> {
    let (major, minor, ..) = object.file().version()?.version.as_tuple().ok()?;
    Some((major, minor))
}

pub(crate) fn same_object(a: &ObjectRef, b: &ObjectRef) -> bool {
    a.path_id() == b.path_id() && Arc::ptr_eq(a.file(), b.file())
}

pub(crate) fn identity_key(object: &ObjectRef) -> (usize, i64, &[usize]) {
    let (input, path) = object.file().occurrence_key();
    (input, object.path_id(), path)
}
