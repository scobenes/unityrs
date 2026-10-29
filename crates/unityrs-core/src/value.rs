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
//! **Memory.** A [`Parsed`] carries the [`Charge`] for every allocation its
//! value holds (each list, map, tuple and primitive run, at
//! [`crate::limits::heap_cost`] of its capacity), charged to the input's
//! meter by the reader before it allocated them. Dropping the `Parsed`
//! returns them, so values a caller keeps count against the input's bound
//! for as long as they are kept. That is why `Parsed` is not `Clone`.
//!
//! Owner: the scaffold (implemented types and helpers); package B may add
//! methods and fast paths but not change these signatures.

use std::sync::Arc;

use bytes::Bytes;

use crate::limits::{Charge, Meter};
use crate::raw::RawStr;
use crate::typetree::{NodeId, NodeList};

/// Index of a [`NodeList`] in [`Parsed::lists`]: the root tree's list, then
/// one per distinct ref-type tree a `ReferencedObject` read through
/// (deduplicated by `Arc` pointer, so the count is bounded by the file's ref
/// types, which the input's meter bounds, and which can pass `u16`). A `u32`
/// costs nothing: [`Map`] stays 32 bytes.
pub type ListIx = u32;

/// One object's value, the node lists that name its keys, and the charge
/// for the value's memory (module docs).
#[derive(Debug)]
pub struct Parsed {
    lists: Box<[Arc<NodeList>]>,
    /// The value.
    pub value: Value,
    /// Where the read ended, from the object's start (the trailing alignment
    /// included); compared with `byte_size` by `check_read`. Signed: the
    /// pure-Python reader's position can end before its start (a negative
    /// `TypelessData` length moves it back; UnityPy then reports, say,
    /// "only read -96 bytes"). UnityPyBoost's is never negative.
    pub bytes_read: i64,
    charge: Charge,
}

impl PartialEq for Parsed {
    /// The value, the lists and `bytes_read`; the charge is bookkeeping.
    fn eq(&self, other: &Self) -> bool {
        self.lists == other.lists
            && self.value == other.value
            && self.bytes_read == other.bytes_read
    }
}

impl Eq for Parsed {}

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

    /// An empty map with room for `n` entries. The reader sizes a class's
    /// map once, by its distinct child names from the compiled tree
    /// (`NodeInfo::slot`), and charges it first (plan section 3.10).
    #[must_use]
    pub fn with_capacity(list: ListIx, n: usize) -> Self {
        Self {
            list,
            entries: Vec::with_capacity(n),
        }
    }

    /// A map over entries the reader grew (in a
    /// [`crate::limits::MeteredVec`], whose charge it handed to the
    /// [`Parsed`]): no copy, so the charge still covers the map. The caller
    /// guarantees the keys are distinct.
    #[must_use]
    pub fn from_entries(list: ListIx, entries: Vec<(NodeId, Value)>) -> Self {
        Self { list, entries }
    }

    /// Which node list names the keys.
    #[must_use]
    pub fn list(&self) -> ListIx {
        self.list
    }

    /// `PyDict_SetItem`: an existing key keeps its position and takes the new value.
    ///
    /// This scans the entries, so it is O(n) per call. The walker never calls
    /// it per child: a class whose children repeat a name keeps its own index
    /// from [`crate::typetree::NodeInfo::slot`] to entry position and uses
    /// [`Self::push_unique`] and [`Self::replace_at`] (plan section 3.10).
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

    /// Replaces the value at entry position `pos`, keeping its key and
    /// position (a repeated name's `PyDict_SetItem`, in O(1)). Returns the
    /// value back when `pos` is not an entry.
    pub fn replace_at(&mut self, pos: usize, value: Value) -> Result<(), Value> {
        match self.entries.get_mut(pos) {
            Some((_, v)) => {
                *v = value;
                Ok(())
            }
            None => Err(value),
        }
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
    /// A value, the lists that name its keys (the root tree's first), and
    /// the charge that paid for the value's allocations.
    #[must_use]
    pub fn new(lists: Box<[Arc<NodeList>]>, value: Value, bytes_read: i64, charge: Charge) -> Self {
        Self {
            lists,
            value,
            bytes_read,
            charge,
        }
    }

    /// A value built by a caller rather than read from an input (a value to
    /// write), on an unlimited meter.
    #[must_use]
    pub fn unmetered(lists: Box<[Arc<NodeList>]>, value: Value, bytes_read: i64) -> Self {
        Self::new(lists, value, bytes_read, Charge::zero(&Meter::unlimited()))
    }

    /// The charge for the value's memory.
    #[must_use]
    pub fn charge(&self) -> &Charge {
        &self.charge
    }

    /// The node lists; `[0]` is the tree the object was read with.
    #[must_use]
    pub fn lists(&self) -> &[Arc<NodeList>] {
        &self.lists
    }

    /// The name of `key` in `map` (empty if the map's list is unknown).
    #[must_use]
    pub fn key_name(&self, map: &Map, key: NodeId) -> &[u8] {
        usize::try_from(map.list)
            .ok()
            .and_then(|i| self.lists.get(i))
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
        assert_eq!(std::mem::size_of::<Map>(), 32);
    }

    #[test]
    fn a_map_can_name_a_list_past_u16() {
        // An object that reads through 65,536 distinct ref-type trees has 65,537
        // lists; the last one's keys must still resolve.
        let n = 65_537_usize;
        let list = Arc::new(
            NodeList::trusted(vec![
                NodeRow::new(0, "Base", "Base"),
                NodeRow::new(1, "int", "x"),
            ])
            .unwrap(),
        );
        let lists: Vec<Arc<NodeList>> = (0..n).map(|_| list.clone()).collect();
        let tail = super::ListIx::try_from(n - 1).unwrap();
        let mut inner = Map::new(tail);
        inner.push_unique(NodeId(1), Value::Int(5));
        let p = Parsed::unmetered(lists.into(), Value::Dict(inner), 4);
        let Value::Dict(m) = &p.value else { panic!() };
        assert_eq!(m.list(), tail);
        assert_eq!(p.key_name(m, NodeId(1)), b"x");
    }

    #[test]
    // The conversion keeps the test meaningful against a narrower field type.
    #[allow(clippy::useless_conversion)]
    fn bytes_read_can_be_negative() {
        // UnityPy's pure-Python reader: a TypelessData length of -100 at byte 200
        // ends the read at 104, so bytes_read is -96 (tests/fixtures: walk).
        let list = Arc::new(NodeList::trusted(vec![NodeRow::new(0, "Base", "Base")]).unwrap());
        let p = Parsed::unmetered(
            vec![list].into(),
            Value::Dict(Map::new(0)),
            (-96_i64).try_into().unwrap(),
        );
        assert_eq!(p.bytes_read, -96);
    }

    #[test]
    fn a_parsed_value_holds_its_charge_until_it_drops() {
        use crate::budget::{Counting, Reservation};
        use crate::limits::{Meter, MeteredVec};
        // A batch budget sees a value a caller keeps, and gets it back on drop.
        let budget = Arc::new(Counting::new(1 << 20));
        let meter = Meter::new(1 << 20, budget.clone(), Reservation::Input);
        let list = Arc::new(NodeList::trusted(vec![NodeRow::new(0, "int", "v")]).unwrap());
        let mut items = MeteredVec::new(&meter);
        for i in 0..100 {
            items.push(Value::Int(i)).unwrap();
        }
        let (items, charge) = items.into_parts();
        let p = Parsed::new(vec![list].into(), Value::List(items), 4, charge);
        assert!(budget.used() >= 100 * 40);
        assert_eq!(meter.used(), p.charge().bytes());
        let kept = vec![p];
        assert!(budget.used() >= 100 * 40, "retained, still booked");
        drop(kept);
        assert_eq!((meter.used(), budget.used()), (0, 0));
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
    fn replace_at_keeps_the_position_and_takes_the_value() {
        let mut m = Map::new(0);
        m.push_unique(NodeId(1), Value::Int(1));
        m.push_unique(NodeId(2), Value::Int(2));
        assert_eq!(m.replace_at(0, Value::Bool(true)), Ok(()));
        assert_eq!(
            m.entries(),
            &[(NodeId(1), Value::Bool(true)), (NodeId(2), Value::Int(2))]
        );
        assert_eq!(m.replace_at(2, Value::Int(9)), Err(Value::Int(9)));
    }

    #[test]
    fn fields_resolve_through_the_node_list() {
        let list = NodeList::trusted(vec![
            NodeRow::new(0, "Base", "Base"),
            NodeRow::new(1, "int", "a"),
            NodeRow::new(1, "string", "m_Name"),
        ])
        .unwrap();
        let mut m = Map::new(0);
        m.push_unique(NodeId(1), Value::Int(7));
        m.push_unique(NodeId(2), Value::Str("x".into()));
        let p = Parsed::unmetered(vec![Arc::new(list)].into(), Value::Dict(m), 12);
        assert_eq!(p.get(&[b"m_Name"]).and_then(Value::as_str), Some("x"));
        assert_eq!(p.get(&[b"a"]).and_then(Value::as_i64), Some(7));
        assert_eq!(p.get(&[b"missing"]), None);
    }
}
