//! The classes a direct read ([`crate::direct`]) takes exact instances of,
//! and the file reader it may copy to an object as `ObjectReader.reader`
//! would, while that property is unreplaced.
use crate::direct::{field, instance_dict, own};
use crate::direct_keys::name;
use pyo3::{ffi, prelude::*, sync::PyOnceLock, types::PyDict};

/// The classes whose exact instances a direct read takes, and the
/// `__dict__` of the first and last (live views of their dicts).
pub(crate) struct Classes {
    pub(crate) object: Py<PyAny>,
    pub(crate) reader: Py<PyAny>,
    file: Py<PyAny>,
    object_dict: Py<PyAny>,
    file_dict: Py<PyAny>,
}

static CLASSES: PyOnceLock<Classes> = PyOnceLock::new();

/// `ObjectReader.reader` and `SerializedFile.reader` as `unityrs`'s import
/// made them ([`_direct_originals`]).
static ORIGINALS: PyOnceLock<(Py<PyAny>, Py<PyAny>)> = PyOnceLock::new();

/// Called once by `unityrs/__init__.py`, as it imports `files`: the two
/// `reader` properties, kept as the originals. A later call changes
/// nothing.
#[pyfunction]
pub(crate) fn _direct_originals(object_reader: Bound<'_, PyAny>, file_reader: Bound<'_, PyAny>) {
    let py = object_reader.py();
    let _ = ORIGINALS.set(py, (object_reader.unbind(), file_reader.unbind()));
}

/// `ObjectReader`, `ObjectFileReader` and `SerializedFile`. Each class's
/// `__dict__` is read where its metaclass is `type` itself, so the view is
/// the type's own dict, which no class can swap for another.
pub(crate) fn classes(py: Python<'_>) -> Option<&Classes> {
    use crate::direct_keys::import_attr;
    CLASSES
        .get_or_try_init(py, || {
            let files = name!(py, c"unityrs.files")?;
            let object = import_attr(py, files, name!(py, c"ObjectReader")?)?;
            let file = import_attr(py, files, name!(py, c"SerializedFile")?)?;
            let dict = |class: &Bound<'_, PyAny>| -> PyResult<Py<PyAny>> {
                let plain = class.get_type().is(py.get_type::<pyo3::types::PyType>());
                Ok(if plain {
                    class.getattr(name!(py, c"__dict__")?)?.unbind()
                } else {
                    py.None()
                })
            };
            Ok::<_, PyErr>(Classes {
                object_dict: dict(&object)?,
                file_dict: dict(&file)?,
                object: object.unbind(),
                reader: import_attr(
                    py,
                    name!(py, c"unityrs._object_reader")?,
                    name!(py, c"ObjectFileReader")?,
                )?
                .unbind(),
                file: file.unbind(),
            })
        })
        .ok()
}

/// Whether `ObjectReader`'s `reader` (and with `file`, `SerializedFile`'s)
/// is still the property `unityrs`'s import made: the class's own dict's
/// entry, which the attribute lookup finds first in its MRO, the very
/// object kept then. A missing entry, a class whose dict was not read, or
/// no originals kept, answers `false`, as does any error.
pub(crate) fn unreplaced(py: Python<'_>, classes: &Classes, file: bool) -> bool {
    let Some((object_reader, file_reader)) = ORIGINALS.get(py) else {
        return false;
    };
    let Ok(key) = name!(py, c"reader") else {
        return false;
    };
    let entry = |dict: &Py<PyAny>, made: &Py<PyAny>| {
        // SAFETY: live objects; a new reference, or null with an exception
        // set, which `own` takes.
        own(py, unsafe {
            ffi::PyObject_GetItem(dict.as_ptr(), key.as_ptr())
        })
        .is_some_and(|now| now.is(made))
    };
    entry(&classes.object_dict, object_reader) && (!file || entry(&classes.file_dict, file_reader))
}

/// The file's reader, where `ObjectReader.reader`'s first use on `me` (an
/// object's `__dict__`) would only copy it to the object's `_reader`: the
/// object's file an exact `SerializedFile`, whose `reader` property
/// returns the `ObjectFileReader` it holds, both properties unreplaced
/// ([`unreplaced`]).
pub(crate) fn file_reader<'py>(
    py: Python<'py>,
    me: &Bound<'py, PyDict>,
    classes: &Classes,
) -> Option<Bound<'py, PyAny>> {
    let assets = field(me, name!(py, c"assets_file").ok()?.as_any())?;
    if !assets.get_type().is(&classes.file) || !unreplaced(py, classes, true) {
        return None;
    }
    let shared = field(
        &instance_dict(&assets)?,
        name!(py, c"_reader").ok()?.as_any(),
    )?;
    shared.get_type().is(&classes.reader).then_some(shared)
}
