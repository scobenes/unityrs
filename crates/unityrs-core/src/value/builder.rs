//! [`ValueBuilder`]: how a reader makes a [`Parsed`], every allocation
//! charged before it is made (the module docs of [`crate::value`]).
//!
//! Owner: the scaffold (implemented); package B may add methods and fast
//! paths but not change these signatures.

use std::collections::HashMap;
use std::sync::Arc;

use super::{Built, ListIx, Map, Parsed, PrimArray, PrimRun, Value};
use crate::error::{LimitError, LimitKind};
use crate::limits::{Charge, ChargeError, Meter, MeteredVec, hash_table_cost};
use crate::typetree::{NodeId, NodeList};

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
    /// Once there are more than [`SCANNED_LISTS`] lists, each one's index by
    /// its address ([`Self::list_index`]).
    by_address: Option<ListTable>,
}

const _: () = crate::send_sync::<ValueBuilder>();

/// The lists [`ValueBuilder::list_index`] finds by a scan; past them, by
/// a table.
const SCANNED_LISTS: usize = 8;

/// Each list's index by its address, charged to the builder's meter before
/// it grows ([`hash_table_cost`]).
struct ListTable {
    index: HashMap<usize, usize>,
    charge: Charge,
}

impl ListTable {
    fn cost(n: usize) -> u64 {
        hash_table_cost(n, size_of::<(usize, usize)>())
    }
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
        Ok(Self {
            lists,
            by_address: None,
        })
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
    /// (deduplicated by `Arc` pointer: one entry per distinct ref-type tree).
    /// The first eight are found by a scan, the rest by a table
    /// from each list's address, so a read of `n` references into `n` ref
    /// types finds them in steps linear in `n` (claude X, review round 2: a
    /// scan of every list made it `n`²). The lists' and the table's growth
    /// are charged; past `u32` it is `LimitKind::Count`.
    pub fn list_index(&mut self, list: &Arc<NodeList>) -> Result<ListIx, ChargeError> {
        let addr = Arc::as_ptr(list).addr();
        let found = match &self.by_address {
            Some(t) => {
                crate::walk::steps(1);
                t.index.get(&addr).copied()
            }
            None => self.lists.as_slice().iter().position(|l| {
                crate::walk::steps(1);
                Arc::ptr_eq(l, list)
            }),
        };
        let at = if let Some(at) = found {
            at
        } else {
            self.push_list(list, addr)?
        };
        ListIx::try_from(at).map_err(|_| {
            ChargeError::Limit(LimitError {
                kind: LimitKind::Count,
                value: u64::try_from(at).unwrap_or(u64::MAX),
                max: u64::from(ListIx::MAX),
            })
        })
    }

    /// Appends `list`, at `addr`, to the lists and to their table (built
    /// once they pass [`SCANNED_LISTS`]), the table charged before it
    /// grows, so a refusal leaves the two as they were.
    fn push_list(&mut self, list: &Arc<NodeList>, addr: usize) -> Result<usize, ChargeError> {
        let at = self.lists.len();
        let n = at + 1;
        match &mut self.by_address {
            Some(t) => t
                .charge
                .grow(ListTable::cost(n).saturating_sub(ListTable::cost(n - 1)))?,
            None if n > SCANNED_LISTS => {
                let charge = self.meter().charge(ListTable::cost(n))?;
                let mut index = HashMap::with_capacity(n);
                for (i, l) in self.lists.as_slice().iter().enumerate() {
                    crate::walk::steps(1);
                    index.insert(Arc::as_ptr(l).addr(), i);
                }
                self.by_address = Some(ListTable { index, charge });
            }
            None => {}
        }
        self.lists.push(list.clone())?;
        if let Some(t) = &mut self.by_address {
            t.index.insert(addr, at);
        }
        Ok(at)
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
