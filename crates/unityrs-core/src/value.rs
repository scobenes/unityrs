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
//! [`crate::limits::heap_cost`] of its capacity, and the table of its node
//! lists), charged to the input's meter by the reader before it allocated
//! them. Dropping the `Parsed` returns them, so values a caller keeps count
//! against the input's bound for as long as they are kept. So the value
//! cannot leave its charge: `Parsed` is not `Clone`, its value is reached
//! by reference only ([`Parsed::value`]), and `Value` is not `Clone` either
//! (a deep copy would be an uncharged allocation the size of the input's
//! value). The reader builds every list, map, tuple and run through a
//! [`ValueBuilder`], the one metered path; `Map` has no growth method.
//!
//! ```compile_fail,E0616
//! // The value cannot be moved out of its `Parsed` (round 3: the charge
//! // dropped with the rest of the `Parsed` while the value lived on).
//! fn take(p: unityrs_core::Parsed) -> unityrs_core::Value {
//!     p.value
//! }
//! ```
//!
//! ```compile_fail,E0599
//! // Nor copied without a charge.
//! fn copy(p: &unityrs_core::Parsed) -> unityrs_core::Value {
//!     p.value().clone()
//! }
//! ```
//!
//! ```compile_fail,E0599
//! // A map does not grow outside the reader's metered builder.
//! let mut m = unityrs_core::Map::new(0);
//! m.push_unique(unityrs_core::typetree::NodeId(1), unityrs_core::Value::Int(1));
//! ```
//!
//! Owner: the scaffold (implemented types and helpers); package B may add
//! methods and fast paths but not change these signatures.

use std::sync::Arc;

use bytes::Bytes;

use crate::error::{LimitError, LimitKind};
use crate::limits::{Charge, ChargeError, Meter, MeteredVec};
use crate::raw::RawStr;
use crate::typetree::{NodeId, NodeList};

/// Index of a [`NodeList`] in [`Parsed::lists`]: the root tree's list, then
/// one per distinct ref-type tree a `ReferencedObject` read through
/// (deduplicated by `Arc` pointer, so the count is bounded by the file's ref
/// types, which the input's meter bounds, and which can pass `u16`). A `u32`
/// costs nothing: [`Map`] stays 32 bytes.
pub type ListIx = u32;

/// One object's value, the node lists that name its keys, and the charge
/// for the value's memory and the lists' table (module docs).
#[derive(Debug)]
pub struct Parsed {
    lists: Vec<Arc<NodeList>>,
    value: Value,
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

/// A value read through a typetree. Not `Clone` (module docs).
#[derive(Debug, PartialEq, Eq)]
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
#[derive(Debug, PartialEq, Eq)]
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

/// A dict in insertion order with `PyDict_SetItem` semantics. Built whole
/// ([`ValueBuilder::map`], or [`Map::from_entries`] for a value a caller
/// makes), never grown in place.
#[derive(Debug, PartialEq, Eq, Default)]
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

    /// A map a *caller* builds (a value to write, a test's), over entries
    /// with distinct keys. Not charged to anything: the reader builds maps
    /// with [`ValueBuilder::map`], which takes a metered vector.
    #[must_use]
    pub fn from_entries(list: ListIx, entries: Vec<(NodeId, Value)>) -> Self {
        Self { list, entries }
    }

    /// Which node list names the keys.
    #[must_use]
    pub fn list(&self) -> ListIx {
        self.list
    }

    /// `PyDict_SetItem` on an unmetered map, for the scaffold's tests: an
    /// existing key keeps its position and takes the new value. Not in the
    /// library: the reader grows a class's entries in a metered vector and
    /// collapses a repeated name with [`Self::replace_at`] (plan 3.10).
    #[cfg(test)]
    pub(crate) fn insert(&mut self, key: NodeId, value: Value) {
        match self.entries.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) => *v = value,
            None => self.entries.push((key, value)),
        }
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

/// Builds one object's value on its input's meter: the reader's only way to
/// make a [`Parsed`] (plan section 3.10).
///
/// Every list, map, tuple and primitive run the reader makes grows in a
/// [`MeteredVec`] from [`Self::vec`], so its capacity is charged before it
/// is allocated, and is turned into its `Value` here, which takes its
/// charge. The table of node lists ([`Parsed::lists`]) grows the same way.
/// [`Self::finish`] hands every charge to the `Parsed`, so the value's heap
/// is never above what it was charged, whatever budget the meter books with.
pub struct ValueBuilder {
    lists: MeteredVec<Arc<NodeList>>,
    charge: Charge,
}

impl std::fmt::Debug for ValueBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValueBuilder")
            .field("lists", &self.lists.len())
            .field("charged", &self.charge.bytes())
            .finish_non_exhaustive()
    }
}

impl ValueBuilder {
    /// A builder charging `meter`, whose first node list is `root` (the tree
    /// the object is read with).
    pub fn new(meter: &Arc<Meter>, root: Arc<NodeList>) -> Result<Self, ChargeError> {
        let mut lists = MeteredVec::new(meter);
        lists.push(root)?;
        Ok(Self {
            lists,
            charge: Charge::zero(meter),
        })
    }

    /// The meter every allocation of the value is charged to.
    #[must_use]
    pub fn meter(&self) -> &Arc<Meter> {
        self.charge.meter()
    }

    /// An empty vector on the meter, for a list, a map's entries, a tuple or
    /// a run.
    #[must_use]
    pub fn vec<T>(&self) -> MeteredVec<T> {
        MeteredVec::new(self.meter())
    }

    /// The index of `list` in the value's lists, appending it the first time
    /// (deduplicated by `Arc` pointer: one entry per distinct ref-type tree,
    /// found by a scan as long as UnityPy's own scan of the file's ref types).
    /// The table's growth is charged; past `u32` it is `LimitKind::Count`.
    pub fn list_index(&mut self, list: &Arc<NodeList>) -> Result<ListIx, ChargeError> {
        let found = self
            .lists
            .as_slice()
            .iter()
            .position(|l| Arc::ptr_eq(l, list));
        let at = if let Some(at) = found {
            at
        } else {
            self.lists.push(list.clone())?;
            self.lists.len() - 1
        };
        ListIx::try_from(at).map_err(|_| {
            ChargeError::Limit(LimitError {
                kind: LimitKind::Count,
                value: u64::try_from(at).unwrap_or(u64::MAX),
                max: u64::from(ListIx::MAX),
            })
        })
    }

    /// A `List` of `items`, keeping their vector's charge.
    pub fn list(&mut self, items: MeteredVec<Value>) -> Value {
        let (items, charge) = items.into_parts();
        self.charge.absorb(charge);
        Value::List(items)
    }

    /// A `Dict` whose keys name nodes of list `list`, over `entries` with
    /// distinct keys, keeping their vector's charge.
    pub fn map(&mut self, list: ListIx, entries: MeteredVec<(NodeId, Value)>) -> Value {
        let (entries, charge) = entries.into_parts();
        self.charge.absorb(charge);
        Value::Dict(Map { list, entries })
    }

    /// A boxed slice of `items` (a `Tuple`'s items, a [`PrimArray`]'s run),
    /// moved into an exact allocation that is charged first
    /// ([`MeteredVec::into_boxed_slice`]), keeping its charge.
    pub fn boxed<T>(&mut self, items: MeteredVec<T>) -> Result<Box<[T]>, ChargeError> {
        let (items, charge) = items.into_boxed_slice()?;
        self.charge.absorb(charge);
        Ok(items)
    }

    /// A `Tuple` of `items` ([`Self::boxed`]).
    pub fn tuple(&mut self, items: MeteredVec<Value>) -> Result<Value, ChargeError> {
        Ok(Value::Tuple(self.boxed(items)?))
    }

    /// The `Parsed`: `value`, where the read ended, the lists, and every
    /// charge taken.
    #[must_use]
    pub fn finish(self, value: Value, bytes_read: i64) -> Parsed {
        let (lists, lists_charge) = self.lists.into_parts();
        let mut charge = self.charge;
        charge.absorb(lists_charge);
        Parsed {
            lists,
            value,
            bytes_read,
            charge,
        }
    }
}

impl Parsed {
    /// A value built by a caller rather than read from an input (a value to
    /// write, a test's), with the lists that name its keys (the root tree's
    /// first). Nothing is charged: the reader's values come from a
    /// [`ValueBuilder`].
    #[must_use]
    pub fn unmetered(lists: Vec<Arc<NodeList>>, value: Value, bytes_read: i64) -> Self {
        Self {
            lists,
            value,
            bytes_read,
            charge: Charge::zero(&Meter::unlimited()),
        }
    }

    /// The value. By reference only: the value lives as long as its charge.
    #[must_use]
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// The charge for the value's memory and the lists' table.
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
        let inner = Map::from_entries(tail, vec![(NodeId(1), Value::Int(5))]);
        let p = Parsed::unmetered(lists, Value::Dict(inner), 4);
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
            vec![list],
            Value::Dict(Map::new(0)),
            (-96_i64).try_into().unwrap(),
        );
        assert_eq!(p.bytes_read, -96);
    }

    #[test]
    fn a_parsed_value_holds_its_charge_until_it_drops() {
        use crate::budget::{Counting, Reservation};
        use crate::limits::Meter;
        // A batch budget sees a value a caller keeps, and gets it back on drop.
        let budget = Arc::new(Counting::new(1 << 20));
        let meter = Meter::new(1 << 20, budget.clone(), Reservation::Input);
        let list = Arc::new(NodeList::trusted(vec![NodeRow::new(0, "int", "v")]).unwrap());
        let mut b = super::ValueBuilder::new(&meter, list).unwrap();
        let mut items = b.vec();
        for i in 0..100 {
            items.push(Value::Int(i)).unwrap();
        }
        let v = b.list(items);
        let p = b.finish(v, 4);
        assert!(budget.used() >= 100 * 40);
        assert_eq!(meter.used(), p.charge().bytes());
        assert_eq!(p.charge().records(), 0, "one permit, merged");
        let kept = vec![p];
        assert!(budget.used() >= 100 * 40, "retained, still booked");
        drop(kept);
        assert_eq!((meter.used(), budget.used()), (0, 0));
    }

    #[test]
    fn the_lists_table_is_charged_and_deduplicated() {
        use crate::budget::{Reservation, Unbounded};
        use crate::limits::{Meter, heap_cost};
        // Round 3 (codex HIGH): `Parsed::lists` was allocated by the caller
        // and charged by nobody.
        let meter = Meter::new(u64::MAX, Arc::new(Unbounded), Reservation::Input);
        let root = Arc::new(NodeList::trusted(vec![NodeRow::new(0, "Base", "Base")]).unwrap());
        let mut b = super::ValueBuilder::new(&meter, root).unwrap();
        let refs: Vec<_> = (0..100)
            .map(|_| Arc::new(NodeList::trusted(vec![NodeRow::new(0, "R", "R")]).unwrap()))
            .collect();
        for r in refs.iter().chain(refs.iter()) {
            b.list_index(r).unwrap();
        }
        assert_eq!(b.list_index(&refs[99]).unwrap(), 100);
        let p = b.finish(Value::Int(0), 0);
        assert_eq!(p.lists().len(), 101, "one per distinct tree");
        let table = 128 * std::mem::size_of::<Arc<NodeList>>();
        assert_eq!(meter.used(), heap_cost(u64::try_from(table).unwrap()));
        drop(p);
        assert_eq!(meter.used(), 0);
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
        let mut m = Map::from_entries(0, vec![(NodeId(1), Value::Int(1)), (NodeId(2), Value::Int(2))]);
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
        let m = Map::from_entries(
            0,
            vec![(NodeId(1), Value::Int(7)), (NodeId(2), Value::Str("x".into()))],
        );
        let p = Parsed::unmetered(vec![Arc::new(list)], Value::Dict(m), 12);
        assert_eq!(p.get(&[b"m_Name"]).and_then(Value::as_str), Some("x"));
        assert_eq!(p.get(&[b"a"]).and_then(Value::as_i64), Some(7));
        assert_eq!(p.get(&[b"missing"]), None);
    }
}
