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
//! time; an unreached malformed node must not fail an object. Compilation
//! itself fails only on a limit (depth).
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

use crate::limits::Limits;
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
    /// A node whose first child's type is `Array` with two children, whatever
    /// the node's own type: `elem` is `Array`'s second child; `run` when the
    /// element kind is an integer, a float, `bool` or `pair` (read as one run,
    /// `:686-699`); `array_align` when the `Array` child carries the flag.
    Vector {
        /// The element node.
        elem: NodeId,
        /// Read as one run, aligned once after.
        run: bool,
        /// The `Array` child's own align flag.
        array_align: bool,
    },
    /// The first child is `Array` but it does not have two children: fails when read.
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
}

impl Tree {
    /// Classifies every node. Fails only when the tree is deeper than
    /// `limits.max_tree_depth`.
    pub fn compile(list: Arc<NodeList>, limits: &Limits) -> Result<Arc<Self>, TreeError> {
        todo!()
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
