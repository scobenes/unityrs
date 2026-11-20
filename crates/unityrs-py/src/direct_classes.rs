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

/// `ObjectReader`'s own dict's entries for `reader`, `__getattribute__`
/// and `__getattr__` (what decides how `obj.reader` is found: the
/// property, and the class's own attribute hooks; with only `object`
/// after it in the MRO, whose own cannot change, these are the ones that
/// can) as `unityrs`'s
/// import made them (`None` for one it lacked), and its `reader`
/// property's getter's code ([`_direct_originals`]).
static ORIGINAL: PyOnceLock<Original> = PyOnceLock::new();

/// [`ORIGINAL`]'s entries, and the getter's code.
type Original = ([Option<Py<PyAny>>; 3], Py<PyAny>);

/// Called once by `unityrs/__init__.py`, as it imports `files`, with
/// `ObjectReader`: those entries and its `reader` getter's code
/// kept as the originals. A later call changes nothing.
#[pyfunction]
pub(crate) fn _direct_originals(object: &Bound<'_, PyAny>) -> PyResult<()> {
    let py = object.py();
    let dict = object.getattr(name!(py, c"__dict__")?)?;
    let keys = [
        name!(py, c"reader")?,
        name!(py, c"__getattribute__")?,
        name!(py, c"__getattr__")?,
    ];
    let mut kept: [Option<Py<PyAny>>; 3] = Default::default();
    for (slot, key) in kept.iter_mut().zip(keys) {
        *slot = dict.get_item(key).ok().map(Bound::unbind);
    }
    let reader = kept[0]
        .as_ref()
        .ok_or_else(|| pyo3::exceptions::PyTypeError::new_err("no reader property"))?;
    let code = reader
        .bind(py)
        .getattr(name!(py, c"fget")?)?
        .getattr(name!(py, c"__code__")?)?;
    let _ = ORIGINAL.set(py, (kept, code.unbind()));
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

/// Whether `ObjectReader`'s attribute lookup is the one `unityrs`'s import
/// made, so `obj.reader` runs the code it ran then: checked before the
/// property is used, on every read. Its MRO is still `(ObjectReader,
/// object)` as then (the same tuple; `object` takes no attributes), its
/// own dict's three entries the very objects kept (or still absent),
/// read from the dict without running any descriptor, the `reader`
/// getter's `__code__` the very code kept, and the class has no `_reader`
/// (no descriptor stands between the getter and the instance's `_reader`).
/// A failure, or nothing kept, answers `false`. Nothing is called.
pub(crate) fn unreplaced(py: Python<'_>, classes: &Classes) -> bool {
    let Some((kept, code)) = ORIGINAL.get(py) else {
        return false;
    };
    let (Ok(reader), Ok(hook), Ok(missing), Ok(shadow), Ok(mro), Ok(fget), Ok(code_name)) = (
        name!(py, c"reader"),
        name!(py, c"__getattribute__"),
        name!(py, c"__getattr__"),
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
    // Absent: a membership test, which raises nothing for a missing key.
    // SAFETY: live objects; 1, 0, or -1 with an exception set.
    let absent = |key: &Bound<'_, pyo3::types::PyString>| match unsafe {
        ffi::PySequence_Contains(classes.object_dict.as_ptr(), key.as_ptr())
    } {
        0 => true,
        -1 => {
            drop(PyErr::take(py));
            false
        }
        _ => false,
    };
    let same = |key, kept: &Option<Py<PyAny>>| match kept {
        None => absent(key),
        Some(kept) => entry(key).is_some_and(|now| now.is(kept)),
    };
    let [kept_reader, kept_hook, kept_missing] = kept;
    classes
        .object
        .bind(py)
        .getattr(mro)
        .is_ok_and(|now| now.is(&classes.object_mro))
        && same(hook, kept_hook)
        && same(missing, kept_missing)
        && same(reader, kept_reader)
        && entry(reader).is_some_and(|now| {
            now.getattr(fget)
                .and_then(|get| get.getattr(code_name))
                .is_ok_and(|now| now.is(code))
        })
        && absent(shadow)
}
