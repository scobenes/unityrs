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

/// One container row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerRow {
    /// The container path, as stored.
    pub path: RawStr,
    /// `AssetInfo.preloadIndex`.
    pub preload_index: i32,
    /// `AssetInfo.preloadSize`.
    pub preload_size: i32,
    /// `AssetInfo.asset.m_FileID`.
    pub file_id: i32,
    /// `AssetInfo.asset.m_PathID`.
    pub path_id: i64,
}

impl ContainerRow {
    /// The row's asset pointer.
    #[must_use]
    pub fn asset(&self) -> PPtr {
        PPtr {
            file_id: self.file_id,
            path_id: self.path_id,
        }
    }
}

/// A file's container, with the charge for its tables.
#[derive(Debug)]
pub struct Container {
    rows: Box<[ContainerRow]>,
    preload_table: Option<Box<[PPtr]>>,
    charge: Charge,
}

impl PartialEq for Container {
    fn eq(&self, other: &Self) -> bool {
        self.rows == other.rows && self.preload_table == other.preload_table
    }
}

impl Eq for Container {}

impl Container {
    /// No rows, no preload table.
    #[must_use]
    pub fn empty(meter: &Arc<Meter>) -> Self {
        Self {
            rows: Box::new([]),
            preload_table: None,
            charge: Charge::zero(meter),
        }
    }

    /// The rows and preload table a read grew on the input's meter. A table
    /// with spare capacity moves into an exact allocation that is charged
    /// first ([`MeteredVec::into_boxed_slice`]).
    pub fn new(
        rows: MeteredVec<ContainerRow>,
        preload_table: Option<MeteredVec<PPtr>>,
    ) -> Result<Self, ChargeError> {
        let (rows, mut charge) = rows.into_boxed_slice()?;
        let preload_table = match preload_table {
            Some(t) => {
                let (t, c) = t.into_boxed_slice()?;
                charge.absorb(c);
                Some(t)
            }
            None => None,
        };
        Ok(Self {
            rows,
            preload_table,
            charge,
        })
    }

    /// Rows in file order.
    #[must_use]
    pub fn rows(&self) -> &[ContainerRow] {
        &self.rows
    }

    /// `m_PreloadTable`, when the AssetBundle has one.
    #[must_use]
    pub fn preload_table(&self) -> Option<&[PPtr]> {
        self.preload_table.as_deref()
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
    let list = view.list;
    // helpers/ContainerHelper.py:21-29: `container.m_PreloadTable`, then
    // `container.m_Container`, each an attribute of the object UnityPy
    // built (object mode: keyed by each child's clean name).
    let root = view.root();
    let preload = view.attr(root, b"m_PreloadTable")?;
    let items = view.attr(root, b"m_Container")?;
    let mut rows = MeteredVec::new(meter);
    let elem = view.element(items.id);
    for item in View::items(items)? {
        // `for key, value in container`: a pair.
        let Value::Tuple(kv) = item else {
            return Err(malformed("an m_Container item is not a pair"));
        };
        let (Some(Value::Str(path)), Some(info), [_, info_id, ..]) =
            (kv.first(), kv.get(1), list.children(elem))
        else {
            return Err(malformed("an m_Container item is not (str, AssetInfo)"));
        };
        let info = Node::new(*info_id, info);
        // `value.asset`, then the dump's `preloadIndex`, `preloadSize`,
        // `asset.m_FileID`, `asset.m_PathID` (tools/parity/unitypy_dump.py).
        let asset = view.attr(info, b"asset")?;
        let pptr = view.pptr(asset)?;
        rows.push(ContainerRow {
            path: path.clone(),
            preload_index: view.int(info, b"preloadIndex")?,
            preload_size: view.int(info, b"preloadSize")?,
            file_id: pptr.file_id,
            path_id: pptr.path_id,
        })?;
    }
    let mut table = MeteredVec::new(meter);
    let elem = view.element(preload.id);
    for p in View::items(preload)? {
        table.push(view.pptr(Node::new(elem, p))?)?;
    }
    Ok(Container::new(rows, Some(table))?)
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

    /// An integer attribute that fits an `i32`.
    fn int(&self, node: Node<'a>, name: &[u8]) -> Result<i32, Error> {
        self.attr(node, name)?
            .value
            .as_i64()
            .and_then(|i| i32::try_from(i).ok())
            .ok_or_else(|| malformed("an AssetInfo index is not an int32"))
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

    /// The elements of a list value (a `vector` or `Array` node's).
    fn items(node: Node<'a>) -> Result<&'a [Value], Error> {
        match node.value {
            Value::List(items) => Ok(items),
            _ => Err(malformed("not a list")),
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
            path: "a".into(),
            preload_index: 0,
            preload_size: 0,
            file_id: 0,
            path_id: i,
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
                .push(PPtr {
                    file_id: 0,
                    path_id: i,
                })
                .unwrap();
        }
        let c = Container::new(rows, Some(preload)).unwrap();
        let exact = |n: usize| heap_cost(u64::try_from(n).unwrap());
        assert_eq!(
            m.used(),
            exact(100 * std::mem::size_of::<ContainerRow>())
                + exact(100 * std::mem::size_of::<PPtr>())
        );
        assert_eq!(
            (c.rows().len(), c.preload_table().map(<[PPtr]>::len)),
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
