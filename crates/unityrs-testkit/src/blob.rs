//! Typetree node lists, written in the blob format (format 10 and 12 up) or
//! the old recursive format (below 10, and 11).
//!
//! The layouts are the ones UnityPy 1.25.2 reads: the blob is
//! `TypeTreeNode.parse_blob` (`helpers/TypeTreeNode.py:111-154`, rows
//! `hBBIIiii` plus `Q` from format 19, `:331-347`) and the old format is
//! `TypeTreeNode.parse` (`:78-109`: each node's fields, then its child count,
//! children following in pre-order). Rows are written in the file's byte
//! order.
//!
//! Owner: package C.

use std::collections::HashMap;

use crate::writer::ByteWriter;

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
    #[must_use]
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
/// `common` is `(offset, string)` pairs used when `strings` is `Common`: a
/// type or name equal to one of them is written as its offset with the high
/// bit set, and every other string goes into the local buffer. Each distinct
/// local string is stored once, in first-use order (type, then name, row by
/// row).
#[must_use]
pub fn blob_tree(
    nodes: &[NodeSpec],
    format: u32,
    big_endian: bool,
    strings: Strings,
    common: &[(u32, &[u8])],
) -> Vec<u8> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut local: HashMap<Vec<u8>, u32> = HashMap::new();
    let mut offset = |s: &[u8]| -> u32 {
        if strings == Strings::Common
            && let Some((o, _)) = common.iter().find(|(_, c)| *c == s)
        {
            return o | 0x8000_0000;
        }
        if let Some(o) = local.get(s) {
            return *o;
        }
        let o = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        buffer.extend_from_slice(s);
        buffer.push(0);
        local.insert(s.to_vec(), o);
        o
    };
    let offsets: Vec<(u32, u32)> = nodes
        .iter()
        .map(|n| {
            let t = offset(&n.ty);
            (t, offset(&n.name))
        })
        .collect();

    let mut w = writer(big_endian);
    w.i32(len_i32(nodes.len()));
    w.i32(len_i32(buffer.len()));
    for (i, (n, (ty, name))) in nodes.iter().zip(offsets).enumerate() {
        // `hBBIIiii`: version, level, type flags, the two offsets, byte size,
        // index, meta flag; then `Q` from format 19.
        w.i16(truncate_i16(n.version));
        w.u8(n.level);
        w.u8(n.type_flags.to_le_bytes()[0]);
        w.u32(ty);
        w.u32(name);
        w.i32(n.byte_size);
        w.i32(n.index.unwrap_or_else(|| len_i32(i)));
        w.i32(n.meta_flag);
        if format >= 19 {
            w.u64(n.ref_type_hash);
        }
    }
    w.bytes(&buffer);
    w.into_inner()
}

/// An old-format tree: each node then its child count, recursively (in
/// pre-order). The format decides the fields, as `TypeTreeNode.parse` reads
/// them: `m_VariableCount` (written 0) only at format 2, `m_Index` and
/// `m_MetaFlag` except at format 3. A node's children are the rows UnityPy's
/// stack rule nests directly under it (`TypeTreeNode.from_list`), so a level
/// jump nests under the previous row as the blob reader would nest it.
#[must_use]
pub fn old_tree(nodes: &[NodeSpec], format: u32, big_endian: bool) -> Vec<u8> {
    let parents = parents(nodes);
    let mut w = writer(big_endian);
    for (i, n) in nodes.iter().enumerate() {
        w.cstr(&n.ty);
        w.cstr(&n.name);
        w.i32(n.byte_size);
        if format == 2 {
            w.i32(0);
        }
        if format != 3 {
            w.i32(n.index.unwrap_or_else(|| len_i32(i)));
        }
        w.i32(n.type_flags);
        w.i32(n.version);
        if format != 3 {
            w.i32(n.meta_flag);
        }
        let children = parents.iter().filter(|p| **p == Some(i)).count();
        w.i32(len_i32(children));
    }
    w.into_inner()
}

/// Each row's parent under UnityPy's stack rule (`TypeTreeNode.from_list`):
/// a row above the previous row's level nests under the previous row, a row
/// at its level is its sibling, and a row below it pops to the nearest
/// ancestor of lower level. `None` for a top-level row.
fn parents(nodes: &[NodeSpec]) -> Vec<Option<usize>> {
    let mut out = Vec::with_capacity(nodes.len());
    // The current parent and the parents above it, the hidden root at -1.
    let root: (Option<usize>, i32) = (None, -1);
    let mut stack: Vec<(Option<usize>, i32)> = Vec::new();
    let mut parent = root;
    let mut prev = root;
    for (i, n) in nodes.iter().enumerate() {
        let level = i32::from(n.level);
        if level > prev.1 {
            stack.push(parent);
            parent = prev;
        } else if level < prev.1 {
            while level <= parent.1 {
                parent = stack.pop().unwrap_or(root);
                if parent == root {
                    break;
                }
            }
        }
        out.push(parent.0);
        prev = (Some(i), level);
    }
    out
}

fn writer(big_endian: bool) -> ByteWriter {
    if big_endian {
        ByteWriter::be()
    } else {
        ByteWriter::le()
    }
}

fn len_i32(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

fn truncate_i16(v: i32) -> i16 {
    let b = v.to_le_bytes();
    i16::from_le_bytes([b[0], b[1]])
}

#[cfg(test)]
mod tests {
    use super::{NodeSpec, Strings, blob_tree, old_tree, parents};

    fn tree() -> Vec<NodeSpec> {
        vec![
            NodeSpec::new(0, "Base", "Base"),
            NodeSpec::new(1, "int", "a").size(4),
            NodeSpec::new(1, "Sub", "s"),
            NodeSpec::new(2, "UInt8", "b").size(1).aligned(),
            NodeSpec::new(1, "int", "c").size(4),
        ]
    }

    #[test]
    fn a_blob_is_counts_rows_and_a_deduplicated_buffer() {
        let b = blob_tree(&tree(), 17, false, Strings::Local, &[]);
        assert_eq!(&b[..4], &5i32.to_le_bytes());
        let buf = b"Base\0int\0a\0Sub\0s\0UInt8\0b\0c\0";
        assert_eq!(&b[4..8], &i32::try_from(buf.len()).unwrap().to_le_bytes());
        assert_eq!(b.len(), 8 + 5 * 24 + buf.len());
        assert_eq!(&b[b.len() - buf.len()..], buf);
        // Row 3: version 1, level 2, flags 0, type "UInt8" at 17, name "b"
        // at 23, size 1, index 3, meta 0x4000.
        let row = &b[8 + 3 * 24..8 + 4 * 24];
        assert_eq!(&row[..4], &[1, 0, 2, 0]);
        assert_eq!(&row[4..8], &17u32.to_le_bytes());
        assert_eq!(&row[8..12], &23u32.to_le_bytes());
        assert_eq!(&row[16..20], &3i32.to_le_bytes());
        assert_eq!(&row[20..24], &0x4000i32.to_le_bytes());
        // Format 19 adds the hash; big-endian swaps every field.
        let b = blob_tree(&tree(), 19, true, Strings::Local, &[]);
        assert_eq!(b.len(), 8 + 5 * 32 + buf.len());
        assert_eq!(&b[..4], &5i32.to_be_bytes());
    }

    #[test]
    fn common_strings_are_offsets_with_the_high_bit() {
        let common: &[(u32, &[u8])] = &[(0, b"AABB"), (5, b"int")];
        let b = blob_tree(&tree(), 17, false, Strings::Common, common);
        let row = &b[8 + 24..8 + 48];
        assert_eq!(&row[4..8], &0x8000_0005u32.to_le_bytes());
        // Strings not in the table stay local.
        let buf = b"Base\0a\0Sub\0s\0UInt8\0b\0c\0";
        assert_eq!(&b[b.len() - buf.len()..], buf);
    }

    #[test]
    fn the_old_format_writes_child_counts_in_pre_order() {
        let t = tree();
        assert_eq!(parents(&t), [None, Some(0), Some(0), Some(2), Some(0)]);
        let b = old_tree(&t, 9, false);
        // Base: "Base\0Base\0", size, index, flags, version, meta, 3 children.
        assert_eq!(&b[..10], b"Base\0Base\0");
        assert_eq!(&b[10 + 20..10 + 24], &3i32.to_le_bytes());
        // Format 3 has no index or meta flag; format 2 a variable count.
        let v3 = old_tree(&t[..1], 3, false);
        assert_eq!(v3.len(), 10 + 16);
        let v2 = old_tree(&t[..1], 2, false);
        assert_eq!(v2.len(), 10 + 28);
        // A level jump nests under the previous row; a second top-level row
        // has no parent.
        let jump = [
            NodeSpec::new(0, "A", "a"),
            NodeSpec::new(2, "B", "b"),
            NodeSpec::new(1, "C", "c"),
            NodeSpec::new(0, "D", "d"),
        ];
        assert_eq!(parents(&jump), [None, Some(0), Some(0), None]);
    }
}
