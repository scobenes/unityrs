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
#[allow(dead_code)] // The pure-Python reader's (package B's next commit).
pub(crate) fn py_kind_of(type_name: &[u8]) -> Kind {
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
    /// first-sighting order. In a `dup_names` class the walker indexes a
    /// per-read scratch by it to find the key's entry position, so a repeated
    /// name costs O(1) ([`crate::value::Map::replace_at`]) and a registry
    /// child skipped at run time cannot misplace a later key (plan section 3.10).
    pub slot: u32,
    /// A lower bound on the bytes a successful read consumes (0 for a class
    /// that can read nothing). Bounds pre-allocation and zero-size elements.
    pub min_size: u32,
    /// A class whose children repeat a name, so inserts must check for an
    /// existing key; otherwise the walker pushes.
    pub dup_names: bool,
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
    #[allow(dead_code)] // The pure-Python reader's (package B's next commit).
    nul_types: bool,
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

fn info_at(info: &MeteredVec<NodeInfo>, node: NodeId) -> NodeInfo {
    info.as_slice().get(node.index()).copied().unwrap_or(BLANK)
}

/// Sets `key` and `slot` for `kids`, the children of one node: `key` is the
/// first sibling with the same `m_Name`, `slot` that key's ordinal among
/// the distinct names in first-sighting order. Sorted by (length, hash,
/// position), so equal names are adjacent; a run of equal length and hash
/// is checked byte for byte against its first member, and a hash collision
/// falls back to a scan of the run's earlier keys.
fn group_by_name(
    list: &NodeList,
    kids: &[NodeId],
    order: &mut MeteredVec<u32>,
    hashes: &mut MeteredVec<(usize, u64)>,
    info: &mut [NodeInfo],
) -> Result<(), ChargeError> {
    order.truncate(0);
    hashes.truncate(0);
    for (j, c) in kids.iter().enumerate() {
        let name = list.name(*c);
        hashes.push((name.len(), name_hash(name)))?;
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
    let sorted = order.as_slice();
    let mut start = 0;
    while start < sorted.len() {
        let first = sorted.get(start).copied().unwrap_or(0);
        let mut end = start + 1;
        while end < sorted.len() && hk(sorted.get(end).copied().unwrap_or(0)) == hk(first) {
            end += 1;
        }
        let run = sorted.get(start..end).unwrap_or_default();
        for (r, j) in run.iter().enumerate().skip(1) {
            let me = kid(*j);
            let name = list.name(me);
            // The run's earlier keys, the first (most likely) first.
            let key = run
                .get(..r)
                .unwrap_or_default()
                .iter()
                .map(|e| kid(*e))
                .filter(|e| info.get(e.index()).is_some_and(|i| i.key == *e))
                .find(|e| list.name(*e) == name)
                .unwrap_or(me);
            if let Some(i) = info.get_mut(me.index()) {
                i.key = key;
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

/// The fewest bytes a successful UnityPyBoost read of `node` consumes; its
/// children's are already known (they follow it in pre-order).
fn min_size_of(list: &NodeList, info: &MeteredVec<NodeInfo>, node: NodeId) -> u32 {
    let me = info_at(info, node);
    let sum_except = |skip: Kind| {
        list.children(node)
            .iter()
            .map(|c| info_at(info, *c))
            .filter(|c| c.kind != skip)
            .fold(0_u32, |a, c| a.saturating_add(c.min_size))
    };
    match me.shape {
        Shape::Prim => me.kind.prim_size().unwrap_or(0),
        // A length.
        Shape::Str | Shape::Bytes | Shape::Vector { .. } => 4,
        // Never read successfully.
        Shape::BadPair | Shape::BadVector => 0,
        Shape::Pair => list
            .children(node)
            .iter()
            .fold(0_u32, |a, c| a.saturating_add(info_at(info, *c).min_size)),
        // `ReferencedObjectData` may be skipped (R14), a nested registry too (R13).
        Shape::RefObject => sum_except(Kind::ReferencedObjectData),
        Shape::Class => sum_except(Kind::ManagedReferencesRegistry),
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
            group_by_name(&list, kids, &mut order, &mut hashes, info.as_mut_slice())?;
            let dup = kids.iter().any(|c| info_at(&info, *c).key != *c);
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

        let (info, c) = info.into_boxed_slice()?;
        charge.absorb(c);
        Ok(Arc::new(Self {
            list,
            info,
            depth,
            nul_types,
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
    #[allow(dead_code)] // The pure-Python reader's (package B's next commit).
    pub(crate) fn py_kind(&self, node: NodeId) -> Kind {
        if self.nul_types {
            return py_kind_of(self.list.type_name(node));
        }
        match self.info(node).map_or(Kind::Unknown, |i| i.kind) {
            Kind::PPtr => Kind::Unknown,
            k => k,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{KINDS, Kind, Shape, Tree, kind_of, py_kind_of};
    use crate::budget::{Reservation, Unbounded};
    use crate::error::LimitKind;
    use crate::limits::{ChargeError, Limits, Meter, MeteredVec};
    use crate::typetree::{NodeId, NodeList, NodeRow, TreeError};

    fn list(rows: &[(i32, &str, &str, i32)]) -> Arc<NodeList> {
        let rows = rows
            .iter()
            .map(|(l, t, n, m)| NodeRow::new(*l, *t, *n).with_meta_flag(*m))
            .collect();
        Arc::new(NodeList::trusted(rows).unwrap())
    }

    fn compile(rows: &[(i32, &str, &str, i32)]) -> Arc<Tree> {
        Tree::compile(list(rows), &Limits::default()).unwrap()
    }

    #[test]
    fn kind_of_is_unitypyboosts_table() {
        // UnityPy TypeTreeHelper.cpp:1075-1103, each name exactly.
        assert_eq!(KINDS.len(), 27);
        for (name, kind) in KINDS {
            assert_eq!(kind_of(name), kind, "{name:?}");
            assert_eq!(py_kind_of(name), kind, "{name:?}");
        }
        assert_eq!(kind_of(b"char"), Kind::U8);
        assert_eq!(kind_of(b"Type*"), Kind::U32);
        assert_eq!(kind_of(b"FileSize"), Kind::U64);
        // The `PPtr<` prefix first; anything else is a class.
        assert_eq!(kind_of(b"PPtr<Texture2D>"), Kind::PPtr);
        assert_eq!(kind_of(b"PPtr<"), Kind::PPtr);
        assert_eq!(kind_of(b"PPtr"), Kind::Unknown);
        assert_eq!(kind_of(b"pptr<X>"), Kind::Unknown);
        assert_eq!(kind_of(b"int "), Kind::Unknown);
        assert_eq!(kind_of(b"Int"), Kind::Unknown);
        assert_eq!(kind_of(b""), Kind::Unknown);
        assert_eq!(kind_of(b"\xff"), Kind::Unknown);
        // A C string ends at its NUL (`strcmp`); Python's `str` does not.
        assert_eq!(kind_of(b"int\0x"), Kind::S32);
        assert_eq!(kind_of(b"PP\0tr<"), Kind::Unknown);
        assert_eq!(py_kind_of(b"int\0x"), Kind::Unknown);
        assert_eq!(py_kind_of(b"PPtr<X>"), Kind::Unknown);
    }

    #[test]
    fn shapes_follow_the_readers_switch() {
        let t = compile(&[
            (0, "Base", "Base", 0),
            (1, "float", "f", 0),             // 1
            (1, "string", "s", 0),            // 2: R9, its own kind first
            (2, "Array", "Array", 0),         // 3
            (3, "int", "size", 0),            // 4
            (3, "char", "data", 0),           // 5
            (1, "TypelessData", "t", 0x4000), // 6
            (1, "pair", "p", 0),              // 7
            (2, "int", "first", 0),           // 8
            (2, "int", "second", 0),          // 9
            (1, "pair", "q", 0),              // 10: one child
            (2, "int", "first", 0),           // 11
            (1, "vector", "v", 0),            // 12
            (2, "Array", "Array", 0x4000),    // 13
            (3, "int", "size", 0),            // 14
            (3, "UInt16", "data", 0),         // 15
            (1, "map", "m", 0),               // 16
            (2, "Array", "Array", 0),         // 17
            (3, "int", "size", 0),            // 18
            (3, "Foo", "data", 0),            // 19
            (1, "PPtr<X>", "bad", 0),         // 20
            (2, "Array", "Array", 0),         // 21: one child
            (3, "int", "size", 0),            // 22
            (1, "ReferencedObject", "r", 0),  // 23
            (1, "Foo", "c", 0),               // 24
        ]);
        let shape = |i: u32| t.info(NodeId(i)).unwrap().shape;
        assert_eq!(shape(0), Shape::Class);
        assert_eq!(shape(1), Shape::Prim);
        assert_eq!(shape(2), Shape::Str);
        assert_eq!(shape(6), Shape::Bytes);
        assert!(t.info(NodeId(6)).unwrap().align);
        assert_eq!(shape(7), Shape::Pair);
        assert_eq!(shape(10), Shape::BadPair);
        assert_eq!(
            shape(12),
            Shape::Vector {
                elem: NodeId(15),
                run: true,
                array_align: true
            }
        );
        assert_eq!(
            shape(16),
            Shape::Vector {
                elem: NodeId(19),
                run: false,
                array_align: false
            }
        );
        assert_eq!(shape(20), Shape::BadVector);
        assert_eq!(shape(23), Shape::RefObject);
        assert_eq!(shape(24), Shape::Class);
        assert_eq!(t.info(NodeId(99)), None);
        assert_eq!(t.depth(), 4);
        assert_eq!(t.root_type(), b"Base");
        // The pure-Python kind: a `PPtr<` is any class.
        assert_eq!(t.py_kind(NodeId(20)), Kind::Unknown);
        assert_eq!(t.py_kind(NodeId(1)), Kind::F32);
    }

    #[test]
    fn repeated_names_share_a_key_and_a_slot() {
        let t = compile(&[
            (0, "Base", "Base", 0),
            (1, "int", "a", 0),
            (1, "int", "b", 0),
            (1, "float", "a", 0),
            (1, "ManagedReferencesRegistry", "c", 0),
            (1, "int", "b", 0),
            (2, "int", "b", 0),
        ]);
        let info = |i: u32| *t.info(NodeId(i)).unwrap();
        let keys: Vec<_> = (1..=5).map(|i| info(i).key.0).collect();
        let slots: Vec<_> = (1..=5).map(|i| info(i).slot).collect();
        assert_eq!(keys, [1, 2, 1, 4, 2]);
        assert_eq!(slots, [0, 1, 0, 2, 1]);
        assert!(info(0).dup_names);
        assert!(info(0).has_registry);
        // A child's own children are keyed among themselves.
        assert_eq!((info(6).key, info(6).slot), (NodeId(6), 0));
        assert!(!info(5).dup_names);
        assert_eq!((info(0).key, info(0).slot), (NodeId(0), 0));
    }

    #[test]
    fn a_wide_class_groups_its_names_without_quadratic_work() {
        // 65,536 children whose names repeat late: key and slot as a dict.
        let mut rows = vec![NodeRow::new(0, "Base", "Base")];
        let names: Vec<String> = (0..32_768).map(|i| format!("n{i}")).collect();
        for n in names.iter().chain(names.iter().rev()) {
            rows.push(NodeRow::new(1, "int", n.as_str()));
        }
        let t = Tree::compile(
            Arc::new(NodeList::trusted(rows).unwrap()),
            &Limits::default(),
        )
        .unwrap();
        let root = t.info(NodeId(0)).unwrap();
        assert!(root.dup_names);
        let first = *t.info(NodeId(1)).unwrap();
        let last = *t.info(NodeId(65_536)).unwrap();
        assert_eq!((first.key, first.slot), (NodeId(1), 0));
        assert_eq!((last.key, last.slot), (NodeId(1), 0));
        let mid = *t.info(NodeId(32_769)).unwrap();
        assert_eq!((mid.key, mid.slot), (NodeId(32_768), 32_767));
    }

    #[test]
    fn min_sizes_bound_a_successful_read() {
        let t = compile(&[
            (0, "Base", "Base", 0),
            (1, "UInt8", "a", 0),  // 1
            (1, "double", "d", 0), // 8
            (1, "string", "s", 0), // 4
            (1, "pair", "p", 0),   // 2 + 4
            (2, "SInt16", "first", 0),
            (2, "TypelessData", "second", 0),
            (1, "vector", "v", 0), // 4
            (2, "Array", "Array", 0),
            (3, "int", "size", 0),
            (3, "double", "data", 0),
            (1, "ManagedReferencesRegistry", "r", 0), // may be skipped
            (2, "int", "version", 0),
            (1, "Empty", "e", 0), // 0
        ]);
        let min = |i: u32| t.info(NodeId(i)).unwrap().min_size;
        assert_eq!(min(1), 1);
        assert_eq!(min(2), 8);
        assert_eq!(min(4), 6);
        assert_eq!(min(7), 4);
        assert_eq!(min(11), 4);
        assert_eq!(min(13), 0);
        assert_eq!(min(0), 1 + 8 + 4 + 6 + 4);
        let r = compile(&[
            (0, "ReferencedObject", "r", 0),
            (1, "SInt64", "rid", 0),
            (1, "ReferencedObjectData", "data", 0),
            (2, "int", "x", 0),
        ]);
        assert_eq!(r.info(NodeId(0)).unwrap().min_size, 8);
    }

    #[test]
    fn a_tree_deeper_than_the_limit_does_not_compile() {
        let chain = |n: i32| {
            let rows = (0..n).map(|l| NodeRow::new(l, "Foo", "f")).collect();
            Arc::new(NodeList::trusted(rows).unwrap())
        };
        let limits = Limits {
            max_tree_depth: 8,
            ..Limits::default()
        };
        assert_eq!(Tree::compile(chain(8), &limits).unwrap().depth(), 8);
        assert_eq!(
            Tree::compile(chain(9), &limits).unwrap_err(),
            TreeError::Limit(crate::LimitError {
                kind: LimitKind::TreeDepth,
                value: 9,
                max: 8
            })
        );
        // A level jump nests one level deeper, whatever the jump.
        let jump = Arc::new(
            NodeList::trusted(vec![NodeRow::new(0, "A", "a"), NodeRow::new(50, "B", "b")]).unwrap(),
        );
        assert_eq!(Tree::compile(jump, &limits).unwrap().depth(), 2);
    }

    #[test]
    fn the_compile_is_charged_to_the_lists_meter() {
        let meter = Meter::new(1 << 20, Arc::new(Unbounded), Reservation::Input);
        let rows = MeteredVec::from_vec(
            vec![
                NodeRow::new(0, "Base", "Base"),
                NodeRow::new(1, "int", "a"),
                NodeRow::new(1, "int", "a"),
            ],
            &meter,
        )
        .unwrap();
        let list = Arc::new(NodeList::from_rows(rows).unwrap());
        let held = meter.used();
        let tree = Tree::compile(list.clone(), &Limits::default()).unwrap();
        assert_eq!(
            meter.used(),
            held + tree.charge().bytes(),
            "only the tree is kept"
        );
        assert!(tree.charge().bytes() >= 3 * std::mem::size_of::<super::NodeInfo>() as u64);
        drop(tree);
        assert_eq!(meter.used(), held);
        // A meter with no room refuses, and holds nothing for the compile.
        let fill = meter.charge(meter.limit() - meter.used()).unwrap();
        let e = Tree::compile(list.clone(), &Limits::default()).unwrap_err();
        assert!(
            matches!(e, TreeError::Charge(ChargeError::Limit(_))),
            "{e:?}"
        );
        drop(fill);
        assert_eq!(meter.used(), held);
        drop(list);
        assert_eq!(meter.used(), 0);
    }
}
