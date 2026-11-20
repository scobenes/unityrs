// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeHelper.py, UnityPyBoost/TypeTreeHelper.cpp. See THIRD_PARTY_LICENSES.
//! Object mode for the direct read ([`crate::direct`]): `read_typetree`
//! with `wrap=True` and no `nodes`, each class built by UnityPyBoost's
//! construction rule (UnityPyBoost/TypeTreeHelper.cpp:458-583) as the full
//! path's `_object` module applies it: a `PPtr<...>` node's class is
//! `classes.PPtr` with the file as `assetsfile`, any other node's the
//! `classes` attribute its type names, called with the dict's fields under
//! their cleaned names.
//!
//! The read finds and checks each class as its dict opens, then makes the
//! dict, its fields' objects in place, and charges the call; only a read
//! that is then accepted (its
//! bytes, its memory and its work) calls the classes, in the order the full
//! path does (each class's fields' before it), once each, after the
//! reader's position is set, putting each object where its dict was. So
//! only a class whose call runs no code of its own is taken
//! (`_object.plain_class`: attrs' generated `__init__`, storing exactly the
//! dict's fields); any other, a missing one (an `UnknownObject`) or a name
//! two fields clean alike leaves the object to the full path before any
//! class is called. A call that fails then raises.
//!
//! The work counted is the full path's: its staging of the tree's nodes
//! (`crate::object_nodes`), its references' index, and the `_object`
//! module's reservations, each value's, each container's and each class's.
use crate::direct_keys::{ObjKeys, import_attr, name};
use pyo3::{ffi, prelude::*, sync::PyOnceLock};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use unityrs_core::typetree::{NodeId, Tree};

/// A class checked for the keys of its fields.
struct Checked {
    _cls: Py<PyAny>,
    _keys: Vec<Py<PyAny>>,
    fields: u64,
    plain: bool,
}

/// Classes checked, by the class's and its field keys' addresses (each
/// held by its entry, so never reused while it is kept).
static CHECKED: Mutex<Option<HashMap<Vec<usize>, Checked>>> = Mutex::new(None);
/// Classes checked kept at most.
const CHECKED_MOST: usize = 4096;

fn checked() -> MutexGuard<'static, Option<HashMap<Vec<usize>, Checked>>> {
    CHECKED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where a class's dict is, and its object goes: each container (and key)
/// held here, from the dict's placing until its object is put, so no
/// code the classes' calls run (a constructor, a collection's callbacks)
/// can free one before the object is put in it.
enum Place<'py> {
    /// The read's value.
    Root,
    /// A field of a class's dict, under a key.
    Dict(Bound<'py, PyAny>, Bound<'py, PyAny>),
    /// An item of a list.
    List(Bound<'py, PyAny>, isize),
    /// An item of a `pair`'s tuple.
    Tuple(Bound<'py, PyAny>, isize),
}

/// A class to call once the read is accepted.
struct Pending<'py> {
    cls: Bound<'py, PyAny>,
    dict: Bound<'py, PyAny>,
    at: Place<'py>,
}

/// The object-mode part of a direct read.
pub(crate) struct ObjMode<'py> {
    pub(crate) keys: Arc<ObjKeys>,
    tree: Arc<Tree>,
    module: Bound<'py, PyAny>,
    assets: Bound<'py, PyAny>,
    /// Each class read, in the order its dict was complete.
    pending: Vec<Pending<'py>>,
    /// The pending classes not yet in a container, the last read last.
    loose: Vec<usize>,
    /// A container's items that are classes' dicts, as it is made.
    found: Vec<(isize, *mut ffi::PyObject)>,
    /// The classes of the dicts being read, and their field counts.
    opened: Vec<(Bound<'py, PyAny>, u64)>,
}

fn is_dict(v: &Bound<'_, PyAny>) -> bool {
    // SAFETY: a type check of a live object.
    unsafe { ffi::PyDict_CheckExact(v.as_ptr()) != 0 }
}

/// A Python failure taken as raised, never empty.
fn fetched(py: Python<'_>) -> PyErr {
    PyErr::take(py)
        .unwrap_or_else(|| pyo3::exceptions::PySystemError::new_err("a direct read's call failed"))
}

impl<'py> ObjMode<'py> {
    pub(crate) fn new(
        py: Python<'py>,
        tree: Arc<Tree>,
        keys: Arc<ObjKeys>,
        assets: Bound<'py, PyAny>,
    ) -> Option<Self> {
        static MODULE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
        let module = MODULE
            .get_or_try_init(py, || {
                PyModule::import(py, name!(py, c"unityrs.classes")?).map(|m| m.into_any().unbind())
            })
            .ok()?
            .bind(py)
            .clone();
        Some(Self {
            keys,
            tree,
            module,
            assets,
            pending: Vec::new(),
            loose: Vec::new(),
            found: Vec::new(),
            opened: Vec::new(),
        })
    }

    /// The work the full path counts beyond the read's and its values':
    /// the tree's nodes staged, and its references' index over the file's
    /// ref types (none of which is this tree's: `direct` checks).
    pub(crate) fn fixed(&self, refs: usize) -> u64 {
        self.keys
            .fixed
            .saturating_add((refs as u64).saturating_add(1).saturating_mul(64))
    }

    /// Field `key`'s name in the class's dict, and its node name's bytes'
    /// length.
    pub(crate) fn key(&self, key: NodeId) -> Option<(*mut ffi::PyObject, u64)> {
        let nk = self.keys.nodes.get(key.0 as usize)?;
        Some((nk.key.as_ptr(), nk.bytes))
    }

    /// `build`'s cost of a dict of class `node`.
    pub(crate) fn costs(&self, node: NodeId) -> u64 {
        self.keys
            .nodes
            .get(node.0 as usize)
            .map_or(u64::MAX, |n| n.costs)
    }

    /// The classes' dicts among a container's items, the last of the loose
    /// ones in order, now at `at` of each.
    fn place(&mut self, at: impl Fn(isize) -> Place<'py>) -> Option<()> {
        let base = self.loose.len().checked_sub(self.found.len())?;
        for (i, &(slot, ptr)) in self.found.iter().enumerate() {
            let p = self.pending.get_mut(*self.loose.get(base + i)?)?;
            if p.dict.as_ptr() != ptr {
                return None;
            }
            p.at = at(slot);
        }
        self.loose.truncate(base);
        self.found.clear();
        Some(())
    }

    /// `value` set in class dict `dict` under `key`.
    pub(crate) fn set(
        &mut self,
        dict: &Bound<'py, PyAny>,
        key: *mut ffi::PyObject,
        value: &Bound<'py, PyAny>,
    ) -> Option<()> {
        if !is_dict(value) {
            return Some(());
        }
        self.found.clear();
        self.found.push((0, value.as_ptr()));
        // SAFETY: `key` is live: the tree's keys hold it.
        let key = unsafe { Bound::from_borrowed_ptr(dict.py(), key) };
        self.place(|_| Place::Dict(dict.clone(), key.clone()))
    }

    /// A list of `items`.
    pub(crate) fn list(
        &mut self,
        py: Python<'py>,
        items: impl ExactSizeIterator<Item = Bound<'py, PyAny>>,
    ) -> Option<Bound<'py, PyAny>> {
        self.found.clear();
        let found = &mut self.found;
        let list = crate::direct::list_of(
            py,
            items.enumerate().map(|(at, item)| {
                if is_dict(&item) {
                    found.push((at.cast_signed(), item.as_ptr()));
                }
                item
            }),
        )?;
        self.place(|at| Place::List(list.clone(), at))?;
        Some(list)
    }

    /// A `pair`'s tuple.
    pub(crate) fn tuple(
        &mut self,
        py: Python<'py>,
        first: &Bound<'py, PyAny>,
        second: &Bound<'py, PyAny>,
    ) -> Option<Bound<'py, PyAny>> {
        let tuple = crate::direct::pair(py, first, second)?;
        self.found.clear();
        for (at, item) in [first, second].into_iter().enumerate() {
            if is_dict(item) {
                self.found.push((at.cast_signed(), item.as_ptr()));
            }
        }
        self.place(|at| Place::Tuple(tuple.clone(), at))?;
        Some(tuple)
    }

    /// Class `node`'s dict opened: its class found and checked before any
    /// of its fields is read; `None` where the full path is to read the
    /// object (the class missing, or not plain).
    pub(crate) fn open(&mut self, node: NodeId) -> Option<()> {
        let found = self.class(node)?;
        self.opened.push(found);
        Some(())
    }

    /// Class `node`'s class and field count, where it is plain.
    pub(crate) fn class(&self, node: NodeId) -> Option<(Bound<'py, PyAny>, u64)> {
        let py = self.module.py();
        let at = node.0 as usize;
        let nk = self.keys.nodes.get(at)?;
        let pptr = nk.pptr;
        let cls = if pptr {
            self.module.getattr(name!(py, c"PPtr").ok()?).ok()?
        } else {
            let ty = nk.ty.bind(py);
            // SAFETY: live objects; a new reference or null with an
            // exception set.
            crate::direct::own(py, unsafe {
                ffi::PyObject_GetAttr(self.module.as_ptr(), ty.as_ptr())
            })?
        };
        let known = match self
            .keys
            .classes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(at)?
        {
            Some((known, fields, plain)) if known.is(&cls) => Some((*fields, *plain)),
            _ => None,
        };
        let (fields, plain) = if let Some(known) = known {
            known
        } else {
            let (fields, plain) = self.check(&cls, node, pptr)?;
            *self
                .keys
                .classes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_mut(at)? = Some((cls.clone().unbind(), fields, plain));
            (fields, plain)
        };
        plain.then_some((cls, fields))
    }

    /// Class `node`'s dict complete: the call's work and memory
    /// (`_construct`'s reservations), to charge now.
    pub(crate) fn close(&mut self, node: NodeId, dict: &Bound<'py, PyAny>) -> Option<(u64, u64)> {
        let py = dict.py();
        let nk = self.keys.nodes.get(node.0 as usize)?;
        let (pptr, costs) = (nk.pptr, nk.costs);
        let (cls, fields) = self.opened.pop()?;
        // `build`'s class task, `_construct`'s reservations.
        let mut work = 256_u64;
        let mut memory = 256_u64;
        if pptr {
            work += 256;
            memory += 2048;
            let key = name!(py, c"assetsfile").ok()?;
            // SAFETY: live objects; the dict takes its own references.
            if unsafe { ffi::PyDict_SetItem(dict.as_ptr(), key.as_ptr(), self.assets.as_ptr()) }
                != 0
            {
                drop(PyErr::take(py));
                return None;
            }
        }
        // SAFETY: a live dict.
        let len = u64::try_from(unsafe { ffi::PyDict_Size(dict.as_ptr()) }).ok()?;
        work = work
            .saturating_add(
                fields
                    .saturating_add(len)
                    .saturating_add(1)
                    .saturating_mul(256),
            )
            .saturating_add(costs);
        memory = memory.saturating_add(fields.saturating_add(len).saturating_mul(256) + 512);
        self.loose.push(self.pending.len());
        self.pending.push(Pending {
            cls,
            dict: dict.clone(),
            at: Place::Root,
        });
        Some((work, memory))
    }

    /// `cls`'s field count and whether it is plain for class `node`'s
    /// fields, as checked before for the class and those keys (by any
    /// tree: the keys are the shared strings), or checked now.
    fn check(&self, cls: &Bound<'py, PyAny>, node: NodeId, pptr: bool) -> Option<(u64, bool)> {
        let keys = self.children(node)?;
        let mut sig = Vec::with_capacity(keys.len() + 2);
        sig.push(cls.as_ptr() as usize);
        sig.push(usize::from(pptr));
        sig.extend(keys.iter().map(|k| *k as usize));
        let found = checked()
            .as_ref()
            .and_then(|c| c.get(&sig).map(|e| (e.fields, e.plain)));
        if let Some(found) = found {
            return Some(found);
        }
        let fields = class_fields(cls, pptr)?;
        let plain = Self::plain(cls, &keys, pptr)?;
        let py = cls.py();
        // Each object the signature names, held while it is kept.
        let held = keys
            .iter()
            // SAFETY: each key is live (the tree's keys hold it).
            .map(|k| unsafe { Bound::from_borrowed_ptr(py, *k) }.unbind())
            .collect();
        let entry = Checked {
            _cls: cls.clone().unbind(),
            _keys: held,
            fields,
            plain,
        };
        let mut dropped = None;
        {
            let mut kept = checked();
            let kept = kept.get_or_insert_with(HashMap::new);
            if kept.len() >= CHECKED_MOST {
                dropped = Some(std::mem::take(kept));
            }
            kept.insert(sig, entry);
        }
        drop(dropped);
        Some((fields, plain))
    }

    /// `_object.plain_class(cls, keys)` for a class's field keys.
    fn plain(
        cls: &Bound<'py, PyAny>,
        tree_keys: &[*mut ffi::PyObject],
        pptr: bool,
    ) -> Option<bool> {
        static PLAIN: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
        let py = cls.py();
        let plain = PLAIN
            .get_or_try_init(py, || {
                import_attr(
                    py,
                    name!(py, c"unityrs._object")?,
                    name!(py, c"plain_class")?,
                )
                .map(Bound::unbind)
            })
            .ok()?
            .bind(py);
        // SAFETY: a new empty list or null.
        let keys = crate::direct::own(py, unsafe { ffi::PyList_New(0) })?;
        let extra = if pptr {
            Some(name!(py, c"assetsfile").ok()?.as_ptr())
        } else {
            None
        };
        for key in tree_keys.iter().copied().chain(extra) {
            // SAFETY: a live list and a live key; the list takes its own
            // reference.
            if unsafe { ffi::PyList_Append(keys.as_ptr(), key) } != 0 {
                drop(PyErr::take(py));
                return None;
            }
        }
        // SAFETY: two live items; a new tuple or null.
        let args = crate::direct::own(py, unsafe {
            ffi::PyTuple_Pack(2, cls.as_ptr(), keys.as_ptr())
        })?;
        // SAFETY: live objects; a new reference or null with an exception set.
        let out = crate::direct::own(py, unsafe {
            ffi::PyObject_Call(plain.as_ptr(), args.as_ptr(), std::ptr::null_mut())
        })?;
        out.is_truthy().ok()
    }

    /// Class `node`'s fields' keys.
    fn children(&self, node: NodeId) -> Option<Vec<*mut ffi::PyObject>> {
        self.tree
            .list()
            .children(node)
            .iter()
            .map(|kid| Some(self.key(*kid)?.0))
            .collect()
    }

    /// The read accepted, its position set: each class called, in order,
    /// its object put where its dict was; the root's object. A call that
    /// fails raises.
    pub(crate) fn finish(mut self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        // SAFETY: a new empty tuple or null with an exception set.
        let empty = unsafe { Bound::from_owned_ptr_or_err(py, ffi::PyTuple_New(0)) }?;
        let mut root = None;
        for p in self.pending.drain(..) {
            // SAFETY: live objects; a new reference or null with an
            // exception set.
            let made = unsafe {
                Bound::from_owned_ptr_or_err(
                    py,
                    ffi::PyObject_Call(p.cls.as_ptr(), empty.as_ptr(), p.dict.as_ptr()),
                )
            }?;
            let failed = match p.at {
                Place::Root => {
                    root = Some(made);
                    0
                }
                // SAFETY: the dict and key are held (`Place`); the call
                // takes its own reference to each.
                Place::Dict(dict, key) => unsafe {
                    ffi::PyDict_SetItem(dict.as_ptr(), key.as_ptr(), made.as_ptr())
                },
                // SAFETY: the list is held (`Place`); the call takes the
                // object's reference, and raises for a slot gone.
                Place::List(list, at) => unsafe {
                    ffi::PyList_SetItem(list.as_ptr(), at, made.into_ptr())
                },
                Place::Tuple(tuple, at) => put_in_tuple(py, tuple, at, made),
            };
            if failed != 0 {
                return Err(fetched(py));
            }
        }
        root.ok_or_else(|| pyo3::exceptions::PySystemError::new_err("a direct read lost its value"))
    }
}

/// `PyTuple_SetItem(tuple, at, made)` for a tuple held by `Place`.
/// `PyTuple_SetItem` refuses a tuple with any reference but one, so this
/// one is let go first, where the tuple has exactly one other (its
/// container's), which keeps it alive through the call: the call runs no
/// Python before its store, and the tuple is not touched after. Any other
/// count is refused as `PyTuple_SetItem` refuses it.
fn put_in_tuple<'py>(
    py: Python<'py>,
    tuple: Bound<'py, PyAny>,
    at: isize,
    made: Bound<'py, PyAny>,
) -> std::ffi::c_int {
    let ptr = tuple.as_ptr();
    // SAFETY: a live object's count.
    if unsafe { ffi::Py_REFCNT(ptr) } != 2 {
        pyo3::exceptions::PySystemError::new_err("a direct read's tuple is shared").restore(py);
        return -1;
    }
    drop(tuple);
    // SAFETY: the tuple's other reference keeps it alive (above); the call
    // takes the object's reference, and raises for a slot out of range.
    unsafe { ffi::PyTuple_SetItem(ptr, at, made.into_ptr()) }
}

/// The fields `_construct` counts for `cls`: `__attrs_attrs__`,
/// `__annotations__` and `__slots__`; `None` for what is no class the rule
/// calls first (UnityPy's `UnknownObject` cases).
fn class_fields(cls: &Bound<'_, PyAny>, pptr: bool) -> Option<u64> {
    let py = cls.py();
    if !pptr {
        // SAFETY: a type check of a live object.
        if unsafe { ffi::PyType_Check(cls.as_ptr()) } == 0 {
            return None;
        }
        let module = cls.getattr(name!(py, c"__module__").ok()?).ok()?;
        if module.extract::<&str>().ok() == Some("builtins") {
            return None;
        }
    }
    let mut fields = 0_u64;
    for name in [
        name!(py, c"__attrs_attrs__").ok()?,
        name!(py, c"__annotations__").ok()?,
        name!(py, c"__slots__").ok()?,
    ] {
        // `getattr(cls, name, default)`: only a missing attribute is the
        // default; any other error is the full path's to raise.
        match cls.getattr(name) {
            Ok(v) => fields = fields.saturating_add(v.len().ok()? as u64),
            Err(e) if e.is_instance_of::<pyo3::exceptions::PyAttributeError>(py) => {}
            Err(_) => return None,
        }
    }
    Some(fields)
}
