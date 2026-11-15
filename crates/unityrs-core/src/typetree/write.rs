// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeNode.py. See THIRD_PARTY_LICENSES.
//! Tree writers: `helpers/TypeTreeNode.py:213-278`. Old trees write each
//! node's own fields and children (Tier C); blob trees optionally reproduce
//! UnityPy's local string for common offset zero. Traversal uses the kept
//! pre-order rows, with bounded stack use. All buffers and indexes are metered.

use super::commonstring::{COMMON_STRINGS, CommonStrings};
use super::{NodeId, NodeList};
use crate::files::save::{SaveError, SaveLayout, Writer, narrow, required};
use crate::limits::{Meter, MeteredVec};
use crate::stream::Endian;
use bytes::Bytes;
use std::cmp::Ordering;

/// Like the reader's name comparisons, precharge the shorter byte span.
/// A refused comparison reads no bytes and returns the same work-limit error.
fn compare(meter: &Meter, a: &[u8], b: &[u8]) -> Result<Ordering, SaveError> {
    meter.count_decode(narrow(a.len().min(b.len()), "comparison length")?)?;
    Ok(a.cmp(b))
}

fn sort_strings<T: Ord>(items: &mut [(&[u8], T)], meter: &Meter) -> Result<(), SaveError> {
    // A fallible heapsort stops at the first refusal, without an unmetered
    // scratch allocation or an infallible comparator hiding a limit error.
    fn compare_at<T: Ord>(
        items: &[(&[u8], T)],
        a: usize,
        b: usize,
        meter: &Meter,
    ) -> Result<Ordering, SaveError> {
        let a = items.get(a).ok_or(SaveError::Missing("string index"))?;
        let b = items.get(b).ok_or(SaveError::Missing("string index"))?;
        Ok(compare(meter, a.0, b.0)?.then_with(|| a.1.cmp(&b.1)))
    }
    fn sift<T: Ord>(
        items: &mut [(&[u8], T)],
        mut root: usize,
        meter: &Meter,
    ) -> Result<(), SaveError> {
        while root < items.len() / 2 {
            let mut child = root * 2 + 1;
            if child + 1 < items.len()
                && compare_at(items, child, child + 1, meter)? == Ordering::Less
            {
                child += 1;
            }
            if compare_at(items, root, child, meter)? != Ordering::Less {
                break;
            }
            items.swap(root, child);
            root = child;
        }
        Ok(())
    }
    for root in (0..items.len() / 2).rev() {
        sift(items, root, meter)?;
    }
    for end in (1..items.len()).rev() {
        items.swap(0, end);
        sift(
            items
                .get_mut(..end)
                .ok_or(SaveError::Missing("string index"))?,
            0,
            meter,
        )?;
    }
    Ok(())
}

/// Find the last equal key, preserving Python's last duplicate offset.
fn find_string<T>(
    items: &[(&[u8], T)],
    key: &[u8],
    meter: &Meter,
) -> Result<Option<usize>, SaveError> {
    let (mut lo, mut hi) = (0, items.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let entry = items.get(mid).ok_or(SaveError::Missing("string index"))?;
        if compare(meter, entry.0, key)? == Ordering::Greater {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    if let Some(at) = lo.checked_sub(1) {
        let entry = items.get(at).ok_or(SaveError::Missing("string index"))?;
        if compare(meter, entry.0, key)? == Ordering::Equal {
            return Ok(Some(at));
        }
    }
    Ok(None)
}

impl NodeList {
    /// Write a blob tree, using the provider's common strings or the built-in
    /// MIT table. `UnityPy` reproduces `dump_blob`'s offset-zero bug.
    pub fn dump_blob(
        &self,
        format: u32,
        endian: Endian,
        layout: SaveLayout,
        common: Option<&CommonStrings>,
    ) -> Result<Bytes, SaveError> {
        let mut out = Writer::new(self.meter(), endian)?;
        self.write_blob(&mut out, format, endian, layout, common)?;
        out.finish()
    }

    /// Write a reloadable old tree (Tier C), including v2's variable count
    /// and omitting the index and meta flag at v3.
    pub fn dump_old(&self, format: u32, endian: Endian) -> Result<Bytes, SaveError> {
        let mut out = Writer::metadata(self.meter(), endian, SaveLayout::Semantic)?;
        self.write_old(&mut out, format)?;
        out.finish()
    }

    pub(crate) fn write_old(&self, out: &mut Writer, format: u32) -> Result<(), SaveError> {
        for (i, row) in self.rows().iter().enumerate() {
            out.cstr(row.ty.as_bytes())?;
            out.cstr(row.name.as_bytes())?;
            out.i32(row.byte_size)?;
            if format == 2 {
                out.i32(required(row.variable_count, "m_VariableCount")?)?;
            }
            if format != 3 {
                out.i32(required(row.index, "m_Index")?)?;
            }
            out.i32(row.type_flags.unwrap_or(0))?;
            out.i32(row.version)?;
            if format != 3 {
                out.i32(narrow(
                    required(row.meta_flag, "m_MetaFlag")?,
                    "m_MetaFlag",
                )?)?;
            }
            out.count(self.children(NodeId(narrow(i, "node index")?)).len())?;
        }
        Ok(())
    }

    pub(crate) fn write_blob(
        &self,
        out: &mut Writer,
        format: u32,
        endian: Endian,
        layout: SaveLayout,
        common: Option<&CommonStrings>,
    ) -> Result<(), SaveError> {
        // Sort references once; binary lookup keeps duplicate strings linear
        // in their length rather than searching the string buffer per node.
        let mut strings = MeteredVec::new(out.meter());
        for row in self.rows() {
            strings.push((row.ty.as_bytes(), None))?;
            strings.push((row.name.as_bytes(), None))?;
        }
        sort_strings(strings.as_mut_slice(), out.meter())?;
        let mut common_index = MeteredVec::new(out.meter());
        if let Some(common) = common {
            for (at, s) in common.iter() {
                common_index.push((s.as_bytes(), at))?;
            }
        } else {
            for &(at, s) in COMMON_STRINGS {
                common_index.push((s, at))?;
            }
        }
        // Python's reverse dict keeps the last offset for duplicate strings.
        sort_strings(common_index.as_mut_slice(), out.meter())?;
        let mut buffer = Writer::new(out.meter(), Endian::Big)?;
        let mut nodes = Writer::new(out.meter(), endian)?;
        for row in self.rows() {
            nodes.i16(narrow(row.version, "m_Version")?)?;
            nodes.bytes(&[
                narrow(row.level, "m_Level")?,
                narrow(required(row.type_flags, "m_TypeFlags")?, "m_TypeFlags")?,
            ])?;
            for s in [row.ty.as_bytes(), row.name.as_bytes()] {
                let at = find_string(strings.as_slice(), s, out.meter())?
                    .ok_or(SaveError::Missing("string index"))?;
                let entry = strings
                    .as_mut_slice()
                    .get_mut(at)
                    .ok_or(SaveError::Missing("string index"))?;
                let offset = if let Some(offset) = entry.1 {
                    offset
                } else {
                    let common_offset = find_string(common_index.as_slice(), s, out.meter())?
                        .and_then(|i| common_index.as_slice().get(i))
                        .map(|(_, offset)| *offset)
                        .filter(|v| *v != 0 || layout == SaveLayout::Semantic);
                    let offset = if let Some(offset) = common_offset {
                        offset | 0x8000_0000
                    } else {
                        let offset: u32 = narrow(buffer.len(), "string offset")?;
                        if offset & 0x8000_0000 != 0 {
                            return Err(SaveError::Range("string offset"));
                        }
                        buffer.cstr(s)?;
                        offset
                    };
                    entry.1 = Some(offset);
                    offset
                };
                nodes.u32(offset)?;
            }
            nodes.i32(row.byte_size)?;
            nodes.i32(required(row.index, "m_Index")?)?;
            nodes.i32(narrow(
                required(row.meta_flag, "m_MetaFlag")?,
                "m_MetaFlag",
            )?)?;
            if format >= 19 {
                nodes.u64(required(row.ref_type_hash, "m_RefTypeHash")?)?;
            }
        }
        out.count(self.len())?;
        out.count(buffer.len())?;
        out.bytes(nodes.slice())?;
        out.bytes(buffer.slice())
    }
}
