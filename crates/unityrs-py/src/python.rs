//! Checked `CPython` allocations. Unsafe code is confined to this ABI boundary.
//! All pointers are borrowed for the duration of the call with the GIL held;
//! a non-null owned result is transferred exactly once to PyO3.
use pyo3::{ffi, prelude::*};

pub(crate) fn bytes(data: &[u8]) -> PyResult<Py<PyAny>> {
    Python::attach(|py| unsafe {
        let n = isize::try_from(data.len())?;
        Bound::from_owned_ptr_or_err(py, ffi::PyBytes_FromStringAndSize(data.as_ptr().cast(), n))
            .map(Bound::unbind)
    })
}

pub(crate) fn string(data: &[u8]) -> PyResult<Py<PyAny>> {
    Python::attach(|py| unsafe {
        let n = isize::try_from(data.len())?;
        Bound::from_owned_ptr_or_err(
            py,
            ffi::PyUnicode_DecodeUTF8(data.as_ptr().cast(), n, c"surrogateescape".as_ptr()),
        )
        .map(Bound::unbind)
    })
}

macro_rules! scalar {
    ($name:ident, $ty:ty, $constructor:path) => {
        pub(crate) fn $name(value: $ty) -> PyResult<Py<PyAny>> {
            Python::attach(|py| unsafe {
                Bound::from_owned_ptr_or_err(py, $constructor(value)).map(Bound::unbind)
            })
        }
    };
}
scalar!(int, i64, ffi::PyLong_FromLongLong);
scalar!(uint, u64, ffi::PyLong_FromUnsignedLongLong);
scalar!(float, f64, ffi::PyFloat_FromDouble);

pub(crate) fn boolean(value: bool) -> PyResult<Py<PyAny>> {
    Python::attach(|py| unsafe {
        Bound::from_owned_ptr_or_err(py, ffi::PyBool_FromLong(value.into())).map(Bound::unbind)
    })
}

pub(crate) fn list() -> PyResult<Py<PyAny>> {
    Python::attach(|py| unsafe {
        Bound::from_owned_ptr_or_err(py, ffi::PyList_New(0)).map(Bound::unbind)
    })
}
pub(crate) fn dict() -> PyResult<Py<PyAny>> {
    Python::attach(|py| unsafe {
        Bound::from_owned_ptr_or_err(py, ffi::PyDict_New()).map(Bound::unbind)
    })
}
pub(crate) fn push(list: &Py<PyAny>, item: &Py<PyAny>) -> PyResult<()> {
    Python::attach(|py| unsafe {
        if ffi::PyList_Append(list.as_ptr(), item.as_ptr()) == -1 {
            Err(PyErr::fetch(py))
        } else {
            Ok(())
        }
    })
}
pub(crate) fn set(dict: &Py<PyAny>, key: &Py<PyAny>, value: &Py<PyAny>) -> PyResult<()> {
    Python::attach(|py| unsafe {
        if ffi::PyDict_SetItem(dict.as_ptr(), key.as_ptr(), value.as_ptr()) == -1 {
            Err(PyErr::fetch(py))
        } else {
            Ok(())
        }
    })
}
pub(crate) fn tuple(list: &Py<PyAny>) -> PyResult<Py<PyAny>> {
    Python::attach(|py| unsafe {
        Bound::from_owned_ptr_or_err(py, ffi::PyList_AsTuple(list.as_ptr())).map(Bound::unbind)
    })
}
pub(crate) fn field(dict: &Py<PyAny>, key: &str, value: &Py<PyAny>) -> PyResult<()> {
    set(dict, &string(key.as_bytes())?, value)
}

#[allow(
    clippy::redundant_closure_for_method_calls,
    reason = "method item is not lifetime-general enough for attach"
)]
pub(crate) fn none() -> Py<PyAny> {
    Python::attach(|py| py.None())
}

pub(crate) fn wide_int(value: i128) -> PyResult<Py<PyAny>> {
    let mut data = [0_u8; 42];
    let mut at = data.len() - 1;
    let mut magnitude = value.unsigned_abs();
    loop {
        at -= 1;
        if let Some(byte) = data.get_mut(at) {
            *byte = b'0' + u8::try_from(magnitude % 10)?;
        }
        magnitude /= 10;
        if magnitude == 0 {
            break;
        }
    }
    if value < 0 {
        at -= 1;
        if let Some(byte) = data.get_mut(at) {
            *byte = b'-';
        }
    }
    Python::attach(|py| unsafe {
        Bound::from_owned_ptr_or_err(
            py,
            ffi::PyLong_FromString(data.as_mut_ptr().add(at).cast(), std::ptr::null_mut(), 10),
        )
        .map(Bound::unbind)
    })
}
