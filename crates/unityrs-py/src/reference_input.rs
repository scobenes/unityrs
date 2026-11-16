// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp:585-681. See THIRD_PARTY_LICENSES.
//! Caller-thread extraction of mutable Python reference types into owned storage.
use crate::errors;
use pyo3::prelude::*;
use std::sync::Arc;
use unityrs_core::{
    RawStr, Value,
    files::SerializedType,
    limits::{Charge, Limits, Meter, MeteredVec},
    typetree::Tree,
    walk::{ReadDetail, ReadError, ReadErrorKind, RefTypes},
};

pub(crate) enum ReferenceInput {
    NoFile,
    Absent,
    Resolver(Arc<Py<PyAny>>),
}

impl ReferenceInput {
    pub(crate) fn resolver(&self) -> Option<&Arc<Py<PyAny>>> {
        match self {
            Self::Resolver(call) => Some(call),
            _ => None,
        }
    }
    pub(crate) fn snapshot(input: &Bound<'_, PyAny>) -> PyResult<Self> {
        match input.get_item(0)?.extract::<u8>()? {
            0 => Ok(Self::NoFile),
            1 => Ok(Self::Absent),
            2 => Ok(Self::Resolver(Arc::new(input.get_item(1)?.unbind()))),
            _ => Err(pyo3::exceptions::PyValueError::new_err(
                "Invalid reference snapshot state",
            )),
        }
    }

    pub(crate) fn view(&self) -> RefTypes<'_> {
        match self {
            Self::NoFile => RefTypes::NoFile,
            Self::Absent => RefTypes::Absent,
            Self::Resolver(_) => RefTypes::List(&[]),
        }
    }
}

pub(crate) struct Call {
    pub(crate) resolver: Arc<Py<PyAny>>,
    pub(crate) names: [RawStr; 3],
    pub(crate) meter: Arc<Meter>,
    pub(crate) limits: Limits,
}

pub(crate) fn invoke(call: &Call) -> PyResult<crate::python::relay::Response> {
    crate::python::relay::attach(|py| {
        let mut staging = Charge::zero(&call.meter);
        staging
            .grow(
                512 + call
                    .names
                    .iter()
                    .map(|s| s.as_bytes().len() as u64 * 4)
                    .sum::<u64>(),
            )
            .map_err(errors::charge)?;
        let [cls, ns, asm] = &call.names;
        let row = call.resolver.bind(py).call1((
            crate::python::string(cls.as_bytes())?,
            crate::python::string(ns.as_bytes())?,
            crate::python::string(asm.as_bytes())?,
        ))?;
        if row.is_none() {
            return Ok(crate::python::relay::Response::ReferenceMissing);
        }
        let rows = row.get_item(3)?;
        let tree = if rows.is_none() {
            None
        } else {
            Some(crate::tree::compile_rows_with(
                &call.meter,
                &call.limits,
                rows.as_unbound(),
            )?)
        };
        Ok(crate::python::relay::Response::Reference(tree))
    })
}

pub(crate) fn resolve(
    resolver: &Arc<Py<PyAny>>,
    meter: &Arc<Meter>,
    limits: Limits,
    names: [&Value; 3],
    at: u64,
    selected: &mut MeteredVec<SerializedType>,
) -> Result<Option<Arc<Tree>>, ReadError> {
    use crate::python::relay::{self, Request, Response};
    let failure = || ReadError {
        kind: ReadErrorKind::RefType,
        at,
        detail: ReadDetail::RefTypeNotFound,
    };
    let [Value::Str(cls), Value::Str(ns), Value::Str(asm)] = names else {
        return Err(failure());
    };
    let result = (|| {
        // Reserve before invoking caller code or retaining the selected tree.
        selected.reserve(1).map_err(errors::charge)?;
        let response = relay::request(Request::Reference(Call {
            resolver: resolver.clone(),
            names: [cls.clone(), ns.clone(), asm.clone()],
            meter: meter.clone(),
            limits,
        }))?;
        let tree = match response {
            Response::Reference(tree) => tree,
            Response::ReferenceMissing => return Ok(Err(failure())),
            _ => {
                return Err(pyo3::exceptions::PyRuntimeError::new_err(
                    "invalid reference relay response",
                ));
            }
        };
        let mut reference = SerializedType::new(0, None);
        reference.class_name = Some(cls.clone());
        reference.namespace = Some(ns.clone());
        reference.assembly = Some(asm.clone());
        reference.nodes = tree.as_ref().map(|t| t.list().clone());
        selected.push(reference).map_err(errors::charge)?;
        Ok(Ok(tree))
    })();
    match result {
        Ok(value) => value,
        Err(error) => {
            relay::fail(error);
            Err(failure())
        }
    }
}
