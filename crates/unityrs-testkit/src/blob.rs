//! Typetree node lists, written in the blob format (format 10 and 12 up) or
//! the old recursive format (below 10, and 11).
//!
//! Owner: package C.

// STUB(C): delete this allow when the builders are implemented.
#![allow(clippy::todo, clippy::must_use_candidate, unused_variables)]

/// One node, as the writer lays it out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeSpec {
    /// Depth (`m_Level`); the root is 0.
    pub level: u8,
    /// `m_Type`.
    pub ty: Vec<u8>,
    /// `m_Name`.
    pub name: Vec<u8>,
    /// `m_ByteSize`.
    pub byte_size: i32,
    /// `m_Version`.
    pub version: i32,
    /// `m_TypeFlags` (`m_IsArray` in the old format).
    pub type_flags: i32,
    /// `m_MetaFlag`; `0x4000` is "align after".
    pub meta_flag: i32,
    /// `m_RefTypeHash` (blob format 19 and up).
    pub ref_type_hash: u64,
    /// `m_Index` as stored; `None` writes the row's pre-order index. The core
    /// keeps it as stored, never recomputed, which a test pins with a value
    /// that differs from the position.
    pub index: Option<i32>,
}

impl NodeSpec {
    /// A node with zero sizes and flags.
    pub fn new(level: u8, ty: &str, name: &str) -> Self {
        Self {
            level,
            ty: ty.as_bytes().to_vec(),
            name: name.as_bytes().to_vec(),
            byte_size: 0,
            version: 1,
            type_flags: 0,
            meta_flag: 0,
            ref_type_hash: 0,
            index: None,
        }
    }

    /// With `m_ByteSize`.
    #[must_use]
    pub fn size(mut self, byte_size: i32) -> Self {
        self.byte_size = byte_size;
        self
    }

    /// With the align flag.
    #[must_use]
    pub fn aligned(mut self) -> Self {
        self.meta_flag |= 0x4000;
        self
    }
}

/// How strings are stored in a blob.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Strings {
    /// Every string in the local buffer.
    #[default]
    Local,
    /// Names found in the common-string table by offset (the caller passes
    /// the table, so the testkit does not copy core's).
    Common,
}

/// A blob-format tree: node count, string-buffer size, nodes, buffer.
/// `common` is `(offset, string)` pairs used when `strings` is `Common`.
pub fn blob_tree(
    nodes: &[NodeSpec],
    format: u32,
    big_endian: bool,
    strings: Strings,
    common: &[(u32, &[u8])],
) -> Vec<u8> {
    todo!()
}

/// An old-format tree: each node then its child count, recursively.
pub fn old_tree(nodes: &[NodeSpec], format: u32, big_endian: bool) -> Vec<u8> {
    todo!()
}
