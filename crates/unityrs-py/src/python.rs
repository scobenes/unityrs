//! Checked `CPython` allocations. Unsafe code is confined to this ABI boundary.
//! All pointers are borrowed for the duration of the call with the GIL held;
//! a non-null owned result is transferred exactly once to PyO3.
use pyo3::{ffi, prelude::*};
pub(crate) mod relay;

/// Constructors on an attached thread, for building many values under one
/// attachment; the free functions below attach (or relay) once per value.
pub(crate) mod held {
    use pyo3::{ffi, prelude::*};

    pub(crate) fn bytes<'py>(py: Python<'py>, data: &[u8]) -> PyResult<Bound<'py, PyAny>> {
        let n = isize::try_from(data.len())?;
        // SAFETY: the slice is borrowed for the call; the result is owned.
        unsafe {
            Bound::from_owned_ptr_or_err(
                py,
                ffi::PyBytes_FromStringAndSize(data.as_ptr().cast(), n),
            )
        }
    }

    pub(crate) fn string<'py>(py: Python<'py>, data: &[u8]) -> PyResult<Bound<'py, PyAny>> {
        let n = isize::try_from(data.len())?;
        // SAFETY: as bytes; the error handler name is a static C string.
        unsafe {
            Bound::from_owned_ptr_or_err(
                py,
                ffi::PyUnicode_DecodeUTF8(data.as_ptr().cast(), n, c"surrogateescape".as_ptr()),
            )
        }
    }

    macro_rules! scalar {
        ($name:ident, $ty:ty, $constructor:path) => {
            pub(crate) fn $name(py: Python<'_>, value: $ty) -> PyResult<Bound<'_, PyAny>> {
                // SAFETY: a scalar constructor returns a new reference or null.
                unsafe { Bound::from_owned_ptr_or_err(py, $constructor(value)) }
            }
        };
    }
    scalar!(int, i64, ffi::PyLong_FromLongLong);
    scalar!(uint, u64, ffi::PyLong_FromUnsignedLongLong);
    scalar!(float, f64, ffi::PyFloat_FromDouble);

    pub(crate) fn boolean(py: Python<'_>, value: bool) -> PyResult<Bound<'_, PyAny>> {
        // SAFETY: PyBool_FromLong returns a new reference.
        unsafe { Bound::from_owned_ptr_or_err(py, ffi::PyBool_FromLong(value.into())) }
    }
    pub(crate) fn list(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
        // SAFETY: a new empty list or null.
        unsafe { Bound::from_owned_ptr_or_err(py, ffi::PyList_New(0)) }
    }
    /// A list of `len` slots, each to be filled once by [`put`] before the
    /// list is shared.
    pub(crate) fn list_of(py: Python<'_>, len: usize) -> PyResult<Bound<'_, PyAny>> {
        let n = isize::try_from(len)?;
        // SAFETY: a new list of null slots or null; `put` fills every slot.
        unsafe { Bound::from_owned_ptr_or_err(py, ffi::PyList_New(n)) }
    }
    /// Fill slot `at` of a list from [`list_of`], taking `item`'s reference.
    pub(crate) fn put(list: &Bound<'_, PyAny>, at: usize, item: Bound<'_, PyAny>) -> PyResult<()> {
        let at = isize::try_from(at)?;
        // SAFETY: PyList_SetItem checks the type and index and steals the
        // item's reference, also when it fails.
        if unsafe { ffi::PyList_SetItem(list.as_ptr(), at, item.into_ptr()) } == -1 {
            Err(PyErr::fetch(list.py()))
        } else {
            Ok(())
        }
    }
    pub(crate) fn dict(py: Python<'_>) -> PyResult<Bound<'_, PyAny>> {
        // SAFETY: a new empty dict or null.
        unsafe { Bound::from_owned_ptr_or_err(py, ffi::PyDict_New()) }
    }
    pub(crate) fn push(list: &Bound<'_, PyAny>, item: &Bound<'_, PyAny>) -> PyResult<()> {
        // SAFETY: both are live borrowed references; PyList_Append rejects a
        // non-list with an exception rather than misreading it.
        if unsafe { ffi::PyList_Append(list.as_ptr(), item.as_ptr()) } == -1 {
            Err(PyErr::fetch(list.py()))
        } else {
            Ok(())
        }
    }
    pub(crate) fn set(
        dict: &Bound<'_, PyAny>,
        key: &Bound<'_, PyAny>,
        value: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        // SAFETY: live borrowed references; PyDict_SetItem checks the type.
        if unsafe { ffi::PyDict_SetItem(dict.as_ptr(), key.as_ptr(), value.as_ptr()) } == -1 {
            Err(PyErr::fetch(dict.py()))
        } else {
            Ok(())
        }
    }
    pub(crate) fn tuple<'py>(list: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
        // SAFETY: a live borrowed list; the result is a new reference or null.
        unsafe { Bound::from_owned_ptr_or_err(list.py(), ffi::PyList_AsTuple(list.as_ptr())) }
    }
}

pub(crate) fn bytes(data: &[u8]) -> PyResult<Py<PyAny>> {
    crate::python::relay::attach(|py| held::bytes(py, data).map(Bound::unbind))
}

pub(crate) fn string(data: &[u8]) -> PyResult<Py<PyAny>> {
    crate::python::relay::attach(|py| held::string(py, data).map(Bound::unbind))
}

macro_rules! scalar {
    ($name:ident, $ty:ty) => {
        pub(crate) fn $name(value: $ty) -> PyResult<Py<PyAny>> {
            crate::python::relay::attach(|py| held::$name(py, value).map(Bound::unbind))
        }
    };
}
scalar!(int, i64);
scalar!(uint, u64);

pub(crate) fn boolean(value: bool) -> PyResult<Py<PyAny>> {
    crate::python::relay::attach(|py| held::boolean(py, value).map(Bound::unbind))
}

pub(crate) fn list() -> PyResult<Py<PyAny>> {
    crate::python::relay::attach(|py| held::list(py).map(Bound::unbind))
}
pub(crate) fn dict() -> PyResult<Py<PyAny>> {
    crate::python::relay::attach(|py| held::dict(py).map(Bound::unbind))
}
pub(crate) fn push(list: &Py<PyAny>, item: &Py<PyAny>) -> PyResult<()> {
    crate::python::relay::attach(|py| held::push(list.bind(py), item.bind(py)))
}
pub(crate) fn set(dict: &Py<PyAny>, key: &Py<PyAny>, value: &Py<PyAny>) -> PyResult<()> {
    crate::python::relay::attach(|py| held::set(dict.bind(py), key.bind(py), value.bind(py)))
}
pub(crate) fn tuple(list: &Py<PyAny>) -> PyResult<Py<PyAny>> {
    crate::python::relay::attach(|py| held::tuple(list.bind(py)).map(Bound::unbind))
}
pub(crate) fn field(dict: &Py<PyAny>, key: &str, value: &Py<PyAny>) -> PyResult<()> {
    set(dict, &string(key.as_bytes())?, value)
}

#[allow(
    clippy::redundant_closure_for_method_calls,
    reason = "method item is not lifetime-general enough for attach"
)]
pub(crate) fn none() -> Py<PyAny> {
    crate::python::relay::attach(|py| py.None())
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
    crate::python::relay::attach(|py| unsafe {
        Bound::from_owned_ptr_or_err(
            py,
            ffi::PyLong_FromString(data.as_mut_ptr().add(at).cast(), std::ptr::null_mut(), 10),
        )
        .map(Bound::unbind)
    })
}
