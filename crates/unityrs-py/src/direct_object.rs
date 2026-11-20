// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeHelper.py, UnityPyBoost/TypeTreeHelper.cpp. See THIRD_PARTY_LICENSES.
//! Object mode for the direct read ([`crate::direct`]): `read_typetree`
//! with `wrap=True` and no `nodes`, each class built by UnityPyBoost's
//! construction rule (UnityPyBoost/TypeTreeHelper.cpp:458-583) as the full
//! path's `_object` module applies it: a `PPtr<...>` node's class is
//! `classes.PPtr` with the file as `assetsfile`, any other node's the
//! `classes` attribute its type names, called with the dict's fields under
//! their cleaned names.
//!
//! The read finds and checks each class as its dict opens, then keeps its
//! fields, and charges the call; only a read that is then accepted (its
//! bytes, its memory and its work) makes the classes' objects, in the
//! order the full path does (each class's fields' before it), once each,
//! after the reader's position is set, each container whole from its
//! parts' objects (until then each pending value is an opaque stand-in
//! in no container, so no code a call runs sees a half-made one). So
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

/// A value to make once the read is accepted, from values each already
/// made then (`Pending` is in completion order: a value's parts first).
/// Each container is made whole, with its parts' objects, only then:
/// none is put in another, or changed, once any code but the read's can
/// see it, as on the full path, which makes each value from the bottom up.
enum Make<'py> {
    /// A class, called with a new dict of these fields.
    Class(
        Bound<'py, PyAny>,
        Vec<(Bound<'py, PyAny>, Bound<'py, PyAny>)>,
    ),
    /// A list of these items.
    List(Vec<Bound<'py, PyAny>>),
    /// A `pair`'s tuple of these items.
    Pair([Bound<'py, PyAny>; 2]),
}

/// A value to make once the read is accepted.
struct Pending<'py> {
    make: Make<'py>,
    /// What stands for the value among the read's until it is made: a new
    /// `object()` of its own, opaque, in no container.
    /// Held, so its address ([`ObjMode::stands`]) is not reused while
    /// the read lasts.
    _stand: Bound<'py, PyAny>,
}

/// A class whose dict is being read.
struct Open<'py> {
    cls: Bound<'py, PyAny>,
    /// `_construct`'s field count for it.
    fields: u64,
    stand: Bound<'py, PyAny>,
    /// Its fields so far, a key set twice keeping its first place and its
    /// last value, as a dict's would.
    items: Vec<(Bound<'py, PyAny>, Bound<'py, PyAny>)>,
}

/// The object-mode part of a direct read.
pub(crate) struct ObjMode<'py> {
    pub(crate) keys: Arc<ObjKeys>,
    tree: Arc<Tree>,
    module: Bound<'py, PyAny>,
    assets: Bound<'py, PyAny>,
    /// Each value to make, in the order it was complete.
    pending: Vec<Pending<'py>>,
    /// Each pending value's index, by its stand-in's address (each held by
    /// its `Pending`, so never reused while the read lasts).
    stands: HashMap<usize, usize>,
    /// The classes whose dicts are being read, the innermost last.
    opened: Vec<Open<'py>>,
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
            stands: HashMap::new(),
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

    /// Whether `value` is the stand-in of a value still to be made.
    fn pending_value(&self, value: &Bound<'py, PyAny>) -> bool {
        !self.stands.is_empty() && self.stands.contains_key(&(value.as_ptr() as usize))
    }

    /// `make` pending: its new stand-in.
    fn defer(&mut self, py: Python<'py>, make: Make<'py>) -> Option<Bound<'py, PyAny>> {
        let stand = py.get_type::<PyAny>().call0().ok()?;
        self.stands
            .insert(stand.as_ptr() as usize, self.pending.len());
        self.pending.push(Pending {
            make,
            _stand: stand.clone(),
        });
        Some(stand)
    }

    /// Field `key` of the innermost class being read (`dict` its stand-in)
    /// set to `value`.
    pub(crate) fn set(
        &mut self,
        dict: &Bound<'py, PyAny>,
        key: *mut ffi::PyObject,
        value: Bound<'py, PyAny>,
    ) -> Option<()> {
        let open = self.opened.last_mut().filter(|o| o.stand.is(dict))?;
        // SAFETY: `key` is live: the tree's keys hold it.
        let key = unsafe { Bound::from_borrowed_ptr(dict.py(), key) };
        put_field(&mut open.items, key, value);
        Some(())
    }

    /// A list of `items`; where one is pending, the list pending too.
    pub(crate) fn list(
        &mut self,
        py: Python<'py>,
        items: impl ExactSizeIterator<Item = Bound<'py, PyAny>>,
    ) -> Option<Bound<'py, PyAny>> {
        let items: Vec<_> = items.collect();
        if items.iter().any(|item| self.pending_value(item)) {
            return self.defer(py, Make::List(items));
        }
        crate::direct::list_of(py, items.into_iter())
    }

    /// A `pair`'s tuple; where an item is pending, the pair pending too.
    pub(crate) fn tuple(
        &mut self,
        py: Python<'py>,
        first: &Bound<'py, PyAny>,
        second: &Bound<'py, PyAny>,
    ) -> Option<Bound<'py, PyAny>> {
        if self.pending_value(first) || self.pending_value(second) {
            return self.defer(py, Make::Pair([first.clone(), second.clone()]));
        }
        crate::direct::pair(py, first, second)
    }

    /// Class `node`'s dict opened: its class found and checked before any
    /// of its fields is read; `None` where the full path is to read the
    /// object (the class missing, or not plain). Its stand-in, which the
    /// read takes for its dict.
    pub(crate) fn open(&mut self, node: NodeId) -> Option<Bound<'py, PyAny>> {
        let (cls, fields) = self.class(node)?;
        let stand = cls.py().get_type::<PyAny>().call0().ok()?;
        self.opened.push(Open {
            cls,
            fields,
            stand: stand.clone(),
            items: Vec::new(),
        });
        Some(stand)
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
        let mut open = self.opened.pop().filter(|o| o.stand.is(dict))?;
        // `build`'s class task, `_construct`'s reservations.
        let mut work = 256_u64;
        let mut memory = 256_u64;
        if pptr {
            work += 256;
            memory += 2048;
            let key = name!(py, c"assetsfile").ok()?.clone().into_any();
            put_field(&mut open.items, key, self.assets.clone());
        }
        let (fields, len) = (open.fields, open.items.len() as u64);
        work = work
            .saturating_add(
                fields
                    .saturating_add(len)
                    .saturating_add(1)
                    .saturating_mul(256),
            )
            .saturating_add(costs);
        memory = memory.saturating_add(fields.saturating_add(len).saturating_mul(256) + 512);
        // The stand-in the read already holds for the dict stays its.
        self.stands
            .insert(open.stand.as_ptr() as usize, self.pending.len());
        self.pending.push(Pending {
            make: Make::Class(open.cls, open.items),
            _stand: open.stand,
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

    /// The read accepted, its position set: each pending value made, in
    /// order, from its parts' objects (each class called with a new dict
    /// of its fields), as the full path makes them; `root`'s object. A
    /// call that fails raises.
    pub(crate) fn finish(
        mut self,
        py: Python<'py>,
        root: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        // SAFETY: a new empty tuple or null with an exception set.
        let empty = unsafe { Bound::from_owned_ptr_or_err(py, ffi::PyTuple_New(0)) }?;
        let lost = || pyo3::exceptions::PySystemError::new_err("a direct read lost its value");
        let pending = std::mem::take(&mut self.pending);
        let mut made: Vec<Bound<'py, PyAny>> = Vec::with_capacity(pending.len());
        // A part's object: its own where it was pending (made before it).
        let part = |made: &[Bound<'py, PyAny>], v: &Bound<'py, PyAny>| match self
            .stands
            .get(&(v.as_ptr() as usize))
        {
            Some(&at) => made.get(at).cloned().ok_or_else(lost),
            None => Ok(v.clone()),
        };
        for p in &pending {
            let object = match &p.make {
                Make::Class(cls, items) => {
                    // SAFETY: a new dict or null with an exception set.
                    let dict = unsafe { Bound::from_owned_ptr_or_err(py, ffi::PyDict_New()) }?;
                    for (key, value) in items {
                        let value = part(&made, value)?;
                        // SAFETY: live objects (exact `str` keys); the dict
                        // takes its own references.
                        if unsafe {
                            ffi::PyDict_SetItem(dict.as_ptr(), key.as_ptr(), value.as_ptr())
                        } != 0
                        {
                            return Err(fetched(py));
                        }
                    }
                    // SAFETY: live objects; a new reference or null with an
                    // exception set.
                    unsafe {
                        Bound::from_owned_ptr_or_err(
                            py,
                            ffi::PyObject_Call(cls.as_ptr(), empty.as_ptr(), dict.as_ptr()),
                        )
                    }?
                }
                Make::List(items) => {
                    let items = items
                        .iter()
                        .map(|v| part(&made, v))
                        .collect::<PyResult<Vec<_>>>()?;
                    pyo3::types::PyList::new(py, items)?.into_any()
                }
                Make::Pair([first, second]) => {
                    let (first, second) = (part(&made, first)?, part(&made, second)?);
                    crate::direct::pair_raised(py, &first, &second)?
                }
            };
            made.push(object);
        }
        part(&made, root)
    }
}

/// `key` set to `value` among a dict's `items`, as `PyDict_SetItem` sets
/// it (the keys are the tree's interned `str`s, alike only when the same).
fn put_field<'py>(
    items: &mut Vec<(Bound<'py, PyAny>, Bound<'py, PyAny>)>,
    key: Bound<'py, PyAny>,
    value: Bound<'py, PyAny>,
) {
    match items.iter_mut().find(|(k, _)| k.is(&key)) {
        Some(item) => item.1 = value,
        None => items.push((key, value)),
    }
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
