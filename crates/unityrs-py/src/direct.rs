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
//! as UnityPyBoost's reader does: only for a read of fewer values than the
//! full path's conversion builds between releases of the GIL (`CHUNK`); a
//! larger one is the full path's, which reads detached. Anywhere it gives
//! up, `None` sends the caller to the full path, which then sets the
//! position and reads (or fails) as it always has; what the direct read did
//! leaves no trace but the Python objects it drops and the work it counts.
//!
//! Memory: the values are charged to the file's meter as they are made, at
//! no less than the full path holds at its peak (its value tree and its
//! conversion's staging, [`crate::value_render`]); a charge the meter has
//! no room for gives up. The charge is returned when the call ends.
//!
//! Work: the read stops as soon as its work passes what the open's
//! allowance still holds, and counts its work only once accepted: work a
//! read that gives up did, the full path does again and counts. An object
//! whose read gave up on its values' count (`CHUNK`) is not tried again.
//!
//! Lists and tuples are made at their final length from items already
//! built, and a primitive run's list is filled out of the collector's
//! sight, so none with an empty slot is ever visible; a dict is complete
//! at each step. Every Python object is made by a call checked for
//! failure: a failed allocation gives up, and the full path meets it and
//! raises; once the read is accepted, it raises.
use crate::direct_classes::classes;
use crate::direct_keys::{TreeKeys, name};
use crate::direct_object::ObjMode;
use pyo3::{ffi, prelude::*, types::PyDict};
use std::sync::Arc;
use unityrs_core::{
    ObjectRef,
    limits::{Charge, Meter},
    typetree::{NodeId, Tree},
    walk::emit::{Run, Scalar, Sink},
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

pub(crate) use crate::direct_list::list_of;

/// A `pair`'s tuple of two items made, a failure raised.
pub(crate) fn pair_raised<'py>(
    py: Python<'py>,
    first: &Bound<'py, PyAny>,
    second: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    // SAFETY: two live items; a new tuple holding them, or null with an
    // exception set.
    unsafe {
        Bound::from_owned_ptr_or_err(py, ffi::PyTuple_Pack(2, first.as_ptr(), second.as_ptr()))
    }
}

/// A `pair`'s tuple of two items made.
pub(crate) fn pair<'py>(
    py: Python<'py>,
    first: &Bound<'py, PyAny>,
    second: &Bound<'py, PyAny>,
) -> Option<Bound<'py, PyAny>> {
    // SAFETY: two live items; a new tuple holding them, or null.
    own(py, unsafe {
        ffi::PyTuple_Pack(2, first.as_ptr(), second.as_ptr())
    })
}

/// A primitive run's list, its slots filled in order.
struct Fill<'py> {
    py: Python<'py>,
    list: crate::direct_list::NewList<'py>,
    at: isize,
}

impl unityrs_core::walk::emit::Elements for Fill<'_> {
    #[inline(always)]
    #[allow(
        clippy::inline_always,
        reason = "once per run element: outlined, it costs a call per element (callgrind)"
    )]
    fn element(&mut self, v: Scalar) -> Option<()> {
        let item = scalar(self.py, v)?;
        if self.at >= self.list.len() {
            return None;
        }
        // SAFETY: `at` is below the list's length, and each slot is filled
        // once, in order.
        unsafe { self.list.put(self.at, item.into_ptr()) };
        self.at += 1;
        Some(())
    }
}

/// Python values for the core's direct read.
struct PySink<'a, 'py> {
    py: Python<'py>,
    tree: &'a Tree,
    /// Each key node's name and its bytes' length; what they hold of their
    /// own.
    names: &'a mut Vec<Option<(Py<PyAny>, u64)>>,
    size: &'a mut u64,
    meter: &'a Arc<Meter>,
    charge: Option<Charge>,
    reserved: u64,
    need: u64,
    /// The conversion's decode work: a string's bytes four times, bytes'
    /// once ([`crate::value_render`]); in object mode also `build`'s.
    work: u64,
    obj: Option<ObjMode<'py>>,
    /// Values built so far ([`Self::pace`]).
    paced: u32,
    /// Where new lists' slots are stored directly
    /// ([`crate::direct_list::slots`]).
    slots: Option<crate::direct_list::Slots>,
}

impl PySink<'_, '_> {
    /// Counts `n` more values to build: `false` past
    /// [`crate::value_render::CHUNK`], where the full path's conversion
    /// would first let other Python threads run. A read that large is left
    /// to the full path, which reads detached; one this size holds the GIL
    /// only as long as the full path's conversion does between releases.
    #[inline]
    fn pace(&mut self, n: usize) -> bool {
        self.paced = self
            .paced
            .saturating_add(u32::try_from(n).unwrap_or(u32::MAX));
        self.paced < crate::value_render::CHUNK
    }

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

    /// The key node's name, made once per tree, and the charge for its
    /// bytes, made before it is.
    fn name(&mut self, key: NodeId) -> Option<*mut ffi::PyObject> {
        let at = key.0 as usize;
        if let Some(Some((name, bytes))) = self.names.get(at) {
            let name = name.as_ptr();
            let charge = bytes.saturating_mul(8);
            return self.bump(charge).then_some(name);
        }
        let bytes = self.tree.list().name(key);
        if !self.bump((bytes.len() as u64).saturating_mul(8)) {
            return None;
        }
        let (made, _) = crate::direct_keys::text(self.py, bytes)?;
        if self.names.len() <= at {
            self.names.resize_with(at + 1, || None);
        }
        *self.size = self
            .size
            .saturating_add(crate::direct_keys::own_bytes(bytes.len()));
        let ptr = made.as_ptr();
        *self.names.get_mut(at)? = Some((made.unbind(), bytes.len() as u64));
        Some(ptr)
    }
}

impl<'py> Sink for PySink<'_, 'py> {
    type Obj = Bound<'py, PyAny>;
    #[inline]
    fn scalar(&mut self, v: Scalar) -> Option<Self::Obj> {
        if !self.pace(1) {
            return None;
        }
        if !self.built(0, 0) {
            return None;
        }
        scalar(self.py, v)
    }
    #[inline]
    fn str(&mut self, v: &[u8]) -> Option<Self::Obj> {
        if !self.pace(1) {
            return None;
        }
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
        if !self.pace(1) {
            return None;
        }
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
        if !self.pace(1) {
            return None;
        }
        if !self.bump(CONTAINER + 2 * ITEM) {
            return None;
        }
        // Its value task, its items' reservation and its `tuple` task.
        if !self.built(512 + 256, 512 + 256) {
            return None;
        }
        // Made from both items, ready: no tuple is visible half filled.
        match &mut self.obj {
            Some(obj) => obj.tuple(self.py, &first, &second),
            None => pair(self.py, &first, &second),
        }
    }
    fn dict(&mut self, node: NodeId, children: usize) -> Option<Self::Obj> {
        if !self.pace(1) {
            return None;
        }
        if !self.bump(CONTAINER.saturating_add((children as u64).saturating_mul(ITEM))) {
            return None;
        }
        // A class's stand-in for its dict ([`ObjMode::open`]).
        let (costs, stand) = match &mut self.obj {
            Some(obj) => {
                let stand = obj.open(node)?;
                (obj.costs(node), Some(stand))
            }
            None => (0, None),
        };
        let items = (children as u64).saturating_mul(256);
        if !self.built(costs, items) {
            return None;
        }
        if stand.is_some() {
            return stand;
        }
        // SAFETY: a new dict or null.
        own(self.py, unsafe { ffi::PyDict_New() })
    }
    #[inline]
    fn set(&mut self, dict: &Self::Obj, key: NodeId, value: Self::Obj) -> Option<()> {
        if let Some(obj) = &self.obj {
            // A class's field, kept until its object is made.
            let (name, bytes) = obj.key(key)?;
            if !self.bump(bytes.saturating_mul(8)) {
                return None;
            }
            return self.obj.as_mut()?.set(dict, name, value);
        }
        let name = self.name(key)?;
        // SAFETY: all three are live (the name is held by the tree's keys);
        // the call takes its own references.
        if unsafe { ffi::PyDict_SetItem(dict.as_ptr(), name, value.as_ptr()) } != 0 {
            drop(PyErr::take(self.py));
            return None;
        }
        Some(())
    }
    fn list(&mut self, items: std::vec::Drain<'_, Self::Obj>) -> Option<Self::Obj> {
        if !self.pace(1) {
            return None;
        }
        let n = (items.len() as u64).saturating_mul(256);
        if !self.built(n, n) {
            return None;
        }
        match &mut self.obj {
            Some(obj) => obj.list(self.py, items),
            None => list_of(self.py, items),
        }
    }
    fn close(&mut self, node: NodeId, dict: Self::Obj) -> Option<Self::Obj> {
        if !self.pace(1) {
            return None;
        }
        let Some(obj) = &mut self.obj else {
            return Some(dict);
        };
        // The class is called once the read is accepted
        // ([`crate::direct_object`]); its call is charged now.
        let (work, memory) = obj.close(node, &dict)?;
        self.work = self.work.saturating_add(work);
        if !self.bump(memory) {
            return None;
        }
        Some(dict)
    }
    fn run(&mut self, run: Run<'_>) -> Option<Self::Obj> {
        if !self.pace(run.len().saturating_add(1)) {
            return None;
        }
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
        let list = crate::direct_list::NewList::new(py, run.len(), self.slots)?;
        // Out of the collector's sight until every slot is filled: only
        // this function holds the list, so with its empty slots it is
        // never visible to a collection or to any other code (with or
        // without a GIL). A failed element drops it unseen, freeing the
        // elements made; its dealloc skips empty slots.
        // SAFETY: a list just made and tracked, held here only.
        unsafe { ffi::PyObject_GC_UnTrack(list.as_ptr().cast()) };
        let mut fill = Fill { py, list, at: 0 };
        run.each_into(&mut fill)?;
        if fill.at != fill.list.len() {
            return None;
        }
        let list = fill.list.into_list();
        // SAFETY: the list, complete, untracked above.
        unsafe { ffi::PyObject_GC_Track(list.as_ptr().cast()) };
        Some(list)
    }
    fn room(&mut self, items: usize) -> bool {
        self.bump(CONTAINER.saturating_add((items as u64).saturating_mul(ITEM)))
    }
}

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
    let Some(cls) = classes(py) else {
        return Ok(None);
    };
    if !reader.get_type().is(&cls.object) {
        return Ok(None);
    }
    // A replaced `ObjectReader.reader` is left to the full path alone, never
    // called here first.
    if !crate::direct_classes::unreplaced(py, cls) {
        return Ok(None);
    }
    // The full path's first step (`uses_original`): the `reader` property
    // (which makes the file's reader on its first use), its value the
    // reader `ready` checks.
    let file_reader = reader.getattr(name!(py, c"reader")?)?;
    let Some((position, assets)) = ready(py, object, reader, &file_reader) else {
        return Ok(None);
    };
    let tree = if let Some(tree) = object.compiled_tree() {
        tree.clone()
    } else {
        // The type record's tree, compiled as the full path's read would
        // compile it next (its position set, files/ObjectReader.py:244-262),
        // a failure raised as that read raises it.
        if object.serialized_type().is_none_or(|ty| ty.nodes.is_none()) {
            return Ok(None);
        }
        match py.detach(|| object.tree(None)) {
            Ok(Some(tree)) => tree.tree,
            Ok(None) => return Ok(None),
            Err(error) => {
                set_end(py, object, &position)?;
                let failure = crate::worker_error::Failure::parse(error, true);
                return crate::worker_error::finish(Err(failure));
            }
        }
    };
    let mut keys = crate::direct_keys::keys_of(&tree);
    let out = if keys.ok {
        direct(
            py,
            object,
            &tree,
            &mut keys,
            check_read,
            wrap.then_some(assets),
        )
    } else {
        None
    };
    crate::direct_keys::keep(keys);
    let Some(done) = out else {
        return Ok(None);
    };
    set_end(py, object, &position)?;
    let value = match done.obj {
        // The classes, called now ([`crate::direct_object`]).
        Some(obj) => obj.finish(py, &done.value)?,
        None => done.value,
    };
    if wrap {
        // `result.set_object_reader(self)`, the full path's last step.
        let method = value.getattr(name!(py, c"set_object_reader")?)?;
        // SAFETY: a live item; a new tuple or null with an exception set.
        let args =
            unsafe { Bound::from_owned_ptr_or_err(py, ffi::PyTuple_Pack(1, reader.as_ptr())) }?;
        // SAFETY: live objects; a new reference or null with an exception set.
        unsafe {
            Bound::from_owned_ptr_or_err(
                py,
                ffi::PyObject_Call(method.as_ptr(), args.as_ptr(), std::ptr::null_mut()),
            )
        }?;
    }
    drop(done.charge);
    Ok(Some(value.unbind()))
}

/// `reset()` and `Position = byte_start + byte_size`: the memory reader's
/// setter takes any position (the full path's checks, passed).
fn set_end(py: Python<'_>, object: &ObjectRef, position: &Bound<'_, PyDict>) -> PyResult<()> {
    let end = object.info().byte_start + i128::from(object.info().byte_size);
    // SAFETY: a new int or null with an exception set.
    let end = unsafe {
        Bound::from_owned_ptr_or_err(
            py,
            match i64::try_from(end) {
                Ok(small) => ffi::PyLong_FromLongLong(small),
                // Past `i64`: within `u64` (a start and a size, both
                // bounded by a file's length).
                Err(_) => ffi::PyLong_FromUnsignedLongLong(u64::try_from(end).unwrap_or(u64::MAX)),
            },
        )
    }?;
    position.set_item(name!(py, c"_position")?, end)
}

/// A direct read accepted: its value, and in object mode the classes to
/// call; its memory's charge, held until the call ends.
struct Done<'py> {
    value: Bound<'py, PyAny>,
    obj: Option<ObjMode<'py>>,
    charge: Option<Charge>,
}

/// The read, in object mode where `assets` is given: where the tree's keys
/// allow it, and no ref type of the file is the tree's own list (whose
/// references the full path would index).
fn direct<'py>(
    py: Python<'py>,
    object: &ObjectRef,
    tree: &Arc<Tree>,
    keys: &mut TreeKeys,
    check_read: bool,
    assets: Option<Bound<'py, PyAny>>,
) -> Option<Done<'py>> {
    let meter = object.file().meter();
    let refs = object.file().ref_types().unwrap_or_default();
    if assets.is_some()
        && refs.iter().any(|r| {
            r.nodes
                .as_ref()
                .is_some_and(|n| std::ptr::from_ref(&**n) == std::ptr::from_ref(&**tree.list()))
        })
    {
        return None;
    }
    let rows = tree.list().rows().len() as u64;
    // Object mode holds what the full path stages beside the value.
    let staged = if assets.is_some() {
        crate::direct_keys::staged(tree)
    } else {
        0
    };
    let wrap = assets.is_some();
    let start = object.info().byte_start;
    if keys.too_large(start, wrap) {
        return None;
    }
    let TreeKeys {
        names,
        obj,
        size,
        declined,
        ..
    } = keys;
    let mut sink = PySink {
        py,
        tree,
        names,
        size,
        meter,
        charge: None,
        reserved: 0,
        need: 0,
        work: 0,
        obj: None,
        paced: 0,
        slots: crate::direct_list::slots(py),
    };
    let held = BASE
        .saturating_add(rows.saturating_mul(16))
        .saturating_add(staged);
    let first = held.saturating_add(u64::from(object.info().byte_size).saturating_mul(64));
    if !sink.bump(first) {
        return None;
    }
    sink.need = held;
    let mut fixed = 0;
    let mut built = false;
    if let Some(assets) = assets {
        let made = if let Some(made) = obj {
            made.clone()
        } else {
            built = true;
            // Made within the reservation, as values are, and only where
            // the work the full path counts for them fits.
            if !sink.pace(usize::try_from(rows).unwrap_or(usize::MAX))
                || crate::direct_keys::fixed_most(tree) > meter.work_left()
            {
                return None;
            }
            let made = crate::direct_keys::obj_keys(py, tree)?;
            *obj = Some(made.clone());
            made
        };
        if !made.ok {
            return None;
        }
        let objects = ObjMode::new(py, tree.clone(), made, assets)?;
        // The root's class, before anything is read.
        objects.class(tree.root())?;
        fixed = objects.fixed(refs.len());
        sink.obj = Some(objects);
    }
    let read = object.emit_typetree_uncounted(tree, check_read, &mut sink, meter.work_left());
    if read.is_err() && sink.paced >= crate::value_render::CHUNK && !built {
        // Its values' count, which no later read of it changes (the keys'
        // rows, counted only as they are made, not among them).
        if let Some(seen) = declined.get_mut(usize::from(wrap)) {
            crate::direct_keys::remember(seen, start);
        }
    }
    match read {
        // Its work, the full path's to do and count.
        Err(_) => None,
        Ok((value, _, work)) => {
            let read = work.saturating_add(sink.work);
            if !meter.try_count_work(read.saturating_add(fixed)) {
                return None;
            }
            Some(Done {
                value,
                obj: sink.obj.take(),
                charge: sink.charge.take(),
            })
        }
    }
}

/// The object's tree and its reader's `__dict__` where every check of the
/// full path holds: `uses_original`, a `byte_size`, no `nodes` and no tree
/// of the caller's (files/ObjectReader.py:244-262 as `read_typetree` takes
/// them), on a memory reader.
fn ready<'py>(
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
