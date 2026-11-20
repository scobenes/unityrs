//! The classes a direct read ([`crate::direct`]) takes exact instances of,
//! and whether `ObjectReader.reader` is still the property `unityrs`'s
//! import made.
use crate::direct::own;
use crate::direct_keys::name;
use pyo3::{ffi, prelude::*, sync::PyOnceLock};

/// The classes whose exact instances a direct read takes, and of
/// `ObjectReader` its `__dict__` (a live view of its own dict) and MRO.
pub(crate) struct Classes {
    pub(crate) object: Py<PyAny>,
    pub(crate) reader: Py<PyAny>,
    object_dict: Py<PyAny>,
    object_mro: Py<PyAny>,
}

static CLASSES: PyOnceLock<Classes> = PyOnceLock::new();

/// `ObjectReader.reader` as `unityrs`'s import made it, and its getter's
/// code ([`_direct_originals`]).
static ORIGINAL: PyOnceLock<(Py<PyAny>, Py<PyAny>)> = PyOnceLock::new();

/// Called once by `unityrs/__init__.py`, as it imports `files`, with
/// `ObjectReader.reader`: kept, with its getter's code, as the original.
/// A later call changes nothing.
#[pyfunction]
pub(crate) fn _direct_originals(reader: Bound<'_, PyAny>) -> PyResult<()> {
    let py = reader.py();
    let code = reader.getattr("fget")?.getattr("__code__")?;
    let _ = ORIGINAL.set(py, (reader.unbind(), code.unbind()));
    Ok(())
}

/// `ObjectReader` and `ObjectFileReader`; `None` where `ObjectReader`'s
/// metaclass is not `type` itself (whose `__dict__` and `__mro__` are the
/// type's own, which no class can intercept).
pub(crate) fn classes(py: Python<'_>) -> Option<&Classes> {
    use crate::direct_keys::import_attr;
    CLASSES
        .get_or_try_init(py, || {
            let object = import_attr(
                py,
                name!(py, c"unityrs.files")?,
                name!(py, c"ObjectReader")?,
            )?;
            if !object.get_type().is(py.get_type::<pyo3::types::PyType>()) {
                return Err(pyo3::exceptions::PyTypeError::new_err("not a plain class"));
            }
            Ok::<_, PyErr>(Classes {
                object_dict: object.getattr(name!(py, c"__dict__")?)?.unbind(),
                object_mro: object.getattr(name!(py, c"__mro__")?)?.unbind(),
                object: object.unbind(),
                reader: import_attr(
                    py,
                    name!(py, c"unityrs._object_reader")?,
                    name!(py, c"ObjectFileReader")?,
                )?
                .unbind(),
            })
        })
        .ok()
}

/// Whether `ObjectReader.reader` is the property `unityrs`'s import made,
/// running the code it ran then: checked before the property is used, on
/// every read. Its MRO is still `(ObjectReader, object)` as then (the same
/// tuple; `object` takes no attributes), its own dict's `reader` entry the
/// very property kept, that property's getter's `__code__` the very code
/// kept, and no class in it has a `_reader` (no descriptor stands between
/// the getter and the instance's `_reader`). A failure, or nothing kept,
/// answers `false`. Neither the entry nor the getter is called.
pub(crate) fn unreplaced(py: Python<'_>, classes: &Classes) -> bool {
    let Some((property, code)) = ORIGINAL.get(py) else {
        return false;
    };
    let (Ok(reader), Ok(shadow), Ok(mro), Ok(fget), Ok(code_name)) = (
        name!(py, c"reader"),
        name!(py, c"_reader"),
        name!(py, c"__mro__"),
        name!(py, c"fget"),
        name!(py, c"__code__"),
    ) else {
        return false;
    };
    let entry = |key: &Bound<'_, pyo3::types::PyString>| {
        // SAFETY: live objects; a new reference, or null with an exception
        // set, which `own` takes.
        own(py, unsafe {
            ffi::PyObject_GetItem(classes.object_dict.as_ptr(), key.as_ptr())
        })
    };
    let object = classes.object.bind(py);
    object
        .getattr(mro)
        .is_ok_and(|now| now.is(&classes.object_mro))
        && entry(reader).is_some_and(|now| {
            now.is(property)
                && now
                    .getattr(fget)
                    .and_then(|get| get.getattr(code_name))
                    .is_ok_and(|now| now.is(code))
        })
        && entry(shadow).is_none()
}
