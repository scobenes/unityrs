// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeNode.py. See THIRD_PARTY_LICENSES.
// Ported from UnityPy git 417d595 (MIT): UnityPy/files/SerializedFile.py, UnityPy/helpers/TypeTreeNode.py. See THIRD_PARTY_LICENSES.
//! HEAD helpers/TypeTreeNode.py:237-277, HEAD helpers/TypeTreeNode.py:359-367,
//! HEAD helpers/TypeTreeNode.py:385-394: envelopes keep their embedded blob
//! version and reference hashes; blob traversal skips shared children.
//! Shared handlers use HEAD's insertion order and last duplicate value.

use super::super::SerializedFile;
use super::super::head::Handler;
use crate::files::save::{SaveError, SaveLayout, SaveOptions, Writer, narrow};
use crate::limits::MeteredVec;
use crate::typetree::NodeList;

impl Handler {
    pub(in crate::files::serialized) fn write(
        &self,
        source: (&SerializedFile, &super::super::SerializedHeader),
        out: &mut Writer,
        options: SaveOptions,
    ) -> Result<(), SaveError> {
        let (file, header) = source;
        out.fixed16(&self.hash, "HEAD type hash")?;
        let Some(nodes) = &self.original_nodes else {
            return out.u32(0);
        };
        let mut info = Writer::metadata(file.meter(), header.endian, options.layout)?;
        info.bytes(b"mhtt")?;
        info.i32(self.version)?;
        // HEAD's dump_blob omits descendants of nodes flagged 32. Format 26
        // resolves shared children on read; formats 23-25 do not, so Semantic
        // must retain their descendants to keep the tree and objects readable.
        let mut rows = MeteredVec::new(file.meter());
        let mut skip_level = None;
        for row in nodes.rows() {
            file.meter().count_decode(1)?;
            if let Some(level) = skip_level {
                if row.level > level {
                    continue;
                }
                skip_level = None;
            }
            rows.push(row.clone())?;
            if row.type_flags.unwrap_or(0) & 32 != 0
                && (options.layout == SaveLayout::UnityPy || header.version >= 26)
            {
                skip_level = Some(row.level);
            }
        }
        let rows = NodeList::from_rows(rows).map_err(crate::Error::from)?;
        rows.write_blob(
            &mut info,
            u32::try_from(self.version).unwrap_or(0),
            header.endian,
            options.layout,
            file.ctx.providers.common_strings(),
        )?;
        // A bare envelope (formats 23-25, Tier C) keeps its layout: no list.
        if self.listed_refs {
            info.u32(narrow(self.refs.len(), "HEAD reference count")?)?;
            for hash in self.refs.as_slice() {
                info.fixed16(hash, "HEAD reference hash")?;
            }
        }
        out.u32(narrow(info.len(), "HEAD serialized size")?)?;
        out.bytes(info.slice())
    }
}

pub(super) fn write_shared(
    source: (&SerializedFile, &super::super::SerializedHeader),
    out: &mut Writer,
    options: SaveOptions,
) -> Result<(), SaveError> {
    let (file, _) = source;
    let shared = file
        .head_handlers
        .as_slice()
        .get(file.head_roots..)
        .ok_or(SaveError::Missing("HEAD shared trees"))?;
    // A sorted fixed-hash index avoids scanning all earlier dictionary keys.
    // As on the read side, charge the worst-case sort bound up front; sorting
    // is in place and allocates no scratch buffer.
    let mut hashes = MeteredVec::with_capacity(file.meter(), shared.len())?;
    let mut order = MeteredVec::with_capacity(file.meter(), shared.len())?;
    for (i, handler) in shared.iter().enumerate() {
        file.meter().count_decode(16)?;
        hashes.push((handler.hash, i))?;
        order.push(None)?;
    }
    file.meter().count_decode(
        (shared.len() as u64)
            .saturating_mul(u64::from(shared.len().bit_width()) + 1)
            .saturating_mul(32),
    )?;
    hashes
        .as_mut_slice()
        .sort_unstable_by(|a, b| a.0.as_slice().cmp(b.0.as_slice()).then(a.1.cmp(&b.1)));
    let mut first = None;
    let mut unique = 0;
    for &(hash, i) in hashes.as_slice() {
        file.meter().count_decode(16)?;
        let at = match first {
            Some((key, at)) if key == hash => at,
            _ => {
                first = Some((hash, i));
                unique += 1;
                i
            }
        };
        // Equal hashes are ordered by source index: first position, last value.
        *order
            .as_mut_slice()
            .get_mut(at)
            .ok_or(SaveError::Missing("HEAD shared index"))? = Some(i);
    }
    out.count(unique)?;
    for index in order.as_slice() {
        file.meter().count_decode(1)?;
        if let Some(i) = *index {
            shared
                .get(i)
                .ok_or(SaveError::Missing("HEAD shared index"))?
                .write(source, out, options)?;
        }
    }
    Ok(())
}
