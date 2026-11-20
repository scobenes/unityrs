// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeHelper.py, UnityPyBoost/TypeTreeHelper.cpp. See THIRD_PARTY_LICENSES.
//! Object mode for the direct read ([`crate::direct`]): `read_typetree`
//! with `wrap=True` and no `nodes`, each class built as its dict is
//! complete, by UnityPyBoost's construction rule
//! (UnityPyBoost/TypeTreeHelper.cpp:458-583) as the full path's `_object`
//! module applies it: a `PPtr<...>` node's class is `classes.PPtr` with
//! the file as `assetsfile`, any other node's the `classes` attribute its
//! type names, called with the dict's fields under their cleaned names.
//! Only what that rule's first call answers is taken here: a node whose
//! class is missing (an `UnknownObject`), a name two fields clean alike, or
//! a constructor that raises gives up, and the full path reads the object.
//!
//! The work counted is the full path's: its staging of the tree's nodes
//! (`crate::object_nodes`), its references' index, and the `_object`
//! module's reservations, each value's, each container's and each class's.
use crate::python::held as h;
use pyo3::{ffi, prelude::*, sync::PyOnceLock};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use unityrs_core::typetree::{NodeId, Shape, Tree};

/// One node's strings and costs, as the `_object` module has them.
struct NodeKeys {
    /// `clean_name(m_Name, ascii_digit=True)`.
    key: Py<PyAny>,
    /// `m_Type`.
    ty: Py<pyo3::types::PyString>,
    pptr: bool,
    /// `build`'s cost of the node (`costs[id(node)]`).
    costs: u64,
}

/// A tree's object-mode keys, made once for each tree of the same rows
/// and kept with its dict keys.
pub(crate) struct ObjKeys {
    pub(crate) ok: bool,
    nodes: Vec<NodeKeys>,
    /// The work of staging the tree's nodes, natively and in `_node_lists`.
    fixed: u64,
    /// The memory the full path charges for that staging, its references'
    /// index and its open (`object_nodes`, `object_result`).
    pub(crate) staged: u64,
    /// Each node's class as last found, and its field count. Never held
    /// while Python runs.
    classes: Mutex<Vec<Option<Known>>>,
}

/// A class as last found for a node, and its field count.
type Known = (Py<PyAny>, u64);

/// Object-mode keys by their tree's rows: what they are made of.
static KEYS: Mutex<Option<HashMap<Vec<u8>, Arc<ObjKeys>>>> = Mutex::new(None);
/// Rows' keys kept at most.
const KEPT: usize = 256;

/// What a tree's object-mode keys are made of: each row's level, flags,
/// type and name, and its children's count.
fn rows_key(tree: &Tree) -> Option<Vec<u8>> {
    let list = tree.list();
    let mut key = Vec::new();
    for (at, row) in list.rows().iter().enumerate() {
        let kids = list.children(NodeId(u32::try_from(at).ok()?)).len();
        key.extend_from_slice(&row.level.to_le_bytes());
        key.extend_from_slice(&u64::try_from(kids).ok()?.to_le_bytes());
        key.push(u8::from(row.aligned()));
        for text in [row.ty.as_bytes(), row.name.as_bytes()] {
            key.extend_from_slice(&u64::try_from(text.len()).ok()?.to_le_bytes());
            key.extend_from_slice(text);
        }
    }
    Some(key)
}

/// The keys of `tree`'s rows, made at the first tree of them.
pub(crate) fn keys_of(py: Python<'_>, tree: &Tree) -> Option<Arc<ObjKeys>> {
    let key = rows_key(tree)?;
    let found = KEYS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|kept| kept.get(&key).cloned());
    if found.is_some() {
        return found;
    }
    // Made unlocked: `clean_name` is Python's.
    let made = Arc::new(ObjKeys::new(py, tree).ok()?);
    let mut kept = KEYS.lock().unwrap_or_else(PoisonError::into_inner);
    let kept = kept.get_or_insert_with(HashMap::new);
    if kept.len() >= KEPT {
        kept.clear();
    }
    Some(kept.entry(key).or_insert(made).clone())
}

static CLEAN: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static MODULE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

fn len_of(s: &Bound<'_, PyAny>) -> u64 {
    // SAFETY: a live `str`.
    u64::try_from(unsafe { ffi::PyUnicode_GetLength(s.as_ptr()) }).unwrap_or(u64::MAX)
}

impl ObjKeys {
    fn new(py: Python<'_>, tree: &Tree) -> PyResult<Self> {
        let clean = CLEAN.get_or_try_init(py, || {
            py.import("unityrs.classes._names")?
                .getattr("clean_name")
                .map(Bound::unbind)
        })?;
        let kwargs = pyo3::types::PyDict::new(py);
        kwargs.set_item("ascii_digit", true)?;
        let list = tree.list();
        let rows = list.rows();
        let mut nodes = Vec::with_capacity(rows.len());
        let mut names = Vec::with_capacity(rows.len());
        let mut fixed = 4096_u64.saturating_mul(rows.len() as u64);
        let mut staged: u64 = 128 + 64 + 64 + 64 + 64 + 128;
        for (at, row) in rows.iter().enumerate() {
            let id = NodeId(u32::try_from(at)?);
            let name = h::string(py, row.name.as_bytes())?;
            let ty = h::string(py, row.ty.as_bytes())?.cast_into::<pyo3::types::PyString>()?;
            let key = clean.bind(py).call((&name,), Some(&kwargs))?;
            let children = list.children(id).len() as u64;
            let text = row.ty.as_bytes().len() as u64 + row.name.as_bytes().len() as u64;
            staged =
                staged.saturating_add(4096 + text.saturating_mul(8) + children.saturating_mul(64));
            // `object_nodes::convert`, then `_node_lists`.
            fixed = fixed
                .saturating_add(4096 + text.saturating_mul(4) + children.saturating_mul(64))
                .saturating_add(children.saturating_mul(64) + 4 * (len_of(&name) + len_of(&ty)));
            names.push(len_of(&name));
            let ty_len = len_of(ty.as_any());
            nodes.push(NodeKeys {
                key: key.unbind(),
                ty: ty.unbind(),
                pptr: row.ty.as_bytes().starts_with(b"PPtr<"),
                costs: 4 * ty_len,
            });
        }
        let mut ok = true;
        for (at, node) in nodes.iter_mut().enumerate() {
            let id = NodeId(u32::try_from(at)?);
            let kids = list.children(id);
            for kid in kids {
                let n = names.get(kid.0 as usize).copied().unwrap_or(u64::MAX);
                node.costs = node.costs.saturating_add(256 + 4 * n);
            }
        }
        for at in 0..rows.len() {
            let id = NodeId(u32::try_from(at)?);
            if tree.info(id).is_none_or(|info| info.shape != Shape::Class) {
                continue;
            }
            let mut seen = HashSet::new();
            for kid in list.children(id) {
                let key = nodes
                    .get(kid.0 as usize)
                    .and_then(|n| n.key.bind(py).extract::<String>().ok());
                ok &= key.is_some_and(|key| seen.insert(key));
            }
        }
        Ok(Self {
            ok,
            classes: Mutex::new((0..nodes.len()).map(|_| None).collect()),
            nodes,
            fixed,
            staged,
        })
    }
}

/// The object-mode part of a direct read.
pub(crate) struct ObjMode<'a, 'py> {
    pub(crate) keys: &'a ObjKeys,
    module: Bound<'py, PyAny>,
    assets: Bound<'py, PyAny>,
    empty: Bound<'py, PyAny>,
}

impl<'a, 'py> ObjMode<'a, 'py> {
    pub(crate) fn new(
        py: Python<'py>,
        keys: &'a ObjKeys,
        assets: Bound<'py, PyAny>,
    ) -> Option<Self> {
        let module = MODULE
            .get_or_try_init(py, || {
                py.import("unityrs.classes").map(|m| m.into_any().unbind())
            })
            .ok()?
            .bind(py)
            .clone();
        Some(Self {
            keys,
            module,
            assets,
            empty: pyo3::types::PyTuple::empty(py).into_any(),
        })
    }

    /// The work the full path counts beyond the read's and its values':
    /// the tree's nodes staged, and its references' index over the file's
    /// ref types (none of which is this tree's: `ready` checks).
    pub(crate) fn fixed(&self, refs: usize) -> u64 {
        self.keys
            .fixed
            .saturating_add((refs as u64).saturating_add(1).saturating_mul(64))
    }

    /// Field `key`'s name in the class's dict.
    pub(crate) fn key(&self, key: NodeId) -> Option<*mut ffi::PyObject> {
        Some(self.keys.nodes.get(key.0 as usize)?.key.as_ptr())
    }

    /// `build`'s cost of a dict of class `node`.
    pub(crate) fn costs(&self, node: NodeId) -> u64 {
        self.keys
            .nodes
            .get(node.0 as usize)
            .map_or(u64::MAX, |n| n.costs)
    }

    /// `_construct(node, values, assetsfile, boost=True)`'s first call:
    /// the class and its work, or `None` where the rule goes further.
    pub(crate) fn construct(
        &mut self,
        node: NodeId,
        dict: &Bound<'py, PyAny>,
        work: &mut u64,
        memory: &mut u64,
    ) -> Option<Bound<'py, PyAny>> {
        let py = dict.py();
        let at = node.0 as usize;
        let nk = self.keys.nodes.get(at)?;
        let (pptr, costs) = (nk.pptr, nk.costs);
        let cls = if pptr {
            self.module.getattr(pyo3::intern!(py, "PPtr")).ok()?
        } else {
            self.module.getattr(nk.ty.bind(py)).ok()?
        };
        let lock = || {
            self.keys
                .classes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
        };
        let known = match lock().get(at)? {
            Some((known, fields)) if known.is(&cls) => Some(*fields),
            _ => None,
        };
        let fields = if let Some(fields) = known {
            fields
        } else {
            let fields = class_fields(&cls, pptr)?;
            *lock().get_mut(at)? = Some((cls.clone().unbind(), fields));
            fields
        };
        // `build`'s class task, `_construct`'s reservations.
        *work = work.saturating_add(256);
        *memory = 256;
        if pptr {
            *work = work.saturating_add(256);
            *memory += 2048;
            // SAFETY: a live dict and two live objects; the dict takes its
            // own references.
            if unsafe {
                ffi::PyDict_SetItem(
                    dict.as_ptr(),
                    pyo3::intern!(py, "assetsfile").as_ptr(),
                    self.assets.as_ptr(),
                )
            } != 0
            {
                drop(PyErr::take(py));
                return None;
            }
        }
        // SAFETY: a live dict.
        let len = u64::try_from(unsafe { ffi::PyDict_Size(dict.as_ptr()) }).ok()?;
        *work = work
            .saturating_add(
                fields
                    .saturating_add(len)
                    .saturating_add(1)
                    .saturating_mul(256),
            )
            .saturating_add(costs);
        *memory = memory.saturating_add(fields.saturating_add(len).saturating_mul(256) + 512);
        // SAFETY: all live; a new reference or null with an exception set.
        crate::direct::own(py, unsafe {
            ffi::PyObject_Call(cls.as_ptr(), self.empty.as_ptr(), dict.as_ptr())
        })
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
        let module = cls.getattr(pyo3::intern!(py, "__module__")).ok()?;
        if module.extract::<&str>().ok() == Some("builtins") {
            return None;
        }
    }
    let mut fields = 0_u64;
    for name in ["__attrs_attrs__", "__annotations__", "__slots__"] {
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
