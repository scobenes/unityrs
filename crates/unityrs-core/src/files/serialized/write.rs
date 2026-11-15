// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/SerializedFile.py, UnityPy/files/ObjectReader.py. See THIRD_PARTY_LICENSES.
// Ported from UnityPy git 417d595 (MIT): UnityPy/files/SerializedFile.py, UnityPy/helpers/TypeTreeNode.py. See THIRD_PARTY_LICENSES.
//! File save (`files/SerializedFile.py:389-493`), type records
//! (`files/SerializedFile.py:151-198`) and object rows
//! (`files/ObjectReader.py:132-179`). Types and metadata retain file order;
//! objects use the already charged path index, including duplicate-id collapse.
//! Every temporary and the returned bytes charge the input's meter.

use super::{SerializedFile, SerializedType};
use crate::files::save::{SaveError, SaveOptions, Writer, narrow, required};
use crate::stream::Endian;
use bytes::Bytes;

#[path = "write_head.rs"]
mod head_writer;

impl SerializedFile {
    /// Save metadata and objects, sorting by signed path id and padding every
    /// data block to eight bytes. The result owns its meter charge. Saving
    /// never clears `is_changed` or rewrites the loaded reader's metadata.
    ///
    /// Old trees, required pre-v13 trees, and pre-v9's data offset are
    /// corrected under both layouts (Tier C writing). Otherwise `UnityPy`
    /// matches UnityPy 1.25.2's uncompressed save through format 22 and
    /// HEAD 417d595 for formats 23-26. HEAD's absent per-type node assertion
    /// and formats above 26 refuse before building or charging output.
    pub fn save(&self, options: SaveOptions) -> Result<Bytes, SaveError> {
        let h = &self.header;
        let v = h.version;
        if v > 26 {
            return Err(SaveError::HeadTierWritesNotImplemented);
        }
        if v >= 23 && self.enable_type_tree {
            for t in self
                .types
                .iter()
                .chain(self.ref_types.iter().flat_map(|r| r.iter()))
            {
                self.meter().count_decode(1)?;
                required(t.nodes.as_ref(), "HEAD type node")?;
            }
        }
        let mut meta = Writer::metadata(self.meter(), h.endian, options.layout)?;
        let mut data = Writer::new(self.meter(), h.endian)?;
        if v >= 7 {
            meta.cstr(required(self.unity_version.as_ref(), "unity_version")?.as_bytes())?;
        }
        if v >= 8 {
            meta.i32(required(self.target_platform, "target_platform")?)?;
        }
        // Below 13 the reader unconditionally expects trees, even when the
        // config flag was false at load. Keep the parsed trees (Tier C).
        let trees = self.enable_type_tree || v < 13;
        if v >= 13 {
            meta.boolean(trees)?;
        }
        meta.count(self.types.len())?;
        for (i, t) in self.types.iter().enumerate() {
            t.write(self, &mut meta, trees, false, i, options)?;
        }
        if (7..14).contains(&v) {
            meta.i32(self.big_id_enabled)?;
        }
        meta.count(self.by_path.len())?;
        // A save sees one consistent set of replacements. No read or write
        // can re-enter this mutex; allocations follow MemoryBudget's contract.
        let edits = self
            .edits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for &index in &self.by_path {
            let row = required(self.object_infos.get(index as usize), "object row")?;
            if self.big_id_enabled != 0 {
                meta.i64(row.path_id)?;
            } else if v < 14 {
                meta.i32(narrow(row.path_id, "path_id")?)?;
            } else {
                meta.align(4)?;
                meta.i64(row.path_id)?;
            }
            let raw;
            let payload = if let Some((_, replacement)) = edits
                .as_slice()
                .binary_search_by_key(&index, |(i, _)| *i)
                .ok()
                .and_then(|at| edits.as_slice().get(at))
            {
                replacement.as_ref()
            } else {
                raw = self.original_data(row)?;
                raw.as_ref()
            };
            if v >= 22 {
                meta.i64(narrow(data.len(), "byte_start")?)?;
            } else {
                meta.u32(narrow(data.len(), "byte_start")?)?;
            }
            meta.u32(narrow(payload.len(), "byte_size")?)?;
            data.bytes(payload)?;
            data.align(8)?;
            meta.i32(row.type_id)?;
            if v < 16 {
                meta.u16(narrow(row.class_id, "class_id")?)?;
            }
            if v < 11 {
                meta.u16(required(row.is_destroyed, "is_destroyed")?)?;
            }
            if (11..17).contains(&v) {
                let t = row.type_index.and_then(|i| self.types.get(i));
                meta.i16(required(t, "serialized_type")?.script_type_index)?;
            }
            if v == 15 || v == 16 {
                meta.bytes(&required(row.is_stripped, "is_stripped")?.to_ne_bytes())?;
            }
        }
        drop(edits);
        self.write_references(&mut meta, trees, options)?;
        meta.end_metadata()?;
        self.finish_file(&meta, &data)
    }

    fn write_references(
        &self,
        meta: &mut Writer,
        trees: bool,
        options: SaveOptions,
    ) -> Result<(), SaveError> {
        let v = self.header.version;
        if v >= 11 {
            meta.count(self.script_types.len())?;
            for s in &self.script_types {
                meta.i32(s.file_index)?;
                if v < 14 {
                    meta.i32(narrow(s.identifier, "script identifier")?)?;
                } else {
                    meta.align(4)?;
                    meta.i64(s.identifier)?;
                }
            }
        }
        let pending = self
            .pending_externals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        meta.count(
            self.externals
                .len()
                .checked_add(pending.len())
                .ok_or(SaveError::Range("externals"))?,
        )?;
        for e in self.externals.iter().chain(pending.as_slice()) {
            if v >= 6 {
                meta.cstr(required(e.temp_empty.as_ref(), "temp_empty")?.as_bytes())?;
            }
            if v >= 5 {
                meta.fixed16(required(e.guid.as_ref(), "guid")?, "guid")?;
                meta.i32(required(e.kind, "external type")?)?;
            }
            meta.cstr(e.path.as_bytes())?;
        }
        if v >= 20 {
            let refs = required(self.ref_types.as_ref(), "ref_types")?;
            meta.count(refs.len())?;
            for (i, t) in refs.iter().enumerate() {
                t.write(
                    self,
                    meta,
                    trees,
                    true,
                    self.types
                        .len()
                        .checked_add(i)
                        .ok_or(SaveError::Range("HEAD type index"))?,
                    options,
                )?;
            }
        }
        if v >= 26 {
            head_writer::write_shared(self, meta, options)?;
        }
        if v >= 5 {
            meta.cstr(required(self.user_information.as_ref(), "userInformation")?.as_bytes())?;
        }
        Ok(())
    }

    fn finish_file(&self, meta: &Writer, data: &Writer) -> Result<Bytes, SaveError> {
        let h = &self.header;
        let v = h.version;
        let mut out = Writer::new(self.meter(), Endian::Big)?;
        let size = meta
            .len()
            .checked_add(usize::from(v < 9))
            .ok_or(SaveError::Range("metadata_size"))?;
        let header: usize = if v >= 22 {
            48
        } else if v >= 9 {
            20
        } else {
            16
        };
        let end = header
            .checked_add(meta.len())
            .ok_or(SaveError::Range("data_offset"))?;
        let offset = if v < 9 {
            16
        } else {
            end.checked_add((16 - end % 16) % 16)
                .ok_or(SaveError::Range("data_offset"))?
        };
        let total = if v < 9 {
            header
                .checked_add(size)
                .and_then(|n| n.checked_add(data.len()))
        } else {
            offset.checked_add(data.len())
        }
        .ok_or(SaveError::Range("file_size"))?;
        out.u32(if v >= 22 {
            0
        } else {
            narrow(size, "metadata_size")?
        })?;
        out.u32(if v >= 22 {
            0
        } else {
            narrow(total, "file_size")?
        })?;
        out.u32(v)?;
        out.u32(if v >= 22 {
            0
        } else {
            narrow(offset, "data_offset")?
        })?;
        if v >= 9 {
            out.boolean(h.endian == Endian::Big)?;
            out.bytes(&required(h.reserved, "reserved")?)?;
            if v >= 22 {
                out.u32(narrow(size, "metadata_size")?)?;
                out.i64(narrow(total, "file_size")?)?;
                out.i64(narrow(offset, "data_offset")?)?;
                out.i64(h.unknown)?;
            }
            out.bytes(meta.slice())?;
            out.align(16)?;
            out.bytes(data.slice())?;
        } else {
            out.bytes(data.slice())?;
            out.boolean(h.endian == Endian::Big)?;
            out.bytes(meta.slice())?;
        }
        out.finish()
    }

    pub(crate) fn original_data(&self, row: &super::ObjectInfo) -> Result<Bytes, crate::Error> {
        let start = self.reader.seek(row.byte_start)?;
        let (range, _) = self
            .reader
            .read(self.data.len(), start, i64::from(row.byte_size))?;
        Ok(self.data.slice(range))
    }
}

impl SerializedType {
    fn write(
        &self,
        file: &SerializedFile,
        out: &mut Writer,
        trees: bool,
        is_ref: bool,
        index: usize,
        options: SaveOptions,
    ) -> Result<(), SaveError> {
        let v = file.header.version;
        out.i32(self.class_id)?;
        if v >= 16 {
            out.boolean(required(self.is_stripped_type, "is_stripped_type")?)?;
        }
        if v >= 17 {
            out.i16(self.script_type_index)?;
        }
        if v >= 13 {
            if (is_ref && self.script_type_index >= 0)
                || (v < 16 && self.class_id < 0)
                || (v >= 16 && self.class_id == 114)
            {
                out.fixed16(required(self.script_id.as_ref(), "script_id")?, "script_id")?;
            }
            out.fixed16(
                required(self.old_type_hash.as_ref(), "old_type_hash")?,
                "old_type_hash",
            )?;
        }
        if trees {
            if v >= 23 {
                required(
                    file.head_handlers.as_slice().get(index),
                    "HEAD type handler",
                )?
                .write(file, out, options)?;
            } else {
                let nodes = required(self.nodes.as_ref(), "node")?;
                if crate::typetree::parse::is_blob_format(v) {
                    nodes.write_blob(
                        out,
                        v,
                        file.header.endian,
                        options.layout,
                        file.ctx.providers.common_strings(),
                    )?;
                } else {
                    nodes.write_old(out, v)?;
                }
            }
            if v >= 21 {
                if is_ref {
                    for s in [&self.class_name, &self.namespace, &self.assembly] {
                        out.cstr(required(s.as_ref(), "ref type name")?.as_bytes())?;
                    }
                } else {
                    let deps = required(self.type_dependencies.as_ref(), "type_dependencies")?;
                    out.count(deps.len())?;
                    for &d in deps {
                        out.i32(d)?;
                    }
                }
            }
        }
        Ok(())
    }
}
