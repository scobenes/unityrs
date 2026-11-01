//! The compiled typetree: what UnityPyBoost precomputes per node, computed
//! once per type and shared by every object of that type through `Arc`.
//!
//! UnityPyBoost classifies each node when it builds it (`TypeTreeNode_init`,
//! `UnityPyBoost/TypeTreeHelper.cpp:1215-1216`): its data type from `m_Type`
//! by exact match (`:1075-1103`, a `PPtr<` prefix first, `:1105-1128`) and its
//! align flag (`m_MetaFlag & 0x4000`). The walker never compares strings: it
//! dispatches on [`NodeInfo::shape`].
//!
//! Malformed shapes compile to [`Shape::BadPair`] and [`Shape::BadVector`],
//! which fail only when a read reaches them, as UnityPyBoost fails at read
//! time; an unreached malformed node must not fail an object. A run of
//! `pair` elements reaches its element when the array is read, before the
//! first element: UnityPyBoost's `read_pair_array` checks the arity once
//! after the length, so an *empty* array of a 3-child `pair` fails
//! (`TypeTreeHelper.cpp:356-360`), where the pure-Python reader returns `[]`.
//! Compilation itself fails only on a limit (depth).
//!
//! Owner: package B.

use std::sync::Arc;

use crate::error::{LimitError, LimitKind};
use crate::limits::{Charge, ChargeError, Limits, MeteredVec, heap_cost};
use crate::typetree::{NodeId, NodeList, TreeError};

/// UnityPyBoost's `NodeDataType`, decided from `m_Type` alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    /// `UInt8`, `char`.
    U8,
    /// `UInt16`, `unsigned short`.
    U16,
    /// `UInt32`, `unsigned int`, `Type*`.
    U32,
    /// `UInt64`, `unsigned long long`, `FileSize`.
    U64,
    /// `SInt8`.
    S8,
    /// `SInt16`, `short`.
    S16,
    /// `SInt32`, `int`.
    S32,
    /// `SInt64`, `long long`.
    S64,
    /// `float`.
    F32,
    /// `double`.
    F64,
    /// `bool`.
    Bool,
    /// `string`.
    Str,
    /// `TypelessData`.
    Bytes,
    /// `pair`.
    Pair,
    /// `Array`.
    Array,
    /// A `PPtr<` prefix: read as a class in dict mode.
    PPtr,
    /// `ReferencedObject`.
    ReferencedObject,
    /// `ReferencedObjectData`.
    ReferencedObjectData,
    /// `ManagedReferencesRegistry`.
    ManagedReferencesRegistry,
    /// Anything else: a class.
    Unknown,
}

/// UnityPyBoost's `typeToNodeDataType` (`UnityPyBoost/TypeTreeHelper.cpp:1075-1103`).
const KINDS: [(&[u8], Kind); 27] = [
    (b"SInt8", Kind::S8),
    (b"UInt8", Kind::U8),
    (b"char", Kind::U8),
    (b"short", Kind::S16),
    (b"SInt16", Kind::S16),
    (b"unsigned short", Kind::U16),
    (b"UInt16", Kind::U16),
    (b"int", Kind::S32),
    (b"SInt32", Kind::S32),
    (b"unsigned int", Kind::U32),
    (b"UInt32", Kind::U32),
    (b"Type*", Kind::U32),
    (b"long long", Kind::S64),
    (b"SInt64", Kind::S64),
    (b"unsigned long long", Kind::U64),
    (b"UInt64", Kind::U64),
    (b"FileSize", Kind::U64),
    (b"float", Kind::F32),
    (b"double", Kind::F64),
    (b"bool", Kind::Bool),
    (b"string", Kind::Str),
    (b"TypelessData", Kind::Bytes),
    (b"pair", Kind::Pair),
    (b"Array", Kind::Array),
    (b"ReferencedObject", Kind::ReferencedObject),
    (b"ReferencedObjectData", Kind::ReferencedObjectData),
    (
        b"ManagedReferencesRegistry",
        Kind::ManagedReferencesRegistry,
    ),
];

/// UnityPyBoost's table: a type name to its [`Kind`] (`get_node_data_type`,
/// `UnityPyBoost/TypeTreeHelper.cpp:1105-1128`). The name is a C string
/// there (`PyUnicode_AsUTF8`, then `strcmp`), so it ends at its first NUL; a
/// `PPtr<` prefix is tested first.
#[must_use]
pub fn kind_of(type_name: &[u8]) -> Kind {
    // UnityPy TypeTreeHelper.cpp:1112: `PyUnicode_AsUTF8` is a C string.
    let c = type_name
        .iter()
        .position(|b| *b == 0)
        .and_then(|n| type_name.get(..n))
        .unwrap_or(type_name);
    // UnityPy TypeTreeHelper.cpp:1113-1116: the `PPtr<` prefix first.
    if c.starts_with(b"PPtr<") {
        return Kind::PPtr;
    }
    // UnityPy TypeTreeHelper.cpp:1119-1125: `strcmp` against each name.
    KINDS
        .iter()
        .find(|(name, _)| *name == c)
        .map_or(Kind::Unknown, |(_, k)| *k)
}

/// The kind UnityPy's pure-Python reader and its writer dispatch on: they
/// compare `m_Type` as a whole Python `str` (`FUNCTION_READ_MAP.get`,
/// `node.m_Type == "pair"`, `TypeTreeHelper.py:190-208`, `:416-429`), with no
/// C string's end at a NUL and no `PPtr<` rule (that is object mode's).
pub(crate) fn py_kind_of(type_name: &[u8]) -> Kind {
    crate::walk::steps(type_name.len());
    if type_name.contains(&0) {
        return Kind::Unknown;
    }
    match kind_of(type_name) {
        Kind::PPtr => Kind::Unknown,
        k => k,
    }
}

impl Kind {
    /// The bytes one scalar of this kind takes, for a primitive.
    #[must_use]
    pub fn prim_size(self) -> Option<u32> {
        match self {
            Self::U8 | Self::S8 | Self::Bool => Some(1),
            Self::U16 | Self::S16 => Some(2),
            Self::U32 | Self::S32 | Self::F32 => Some(4),
            Self::U64 | Self::S64 | Self::F64 => Some(8),
            _ => None,
        }
    }

    /// Whether an array of this element kind is read as one run
    /// (`SUPPORTED_VALUE_ARRAY_READ_TYPES`, `UnityPyBoost/TypeTreeHelper.cpp:686-699`).
    #[must_use]
    pub fn reads_as_run(self) -> bool {
        self.prim_size().is_some() || self == Self::Pair
    }
}

/// What the walker does with a node (`read_typetree_value`'s switch,
/// `UnityPyBoost/TypeTreeHelper.cpp:702-884`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// A scalar of the node's [`Kind`] (integers, floats, bool).
    Prim,
    /// `string`: always aligned after, whatever its flag says.
    Str,
    /// `TypelessData`: aligned only by its own flag.
    Bytes,
    /// `pair` with exactly two children.
    Pair,
    /// `pair` without exactly two children: fails when read ("Pair node must have 2 children").
    BadPair,
    /// `ReferencedObject`.
    RefObject,
    /// A node whose [`Kind`] the reader's switch does not handle itself
    /// (`Unknown`, `PPtr`, `Array`, `ReferencedObjectData`,
    /// `ManagedReferencesRegistry`: the `default:` branch,
    /// `TypeTreeHelper.cpp:812-861`) and whose first child's kind is `Array`
    /// with two children. Every other kind takes its own shape whatever its
    /// children: an `int`, `string`, `TypelessData` or `float` node with an
    /// `Array` first child reads as a scalar, a `pair` as a pair, a
    /// `ReferencedObject` as one (probed on UnityPy 1.25.2, both readers).
    /// `elem` is `Array`'s second child; `run` when the element kind is an
    /// integer, a float, `bool` or `pair` (read as one run, `:686-699`);
    /// `array_align` when the `Array` child carries the flag.
    Vector {
        /// The element node.
        elem: NodeId,
        /// Read as one run, aligned once after.
        run: bool,
        /// The `Array` child's own align flag.
        array_align: bool,
    },
    /// A `Vector`-eligible node whose first child is `Array` without two
    /// children: fails when read ("Array node must have 2 children").
    BadVector,
    /// Everything else: children in order into a dict (`PPtr<..>`, registries,
    /// `ReferencedObjectData` outside a `ReferencedObject`, unknown types).
    Class,
}

/// Per-node facts, precomputed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent facts, no state machine
pub struct NodeInfo {
    /// The data type.
    pub kind: Kind,
    /// What reading it does.
    pub shape: Shape,
    /// `m_MetaFlag & 0x4000`.
    pub align: bool,
    /// The dict key this node's value is stored under in its parent: the first
    /// sibling with the same `m_Name` (a Python dict's collapse in O(1); the dict keeps
    /// that first position and the last value).
    pub key: NodeId,
    /// The ordinal of [`Self::key`] among the parent's distinct child names, in
    /// first-sighting order. A dict whose names may repeat (a `dup_names`
    /// class, a `ReferencedObject`) keeps, while its node is read, a table
    /// from this slot to the key's entry position, grown to each slot as its
    /// children are set, so a repeated name replaces its first entry's value
    /// in O(1) ([`crate::limits::MeteredVec::set_entry`], which keeps the new
    /// value's charge) and a registry child skipped at run time cannot
    /// misplace a later key; the writer keeps one too, for a `dup_names`
    /// class's dict (plan section 3.10, "Linear dicts").
    pub slot: u32,
    /// A lower bound on the bytes a successful read of the node consumes,
    /// in either reader and whatever registry is in scope (0 for a class
    /// that can read nothing): a `ReferencedObjectData` or
    /// `ManagedReferencesRegistry` child a read may skip adds nothing, and a
    /// malformed `pair` or `Array`, which UnityPyBoost never reads, takes
    /// what the pure-Python reader reads of it (`tests/walk_min_size.rs`).
    /// It sizes a list's first reservation (plan 3.10, "Bounds"). It is no
    /// bound where the pure-Python reader goes back (a negative
    /// `TypelessData` length) or reads a type name holding a NUL as a class,
    /// and the zero-size bound counts what each element really read instead
    /// ([`crate::limits::Limits::max_zero_size_elements`]).
    pub min_size: u32,
    /// A class whose children repeat a name, so inserts must check for an
    /// existing key; otherwise the walker pushes.
    pub dup_names: bool,
    /// A sibling has this node's `m_Name`: a read keeps only the last of
    /// their values (a dict's first position, last value), and a write
    /// writes one value through each of them. What either does under such
    /// a node counts against
    /// [`crate::limits::Limits::max_values_per_object`], which bounds the
    /// work no byte and no charge does (plan section 3.10, "Bounds").
    pub repeated: bool,
    /// A direct child is a `ManagedReferencesRegistry` (the registry scope).
    pub has_registry: bool,
}

/// A compiled typetree.
#[derive(Debug)]
pub struct Tree {
    list: Arc<NodeList>,
    info: Box<[NodeInfo]>,
    depth: u32,
    /// Some node's `m_Type` holds a NUL, so the pure-Python reader's and the
    /// writer's whole-string kind can differ from UnityPyBoost's C-string
    /// one ([`py_kind_of`]). False for every tree a parser builds (its names
    /// end at a NUL), so they never compare a string.
    nul_types: bool,
    /// Each node's [`py_kind_of`] when [`Self::nul_types`], found once here
    /// (a read or a write asks it at every visit of a node, and a type name
    /// is as long as its input makes it: review round 6's sweep); empty
    /// otherwise.
    py: Box<[Kind]>,
    /// `info` and the compile's own allocations, charged to the list's
    /// meter ([`NodeList::meter`]) before they are made.
    charge: Charge,
}

const _: () = crate::send_sync::<Tree>();
const _: () = crate::send_sync::<NodeInfo>();
const _: () = crate::send_sync::<Shape>();
const _: () = crate::send_sync::<Kind>();

/// A node the compile has not classified yet.
const BLANK: NodeInfo = NodeInfo {
    kind: Kind::Unknown,
    shape: Shape::Class,
    align: false,
    key: NodeId(0),
    slot: 0,
    min_size: 0,
    dup_names: false,
    repeated: false,
    has_registry: false,
};

/// FNV-1a over a name, to group a class's children by name in
/// `O(k log k)` plus their names' length (a repeated long name is compared
/// in full once per child, never once per comparison of the sort).
fn name_hash(name: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn as_u64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

fn id(i: usize) -> NodeId {
    // The list's length was checked against `u32` before any id is made.
    NodeId(u32::try_from(i).unwrap_or(u32::MAX))
}

/// Each row's [`py_kind_of`] when `nul_types` (else none), charged to
/// `charge`'s meter first and kept in `charge`.
fn py_kinds(
    list: &NodeList,
    nul_types: bool,
    charge: &mut Charge,
) -> Result<Box<[Kind]>, ChargeError> {
    let mut py = MeteredVec::new(charge.meter());
    if nul_types {
        py.reserve_exact(list.rows().len())?;
        for row in list.rows() {
            py.push(py_kind_of(row.ty.as_bytes()))?;
        }
    }
    let (py, c) = py.into_boxed_slice()?;
    charge.absorb(c);
    Ok(py)
}

fn info_at(info: &MeteredVec<NodeInfo>, node: NodeId) -> NodeInfo {
    info.as_slice().get(node.index()).copied().unwrap_or(BLANK)
}

/// Marks each of one class's children whose name a sibling repeats
/// ([`NodeInfo::repeated`]), the name's first child included, once
/// [`group_by_name`] has set their keys; whether any is.
fn mark_repeated(info: &mut [NodeInfo], kids: &[NodeId]) -> bool {
    let mut dup = false;
    for &c in kids {
        let key = info.get(c.index()).map_or(c, |i| i.key);
        if key != c {
            dup = true;
            for n in [c, key] {
                if let Some(i) = info.get_mut(n.index()) {
                    i.repeated = true;
                }
            }
        }
    }
    dup
}

#[cfg(test)]
std::thread_local! {
    /// Name comparisons [`group_by_name`] made on this thread (tests only).
    static COMPARED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// `a == b` on two names, counted (tests only).
fn same(a: &[u8], b: &[u8]) -> bool {
    #[cfg(test)]
    COMPARED.with(|c| c.set(c.get() + 1));
    a == b
}

/// Sets `key` and `slot` for `kids`, the children of one node: `key` is the
/// first sibling with the same `m_Name`, `slot` that key's ordinal among
/// the distinct names in first-sighting order. Sorted by (length, `hash`,
/// position), so equal names are adjacent; a run of equal length and hash
/// is checked byte for byte against its first member, which keys it when
/// every member has its name. A run that holds more than one name (a hash
/// collision, which a hostile tree can make of any number of names: FNV-1a
/// is no defence) is sorted by name, then position, and each name keyed by
/// its first member: `O(k log k)` comparisons of its names, where a scan
/// of the run's earlier keys for each member was `O(k²)` (review round 2).
fn group_by_name(
    list: &NodeList,
    kids: &[NodeId],
    order: &mut MeteredVec<u32>,
    hashes: &mut MeteredVec<(usize, u64)>,
    info: &mut [NodeInfo],
    hash: fn(&[u8]) -> u64,
) -> Result<(), ChargeError> {
    order.truncate(0);
    hashes.truncate(0);
    for (j, c) in kids.iter().enumerate() {
        let name = list.name(*c);
        hashes.push((name.len(), hash(name)))?;
        order.push(u32::try_from(j).unwrap_or(u32::MAX))?;
    }
    let hk = |j: u32| -> (usize, u64) {
        hashes
            .as_slice()
            .get(j as usize)
            .copied()
            .unwrap_or_default()
    };
    order
        .as_mut_slice()
        .sort_unstable_by(|a, b| hk(*a).cmp(&hk(*b)).then(a.cmp(b)));
    let kid = |j: u32| kids.get(j as usize).copied().unwrap_or(NodeId(0));
    let name = |j: u32| list.name(kid(j));
    let mut start = 0;
    while start < order.len() {
        let sorted = order.as_slice();
        let first = sorted.get(start).copied().unwrap_or(0);
        let mut end = start + 1;
        while end < sorted.len() && hk(sorted.get(end).copied().unwrap_or(0)) == hk(first) {
            end += 1;
        }
        let run = sorted.get(start..end).unwrap_or_default();
        let one_name = run.iter().skip(1).all(|j| same(name(*j), name(first)));
        if !one_name && let Some(run) = order.as_mut_slice().get_mut(start..end) {
            run.sort_unstable_by(|a, b| {
                #[cfg(test)]
                COMPARED.with(|c| c.set(c.get() + 1));
                name(*a).cmp(name(*b)).then(a.cmp(b))
            });
        }
        // Each member after its name's first takes that first as its key.
        let run = order.as_slice().get(start..end).unwrap_or_default();
        let mut leader = first;
        for (r, j) in run.iter().enumerate() {
            if r == 0 {
                leader = *j;
                continue;
            }
            if one_name || same(name(*j), name(leader)) {
                if let Some(i) = info.get_mut(kid(*j).index()) {
                    i.key = kid(leader);
                }
            } else {
                leader = *j;
            }
        }
        start = end;
    }
    let mut next = 0_u32;
    for c in kids {
        let key = info.get(c.index()).map_or(*c, |i| i.key);
        let slot = if key == *c {
            next = next.saturating_add(1);
            next - 1
        } else {
            info.get(key.index()).map_or(0, |i| i.slot)
        };
        if let Some(i) = info.get_mut(c.index()) {
            i.slot = slot;
        }
    }
    Ok(())
}

/// What UnityPyBoost's `read_typetree_value` does with `node`
/// (UnityPy TypeTreeHelper.cpp:702-884).
fn shape_of(list: &NodeList, info: &MeteredVec<NodeInfo>, node: NodeId) -> Shape {
    let kids = list.children(node);
    let kind = info_at(info, node).kind;
    if kind.prim_size().is_some() {
        return Shape::Prim;
    }
    match kind {
        Kind::Str => Shape::Str,
        Kind::Bytes => Shape::Bytes,
        // UnityPy TypeTreeHelper.cpp:328-332: exactly two children.
        Kind::Pair if kids.len() == 2 => Shape::Pair,
        Kind::Pair => Shape::BadPair,
        Kind::ReferencedObject => Shape::RefObject,
        // UnityPy TypeTreeHelper.cpp:812-861: the `default:` branch.
        _ => match kids.first() {
            Some(first) if info_at(info, *first).kind == Kind::Array => {
                let array = list.children(*first);
                match array {
                    // UnityPy TypeTreeHelper.cpp:820-833: `Array` needs two children.
                    [_, elem] => Shape::Vector {
                        elem: *elem,
                        run: info_at(info, *elem).kind.reads_as_run(),
                        array_align: info_at(info, *first).align,
                    },
                    _ => Shape::BadVector,
                }
            }
            _ => Shape::Class,
        },
    }
}

/// [`NodeInfo::min_size`] of `node`: the fewest bytes a successful read of
/// it consumes in either reader, whatever registry is in scope; its
/// children's are already known (they follow it in pre-order).
fn min_size_of(list: &NodeList, info: &MeteredVec<NodeInfo>, node: NodeId) -> u32 {
    let me = info_at(info, node);
    let kids = list.children(node);
    let sum = |kids: &[NodeId], skip: Option<Kind>| {
        kids.iter()
            .map(|c| info_at(info, *c))
            .filter(|c| Some(c.kind) != skip)
            .fold(0_u32, |a, c| a.saturating_add(c.min_size))
    };
    match me.shape {
        Shape::Prim => me.kind.prim_size().unwrap_or(0),
        // A length; the pure-Python reader reads a malformed `Array`'s too,
        // then its elements through `m_Children[1]` (`TypeTreeHelper.py:211-216`).
        Shape::Str | Shape::Bytes | Shape::Vector { .. } | Shape::BadVector => 4,
        Shape::Pair => sum(kids, None),
        // The pure-Python reader reads a malformed `pair`'s first two
        // children (`TypeTreeHelper.py:193-195`); UnityPyBoost never reads it.
        Shape::BadPair => sum(kids.get(..2).unwrap_or(kids), None),
        // `ReferencedObjectData` may be skipped (R14), a registry too once
        // one is in scope (R13).
        Shape::RefObject => sum(kids, Some(Kind::ReferencedObjectData)),
        Shape::Class => sum(kids, Some(Kind::ManagedReferencesRegistry)),
    }
}

impl Tree {
    /// Classifies every node. Fails when the tree is deeper than
    /// `limits.max_tree_depth`, or when the list's meter refuses the
    /// tree's memory ([`TreeError::Charge`]): `info` and any scratch grow
    /// through [`crate::limits::MeteredVec`] on [`NodeList::meter`], so a
    /// tree compiled from an input's rows is charged to that input.
    pub fn compile(list: Arc<NodeList>, limits: &Limits) -> Result<Arc<Self>, TreeError> {
        let meter = list.meter().clone();
        let n = list.len();
        if u32::try_from(n).is_err() {
            return Err(TreeError::TooManyNodes(n));
        }
        // The `Arc` the tree is kept in, charged with it (as
        // `NodeList::from_rows` charges its own).
        let mut charge = meter.charge(heap_cost(as_u64(std::mem::size_of::<Self>() + 16)))?;

        // Depth, in pre-order: a child's index is above its parent's.
        let depth = {
            let mut depth: MeteredVec<u32> = MeteredVec::with_capacity(&meter, n)?;
            for _ in 0..n {
                depth.push(0)?;
            }
            if let Some(d) = depth.as_mut_slice().first_mut() {
                *d = 1;
            }
            let mut max = 0;
            for i in 0..n {
                let d = depth.as_slice().get(i).copied().unwrap_or(0);
                if d > limits.max_tree_depth {
                    return Err(TreeError::Limit(LimitError {
                        kind: LimitKind::TreeDepth,
                        value: u64::from(d),
                        max: u64::from(limits.max_tree_depth),
                    }));
                }
                max = max.max(d);
                for c in list.children(id(i)) {
                    if let Some(cd) = depth.as_mut_slice().get_mut(c.index()) {
                        *cd = d.saturating_add(1);
                    }
                }
            }
            max
        };

        // Kinds and align flags (`TypeTreeNode_init`, UnityPy TypeTreeHelper.cpp:1215-1216).
        let mut info: MeteredVec<NodeInfo> = MeteredVec::with_capacity(&meter, n)?;
        let mut nul_types = false;
        for (i, row) in list.rows().iter().enumerate() {
            let ty = row.ty.as_bytes();
            nul_types |= ty.contains(&0);
            info.push(NodeInfo {
                kind: kind_of(ty),
                // UnityPy TypeTreeHelper.cpp:1216: `m_MetaFlag & 0x4000`.
                align: row.aligned(),
                key: id(i),
                ..BLANK
            })?;
        }

        // Dict keys: a class's children grouped by name (a Python dict
        // keeps a repeated key's first position, UnityPy TypeTreeHelper.cpp:427).
        let widest = (0..n)
            .map(|i| list.children(id(i)).len())
            .max()
            .unwrap_or(0);
        let mut order: MeteredVec<u32> = MeteredVec::with_capacity(&meter, widest)?;
        let mut hashes: MeteredVec<(usize, u64)> = MeteredVec::with_capacity(&meter, widest)?;
        for p in 0..n {
            let kids = list.children(id(p));
            if kids.is_empty() {
                continue;
            }
            group_by_name(
                &list,
                kids,
                &mut order,
                &mut hashes,
                info.as_mut_slice(),
                name_hash,
            )?;
            let dup = mark_repeated(info.as_mut_slice(), kids);
            let registry = kids
                .iter()
                .any(|c| info_at(&info, *c).kind == Kind::ManagedReferencesRegistry);
            if let Some(pi) = info.as_mut_slice().get_mut(p) {
                pi.dup_names = dup;
                pi.has_registry = registry;
            }
        }
        drop((order, hashes));

        // Shapes (`read_typetree_value`'s switch, UnityPy TypeTreeHelper.cpp:702-884).
        for p in 0..n {
            let shape = shape_of(&list, &info, id(p));
            if let Some(pi) = info.as_mut_slice().get_mut(p) {
                pi.shape = shape;
            }
        }

        // Minimum sizes, children first (reverse pre-order).
        for p in (0..n).rev() {
            let min = min_size_of(&list, &info, id(p));
            if let Some(pi) = info.as_mut_slice().get_mut(p) {
                pi.min_size = min;
            }
        }

        let py = py_kinds(&list, nul_types, &mut charge)?;
        let (info, c) = info.into_boxed_slice()?;
        charge.absorb(c);
        Ok(Arc::new(Self {
            list,
            info,
            depth,
            nul_types,
            py,
            charge,
        }))
    }

    /// A tree with no node facts, for the scaffold's own tests of code that
    /// only passes trees around (the provider cache) before package B lands.
    #[cfg(test)]
    pub(crate) fn for_tests(list: Arc<NodeList>) -> Arc<Self> {
        let charge = Charge::zero(list.meter());
        Arc::new(Self {
            list,
            info: Box::new([]),
            depth: 1,
            nul_types: false,
            py: Box::new([]),
            charge,
        })
    }

    /// The rows and structure this tree compiles.
    #[must_use]
    pub fn list(&self) -> &Arc<NodeList> {
        &self.list
    }

    /// The root node.
    #[must_use]
    pub fn root(&self) -> NodeId {
        self.list.root()
    }

    /// A node's precomputed facts.
    #[must_use]
    pub fn info(&self, id: NodeId) -> Option<&NodeInfo> {
        self.info.get(id.index())
    }

    /// The tree's depth (the root alone is 1).
    #[must_use]
    pub fn depth(&self) -> u32 {
        self.depth
    }

    /// The root's `m_Type`: the dump's `l2.root_type`.
    #[must_use]
    pub fn root_type(&self) -> &[u8] {
        self.list.type_name(self.list.root())
    }

    /// The charge for the compiled tables.
    #[must_use]
    pub fn charge(&self) -> &Charge {
        &self.charge
    }

    /// The kind UnityPy's pure-Python reader and writer give the node: its
    /// `m_Type` as a whole string ([`py_kind_of`]), which is the compiled
    /// [`Kind`] (a `PPtr<` read as any class) unless a type holds a NUL.
    pub(crate) fn py_kind(&self, node: NodeId) -> Kind {
        if self.nul_types {
            return self.py.get(node.index()).copied().unwrap_or(Kind::Unknown);
        }
        match self.info(node).map_or(Kind::Unknown, |i| i.kind) {
            Kind::PPtr => Kind::Unknown,
            k => k,
        }
    }
}

#[cfg(test)]
mod tests;
