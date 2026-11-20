//! The classes a direct read ([`crate::direct`]) takes exact instances of,
//! and the file reader it may copy to an object as `ObjectReader.reader`
//! would.
use crate::direct::{field, instance_dict};
use crate::direct_keys::name;
use pyo3::{prelude::*, sync::PyOnceLock, types::PyDict};

/// The classes whose exact instances a direct read takes, and the
/// `reader` properties of the first and last as the module made them.
pub(crate) struct Classes {
    pub(crate) object: Py<PyAny>,
    pub(crate) reader: Py<PyAny>,
    file: Py<PyAny>,
    object_reader: Py<PyAny>,
    file_reader: Py<PyAny>,
}

static CLASSES: PyOnceLock<Classes> = PyOnceLock::new();

/// `ObjectReader`, `ObjectFileReader` and `SerializedFile`.
pub(crate) fn classes(py: Python<'_>) -> Option<&Classes> {
    use crate::direct_keys::import_attr;
    CLASSES
        .get_or_try_init(py, || {
            let files = name!(py, c"unityrs.files")?;
            let object = import_attr(py, files, name!(py, c"ObjectReader")?)?;
            let file = import_attr(py, files, name!(py, c"SerializedFile")?)?;
            let property = name!(py, c"reader")?;
            Ok::<_, PyErr>(Classes {
                object_reader: made(py, &object, property, "ObjectReader.reader")?,
                file_reader: made(py, &file, property, "SerializedFile.reader")?,
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

/// `class`'s `reader` where it is still the property `unityrs.files`
/// defines (`qualname`, a `property` whose getter that module made);
/// otherwise `None`, which no class's `reader` is taken for, so a
/// replacement made before the first direct read runs too.
fn made(
    py: Python<'_>,
    class: &Bound<'_, PyAny>,
    property: &Bound<'_, pyo3::types::PyString>,
    qualname: &str,
) -> PyResult<Py<PyAny>> {
    let now = class.getattr(property)?;
    let property_type = py.import("builtins")?.getattr("property")?;
    let original = now.get_type().is(&property_type) && {
        let getter = now.getattr("fget")?;
        getter.getattr("__module__")?.extract::<String>()? == "unityrs.files"
            && getter.getattr("__qualname__")?.extract::<String>()? == qualname
    };
    Ok(if original { now.unbind() } else { py.None() })
}

/// The file's reader, where `ObjectReader.reader`'s first use on `me` (an
/// object's `__dict__`) would only copy it to the object's `_reader`, as
/// [`ready`] does: the object's file an exact `SerializedFile`, whose
/// `reader` property returns the `ObjectFileReader` it holds, and neither
/// class's `reader` replaced since the module made it (a replacement runs
/// as the full path would run it).
pub(crate) fn file_reader<'py>(
    py: Python<'py>,
    me: &Bound<'py, PyDict>,
    classes: &Classes,
) -> Option<Bound<'py, PyAny>> {
    let assets = field(me, name!(py, c"assets_file").ok()?.as_any())?;
    if !assets.get_type().is(&classes.file) {
        return None;
    }
    let property = name!(py, c"reader").ok()?;
    let original = |class: &Py<PyAny>, made: &Py<PyAny>| {
        class
            .bind(py)
            .getattr(property)
            .is_ok_and(|now| now.is(made))
    };
    if !original(&classes.object, &classes.object_reader)
        || !original(&classes.file, &classes.file_reader)
    {
        return None;
    }
    let shared = field(
        &instance_dict(&assets)?,
        name!(py, c"_reader").ok()?.as_any(),
    )?;
    shared.get_type().is(&classes.reader).then_some(shared)
}
