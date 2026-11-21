// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/ObjectReader.py. See THIRD_PARTY_LICENSES.
//! Whether every check of the full path's read holds for a direct read
//! ([`crate::direct`]): the object's, its file's and its reader's
//! attributes as the full path finds them.
use crate::direct::own;
use crate::direct_classes::classes;
use crate::direct_keys::name;
use pyo3::{ffi, prelude::*, types::PyDict};
use unityrs_core::object::ObjectRef;

/// An attribute in an instance's `__dict__`, read with no code of the
/// caller's run (the `__dict__` of an instance of a class of ours, a `str`
/// key), as a strong reference taken in the lookup.
fn field<'py>(d: &Bound<'py, PyDict>, name: &Bound<'py, PyAny>) -> Option<Bound<'py, PyAny>> {
    d.get_item(name).ok().flatten()
}

/// An instance's `__dict__`, as `object.__dict__` gives it (the attribute of
/// every class here, none of which defines its own).
fn instance_dict<'py>(obj: &Bound<'py, PyAny>) -> Option<Bound<'py, PyDict>> {
    // SAFETY: a live object; a new reference or null with an exception set.
    own(obj.py(), unsafe {
        ffi::PyObject_GenericGetDict(obj.as_ptr(), std::ptr::null_mut())
    })?
    .cast_into_exact::<PyDict>()
    .ok()
}

fn exact_int(v: &Bound<'_, PyAny>) -> Option<i128> {
    // SAFETY: a type check of a live object.
    if unsafe { ffi::PyLong_CheckExact(v.as_ptr()) } == 0 {
        return None;
    }
    match v.extract::<i64>() {
        Ok(small) => Some(i128::from(small)),
        Err(_) => v.extract().ok(),
    }
}

/// The object's tree and its reader's `__dict__` where every check of the
/// full path holds: `uses_original`, a `byte_size`, no `nodes` and no tree
/// of the caller's (files/ObjectReader.py:244-262 as `read_typetree` takes
/// them), on a memory reader.
pub(crate) fn ready<'py>(
    py: Python<'py>,
    object: &ObjectRef,
    obj: &Bound<'py, PyAny>,
    reader: &Bound<'py, PyAny>,
) -> Option<(Bound<'py, PyDict>, Bound<'py, PyAny>)> {
    let file = object.file();
    let info = object.info();
    if info.byte_size == 0 || file.reader_kind() != unityrs_core::stream::ReaderKind::Memory {
        return None;
    }
    let cls = classes(py)?;
    if !obj.get_type().is(&cls.object) {
        return None;
    }
    let me = instance_dict(obj)?;
    let assets = field(&me, name!(py, c"assets_file").ok()?.as_any())?;
    let ad = instance_dict(&assets)?;
    if !reader.get_type().is(&cls.reader) {
        return None;
    }
    let rd = instance_dict(reader)?;
    let has = |d: &Bound<'py, PyDict>, name: &Bound<'py, pyo3::types::PyString>| {
        field(d, name.as_any()).is_some()
    };
    if has(&rd, name!(py, c"read").ok()?)
        || has(&rd, name!(py, c"read_bytes").ok()?)
        || has(&ad, name!(py, c"_ref_types").ok()?)
        || has(&me, name!(py, c"_node").ok()?)
    {
        return None;
    }
    let native = field(&ad, name!(py, c"_native").ok()?.as_any())?;
    if !field(&rd, name!(py, c"_native").ok()?.as_any())?.is(&native) {
        return None;
    }
    let endian = field(&rd, name!(py, c"_endian").ok()?.as_any())?;
    let want = if file.header().endian == unityrs_core::stream::Endian::Little {
        "<"
    } else {
        ">"
    };
    let endian = endian.cast_exact::<pyo3::types::PyString>().ok()?;
    if endian.to_str().ok()? != want {
        return None;
    }
    let size = exact_int(&field(&me, name!(py, c"byte_size").ok()?.as_any())?)?;
    let start = exact_int(&field(&me, name!(py, c"byte_start").ok()?.as_any())?)?;
    if size != i128::from(info.byte_size) || start != info.byte_start {
        return None;
    }
    let embedded = field(&me, name!(py, c"_has_embedded_nodes").ok()?.as_any())?;
    if !embedded.is(pyo3::types::PyBool::new(py, true).as_any()) {
        return None;
    }
    let type_id = field(&me, name!(py, c"type_id").ok()?.as_any())?;
    exact_int(&type_id)?;
    let shared = field(&ad, name!(py, c"_type_nodes").ok()?.as_any())?
        .cast_into_exact::<PyDict>()
        .ok()?;
    let head = field(&ad, name!(py, c"_head_nodes").ok()?.as_any())?
        .cast_into_exact::<PyDict>()
        .ok()?;
    if !head.is_empty() || shared.contains(&type_id).ok()? {
        return None;
    }
    Some((rd, assets))
}
