//! Checked dict reads shared by the discovery and binding tables.
use std::sync::Arc;
use unityrs_core::ObjectRef;
use unityrs_core::env::PPtr;
use unityrs_core::limits::Meter;
use unityrs_core::value::{Parsed, Value};

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
}

pub(crate) fn version(object: &ObjectRef) -> Option<(u16, u16)> {
    let (major, minor, ..) = object.file().version()?.version.as_tuple().ok()?;
    Some((major, minor))
}

pub(crate) fn same_object(a: &ObjectRef, b: &ObjectRef) -> bool {
    a.path_id() == b.path_id() && Arc::ptr_eq(a.file(), b.file())
}

pub(crate) fn identity_key(object: &ObjectRef) -> (usize, i64) {
    (Arc::as_ptr(object.file()) as usize, object.path_id())
}
