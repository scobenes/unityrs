//! The AssetBundle container (`files/SerializedFile.py:316-324`,
//! `classes/generated.py` `AssetBundle`, `environment.py:207-224`).
//!
//! A `SerializedFile`'s container is the `m_Container` of the first
//! AssetBundle object in dict order, as `(path, AssetInfo)` pairs in file
//! order, repeated paths kept (the dump's L1 `container` rows). Each row
//! points at its asset with `asset.m_FileID`/`m_PathID`; `preload_index` and
//! `preload_size` index `m_PreloadTable`, which the environment's container
//! preload walks (plan section 3.13).
//!
//! **What the load needs.** UnityPy's load reads only `m_PreloadTable` (kept
//! as it is), and each `m_Container` item's key and `value.asset.path_id`
//! (`helpers/ContainerHelper.py:21-29`): the asset must be a `PPtr` (a
//! `PPtr<...>` node UnityPy could build one from), and nothing else is
//! looked at until the preload or a caller does. So on a hostile tree an
//! `AssetInfo` that lacks `preloadIndex`, a key that is not a string, an
//! index of another type, a preload table that is not a list or holds
//! something that is no pointer, all load: a row keeps `None` for each
//! member UnityPy's object does not hold as a string or an `int` (the
//! dump's `null`), and the preload raises where UnityPy's would.
//!
//! **Memory.** The rows and the preload table grow in
//! [`crate::limits::MeteredVec`]s on the file's input meter, and the
//! [`Container`] keeps their charge (the AssetBundle's value, read to get
//! them, is dropped after). The tables are reached by reference only, so
//! they cannot outlive the charge:
//!
//! ```compile_fail,E0616
//! fn take(c: unityrs_core::container::Container) -> Box<[unityrs_core::container::ContainerRow]> {
//!     c.rows
//! }
//! ```
//!
//! Owner: package D. [`Container`]'s constructors are the scaffold's.

use std::sync::Arc;

use crate::env::PPtr;
use crate::error::Error;
use crate::files::SerializedFile;
use crate::limits::{Charge, ChargeError, Meter, MeteredVec};
use crate::raw::RawStr;
use crate::typetree::{NodeId, NodeList};
use crate::value::{Map, Parsed, Value};

/// One container row. Each member is what UnityPy's object holds there:
/// `None` where it holds no string (the path) or no `int` (the others: a
/// member the `AssetInfo` lacks, a `bool`, a float, a string, a class), which
/// only a hostile tree makes (module docs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerRow {
    /// The container path, as stored.
    pub path: Option<RawStr>,
    /// `AssetInfo.preloadIndex`.
    pub preload_index: Option<i128>,
    /// `AssetInfo.preloadSize`.
    pub preload_size: Option<i128>,
    /// `AssetInfo.asset.m_FileID`.
    pub file_id: Option<i128>,
    /// `AssetInfo.asset.m_PathID`.
    pub path_id: Option<i128>,
}

impl ContainerRow {
    /// The row's asset pointer, when its ids are an `int32` and an `int64`.
    #[must_use]
    pub fn asset(&self) -> Option<PPtr> {
        Some(PPtr {
            file_id: i32::try_from(self.file_id?).ok()?,
            path_id: i64::try_from(self.path_id?).ok()?,
        })
    }
}

/// `m_PreloadTable` as the load left it.
#[derive(Debug, PartialEq, Eq)]
enum Preload {
    /// No AssetBundle.
    None,
    /// A list: each element a pointer, or `None` for one that is no
    /// `PPtr` of an `int32` and an `int64`.
    List(Box<[Option<PPtr>]>),
    /// Not a list (the preload's `len()` raises).
    NotAList,
}

/// A file's container, with the charge for its tables.
#[derive(Debug)]
pub struct Container {
    rows: Box<[ContainerRow]>,
    preload: Preload,
    charge: Charge,
}

impl PartialEq for Container {
    fn eq(&self, other: &Self) -> bool {
        self.rows == other.rows && self.preload == other.preload
    }
}

impl Eq for Container {}

impl Container {
    /// No rows, no preload table.
    #[must_use]
    pub fn empty(meter: &Arc<Meter>) -> Self {
        Self {
            rows: Box::new([]),
            preload: Preload::None,
            charge: Charge::zero(meter),
        }
    }

    /// The rows and preload table a read grew on the input's meter. A table
    /// with spare capacity moves into an exact allocation that is charged
    /// first ([`MeteredVec::into_boxed_slice`]).
    pub fn new(
        rows: MeteredVec<ContainerRow>,
        preload_table: Option<MeteredVec<Option<PPtr>>>,
    ) -> Result<Self, ChargeError> {
        let (rows, mut charge) = rows.into_boxed_slice()?;
        let preload = match preload_table {
            Some(t) => {
                let (t, c) = t.into_boxed_slice()?;
                charge.absorb(c);
                Preload::List(t)
            }
            None => Preload::None,
        };
        Ok(Self {
            rows,
            preload,
            charge,
        })
    }

    /// The rows, with an `m_PreloadTable` that is not a list.
    pub fn with_preload_not_a_list(rows: MeteredVec<ContainerRow>) -> Result<Self, ChargeError> {
        let (rows, charge) = rows.into_boxed_slice()?;
        Ok(Self {
            rows,
            preload: Preload::NotAList,
            charge,
        })
    }

    /// Rows in file order.
    #[must_use]
    pub fn rows(&self) -> &[ContainerRow] {
        &self.rows
    }

    /// `m_PreloadTable`, when the AssetBundle has one: `Ok(None)` without an
    /// AssetBundle, an error when it is not a list (UnityPy's preload
    /// raises `TypeError` at its `len()`, `helpers/ContainerHelper.py:38`).
    pub fn preload_table(&self) -> Result<Option<&[Option<PPtr>]>, Error> {
        match &self.preload {
            Preload::None => Ok(None),
            Preload::List(t) => Ok(Some(t)),
            Preload::NotAList => Err(malformed("m_PreloadTable is not a list")),
        }
    }

    /// The charge for the tables.
    #[must_use]
    pub fn charge(&self) -> &Charge {
        &self.charge
    }

    /// Whether the file has no container rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Reads the container of the first AssetBundle in dict order with
/// `check_read`. A file without one has an empty container; a failing read
/// is an error (it already failed the file at load under
/// [`crate::FileContext::eager_asset_bundle`]).
pub fn read_container(file: &Arc<SerializedFile>) -> Result<Container, Error> {
    // UnityPy files/SerializedFile.py:316-324: the first AssetBundle in
    // `objects` dict order, read with `check_read`.
    let meter = file.meter();
    let Some(obj) = file.objects().find(|o| o.class_id() == ASSET_BUNDLE) else {
        return Ok(Container::empty(meter));
    };
    let parsed = obj
        .read_typetree(None, true)
        .map_err(|e| Error::AssetBundle(Box::new(e)))?;
    let Some(view) = View::new(&parsed) else {
        return Err(malformed("no node list"));
    };
    // helpers/ContainerHelper.py:21-29: `container.m_PreloadTable`, then
    // `container.m_Container`, each an attribute of the object UnityPy
    // built (object mode: keyed by each child's clean name).
    let root = view.root();
    let preload = view.attr(root, b"m_PreloadTable")?;
    let items = view.attr(root, b"m_Container")?;
    let mut rows = MeteredVec::new(meter);
    // `{key: value.asset for key, value in container}`, then
    // `{value.asset.path_id: key ...}`: each item unpacked to two, its key
    // and its asset's `path_id` hashed. Either failing fails the load.
    for item in view.sequence(items)?.nodes() {
        let Some([key, info]) = view.unpack2(item) else {
            return Err(malformed("an m_Container item is not a pair"));
        };
        let asset = view.attr(info, b"asset")?;
        let (file_id, path_id) = match view.pptr_object(asset) {
            Some((file_id, path_id)) => (Some(file_id), path_id),
            // Any other object has a `path_id` only as a child so named.
            None => (
                view.attr(asset, b"m_FileID").ok(),
                view.attr(asset, b"path_id")?,
            ),
        };
        if !view.hashable(key) || !view.hashable(path_id) {
            return Err(malformed("an m_Container key or path_id is unhashable"));
        }
        // The dump's members (tools/parity/unitypy_dump.py `container_row`).
        let path_id = match view.pptr_object(asset) {
            Some(_) => Some(path_id),
            None => view.attr(asset, b"m_PathID").ok(),
        };
        let int = |n: Option<Node<'_>>| n.and_then(|n| int(n.value));
        rows.push(ContainerRow {
            path: match key.value {
                Value::Str(s) => Some(s.clone()),
                _ => None,
            },
            preload_index: int(view.attr(info, b"preloadIndex").ok()),
            preload_size: int(view.attr(info, b"preloadSize").ok()),
            file_id: int(file_id),
            path_id: int(path_id),
        })?;
    }
    // `m_PreloadTable` is kept as it is; only the preload reads it.
    let mut table = MeteredVec::new(meter);
    match preload.value {
        Value::List(_) => {
            for p in view.sequence(preload)?.nodes() {
                table.push(view.pointer(p))?;
            }
        }
        // Numbers: no pointer among them.
        Value::Prims(p) => {
            for _ in 0..p.len() {
                table.push(None)?;
            }
        }
        _ => return Ok(Container::with_preload_not_a_list(rows)?),
    }
    Ok(Container::new(rows, Some(table))?)
}

/// A Python `int`: an integer value; not a `bool`, a float or anything else.
fn int(v: &Value) -> Option<i128> {
    match v {
        Value::Int(i) => Some(i128::from(*i)),
        Value::UInt(u) => Some(i128::from(*u)),
        _ => None,
    }
}

/// `ClassIDType.AssetBundle`.
const ASSET_BUNDLE: i32 = 142;

fn malformed(detail: &str) -> Error {
    Error::Malformed {
        what: "AssetBundle",
        detail: detail.into(),
    }
}

/// A value and the node it was read with.
#[derive(Clone, Copy)]
pub(crate) struct Node<'a> {
    id: NodeId,
    value: &'a Value,
}

impl<'a> Node<'a> {
    fn new(id: NodeId, value: &'a Value) -> Self {
        Self { id, value }
    }

    /// The value.
    pub(crate) fn value(&self) -> &'a Value {
        self.value
    }
}

/// The AssetBundle's value with its root node list, read as UnityPy's
/// object mode reads it (`UnityPyBoost/TypeTreeHelper.cpp:458-582`): a class
/// is built from keyword arguments keyed by each child's clean name, so an
/// attribute is the value of the *last* child whose name cleans to it.
/// The Environment's `MonoBehaviour` hook reads `m_Script` and a
/// `MonoScript`'s names through it too.
pub(crate) struct View<'a> {
    parsed: &'a Parsed,
    list: &'a NodeList,
}

impl<'a> View<'a> {
    /// The view of a value read with its root node list.
    pub(crate) fn new(parsed: &'a Parsed) -> Option<Self> {
        let list = parsed.lists().first()?;
        Some(Self { parsed, list })
    }

    /// The root object.
    pub(crate) fn root(&self) -> Node<'a> {
        Node::new(self.list.root(), self.parsed.value())
    }

    /// `getattr(obj, name)` on the object a class node built: the last
    /// child of `node` whose clean name is `name`, as a field of the dict
    /// the reader made (`AttributeError` when there is none).
    pub(crate) fn attr(&self, node: Node<'a>, name: &[u8]) -> Result<Node<'a>, Error> {
        let Value::Dict(map) = node.value else {
            return Err(malformed("an attribute of a value that is not a class"));
        };
        let child = self
            .list
            .children(node.id)
            .iter()
            .rev()
            .find(|c| clean_name_is(self.list.name(**c), name))
            .copied();
        let found = child.and_then(|c| self.field(map, self.list.name(c)).map(|v| (c, v)));
        let Some((id, value)) = found else {
            return Err(Error::Malformed {
                what: "AssetBundle",
                detail: format!("no attribute {}", name.escape_ascii()).into(),
            });
        };
        Ok(Node { id, value })
    }

    fn field(&self, map: &'a Map, raw_name: &[u8]) -> Option<&'a Value> {
        self.parsed.field(map, raw_name)
    }

    /// A `PPtr` value: a `PPtr<...>` node read as `{m_FileID, m_PathID}`.
    pub(crate) fn pptr(&self, node: Node<'a>) -> Result<PPtr, Error> {
        let file_id = self
            .attr(node, b"m_FileID")?
            .value
            .as_i64()
            .and_then(|i| i32::try_from(i).ok());
        let path_id = self.attr(node, b"m_PathID")?.value.as_i64();
        match (file_id, path_id) {
            (Some(file_id), Some(path_id)) => Ok(PPtr { file_id, path_id }),
            _ => Err(malformed("a pointer is not (int32, int64)")),
        }
    }

    /// What `PPtr(**kwargs)` built from a `PPtr<...>` node holds, its
    /// `m_FileID` and `m_PathID`: UnityPyBoost calls the class with the
    /// children's clean names and `assetsfile` (`TypeTreeHelper.cpp:476-501`),
    /// which takes exactly those three fields (`classes/PPtr.py:16-19`) and is
    /// slotted, so any other child or a missing id makes an `UnknownObject`
    /// (`TypeTreeHelper.cpp:505-513`). `None` for that and for every other
    /// value.
    fn pptr_object(&self, node: Node<'a>) -> Option<(Node<'a>, Node<'a>)> {
        const FIELDS: [&[u8]; 3] = [b"m_FileID", b"m_PathID", b"assetsfile"];
        if !matches!(node.value, Value::Dict(_))
            || !self.list.type_name(node.id).starts_with(b"PPtr<")
            || !self.list.children(node.id).iter().all(|&c| {
                let name = self.list.name(c);
                FIELDS.iter().any(|f| clean_name_is(name, f))
            })
        {
            return None;
        }
        Some((
            self.attr(node, b"m_FileID").ok()?,
            self.attr(node, b"m_PathID").ok()?,
        ))
    }

    /// A preload table element as a pointer: a `PPtr` whose ids are an
    /// `int32` and an `int64`, else `None`.
    fn pointer(&self, node: Node<'a>) -> Option<PPtr> {
        let (f, p) = self.pptr_object(node)?;
        Some(PPtr {
            file_id: i32::try_from(int(f.value)?).ok()?,
            path_id: i64::try_from(int(p.value)?).ok()?,
        })
    }

    /// Whether `hash()` of the value succeeds: a list never; a tuple when
    /// its items are; a `PPtr` when its ids are (`classes/PPtr.py:101-102`);
    /// any other object (an `UnknownObject` hashes by identity) and every
    /// scalar always.
    fn hashable(&self, node: Node<'a>) -> bool {
        match node.value {
            Value::List(_) | Value::Prims(_) => false,
            Value::Tuple(items) => Seq::Tuple(self.list.children(node.id), node.id, items)
                .nodes()
                .all(|n| self.hashable(n)),
            Value::Dict(_) => self
                .pptr_object(node)
                .is_none_or(|(f, p)| self.hashable(f) && self.hashable(p)),
            _ => true,
        }
    }

    /// `iter(value)`: a list's or a tuple's items. A string's, bytes' or
    /// number run's items (characters or numbers) are no pair and have no
    /// `asset`, so an empty one iterates to nothing and any other fails at
    /// its first item; every other value is not iterable (`TypeError`).
    fn sequence(&self, node: Node<'a>) -> Result<Seq<'a>, Error> {
        match node.value {
            Value::List(items) => Ok(Seq::List(self.element(node.id), items)),
            Value::Tuple(items) => Ok(Seq::Tuple(self.list.children(node.id), node.id, items)),
            Value::Prims(p) if p.is_empty() => Ok(Seq::List(node.id, &[])),
            Value::Str(s) if s.as_bytes().is_empty() => Ok(Seq::List(node.id, &[])),
            Value::Bytes(b) if b.is_empty() => Ok(Seq::List(node.id, &[])),
            Value::Prims(_) | Value::Str(_) | Value::Bytes(_) => {
                Err(malformed("an m_Container item is not a pair"))
            }
            _ => Err(malformed("not iterable")),
        }
    }

    /// `key, value = item`: a list's or a tuple's two items. Any other
    /// value either does not unpack to two or holds no `asset`.
    fn unpack2(&self, item: Node<'a>) -> Option<[Node<'a>; 2]> {
        if !matches!(item.value, Value::List(_) | Value::Tuple(_)) {
            return None;
        }
        let mut it = self.sequence(item).ok()?.nodes();
        match (it.next(), it.next(), it.next()) {
            (Some(k), Some(v), None) => Some([k, v]),
            _ => None,
        }
    }

    /// The element node of an array node: the `Array` child's data child
    /// for a vector, the data child for an `Array` itself (R9).
    fn element(&self, id: NodeId) -> NodeId {
        let array = match self.list.children(id).first() {
            Some(&a) if self.list.type_name(a) == b"Array" => a,
            _ => id,
        };
        self.list.children(array).get(1).copied().unwrap_or(array)
    }
}

/// A list's or a tuple's items, each with its node.
#[derive(Clone, Copy)]
enum Seq<'a> {
    /// A list: the element node and the items.
    List(NodeId, &'a [Value]),
    /// A tuple: its node's children, the node, and the items.
    Tuple(&'a [NodeId], NodeId, &'a [Value]),
}

impl<'a> Seq<'a> {
    /// The items, each with its node.
    fn nodes(self) -> impl Iterator<Item = Node<'a>> {
        let items = match self {
            Self::List(_, v) | Self::Tuple(_, _, v) => v,
        };
        items.iter().enumerate().map(move |(i, v)| {
            let id = match self {
                Self::List(elem, _) => elem,
                Self::Tuple(kids, id, _) => kids.get(i).copied().unwrap_or(id),
            };
            Node::new(id, v)
        })
    }
}

/// Whether Python's `clean_name(name)` (`helpers/TypeTreeNode.py:350-363`)
/// equals `target`, a plain identifier: a leading `(int&)` and one trailing
/// `?` dropped, and each of ` .:-[]` read as `_` (the `pass`/`from` and
/// leading-digit rules cannot make such a target).
fn clean_name_is(name: &[u8], target: &[u8]) -> bool {
    let name = name.strip_prefix(b"(int&)").unwrap_or(name);
    let name = name.strip_suffix(b"?").unwrap_or(name);
    name.len() == target.len()
        && name.iter().zip(target).all(|(&a, &b)| {
            let a = if b" .:-[]".contains(&a) { b'_' } else { a };
            a == b
        })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{Container, ContainerRow, clean_name_is};
    use crate::budget::{Counting, Reservation};
    use crate::env::PPtr;
    use crate::limits::{Meter, MeteredVec, heap_cost};

    fn row(i: i64) -> ContainerRow {
        ContainerRow {
            path: Some("a".into()),
            preload_index: Some(0),
            preload_size: Some(0),
            file_id: Some(0),
            path_id: Some(i128::from(i)),
        }
    }

    #[test]
    fn the_tables_are_charged_at_their_exact_size() {
        // Round 3: `into_boxed_slice` moved a table with spare capacity into
        // a new allocation nobody charged.
        let budget = Arc::new(Counting::new(u64::MAX));
        let m = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
        let mut rows = MeteredVec::new(&m);
        let mut preload = MeteredVec::new(&m);
        for i in 0..100 {
            rows.push(row(i)).unwrap();
            preload
                .push(Some(PPtr {
                    file_id: 0,
                    path_id: i,
                }))
                .unwrap();
        }
        let c = Container::new(rows, Some(preload)).unwrap();
        let exact = |n: usize| heap_cost(u64::try_from(n).unwrap());
        assert_eq!(
            m.used(),
            exact(100 * std::mem::size_of::<ContainerRow>())
                + exact(100 * std::mem::size_of::<Option<PPtr>>())
        );
        assert_eq!(
            (
                c.rows().len(),
                c.preload_table().unwrap().map(<[Option<PPtr>]>::len)
            ),
            (100, Some(100))
        );
        assert_eq!(c.charge().records(), 0);
        drop(c);
        assert_eq!((m.used(), budget.used()), (0, 0));
    }

    #[test]
    fn clean_names_match_as_python_cleans_them() {
        assert!(clean_name_is(b"m_Container", b"m_Container"));
        assert!(clean_name_is(b"m Container", b"m_Container"));
        assert!(clean_name_is(b"(int&)m.Container?", b"m_Container"));
        assert!(clean_name_is(b"m-Container", b"m_Container"));
        assert!(!clean_name_is(b"m_Container??", b"m_Container"));
        assert!(!clean_name_is(b"m_container", b"m_Container"));
        assert!(!clean_name_is(b"", b"asset"));
    }
}
