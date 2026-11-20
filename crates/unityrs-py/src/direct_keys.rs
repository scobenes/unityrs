//! The strings a direct read ([`crate::direct`]) keys its values with, made
//! once and kept within bounds: each name's and type's `str` (and a name's
//! `clean_name`) by its bytes, shared by every tree that has them; and each
//! tree's keys by the tree, for the trees read last.
//!
//! What is kept is bounded: strings of at most [`LONGEST`] bytes, at most
//! [`MOST`] of them and [`TEXT`] bytes in all (past either, all are dropped
//! and made again as met); trees of at most [`MOST_ROWS`] rows, the last
//! [`KEPT`] read, holding at most [`KEPT_BYTES`] in all. A read makes them
//! only within its reservation on the file's meter. No lock is held while
//! Python runs or a kept object is dropped.
use pyo3::{ffi, prelude::*, sync::PyOnceLock, types::PyString};
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use unityrs_core::typetree::{NodeId, Shape, Tree};

/// A static name's interned `str`, made once (`PyResult`: an allocation
/// that fails raises).
macro_rules! name {
    ($py:expr, $text:literal) => {{
        static CELL: pyo3::sync::PyOnceLock<pyo3::Py<pyo3::types::PyString>> =
            pyo3::sync::PyOnceLock::new();
        $crate::direct_keys::interned($py, &CELL, $text)
    }};
}
pub(crate) use name;

pub(crate) fn interned<'a>(
    py: Python<'a>,
    cell: &'a PyOnceLock<Py<PyString>>,
    text: &CStr,
) -> PyResult<&'a Bound<'a, PyString>> {
    let made = cell.get_or_try_init(py, || {
        // SAFETY: a C string; a new reference or null with an exception set.
        let made = unsafe {
            Bound::from_owned_ptr_or_err(py, ffi::PyUnicode_InternFromString(text.as_ptr()))
        }?;
        Ok::<_, PyErr>(made.cast_into::<PyString>()?.unbind())
    })?;
    Ok(made.bind(py))
}

/// A module's attribute, by checked names.
pub(crate) fn import_attr<'py>(
    py: Python<'py>,
    module: &Bound<'py, PyString>,
    attr: &Bound<'py, PyString>,
) -> PyResult<Bound<'py, PyAny>> {
    PyModule::import(py, module)?.getattr(attr)
}

/// Strings longer than this are not kept.
const LONGEST: usize = 1 << 10;
/// Strings kept at most.
const MOST: usize = 1 << 16;
/// Their bytes, at most.
const TEXT: usize = 4 << 20;
/// A tree of more rows is left to the full path.
pub(crate) const MOST_ROWS: usize = 1 << 16;
/// Trees whose keys are kept.
const KEPT: usize = 64;
/// What their keys hold beside the shared strings, at most.
const KEPT_BYTES: u64 = 16 << 20;

/// A kept string: its `str`, length, and as a name its `clean_name`.
struct Entry {
    text: Py<PyAny>,
    len: u64,
    key: Option<(Py<PyAny>, Arc<str>)>,
}

#[derive(Default)]
struct Table {
    map: HashMap<Box<[u8]>, Entry>,
    bytes: usize,
}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);

fn table() -> MutexGuard<'static, Option<Table>> {
    TABLE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A name's length and `clean_name` (and that as text).
pub(crate) struct Named<'py> {
    pub(crate) len: u64,
    pub(crate) key: Bound<'py, PyAny>,
    pub(crate) key_text: Arc<str>,
}

fn len_of(s: &Bound<'_, PyAny>) -> u64 {
    // SAFETY: a live `str`.
    u64::try_from(unsafe { ffi::PyUnicode_GetLength(s.as_ptr()) }).unwrap_or(u64::MAX)
}

/// `bytes` as UnityPy decodes a name (`surrogateescape`), and its length.
pub(crate) fn text<'py>(py: Python<'py>, bytes: &[u8]) -> Option<(Bound<'py, PyAny>, u64)> {
    if bytes.len() <= LONGEST
        && let Some(e) = table().as_ref().and_then(|t| t.map.get(bytes))
    {
        return Some((e.text.bind(py).clone(), e.len));
    }
    let len = isize::try_from(bytes.len()).ok()?;
    // SAFETY: the slice is borrowed for the call; the handler is static.
    let made = crate::direct::own(py, unsafe {
        ffi::PyUnicode_DecodeUTF8(bytes.as_ptr().cast(), len, c"surrogateescape".as_ptr())
    })?;
    let n = len_of(&made);
    keep_text(py, bytes, &made, n, None);
    Some((made, n))
}

/// `bytes`' strings as a name, `clean_name(name, ascii_digit=True)` called
/// once for the bytes while they are kept.
pub(crate) fn named<'py>(py: Python<'py>, bytes: &[u8]) -> Option<Named<'py>> {
    if bytes.len() <= LONGEST
        && let Some(e) = table().as_ref().and_then(|t| t.map.get(bytes))
        && let Some((key, key_text)) = &e.key
    {
        return Some(Named {
            len: e.len,
            key: key.bind(py).clone(),
            key_text: key_text.clone(),
        });
    }
    let (text, len) = text(py, bytes)?;
    let clean = clean(py).ok()?;
    // SAFETY: a new dict or null.
    let kwargs = crate::direct::own(py, unsafe { ffi::PyDict_New() })?;
    let flag = name!(py, c"ascii_digit").ok()?;
    // SAFETY: live objects; `True` is immortal.
    if unsafe { ffi::PyDict_SetItem(kwargs.as_ptr(), flag.as_ptr(), ffi::Py_True()) } != 0 {
        drop(PyErr::take(py));
        return None;
    }
    // SAFETY: one live item; a new tuple or null.
    let args = crate::direct::own(py, unsafe { ffi::PyTuple_Pack(1, text.as_ptr()) })?;
    // SAFETY: all live; a new reference or null with an exception set.
    let key = crate::direct::own(py, unsafe {
        ffi::PyObject_Call(clean.as_ptr(), args.as_ptr(), kwargs.as_ptr())
    })?;
    let key_text: Arc<str> = Arc::from(key.cast::<PyString>().ok()?.to_str().ok()?);
    keep_text(py, bytes, &text, len, Some((&key, &key_text)));
    Some(Named { len, key, key_text })
}

fn clean(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static CLEAN: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
    let made = CLEAN.get_or_try_init(py, || {
        let module = name!(py, c"unityrs.classes._names")?;
        import_attr(py, module, name!(py, c"clean_name")?).map(Bound::unbind)
    })?;
    Ok(made.bind(py))
}

fn keep_text(
    py: Python<'_>,
    bytes: &[u8],
    text: &Bound<'_, PyAny>,
    len: u64,
    key: Option<(&Bound<'_, PyAny>, &Arc<str>)>,
) {
    if bytes.len() > LONGEST {
        return;
    }
    let mut dropped = None;
    {
        let mut kept = table();
        let t = kept.get_or_insert_with(Table::default);
        if !t.map.contains_key(bytes) && (t.map.len() >= MOST || t.bytes + bytes.len() > TEXT) {
            dropped = Some(std::mem::take(t));
        }
        let mut added = false;
        let e = t.map.entry(bytes.into()).or_insert_with(|| {
            added = true;
            Entry {
                text: text.clone().unbind(),
                len,
                key: None,
            }
        });
        if e.key.is_none() {
            e.key = key.map(|(k, s)| (k.clone().unbind(), s.clone()));
        }
        if added {
            t.bytes += bytes.len();
        }
    }
    drop(dropped);
    let _ = py;
}

/// What a tree's keys hold of a string of `len` bytes beside the kept ones.
pub(crate) fn own_bytes(len: usize) -> u64 {
    if len > LONGEST { len as u64 } else { 0 }
}

/// One node's strings and costs, as the `_object` module has them.
pub(crate) struct NodeKeys {
    /// `clean_name(m_Name, ascii_digit=True)`.
    pub(crate) key: Py<PyAny>,
    /// `m_Type`.
    pub(crate) ty: Py<PyAny>,
    pub(crate) pptr: bool,
    /// `m_Name`'s bytes' length.
    pub(crate) bytes: u64,
    /// `build`'s cost of the node (`costs[id(node)]`).
    pub(crate) costs: u64,
}

/// A tree's object-mode keys, kept with its dict keys.
pub(crate) struct ObjKeys {
    pub(crate) ok: bool,
    pub(crate) nodes: Vec<NodeKeys>,
    /// The work of staging the tree's nodes, natively and in `_node_lists`.
    pub(crate) fixed: u64,
    /// Its strings' bytes not kept shared ([`own_bytes`]).
    long: u64,
    /// Each node's class as last found, its field count and whether the
    /// read may call it ([`crate::direct_object`]). Never held while Python
    /// runs.
    pub(crate) classes: Mutex<Vec<Option<Known>>>,
}

/// A class as last found for a node, its field count and whether it is
/// plain (`_object.plain_class`).
pub(crate) type Known = (Py<PyAny>, u64, bool);

/// The memory the full path charges for staging the tree's nodes, its
/// references' index and its open (`object_nodes`, `object_result`): the
/// read's reservation, made before the keys are.
pub(crate) fn staged(tree: &Tree) -> u64 {
    let list = tree.list();
    let mut staged: u64 = 128 + 64 + 64 + 64 + 64 + 128;
    for (at, row) in list.rows().iter().enumerate() {
        let children = u32::try_from(at).map_or(0, |at| list.children(NodeId(at)).len()) as u64;
        let text = row.ty.as_bytes().len() as u64 + row.name.as_bytes().len() as u64;
        staged = staged.saturating_add(4096 + text.saturating_mul(8) + children.saturating_mul(64));
    }
    staged
}

/// [`ObjKeys::fixed`] at most (each string's length its bytes'), known
/// before the keys are made.
pub(crate) fn fixed_most(tree: &Tree) -> u64 {
    let list = tree.list();
    let mut fixed = 4096_u64.saturating_mul(list.rows().len() as u64);
    for (at, row) in list.rows().iter().enumerate() {
        let children = u32::try_from(at).map_or(0, |at| list.children(NodeId(at)).len()) as u64;
        let text = row.ty.as_bytes().len() as u64 + row.name.as_bytes().len() as u64;
        fixed = fixed.saturating_add(4096 + text.saturating_mul(8) + children.saturating_mul(128));
    }
    fixed
}

impl ObjKeys {
    /// What the keys hold beside the shared strings, as kept.
    fn size(&self) -> u64 {
        (self.nodes.len() as u64)
            .saturating_mul(64)
            .saturating_add(self.long)
    }

    fn new(py: Python<'_>, tree: &Tree) -> Option<Self> {
        let list = tree.list();
        let rows = list.rows();
        let mut nodes = Vec::with_capacity(rows.len());
        let mut names = Vec::with_capacity(rows.len());
        let mut texts = Vec::with_capacity(rows.len());
        let mut fixed = 4096_u64.saturating_mul(rows.len() as u64);
        let mut long = 0_u64;
        for (at, row) in rows.iter().enumerate() {
            let id = NodeId(u32::try_from(at).ok()?);
            long = long
                .saturating_add(own_bytes(row.name.len()).saturating_mul(2))
                .saturating_add(own_bytes(row.ty.len()));
            let name = named(py, row.name.as_bytes())?;
            let (ty, ty_len) = text(py, row.ty.as_bytes())?;
            let children = list.children(id).len() as u64;
            let text = row.ty.as_bytes().len() as u64 + row.name.as_bytes().len() as u64;
            // `object_nodes::convert`, then `_node_lists`.
            fixed = fixed
                .saturating_add(4096 + text.saturating_mul(4) + children.saturating_mul(64))
                .saturating_add(children.saturating_mul(64) + 4 * (name.len + ty_len));
            names.push(name.len);
            texts.push(name.key_text);
            nodes.push(NodeKeys {
                key: name.key.unbind(),
                ty: ty.unbind(),
                pptr: row.ty.as_bytes().starts_with(b"PPtr<"),
                bytes: row.name.len() as u64,
                costs: 4 * ty_len,
            });
        }
        let mut ok = true;
        for (at, node) in nodes.iter_mut().enumerate() {
            for kid in list.children(NodeId(u32::try_from(at).ok()?)) {
                let n = names.get(kid.0 as usize).copied().unwrap_or(u64::MAX);
                node.costs = node.costs.saturating_add(256 + 4 * n);
            }
        }
        for at in 0..rows.len() {
            let id = NodeId(u32::try_from(at).ok()?);
            if tree.info(id).is_none_or(|info| info.shape != Shape::Class) {
                continue;
            }
            // A name two fields clean alike: the full path's.
            let mut seen = HashSet::new();
            for kid in list.children(id) {
                ok &= texts.get(kid.0 as usize).is_some_and(|t| seen.insert(&**t));
            }
        }
        Some(Self {
            ok,
            classes: Mutex::new((0..nodes.len()).map(|_| None).collect()),
            nodes,
            fixed,
            long,
        })
    }
}

/// A tree's keys, made as read and kept while the tree lives.
pub(crate) struct TreeKeys {
    tree: Weak<Tree>,
    pub(crate) ok: bool,
    /// Each key node's name and its length, made as met.
    pub(crate) names: Vec<Option<(Py<PyAny>, u64)>>,
    /// Object mode's, made at its first read of the tree.
    pub(crate) obj: Option<Arc<ObjKeys>>,
    /// What `names` holds beside the shared strings.
    pub(crate) size: u64,
}

impl TreeKeys {
    fn held(&self) -> u64 {
        let obj = self.obj.as_ref().map_or(0, |o| o.size());
        self.size
            .saturating_add((self.names.capacity() as u64).saturating_mul(16))
            .saturating_add(obj)
    }
}

static TREES: Mutex<Vec<TreeKeys>> = Mutex::new(Vec::new());

fn trees() -> MutexGuard<'static, Vec<TreeKeys>> {
    TREES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `tree`'s keys, as kept or new; [`keep`] gives them back.
pub(crate) fn keys_of(tree: &Arc<Tree>) -> TreeKeys {
    let mut dead = Vec::new();
    let found = {
        let mut kept = trees();
        let at = kept
            .iter()
            .rposition(|n| std::ptr::eq(n.tree.as_ptr(), Arc::as_ptr(tree)));
        if let Some(at) = at {
            Some(kept.remove(at))
        } else {
            // The keys of trees gone, dropped once the lock is.
            let mut at = 0;
            while let Some(n) = kept.get(at) {
                if n.tree.strong_count() == 0 {
                    dead.push(kept.remove(at));
                } else {
                    at += 1;
                }
            }
            None
        }
    };
    drop(dead);
    found.unwrap_or_else(|| TreeKeys {
        tree: Arc::downgrade(tree),
        ok: tree.list().rows().len() <= MOST_ROWS && unityrs_core::walk::emit::supported(tree),
        names: Vec::new(),
        obj: None,
        size: 0,
    })
}

/// Keeps `keys` as the last read, within the bounds (module docs).
pub(crate) fn keep(keys: TreeKeys) {
    if keys.held() > KEPT_BYTES / 4 {
        return;
    }
    let mut dropped = Vec::new();
    {
        let mut kept = trees();
        kept.push(keys);
        let mut total: u64 = kept.iter().map(TreeKeys::held).sum();
        while kept.len() > KEPT || total > KEPT_BYTES {
            let first = kept.remove(0);
            total = total.saturating_sub(first.held());
            dropped.push(first);
        }
    }
    drop(dropped);
}

/// `tree`'s object-mode keys, made now.
pub(crate) fn obj_keys(py: Python<'_>, tree: &Tree) -> Option<Arc<ObjKeys>> {
    ObjKeys::new(py, tree).map(Arc::new)
}
