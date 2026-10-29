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
//! **A built value travels with its charge.** The builder hands each list,
//! map, tuple and run back as a [`Built`]: the value and the charge for its
//! heap, which no public method separates. A `Built` goes into its parent's
//! metered vector ([`MeteredVec::push_built`], [`MeteredVec::push_entry`],
//! [`MeteredVec::set_entry`]), which takes its charge along, or into the
//! `Parsed` ([`ValueBuilder::finish`]); dropped anywhere, its value and its
//! charge go together. So no value the builder made can outlive its charge,
//! whenever the builder itself is dropped (round 5: the builder kept every
//! charge until `finish`, and a value taken before a builder was dropped
//! lived on uncharged).
//!
//! **What the types pin, and what a test pins.** A `Parsed` has no public
//! constructor: [`ValueBuilder::finish`] is the only way to one, so every
//! `Parsed` holds the charges of what it holds. A caller's own value (one to
//! write, a test's) goes through a builder too, on [`Meter::unlimited`] when
//! it is not an input's; [`ValueBuilder::scalar`] wraps a value that owns no
//! heap, and refuses a list, map, tuple or run that owns an allocation. It
//! cannot refuse a `Str` or `Bytes` that owns a plain `Vec`, since a
//! `bytes::Bytes` does not say whether it is a slice of a charged buffer
//! (the file's, [`crate::RawStr::charged`], a decoded block's) or owns
//! memory no meter saw, so a string or bytes passed to it must be such a
//! slice (round 6). `Value`'s variants are public data,
//! though, so the types cannot prove that a value pushed into a vector as a
//! plain `Value` owns no uncharged heap (a `List` made from a plain `Vec`).
//! That the reader never does that is its discipline, checked by package
//! B's counting-allocator test (plan section 7.3), as `tests/memory_model.rs`
//! checks the builder itself.
//!
//! The snippets below must not compile. Stable rustdoc does not check a
//! `compile_fail` test's error code, so each is written to fail one way
//! only: the item it names is missing, private or not `Clone`.
//!
//! ```compile_fail,E0616
//! // The value cannot be moved out of its `Parsed` (round 3: the charge
//! // dropped with the rest of the `Parsed` while the value lived on).
//! fn take(p: unityrs_core::Parsed) -> unityrs_core::Value {
//!     p.value
//! }
//! ```
//!
//! ```compile_fail,E0277
//! // Nor copied without a charge: `Value` is not `Clone`.
//! fn clone<T: Clone>() {}
//! clone::<unityrs_core::Value>();
//! ```
//!
//! ```compile_fail,E0277
//! fn clone<T: Clone>() {}
//! clone::<unityrs_core::Parsed>();
//! ```
//!
//! ```compile_fail,E0277
//! fn clone<T: Clone>() {}
//! clone::<unityrs_core::Map>();
//! ```
//!
//! ```compile_fail,E0277
//! fn clone<T: Clone>() {}
//! clone::<unityrs_core::PrimArray>();
//! ```
//!
//! ```compile_fail,E0599
//! // A map does not grow outside the reader's metered builder.
//! let _ = unityrs_core::Map::push_unique;
//! ```
//!
//! ```compile_fail,E0599
//! // Not by insertion either (a test-only helper inside the crate).
//! let _ = unityrs_core::Map::insert;
//! ```
//!
//! ```compile_fail,E0599
//! // Nor is one allocated at an unmetered capacity.
//! let _ = unityrs_core::Map::with_capacity;
//! ```
//!
//! ```compile_fail,E0599
//! // A `Parsed` is not assembled from charges the caller picked, with any
//! // arguments: the reader finishes a `ValueBuilder`.
//! let _ = unityrs_core::Parsed::new;
//! ```
//!
//! ```compile_fail,E0599
//! // Nor without a charge (round 4): a caller's value goes through a
//! // builder on `Meter::unlimited()`.
//! let _ = unityrs_core::Parsed::unmetered;
//! ```
//!
//! ```compile_fail,E0616
//! // A built value does not leave its charge (round 5): its fields are
//! // private, and no method moves the value out.
//! fn take(b: unityrs_core::value::Built) -> unityrs_core::Value {
//!     b.value
//! }
//! ```
//!
//! ```compile_fail,E0599
//! let _ = unityrs_core::value::Built::into_value;
//! ```
//!
//! ```
//! // What the snippets above refuse, a caller does this way.
//! use std::sync::Arc;
//! use unityrs_core::limits::Meter;
//! use unityrs_core::typetree::{NodeList, NodeRow};
//! use unityrs_core::{Value, ValueBuilder};
//! let root = Arc::new(NodeList::trusted(vec![NodeRow::new(0, "int", "v")]).unwrap());
//! let b = ValueBuilder::new(&Meter::unlimited(), root).unwrap();
//! let mut items = b.vec();
//! items.push(Value::Int(1)).unwrap();
//! let list = b.list(items);
//! let p = b.finish(list, 4);
//! assert_eq!(p.value(), &Value::List(vec![Value::Int(1)]));
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

/// A list, map, tuple or primitive run a [`ValueBuilder`] made, with the
/// charge for its heap (module docs). No public method separates the two:
/// the value goes on only into a parent's metered vector
/// ([`MeteredVec::push_built`], [`MeteredVec::push_entry`],
/// [`MeteredVec::set_entry`]) or into the `Parsed` ([`ValueBuilder::finish`]),
/// each of which takes the charge along, and dropped anywhere the two go
/// together.
#[derive(Debug)]
pub struct Built {
    value: Value,
    charge: Charge,
}

impl Built {
    /// The value, by reference.
    #[must_use]
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// The charge for the value's heap.
    #[must_use]
    pub fn charge(&self) -> &Charge {
        &self.charge
    }
}

impl MeteredVec<Value> {
    /// Appends a built value (a list's or a tuple's item); this vector keeps
    /// its charge from then on. A refused growth drops both.
    pub fn push_built(&mut self, item: Built) -> Result<(), ChargeError> {
        self.push_charged(item.value, item.charge)
    }
}

impl MeteredVec<(NodeId, Value)> {
    /// Appends a dict entry whose value was built; this vector keeps its
    /// charge from then on. A refused growth drops both.
    pub fn push_entry(&mut self, key: NodeId, item: Built) -> Result<(), ChargeError> {
        self.push_charged((key, item.value), item.charge)
    }

    /// Replaces the value of entry `pos` with a built one, keeping its key
    /// and position (a repeated name's `PyDict_SetItem`, plan 3.10's linear
    /// dict), and keeps its charge. The value replaced is dropped; its charge
    /// stays with this vector until it drops (charged, never under). False,
    /// with `item` dropped whole, when `pos` is not an entry.
    pub fn set_entry(&mut self, pos: usize, item: Built) -> bool {
        let Built { value, charge } = item;
        match self.as_mut_slice().get_mut(pos) {
            Some((_, v)) => {
                *v = value;
                self.keep_charge(charge);
                true
            }
            None => false,
        }
    }
}

/// A primitive run, as the reader collects it, for [`ValueBuilder::prims`].
#[derive(Debug)]
pub enum PrimRun {
    /// `vector<bool>`.
    Bool(MeteredVec<bool>),
    /// `vector<SInt8>`.
    I8(MeteredVec<i8>),
    /// `vector<UInt16>`.
    U16(MeteredVec<u16>),
    /// `vector<SInt16>`.
    I16(MeteredVec<i16>),
    /// `vector<UInt32>`.
    U32(MeteredVec<u32>),
    /// `vector<SInt32>`.
    I32(MeteredVec<i32>),
    /// `vector<UInt64>`.
    U64(MeteredVec<u64>),
    /// `vector<SInt64>`.
    I64(MeteredVec<i64>),
    /// `vector<float>`: bit patterns.
    F32(MeteredVec<u32>),
    /// `vector<double>`: bit patterns.
    F64(MeteredVec<u64>),
}

/// Builds one object's value on its input's meter: the reader's only way to
/// make a [`Parsed`] (plan section 3.10).
///
/// Every list, map, tuple and primitive run the reader makes grows in a
/// [`MeteredVec`] from [`Self::vec`], so its capacity is charged before it
/// is allocated, and is turned into a [`Built`] here, which carries its
/// charge into its parent and on into the `Parsed`. The table of node lists
/// ([`Parsed::lists`]) grows the same way. The builder keeps no value's
/// charge, so dropping it (a read that failed) leaves nothing uncharged.
pub struct ValueBuilder {
    lists: MeteredVec<Arc<NodeList>>,
}

impl std::fmt::Debug for ValueBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValueBuilder")
            .field("lists", &self.lists.len())
            .finish_non_exhaustive()
    }
}

/// Whether `value` itself owns heap (its children aside): a list, map,
/// tuple or run with an allocation.
fn owns_heap(value: &Value) -> bool {
    match value {
        Value::List(v) => v.capacity() > 0,
        Value::Tuple(v) => !v.is_empty(),
        Value::Dict(m) => m.entries.capacity() > 0,
        Value::Prims(PrimArray::U8(_)) => false,
        Value::Prims(p) => !p.is_empty(),
        _ => false,
    }
}

impl ValueBuilder {
    /// A builder charging `meter`, whose first node list is `root` (the tree
    /// the object is read with).
    pub fn new(meter: &Arc<Meter>, root: Arc<NodeList>) -> Result<Self, ChargeError> {
        let mut lists = MeteredVec::new(meter);
        lists.push(root)?;
        Ok(Self { lists })
    }

    /// The meter every allocation of the value is charged to.
    #[must_use]
    pub fn meter(&self) -> &Arc<Meter> {
        self.lists.charge().meter()
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

    /// A value that owns no heap of its own (a scalar, a string or bytes
    /// sliced from a charged buffer, an empty list or map), as a [`Built`]
    /// with an empty charge; a list, map, tuple or run that owns an
    /// allocation is handed back, since only the builder's own methods know
    /// what it was charged. A `Str` or `Bytes` is taken as it is: it must be
    /// a slice of a charged buffer (the file's, [`crate::RawStr::charged`],
    /// a decoded block's), never one made from a `Vec` no meter charged,
    /// which this cannot see (module docs).
    pub fn scalar(&self, value: Value) -> Result<Built, Value> {
        if owns_heap(&value) {
            return Err(value);
        }
        Ok(Built {
            value,
            charge: Charge::zero(self.meter()),
        })
    }

    /// A `List` of `items`, with their vector's charge.
    #[must_use]
    pub fn list(&self, items: MeteredVec<Value>) -> Built {
        let (items, charge) = items.into_parts();
        Built {
            value: Value::List(items),
            charge,
        }
    }

    /// A `Dict` whose keys name nodes of list `list`, over `entries` with
    /// distinct keys, with their vector's charge.
    #[must_use]
    pub fn map(&self, list: ListIx, entries: MeteredVec<(NodeId, Value)>) -> Built {
        let (entries, charge) = entries.into_parts();
        Built {
            value: Value::Dict(Map { list, entries }),
            charge,
        }
    }

    /// A `Tuple` of `items`, moved into an exact allocation that is charged
    /// first ([`MeteredVec::into_boxed_slice`]), with its charge.
    pub fn tuple(&self, items: MeteredVec<Value>) -> Result<Built, ChargeError> {
        let (items, charge) = items.into_boxed_slice()?;
        Ok(Built {
            value: Value::Tuple(items),
            charge,
        })
    }

    /// A `Prims` value of `run`, moved into an exact allocation that is
    /// charged first, with its charge. (`vector<UInt8>`/`char` is a
    /// zero-copy slice of the file, [`PrimArray::U8`], through
    /// [`Self::scalar`].)
    pub fn prims(&self, run: PrimRun) -> Result<Built, ChargeError> {
        fn boxed<T>(
            items: MeteredVec<T>,
            wrap: fn(Box<[T]>) -> PrimArray,
        ) -> Result<Built, ChargeError> {
            let (items, charge) = items.into_boxed_slice()?;
            Ok(Built {
                value: Value::Prims(wrap(items)),
                charge,
            })
        }
        match run {
            PrimRun::Bool(v) => boxed(v, PrimArray::Bool),
            PrimRun::I8(v) => boxed(v, PrimArray::I8),
            PrimRun::U16(v) => boxed(v, PrimArray::U16),
            PrimRun::I16(v) => boxed(v, PrimArray::I16),
            PrimRun::U32(v) => boxed(v, PrimArray::U32),
            PrimRun::I32(v) => boxed(v, PrimArray::I32),
            PrimRun::U64(v) => boxed(v, PrimArray::U64),
            PrimRun::I64(v) => boxed(v, PrimArray::I64),
            PrimRun::F32(v) => boxed(v, PrimArray::F32),
            PrimRun::F64(v) => boxed(v, PrimArray::F64),
        }
    }

    /// The `Parsed`: `value`, where the read ended, the lists, and the
    /// charges of both.
    #[must_use]
    pub fn finish(self, value: Built, bytes_read: i64) -> Parsed {
        let (lists, lists_charge) = self.lists.into_parts();
        let Built { value, mut charge } = value;
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
    /// A value with lists the crate's tests make (65,537 copies of one list,
    /// which a builder would deduplicate), charging nothing.
    #[cfg(test)]
    pub(crate) fn for_tests(lists: Vec<Arc<NodeList>>, value: Value, bytes_read: i64) -> Self {
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
mod tests;
