// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp:585-681. See THIRD_PARTY_LICENSES.
//! Caller-thread extraction of mutable Python reference types into owned storage.
use crate::errors;
use pyo3::{
    ffi,
    prelude::*,
    types::{PyBytes, PyString},
};
use std::sync::Arc;
use unityrs_core::{
    RawStr,
    files::SerializedType,
    limits::{Limits, Meter, MeteredVec},
    walk::RefTypes,
};

pub(crate) enum ReferenceInput {
    NoFile,
    Absent,
    List(MeteredVec<SerializedType>),
}

impl ReferenceInput {
    pub(crate) fn snapshot(
        input: &Bound<'_, PyAny>,
        meter: &Arc<Meter>,
        limits: &Limits,
    ) -> PyResult<Self> {
        match input.get_item(0)?.extract::<u8>()? {
            0 => Ok(Self::NoFile),
            1 => Ok(Self::Absent),
            2 => {
                let mut refs = MeteredVec::new(meter);
                let iter = input.get_item(1)?.try_iter()?;
                loop {
                    meter.count_decode(1).map_err(errors::charge)?;
                    let Some(row) = iter.clone().next() else {
                        break;
                    };
                    let row = row?;
                    refs.reserve(1).map_err(errors::charge)?;
                    let mut reference = SerializedType::new(0, None);
                    reference.class_name = text(&row.get_item(0)?, meter)?;
                    reference.namespace = text(&row.get_item(1)?, meter)?;
                    reference.assembly = text(&row.get_item(2)?, meter)?;
                    let rows = row.get_item(3)?;
                    if !rows.is_none() {
                        let tree =
                            crate::tree::compile_rows_with(meter, limits, rows.as_unbound())?;
                        reference.nodes = Some(tree.list().clone());
                    }
                    refs.push(reference).map_err(errors::charge)?;
                }
                Ok(Self::List(refs))
            }
            _ => Err(pyo3::exceptions::PyValueError::new_err(
                "Invalid reference snapshot state",
            )),
        }
    }

    pub(crate) fn view(&self) -> RefTypes<'_> {
        match self {
            Self::NoFile => RefTypes::NoFile,
            Self::Absent => RefTypes::Absent,
            Self::List(refs) => RefTypes::List(refs.as_slice()),
        }
    }
}

fn text(value: &Bound<'_, PyAny>, meter: &Arc<Meter>) -> PyResult<Option<RawStr>> {
    let Ok(text) = value.cast::<PyString>() else {
        // Non-Unicode metadata cannot match a parsed Unicode descriptor;
        // preserve the accelerated reader's ordinary reference failure.
        return Ok(None);
    };
    // Bypass overridable Python encoding and sizing protocols. The temporary
    // encoded bytes and retained RawStr both reserve before allocating.
    let len = usize::try_from(unsafe { ffi::PyUnicode_GetLength(text.as_ptr()) })?;
    let bound = (len as u64).saturating_mul(4);
    let _encoding = meter
        .charge(256_u64.saturating_add(bound))
        .map_err(errors::charge)?;
    meter.count_decode(bound).map_err(errors::charge)?;
    let bytes = unsafe {
        Bound::<PyAny>::from_owned_ptr_or_err(
            value.py(),
            ffi::PyUnicode_AsEncodedString(
                text.as_ptr(),
                c"utf-8".as_ptr(),
                c"surrogateescape".as_ptr(),
            ),
        )?
    };
    let bytes = bytes.cast::<PyBytes>()?;
    let raw = bytes.as_bytes();
    value.py().detach(|| {
        RawStr::charged(raw, meter)
            .map(Some)
            .map_err(errors::charge)
    })
}
