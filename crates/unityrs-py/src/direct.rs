// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/ObjectReader.py. See THIRD_PARTY_LICENSES.
//! Dict mode read straight into Python objects (`ObjectReader.read_typetree`
//! with no `nodes`, files/ObjectReader.py:244-262), as UnityPyBoost builds
//! them while it reads: the core's direct read ([`unityrs_core::walk::emit`])
//! into a sink of Python values, with no native value tree and no second
//! pass. It answers only where the result is the full path's: on the
//! object's own compiled tree, the reader the file made and nothing a caller
//! replaced, a memory reader. The read keeps its values being read on the
//! heap ([`unityrs_core::walk::emit`]), so it takes the same stack at any
//! depth and runs on the caller's thread, which holds the GIL throughout,
//! as UnityPyBoost's reader does. Anywhere it
//! gives up, `None` sends the caller to the full path, which then sets the
//! position and reads (or fails) as it always has; what the direct read did
//! leaves no trace but the Python objects it drops.
//!
//! Memory: the values are charged to the file's meter as they are made, at
//! no less than the full path holds at its peak (its value tree and its
//! conversion's staging, [`crate::value_render`]); a charge the meter has
//! no room for gives up. The charge is returned when the call ends.
//!
//! Lists are made at their final length from items already built, so no
//! list with an empty slot is ever visible to a collection; a dict is
//! complete at each step.
use crate::direct_object::{ObjKeys, ObjMode};
use pyo3::{ffi, prelude::*, sync::PyOnceLock, types::PyDict};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use unityrs_core::{
    ObjectRef,
    limits::{Charge, Meter},
    typetree::{NodeId, Tree},
    walk::emit::{self, Run, Scalar, Sink},
};

/// A dict entry's, a list item's or a tuple item's charge: above the value
/// tree's slot and spare room (under 128 bytes) and the conversion's staging
/// (272 to 512 bytes).
const ITEM: u64 = 1024;
/// A primitive run element's: the run's element and the conversion's 64.
const ELEMENT: u64 = 128;
/// A container's own.
const CONTAINER: u64 = 512;
/// The call's own: the conversion's work list, key tables and staging.
const BASE: u64 = 64 << 10;
/// A tree of more rows than this is left to the full path (its names would
/// be kept here).
const MOST_ROWS: usize = 1 << 16;
/// Trees whose names are kept between calls.
const KEPT: usize = 64;

/// A tree's key strings, made once and kept while the tree lives.
struct TreeKeys {
    tree: Weak<Tree>,
    ok: bool,
    names: Vec<Option<(Py<PyAny>, u64)>>,
    /// Object mode's, made at its first read of the tree.
    obj: Option<ObjKeys>,
}

static NAMES: Mutex<Vec<TreeKeys>> = Mutex::new(Vec::new());

fn names_of(tree: &Arc<Tree>) -> TreeKeys {
    let mut kept = NAMES.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(at) = kept
        .iter()
        .position(|n| std::ptr::eq(n.tree.as_ptr(), Arc::as_ptr(tree)))
    {
        return kept.swap_remove(at);
    }
    kept.retain(|n| n.tree.strong_count() > 0);
    drop(kept);
    TreeKeys {
        tree: Arc::downgrade(tree),
        ok: tree.list().rows().len() <= MOST_ROWS && emit::supported(tree),
        names: Vec::new(),
        obj: None,
    }
}

fn keep(names: TreeKeys) {
    let mut kept = NAMES.lock().unwrap_or_else(PoisonError::into_inner);
    if kept.len() >= KEPT {
        kept.remove(0);
    }
    kept.push(names);
}

/// Owns a new reference, or takes the exception of a null one: the full
/// path meets the same failure, and raises it.
pub(crate) fn own(py: Python<'_>, ptr: *mut ffi::PyObject) -> Option<Bound<'_, PyAny>> {
    // SAFETY: `ptr` is a new reference or null with an exception set.
    let out = unsafe { Bound::from_owned_ptr_or_opt(py, ptr) };
    if out.is_none() {
        drop(PyErr::take(py));
    }
    out
}

/// A scalar's Python object.
#[inline]
fn scalar(py: Python<'_>, v: Scalar) -> Option<Bound<'_, PyAny>> {
    // SAFETY: each constructor returns a new reference or null.
    own(py, unsafe {
        match v {
            Scalar::Bool(v) => ffi::PyBool_FromLong(v.into()),
            Scalar::Int(v) => ffi::PyLong_FromLongLong(v),
            Scalar::UInt(v) => ffi::PyLong_FromUnsignedLongLong(v),
            Scalar::Float(v) => ffi::PyFloat_FromDouble(v),
        }
    })
}

/// A list of `items`, each moved in. The items are all made before the
/// list is, and nothing allocates while its slots are filled, so no list
/// with an empty slot is ever reachable.
fn list_of<'py>(
    py: Python<'py>,
    items: impl ExactSizeIterator<Item = Bound<'py, PyAny>>,
) -> Option<Bound<'py, PyAny>> {
    let len = isize::try_from(items.len()).ok()?;
    // SAFETY: a new list of `len` empty slots or null.
    let list = own(py, unsafe { ffi::PyList_New(len) })?;
    for (at, item) in (0..len).zip(items) {
        // SAFETY: `at` is in range; the call steals the item.
        unsafe { ffi::PyList_SetItem(list.as_ptr(), at, item.into_ptr()) };
    }
    Some(list)
}

/// Python values for the core's direct read.
struct PySink<'a, 'py> {
    py: Python<'py>,
    tree: &'a Tree,
    /// Each key node's name and its bytes.
    names: &'a mut Vec<Option<(Py<PyAny>, u64)>>,
    meter: &'a Arc<Meter>,
    charge: Option<Charge>,
    reserved: u64,
    need: u64,
    /// The conversion's decode work: a string's bytes four times, bytes'
    /// once ([`crate::value_render`]); in object mode also `build`'s.
    work: u64,
    obj: Option<ObjMode<'a, 'py>>,
}

impl PySink<'_, '_> {
    /// Object mode's `build` for one more value: its task's reservation
    /// (256 of memory and of work) and `work` and `memory` more, as
    /// `_object.build` reserves them; `false` when the meter has no room.
    #[inline]
    fn built(&mut self, work: u64, memory: u64) -> bool {
        if self.obj.is_none() {
            return true;
        }
        self.work = self.work.saturating_add(256).saturating_add(work);
        self.bump(memory.saturating_add(256))
    }

    /// Charges `n` more; `false` when the meter has no room.
    #[inline]
    fn bump(&mut self, n: u64) -> bool {
        self.need = self.need.saturating_add(n);
        self.need <= self.reserved || self.grow()
    }

    /// Grows the reservation geometrically to cover what is needed, checked
    /// against the meter first so that nothing is refused.
    #[cold]
    fn grow(&mut self) -> bool {
        let more = (self.need - self.reserved).max(self.reserved);
        if self.meter.used().saturating_add(more) > self.meter.limit() {
            return false;
        }
        let grown = match &mut self.charge {
            Some(charge) => charge.grow(more).is_ok(),
            None => self
                .meter
                .charge(more)
                .map(|c| self.charge = Some(c))
                .is_ok(),
        };
        if grown {
            self.reserved += more;
        }
        grown
    }

    /// The key node's name, made once per tree.
    fn name(&mut self, key: NodeId) -> Option<(*mut ffi::PyObject, u64)> {
        let at = key.0 as usize;
        if let Some(Some((name, len))) = self.names.get(at) {
            return Some((name.as_ptr(), *len));
        }
        let bytes = self.tree.list().name(key);
        let len = isize::try_from(bytes.len()).ok()?;
        // SAFETY: the slice is borrowed for the call; the handler is static.
        let made = own(self.py, unsafe {
            ffi::PyUnicode_DecodeUTF8(bytes.as_ptr().cast(), len, c"surrogateescape".as_ptr())
        })?;
        if self.names.len() <= at {
            self.names.resize_with(at + 1, || None);
        }
        let ptr = made.as_ptr();
        let size = bytes.len() as u64;
        *self.names.get_mut(at)? = Some((made.unbind(), size));
        Some((ptr, size))
    }
}

impl<'py> Sink for PySink<'_, 'py> {
    type Obj = Bound<'py, PyAny>;
    #[inline]
    fn scalar(&mut self, v: Scalar) -> Option<Self::Obj> {
        if !self.built(0, 0) {
            return None;
        }
        scalar(self.py, v)
    }
    fn str(&mut self, v: &[u8]) -> Option<Self::Obj> {
        let n = v.len() as u64;
        if !self.bump(n.saturating_mul(8).saturating_add(64)) {
            return None;
        }
        self.work = self.work.saturating_add(n.saturating_mul(4));
        if !self.built(0, 0) {
            return None;
        }
        let len = isize::try_from(v.len()).ok()?;
        // SAFETY: the slice is borrowed for the call; the handler name is static.
        own(self.py, unsafe {
            ffi::PyUnicode_DecodeUTF8(v.as_ptr().cast(), len, c"surrogateescape".as_ptr())
        })
    }
    fn bytes(&mut self, v: &[u8]) -> Option<Self::Obj> {
        let n = v.len() as u64;
        if !self.bump(n.saturating_mul(4).saturating_add(64)) {
            return None;
        }
        self.work = self.work.saturating_add(n);
        if !self.built(0, 0) {
            return None;
        }
        let len = isize::try_from(v.len()).ok()?;
        // SAFETY: the slice is borrowed for the call.
        own(self.py, unsafe {
            ffi::PyBytes_FromStringAndSize(v.as_ptr().cast(), len)
        })
    }
    fn tuple(&mut self, first: Self::Obj, second: Self::Obj) -> Option<Self::Obj> {
        if !self.bump(CONTAINER + 2 * ITEM) {
            return None;
        }
        // Its value task, its items' reservation and its `tuple` task.
        if !self.built(512 + 256, 512 + 256) {
            return None;
        }
        // Made from both items, ready: no tuple is visible half filled.
        match pyo3::types::PyTuple::new(self.py, [first, second]) {
            Ok(tuple) => Some(tuple.into_any()),
            Err(_) => None,
        }
    }
    fn dict(&mut self, node: NodeId, children: usize) -> Option<Self::Obj> {
        if !self.bump(CONTAINER.saturating_add((children as u64).saturating_mul(ITEM))) {
            return None;
        }
        let costs = self.obj.as_ref().map_or(0, |o| o.costs(node));
        let items = (children as u64).saturating_mul(256);
        if !self.built(costs, items) {
            return None;
        }
        // SAFETY: a new dict or null.
        own(self.py, unsafe { ffi::PyDict_New() })
    }
    fn set(&mut self, dict: &Self::Obj, key: NodeId, value: Self::Obj) -> Option<()> {
        let (mut name, len) = self.name(key)?;
        if let Some(obj) = &self.obj {
            name = obj.key(key)?;
        }
        if !self.bump(len.saturating_mul(8)) {
            return None;
        }
        // SAFETY: all three are live (the name is held by `names`); the
        // call takes its own references.
        if unsafe { ffi::PyDict_SetItem(dict.as_ptr(), name, value.as_ptr()) } == 0 {
            Some(())
        } else {
            drop(PyErr::take(self.py));
            None
        }
    }
    fn list(&mut self, items: std::vec::Drain<'_, Self::Obj>) -> Option<Self::Obj> {
        let n = (items.len() as u64).saturating_mul(256);
        if !self.built(n, n) {
            return None;
        }
        list_of(self.py, items)
    }
    fn close(&mut self, node: NodeId, dict: Self::Obj) -> Option<Self::Obj> {
        let Some(obj) = &mut self.obj else {
            return Some(dict);
        };
        let mut work = self.work;
        let mut memory = 0;
        let made = obj.construct(node, &dict, &mut work, &mut memory);
        self.work = work;
        if !self.bump(memory) {
            return None;
        }
        made
    }
    fn run(&mut self, run: Run<'_>) -> Option<Self::Obj> {
        let count = run.len() as u64;
        if !self.bump(CONTAINER.saturating_add(count.saturating_mul(ELEMENT))) {
            return None;
        }
        // The list's reservation, and each element's value task.
        let n = count.saturating_mul(512);
        if !self.built(n, n) {
            return None;
        }
        let py = self.py;
        let len = isize::try_from(run.len()).ok()?;
        // SAFETY: a new list of `len` empty slots or null. Between its
        // creation and its last slot's filling only `bool`, `int` and
        // `float` constructors run: they allocate no object the collector
        // tracks, so no collection, finalizer or other Python code can run
        // and reach the list, and the GIL is held throughout. A failed
        // constructor leaves the list to be freed, which skips empty slots.
        let list = own(py, unsafe { ffi::PyList_New(len) })?;
        let mut at = 0;
        run.each(|v| {
            let item = scalar(py, v)?;
            // SAFETY: `at` is below `len` (the run has exactly `len`
            // elements); the call steals the item.
            unsafe { ffi::PyList_SetItem(list.as_ptr(), at, item.into_ptr()) };
            at += 1;
            Some(())
        })?;
        Some(list)
    }
    fn room(&mut self, items: usize) -> bool {
        self.bump(CONTAINER.saturating_add((items as u64).saturating_mul(ITEM)))
    }
}

/// An attribute in an instance's `__dict__`, read with no code of the
/// caller's run (the `__dict__` of an instance of a class of ours).
fn field<'py>(d: &Bound<'py, PyDict>, name: &Bound<'py, PyAny>) -> Option<Bound<'py, PyAny>> {
    // SAFETY: both live; a borrowed reference or null, with no exception for
    // a `str` key.
    let item = unsafe { ffi::PyDict_GetItem(d.as_ptr(), name.as_ptr()) };
    // SAFETY: a borrowed reference, owned from here.
    unsafe { Bound::from_borrowed_ptr_or_opt(d.py(), item) }
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

static CLASSES: PyOnceLock<(Py<PyAny>, Py<PyAny>)> = PyOnceLock::new();

/// `(ObjectReader, ObjectFileReader)`.
fn classes(py: Python<'_>) -> Option<&(Py<PyAny>, Py<PyAny>)> {
    CLASSES
        .get_or_try_init(py, || {
            let files = py.import("unityrs.files")?;
            let readers = py.import("unityrs._object_reader")?;
            Ok::<_, PyErr>((
                files.getattr("ObjectReader")?.unbind(),
                readers.getattr("ObjectFileReader")?.unbind(),
            ))
        })
        .ok()
}

/// `reader.read_typetree()`'s dict, or with `wrap` its object
/// ([`crate::direct_object`]); `None` for the full path (module docs). The
/// caller holds the environment's lock.
pub(crate) fn read(
    py: Python<'_>,
    object: &ObjectRef,
    reader: &Bound<'_, PyAny>,
    check_read: bool,
    wrap: bool,
) -> PyResult<Option<Py<PyAny>>> {
    let Some(found) = ready(py, object, reader) else {
        return Ok(None);
    };
    let (tree, position, assets) = found;
    let mut names = names_of(&tree);
    let out = if !names.ok {
        None
    } else if wrap {
        if names.obj.is_none() {
            names.obj = Some(ObjKeys::new(py, &tree)?);
        }
        objects(py, object, &tree, &mut names, check_read, assets)
    } else {
        direct(py, object, &tree, &mut names.names, check_read, None)
    };
    keep(names);
    let Some(value) = out else {
        return Ok(None);
    };
    // `reset()` and `Position = byte_start + byte_size`: the memory
    // reader's setter takes any position (the full path's checks, passed).
    let end = object.info().byte_start + i128::from(object.info().byte_size);
    let end = match i64::try_from(end) {
        Ok(small) => small.into_pyobject(py)?.into_any(),
        Err(_) => end.into_pyobject(py)?.into_any(),
    };
    position.set_item(pyo3::intern!(py, "_position"), end)?;
    if wrap {
        // `result.set_object_reader(self)`, the full path's last step.
        value.call_method1(pyo3::intern!(py, "set_object_reader"), (reader,))?;
    }
    Ok(Some(value.unbind()))
}

/// Object mode: where the tree's keys allow it, and no ref type of the file
/// is the tree's own list (whose references the full path would index).
fn objects<'py>(
    py: Python<'py>,
    object: &ObjectRef,
    tree: &Arc<Tree>,
    names: &mut TreeKeys,
    check_read: bool,
    assets: Bound<'py, PyAny>,
) -> Option<Bound<'py, PyAny>> {
    let refs = object.file().ref_types().unwrap_or_default();
    if refs.iter().any(|r| {
        r.nodes
            .as_ref()
            .is_some_and(|n| std::ptr::from_ref(&**n) == std::ptr::from_ref(&**tree.list()))
    }) {
        return None;
    }
    let TreeKeys { names, obj, .. } = names;
    let keys = obj.as_mut().filter(|k| k.ok)?;
    let mode = ObjMode::new(py, keys, assets)?;
    let fixed = mode.fixed(refs.len());
    direct(py, object, tree, names, check_read, Some((mode, fixed)))
}

fn direct<'py>(
    py: Python<'py>,
    object: &ObjectRef,
    tree: &Arc<Tree>,
    names: &mut Vec<Option<(Py<PyAny>, u64)>>,
    check_read: bool,
    obj: Option<(ObjMode<'_, 'py>, u64)>,
) -> Option<Bound<'py, PyAny>> {
    let meter = object.file().meter();
    let rows = tree.list().rows().len() as u64;
    let (obj, fixed) = obj.map_or((None, 0), |(mode, fixed)| (Some(mode), fixed));
    // Object mode holds what the full path stages beside the value.
    let staged = obj.as_ref().map_or(0, |mode| mode.keys.staged);
    let mut sink = PySink {
        py,
        tree,
        names,
        meter,
        charge: None,
        reserved: 0,
        need: 0,
        work: 0,
        obj,
    };
    let held = BASE
        .saturating_add(rows.saturating_mul(16))
        .saturating_add(staged);
    let first = held.saturating_add(u64::from(object.info().byte_size).saturating_mul(64));
    if !sink.bump(first) {
        return None;
    }
    sink.need = held;
    let (value, _) = object.emit_typetree(tree, check_read, &mut sink, |s| {
        s.work.saturating_add(fixed)
    })?;
    Some(value)
}

/// The object's tree and its reader's `__dict__` where every check of the
/// full path holds: `uses_original`, a `byte_size`, no `nodes` and no tree
/// of the caller's (files/ObjectReader.py:244-262 as `read_typetree` takes
/// them), on a memory reader.
fn ready<'py>(
    py: Python<'py>,
    object: &ObjectRef,
    obj: &Bound<'py, PyAny>,
) -> Option<(Arc<Tree>, Bound<'py, PyDict>, Bound<'py, PyAny>)> {
    let file = object.file();
    let info = object.info();
    if info.byte_size == 0 || file.reader_kind() != unityrs_core::stream::ReaderKind::Memory {
        return None;
    }
    let tree = object.compiled_tree()?.clone();
    let (object_class, reader_class) = classes(py)?;
    if !obj.get_type().is(object_class) {
        return None;
    }
    let me = instance_dict(obj)?;
    let assets = field(&me, pyo3::intern!(py, "assets_file").as_any())?;
    let ad = instance_dict(&assets)?;
    let key = pyo3::intern!(py, "_reader");
    let reader = if let Some(reader) = field(&me, key.as_any()) {
        reader
    } else {
        // The `reader` property's first use, which the full path makes
        // first too (`uses_original`): the file's reader, once it has one.
        let shared = field(&ad, key.as_any())?;
        if !shared.get_type().is(reader_class) {
            return None;
        }
        me.set_item(key, &shared).ok()?;
        shared
    };
    if !reader.get_type().is(reader_class) {
        return None;
    }
    let rd = instance_dict(&reader)?;
    let has = |d: &Bound<'py, PyDict>, name: &Bound<'py, pyo3::types::PyString>| {
        field(d, name.as_any()).is_some()
    };
    if has(&rd, pyo3::intern!(py, "read"))
        || has(&rd, pyo3::intern!(py, "read_bytes"))
        || has(&ad, pyo3::intern!(py, "_ref_types"))
        || has(&me, pyo3::intern!(py, "_node"))
    {
        return None;
    }
    let native = field(&ad, pyo3::intern!(py, "_native").as_any())?;
    if !field(&rd, pyo3::intern!(py, "_native").as_any())?.is(&native) {
        return None;
    }
    let endian = field(&rd, pyo3::intern!(py, "_endian").as_any())?;
    let want = if file.header().endian == unityrs_core::stream::Endian::Little {
        "<"
    } else {
        ">"
    };
    let endian = endian.cast_exact::<pyo3::types::PyString>().ok()?;
    if endian.to_str().ok()? != want {
        return None;
    }
    let size = exact_int(&field(&me, pyo3::intern!(py, "byte_size").as_any())?)?;
    let start = exact_int(&field(&me, pyo3::intern!(py, "byte_start").as_any())?)?;
    if size != i128::from(info.byte_size) || start != info.byte_start {
        return None;
    }
    let embedded = field(&me, pyo3::intern!(py, "_has_embedded_nodes").as_any())?;
    if !embedded.is(pyo3::types::PyBool::new(py, true).as_any()) {
        return None;
    }
    let type_id = field(&me, pyo3::intern!(py, "type_id").as_any())?;
    exact_int(&type_id)?;
    let shared = field(&ad, pyo3::intern!(py, "_type_nodes").as_any())?
        .cast_into_exact::<PyDict>()
        .ok()?;
    let head = field(&ad, pyo3::intern!(py, "_head_nodes").as_any())?
        .cast_into_exact::<PyDict>()
        .ok()?;
    if !head.is_empty() || shared.contains(&type_id).ok()? {
        return None;
    }
    Some((tree, rd, assets))
}
