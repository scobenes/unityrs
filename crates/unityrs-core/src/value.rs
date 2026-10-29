//! The dynamic value: UnityPy's `parse_as_dict()` model, made self-describing.
//!
//! Each variant is one Python type with the distinctions the canonical dump
//! keeps (`docs/CANONICAL_DUMP.md` section 3), so the encoder and the Python
//! conversion need no typetree, only the names:
//!
//! | UnityPy (Boost path) | `Value` |
//! |---|---|
//! | `int` (any width, `bool` apart) | `Int`, or `UInt` above `i64::MAX` |
//! | `float` from a `float` node | `F32` (the bit pattern) |
//! | `float` from a `double` node | `F64` (the bit pattern) |
//! | `str` (surrogate-escaped) | `Str` (the raw bytes) |
//! | `bytes` (`TypelessData`) | `Bytes` (a zero-copy slice of the file) |
//! | `list` read as one run of a primitive | `Prims` (`vector<UInt8>`/`char` as zero-copy bytes) |
//! | any other `list` (classes, strings, pairs, vectors) | `List` |
//! | `tuple` (`pair`; the pure-Python path's aligned primitive arrays) | `Tuple` |
//! | `dict` (insertion order, `PyDict_SetItem`) | `Dict` |
//!
//! Dict keys are [`NodeId`]s of the [`NodeList`] the map was read with, so a
//! field costs no allocation and no reference count. A [`Parsed`] holds those
//! lists: the root tree's first, then each ref-type tree a `ReferencedObject`
//! read through. [`Map::list`] says which list a map's keys belong to.
//!
//! Owner: the scaffold (implemented types and helpers); package B may add
//! methods and fast paths but not change these signatures.

use std::sync::Arc;

use bytes::Bytes;

use crate::raw::RawStr;
use crate::typetree::{NodeId, NodeList};

/// Index of a [`NodeList`] in [`Parsed::lists`].
pub type ListIx = u16;

/// One object's value and the node lists that name its keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Parsed {
    lists: Box<[Arc<NodeList>]>,
    /// The value.
    pub value: Value,
    /// Where the read ended, from the object's start (the trailing alignment
    /// included); compared with `byte_size` by `check_read`.
    pub bytes_read: u64,
}

/// A value read through a typetree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// `bool` (any non-zero byte is true).
    Bool(bool),
    /// Every integer that fits `i64`.
    Int(i64),
    /// A `UInt64`/`FileSize` value above `i64::MAX`.
    UInt(u64),
    /// A `float` node's bits, after [`crate::walk::FloatMode`].
    F32(u32),
    /// A `double` node's bits, every bit kept.
    F64(u64),
    /// A `string`: raw bytes, valid UTF-8 or not.
    Str(RawStr),
    /// `TypelessData`.
    Bytes(Bytes),
    /// A primitive array read as one run.
    Prims(PrimArray),
    /// Any other array.
    List(Vec<Value>),
    /// A `pair` (two items), or an aligned primitive array on the pure-Python path.
    Tuple(Box<[Value]>),
    /// A class, `PPtr`, registry or `ReferencedObject`.
    Dict(Map),
}

/// A run of primitives, decoded to native values (floats as bit patterns).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrimArray {
    /// `vector<bool>`.
    Bool(Box<[bool]>),
    /// `vector<UInt8>` and `vector<char>`: a zero-copy slice of the file.
    U8(Bytes),
    /// `vector<SInt8>`.
    I8(Box<[i8]>),
    /// `vector<UInt16>`.
    U16(Box<[u16]>),
    /// `vector<SInt16>`.
    I16(Box<[i16]>),
    /// `vector<UInt32>`.
    U32(Box<[u32]>),
    /// `vector<SInt32>`.
    I32(Box<[i32]>),
    /// `vector<UInt64>`.
    U64(Box<[u64]>),
    /// `vector<SInt64>`.
    I64(Box<[i64]>),
    /// `vector<float>`: bit patterns.
    F32(Box<[u32]>),
    /// `vector<double>`: bit patterns.
    F64(Box<[u64]>),
}

impl PrimArray {
    /// Number of elements.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Bool(v) => v.len(),
            Self::U8(v) => v.len(),
            Self::I8(v) => v.len(),
            Self::U16(v) => v.len(),
            Self::I16(v) => v.len(),
            Self::U32(v) | Self::F32(v) => v.len(),
            Self::I32(v) => v.len(),
            Self::U64(v) | Self::F64(v) => v.len(),
            Self::I64(v) => v.len(),
        }
    }

    /// Whether the array is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A dict in insertion order with `PyDict_SetItem` semantics.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Map {
    list: ListIx,
    entries: Vec<(NodeId, Value)>,
}

impl Map {
    /// An empty map whose keys name nodes of `Parsed::lists()[list]`.
    #[must_use]
    pub fn new(list: ListIx) -> Self {
        Self {
            list,
            entries: Vec::new(),
        }
    }

    /// An empty map with room for `n` entries.
    #[must_use]
    pub fn with_capacity(list: ListIx, n: usize) -> Self {
        Self {
            list,
            entries: Vec::with_capacity(n),
        }
    }

    /// Which node list names the keys.
    #[must_use]
    pub fn list(&self) -> ListIx {
        self.list
    }

    /// `PyDict_SetItem`: an existing key keeps its position and takes the new value.
    pub fn insert(&mut self, key: NodeId, value: Value) {
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => *v = value,
            None => self.entries.push((key, value)),
        }
    }

    /// Appends without looking for the key: the caller knows it is new
    /// (the node's children have unique names).
    pub fn push_unique(&mut self, key: NodeId, value: Value) {
        self.entries.push((key, value));
    }

    /// The entries in order.
    #[must_use]
    pub fn entries(&self) -> &[(NodeId, Value)] {
        &self.entries
    }

    /// The value under `key`.
    #[must_use]
    pub fn get(&self, key: NodeId) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the map is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Parsed {
    /// A value and the lists that name its keys (the root tree's first).
    #[must_use]
    pub fn new(lists: Box<[Arc<NodeList>]>, value: Value, bytes_read: u64) -> Self {
        Self {
            lists,
            value,
            bytes_read,
        }
    }

    /// The node lists; `[0]` is the tree the object was read with.
    #[must_use]
    pub fn lists(&self) -> &[Arc<NodeList>] {
        &self.lists
    }

    /// The name of `key` in `map` (empty if the map's list is unknown).
    #[must_use]
    pub fn key_name(&self, map: &Map, key: NodeId) -> &[u8] {
        self.lists
            .get(usize::from(map.list))
            .map_or(&[], |l| l.name(key))
    }

    /// A map's entries with their names.
    pub fn entries<'a>(
        &'a self,
        map: &'a Map,
    ) -> impl ExactSizeIterator<Item = (&'a [u8], &'a Value)> + 'a {
        map.entries()
            .iter()
            .map(move |(k, v)| (self.key_name(map, *k), v))
    }

    /// The value under `name` in `map`.
    #[must_use]
    pub fn field<'a>(&'a self, map: &'a Map, name: &[u8]) -> Option<&'a Value> {
        self.entries(map).find(|(n, _)| *n == name).map(|(_, v)| v)
    }

    /// Follows dict fields from the root value: `get(&[b"m_Name"])`.
    #[must_use]
    pub fn get(&self, path: &[&[u8]]) -> Option<&Value> {
        let mut v = &self.value;
        for name in path {
            let Value::Dict(m) = v else { return None };
            v = self.field(m, name)?;
        }
        Some(v)
    }
}

impl Value {
    /// The integer, if this is one (not a bool, as UnityPy's dict keeps them apart).
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            Self::UInt(u) => i64::try_from(*u).ok(),
            _ => None,
        }
    }

    /// The unsigned integer, if this is a non-negative integer.
    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Int(i) => u64::try_from(*i).ok(),
            Self::UInt(u) => Some(*u),
            _ => None,
        }
    }

    /// The float as Python holds it (an `F32` widened exactly).
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::F32(b) => Some(f64::from(f32::from_bits(*b))),
            Self::F64(b) => Some(f64::from_bits(*b)),
            _ => None,
        }
    }

    /// The bool.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The string, when it is valid UTF-8.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => s.to_str(),
            _ => None,
        }
    }

    /// Raw bytes: a string's, `TypelessData`'s, or a `vector<UInt8>`'s.
    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Str(s) => Some(s.as_bytes()),
            Self::Bytes(b) | Self::Prims(PrimArray::U8(b)) => Some(b),
            _ => None,
        }
    }

    /// The zero-copy buffer of `TypelessData` or a `vector<UInt8>`, for a
    /// hand-off to another stage without copying.
    #[must_use]
    pub fn as_shared_bytes(&self) -> Option<&Bytes> {
        match self {
            Self::Bytes(b) | Self::Prims(PrimArray::U8(b)) => Some(b),
            _ => None,
        }
    }

    /// The map, if this is a dict.
    #[must_use]
    pub fn as_dict(&self) -> Option<&Map> {
        match self {
            Self::Dict(m) => Some(m),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{Map, Parsed, Value};
    use crate::typetree::{NodeId, NodeList, NodeRow};

    #[test]
    fn value_stays_small() {
        assert!(
            std::mem::size_of::<Value>() <= 40,
            "{}",
            std::mem::size_of::<Value>()
        );
    }

    #[test]
    fn a_repeated_key_keeps_its_first_position_and_the_last_value() {
        let mut m = Map::new(0);
        m.insert(NodeId(1), Value::Int(1));
        m.insert(NodeId(2), Value::Int(2));
        m.insert(NodeId(1), Value::F32(0x4000_0000));
        assert_eq!(
            m.entries(),
            &[
                (NodeId(1), Value::F32(0x4000_0000)),
                (NodeId(2), Value::Int(2))
            ]
        );
    }

    #[test]
    fn fields_resolve_through_the_node_list() {
        let list = NodeList::from_rows(vec![
            NodeRow::new(0, "Base", "Base"),
            NodeRow::new(1, "int", "a"),
            NodeRow::new(1, "string", "m_Name"),
        ])
        .unwrap();
        let mut m = Map::new(0);
        m.push_unique(NodeId(1), Value::Int(7));
        m.push_unique(NodeId(2), Value::Str("x".into()));
        let p = Parsed::new(vec![Arc::new(list)].into(), Value::Dict(m), 12);
        assert_eq!(p.get(&[b"m_Name"]).and_then(Value::as_str), Some("x"));
        assert_eq!(p.get(&[b"a"]).and_then(Value::as_i64), Some(7));
        assert_eq!(p.get(&[b"missing"]), None);
    }
}
