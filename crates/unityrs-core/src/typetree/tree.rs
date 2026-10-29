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

// STUB(B): delete this allow when the module is implemented.
#![allow(
    clippy::todo,
    clippy::unused_self,
    clippy::needless_pass_by_value,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    unused_variables,
    dead_code
)]

use std::sync::Arc;

use crate::limits::{Charge, Limits};
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

/// UnityPyBoost's table: a type name to its [`Kind`].
pub fn kind_of(type_name: &[u8]) -> Kind {
    todo!()
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
    /// `info` and the compile's own allocations, charged to the list's
    /// meter ([`NodeList::meter`]) before they are made.
    charge: Charge,
}

impl Tree {
    /// Classifies every node. Fails when the tree is deeper than
    /// `limits.max_tree_depth`, or when the list's meter refuses the
    /// tree's memory ([`TreeError::Charge`]): `info` and any scratch grow
    /// through [`crate::limits::MeteredVec`] on [`NodeList::meter`], so a
    /// tree compiled from an input's rows is charged to that input.
    pub fn compile(list: Arc<NodeList>, limits: &Limits) -> Result<Arc<Self>, TreeError> {
        todo!()
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
            charge,
        })
    }

    /// The rows and structure this tree compiles.
    pub fn list(&self) -> &Arc<NodeList> {
        &self.list
    }

    /// The root node.
    pub fn root(&self) -> NodeId {
        self.list.root()
    }

    /// A node's precomputed facts.
    pub fn info(&self, id: NodeId) -> Option<&NodeInfo> {
        self.info.get(id.index())
    }

    /// The tree's depth (the root alone is 1).
    pub fn depth(&self) -> u32 {
        self.depth
    }

    /// The root's `m_Type`: the dump's `l2.root_type`.
    pub fn root_type(&self) -> &[u8] {
        self.list.type_name(self.list.root())
    }
}
