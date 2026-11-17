//! Flat native handoff: recursive values are destroyed on the parser worker.
//! Python allocation and protocol calls run only when the caller renders it.
use super as p;
use crate::{errors, worker_error::Failure};
use pyo3::prelude::*;
use std::{cell::RefCell, sync::Arc};
use unityrs_core::{
    Bytes, RawStr,
    limits::{Charge, Meter, MeteredVec},
};

pub(crate) type Id = usize;
enum Op {
    None,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    String(Id),
    Bytes(Id),
    List,
    Dict,
    Tuple(Id),
    Push(Id, Id),
    Set(Id, Id, Id),
}

pub(crate) struct Plan {
    ops: RefCell<MeteredVec<Op>>,
    data: RefCell<MeteredVec<RawStr>>,
    pub(crate) charge: Charge,
    pub(crate) root: Id,
}

impl Plan {
    pub(crate) fn new(meter: &Arc<Meter>) -> Self {
        Self {
            ops: RefCell::new(MeteredVec::new(meter)),
            data: RefCell::new(MeteredVec::new(meter)),
            charge: Charge::zero(meter),
            root: 0,
        }
    }
    fn add(&self, op: Op) -> crate::worker_error::Result<Id> {
        let id = self.ops.borrow().len();
        self.ops.borrow_mut().push(op).map_err(Failure::charge)?;
        Ok(id)
    }
    pub(crate) fn none(&self) -> crate::worker_error::Result<Id> {
        self.add(Op::None)
    }
    pub(crate) fn boolean(&self, value: bool) -> crate::worker_error::Result<Id> {
        self.add(Op::Bool(value))
    }
    pub(crate) fn int(&self, value: i64) -> crate::worker_error::Result<Id> {
        self.add(Op::Int(value))
    }
    pub(crate) fn uint(&self, value: u64) -> crate::worker_error::Result<Id> {
        self.add(Op::UInt(value))
    }
    pub(crate) fn float(&self, value: f64) -> crate::worker_error::Result<Id> {
        self.add(Op::Float(value))
    }
    pub(crate) fn string(&self, value: &[u8]) -> crate::worker_error::Result<Id> {
        let text = RawStr::charged(value, self.charge.meter()).map_err(Failure::charge)?;
        self.raw_string(&text)
    }
    pub(crate) fn raw_string(&self, value: &RawStr) -> crate::worker_error::Result<Id> {
        // Input slices and computed strings already retain their metered
        // owners. Cloning the handle must not copy or charge the payload again.
        let id = self.data.borrow().len();
        self.data
            .borrow_mut()
            .push(value.clone())
            .map_err(Failure::charge)?;
        self.add(Op::String(id))
    }
    pub(crate) fn bytes(&self, value: &Bytes) -> crate::worker_error::Result<Id> {
        let id = self.data.borrow().len();
        self.data
            .borrow_mut()
            .push(RawStr::from_shared(value.clone()))
            .map_err(Failure::charge)?;
        self.add(Op::Bytes(id))
    }
    pub(crate) fn list(&self) -> crate::worker_error::Result<Id> {
        self.add(Op::List)
    }
    pub(crate) fn dict(&self) -> crate::worker_error::Result<Id> {
        self.add(Op::Dict)
    }
    pub(crate) fn tuple(&self, items: Id) -> crate::worker_error::Result<Id> {
        self.add(Op::Tuple(items))
    }
    pub(crate) fn push(&self, list: Id, item: Id) -> crate::worker_error::Result<()> {
        self.add(Op::Push(list, item)).map(|_| ())
    }
    pub(crate) fn set(&self, dict: Id, key: Id, value: Id) -> crate::worker_error::Result<()> {
        self.add(Op::Set(dict, key, value)).map(|_| ())
    }
    pub(crate) fn handoff(self) -> PyResult<(Py<PyAny>, Charge)> {
        let value = self.render()?;
        // Release the native instructions before metadata/construction adds
        // its staging, while retaining the rendered values' reservation.
        Ok((value, self.charge))
    }
    pub(crate) fn render(&self) -> PyResult<Py<PyAny>> {
        let mut objects = MeteredVec::with_capacity(self.charge.meter(), self.ops.borrow().len())
            .map_err(errors::charge)?;
        let get = |objects: &MeteredVec<Option<Py<PyAny>>>, id: Id| -> PyResult<Py<PyAny>> {
            let value = objects
                .as_slice()
                .get(id)
                .and_then(Option::as_ref)
                .ok_or_else(|| {
                    pyo3::exceptions::PyValueError::new_err("invalid handoff reference")
                })?;
            crate::python::relay::attach(|py| Ok(value.clone_ref(py)))
        };
        let data = self.data.borrow();
        let bytes = |id: Id| -> PyResult<&[u8]> {
            data.as_slice()
                .get(id)
                .map(RawStr::as_bytes)
                .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("invalid handoff buffer"))
        };
        for op in self.ops.borrow().as_slice() {
            let value = match op {
                Op::None => p::none(),
                Op::Bool(value) => p::boolean(*value)?,
                Op::Int(value) => p::int(*value)?,
                Op::UInt(value) => p::uint(*value)?,
                Op::Float(value) => p::float(*value)?,
                Op::String(id) => p::string(bytes(*id)?)?,
                Op::Bytes(id) => p::bytes(bytes(*id)?)?,
                Op::List => p::list()?,
                Op::Dict => p::dict()?,
                Op::Tuple(id) => p::tuple(&get(&objects, *id)?)?,
                Op::Push(list, item) => {
                    p::push(&get(&objects, *list)?, &get(&objects, *item)?)?;
                    objects.push(None).map_err(errors::charge)?;
                    continue;
                }
                Op::Set(dict, key, value) => {
                    p::set(
                        &get(&objects, *dict)?,
                        &get(&objects, *key)?,
                        &get(&objects, *value)?,
                    )?;
                    objects.push(None).map_err(errors::charge)?;
                    continue;
                }
            };
            objects.push(Some(value)).map_err(errors::charge)?;
        }
        get(&objects, self.root)
    }
}
