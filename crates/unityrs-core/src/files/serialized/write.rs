// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/SerializedFile.py, UnityPy/files/ObjectReader.py. See THIRD_PARTY_LICENSES.
// Ported from UnityPy git 417d595 (MIT): UnityPy/files/SerializedFile.py, UnityPy/helpers/TypeTreeNode.py. See THIRD_PARTY_LICENSES.
//! File save (`files/SerializedFile.py:389-493`), type records
//! (`files/SerializedFile.py:151-198`) and object rows
//! (`files/ObjectReader.py:132-179`). Types and metadata retain file order;
//! objects use the already charged path index, including duplicate-id collapse.
//! Every temporary and the returned bytes charge the input's meter.

use super::{External, ObjectInfo, SerializedFile, SerializedHeader, SerializedType};
use crate::RawStr;
use crate::files::save::{SaveError, SaveLayout, SaveOptions, Writer, narrow, required};
use crate::stream::Endian;
use bytes::Bytes;

#[path = "write_head.rs"]
mod head_writer;

/// Caller-owned metadata and object payload snapshot. All retained storage must
/// be charged by the caller before constructing it. Object rows are emitted in
/// supplied order, including repeated path ids; dictionary keys are irrelevant.
pub struct SerializedSaveSnapshot<'a> {
    /// Current header (sizes and offsets are recomputed).
    pub header: SerializedHeader,
    /// Current Unity version string.
    pub unity_version: Option<RawStr>,
    /// Current raw build target.
    pub target_platform: Option<i32>,
    /// Whether type records carry trees.
    pub enable_type_tree: bool,
    /// Pre-format-14 path-id width switch.
    pub big_id_enabled: i32,
    /// Current object metadata and payloads, sorted by signed path id. Each
    /// row's `byte_size` is its logical payload length, which may differ from
    /// the buffer's byte count (Python's `len(data)` versus `nbytes`).
    pub objects: &'a [(ObjectInfo, Bytes)],
    /// Current external reference list, replacing loaded and pending entries.
    pub externals: &'a [External],
    /// Current user information string.
    pub user_information: Option<RawStr>,
}
const _: () = crate::send_sync::<SerializedSaveSnapshot<'static>>();

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
    ///
    /// `Semantic` refuses with a `Range` collision an output that detection
    /// would not reload as a SerializedFile (a bundle signature or gzip magic
    /// in the header's sizes, the brotli marker at 0x20 in metadata or object
    /// bytes below format 22); the `UnityPy` layout writes it unchecked.
    pub fn save(&self, options: SaveOptions) -> Result<Bytes, SaveError> {
        self.save_impl(options, None)
    }

    /// Serialize a metered snapshot without modifying the loaded file or edits.
    pub fn save_with_snapshot(
        &self,
        options: SaveOptions,
        snapshot: &SerializedSaveSnapshot<'_>,
    ) -> Result<Bytes, SaveError> {
        self.save_impl(options, Some(snapshot))
    }

    fn save_impl(
        &self,
        options: SaveOptions,
        snapshot: Option<&SerializedSaveSnapshot<'_>>,
    ) -> Result<Bytes, SaveError> {
        let h = snapshot.map_or(&self.header, |s| &s.header);
        let v = h.version;
        let enabled = snapshot.map_or(self.enable_type_tree, |s| s.enable_type_tree);
        let big_id = snapshot.map_or(self.big_id_enabled, |s| s.big_id_enabled);
        if v > 26 {
            return Err(SaveError::HeadTierWritesNotImplemented);
        }
        if v >= 23 && enabled {
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
            meta.cstr(
                required(
                    snapshot.map_or(self.unity_version.as_ref(), |s| s.unity_version.as_ref()),
                    "unity_version",
                )?
                .as_bytes(),
            )?;
        }
        if v >= 8 {
            meta.i32(required(
                snapshot.map_or(self.target_platform, |s| s.target_platform),
                "target_platform",
            )?)?;
        }
        // Below 13 the reader unconditionally expects trees, even when the
        // config flag was false at load. Keep the parsed trees (Tier C).
        let trees = enabled || v < 13;
        if v >= 13 {
            meta.boolean(trees)?;
        }
        meta.count(self.types.len())?;
        for (i, t) in self.types.iter().enumerate() {
            t.write((self, h), &mut meta, trees, false, i, options)?;
        }
        if (7..14).contains(&v) {
            meta.i32(big_id)?;
        }
        meta.count(snapshot.map_or(self.by_path.len(), |s| s.objects.len()))?;
        // A save sees one consistent set of replacements. No read or write
        // can re-enter this mutex; allocations follow MemoryBudget's contract.
        let edits = self
            .edits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let count = snapshot.map_or(self.by_path.len(), |s| s.objects.len());
        for position in 0..count {
            let raw;
            let (row, payload) = if let Some(snapshot) = snapshot {
                let (row, bytes) = required(snapshot.objects.get(position), "object row")?;
                (row, bytes.as_ref())
            } else {
                let index = *required(self.by_path.get(position), "object index")?;
                let row = required(self.object_infos.get(index as usize), "object row")?;
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
                (row, payload)
            };
            self.write_object(
                row,
                payload,
                &mut meta,
                &mut data,
                (v, big_id, snapshot.is_none()),
            )?;
        }
        drop(edits);
        self.write_references(&mut meta, trees, options, snapshot)?;
        meta.end_metadata()?;
        self.finish_file(h, &meta, &data, options.layout)
    }

    fn write_object(
        &self,
        row: &ObjectInfo,
        payload: &[u8],
        meta: &mut Writer,
        data: &mut Writer,
        settings: (u32, i32, bool),
    ) -> Result<(), SaveError> {
        let (v, big_id, native) = settings;
        if big_id != 0 {
            meta.i64(row.path_id)?;
        } else if v < 14 {
            meta.i32(narrow(row.path_id, "path_id")?)?;
        } else {
            meta.align(4)?;
            meta.i64(row.path_id)?;
        }
        if v >= 22 {
            meta.i64(narrow(data.len(), "byte_start")?)?;
        } else {
            meta.u32(narrow(data.len(), "byte_start")?)?;
        }
        meta.u32(if native {
            narrow(payload.len(), "byte_size")?
        } else {
            row.byte_size
        })?;
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
            let script = if native {
                row.type_index
                    .and_then(|i| self.types.get(i))
                    .map(|t| t.script_type_index)
            } else {
                row.script_type_index
            };
            meta.i16(required(script, "script_type_index")?)?;
        }
        if v == 15 || v == 16 {
            meta.bytes(&required(row.is_stripped, "is_stripped")?.to_ne_bytes())?;
        }
        Ok(())
    }

    fn write_references(
        &self,
        meta: &mut Writer,
        trees: bool,
        options: SaveOptions,
        snapshot: Option<&SerializedSaveSnapshot<'_>>,
    ) -> Result<(), SaveError> {
        let h = snapshot.map_or(&self.header, |s| &s.header);
        let v = h.version;
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
            snapshot
                .map_or(self.externals.len(), |s| s.externals.len())
                .checked_add(if snapshot.is_some() { 0 } else { pending.len() })
                .ok_or(SaveError::Range("externals"))?,
        )?;
        let externals = snapshot.map_or(self.externals.as_ref(), |s| s.externals);
        for e in externals
            .iter()
            .chain(pending.as_slice().iter().filter(|_| snapshot.is_none()))
        {
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
                    (self, h),
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
            head_writer::write_shared((self, h), meta, options)?;
        }
        if v >= 5 {
            meta.cstr(
                required(
                    snapshot.map_or(self.user_information.as_ref(), |s| {
                        s.user_information.as_ref()
                    }),
                    "userInformation",
                )?
                .as_bytes(),
            )?;
        }
        Ok(())
    }

    fn finish_file(
        &self,
        h: &SerializedHeader,
        meta: &Writer,
        data: &Writer,
        layout: SaveLayout,
    ) -> Result<Bytes, SaveError> {
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
        if layout == SaveLayout::Semantic {
            reloads_as_serialized(out.slice())?;
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

/// The semantic layout's reload guard, a pure prefix-signature check: it
/// runs detection's steps as a bundle or WebFile entry gets them
/// (`check_entry_type`, `helpers/ImportHelper.py:44-124`) on `bytes`, the
/// whole output, in place, up to the gzip probe, and neither allocates nor
/// decodes. The output must detect as an `AssetsFile`, or, under 128 bytes,
/// as the `ResourceFile` every such small file is. Below format 22 the
/// header's sizes and the metadata or object bytes that follow can spell a
/// signature, gzip magic or the brotli marker. Gzip magic is refused without
/// inflating, conservatively: detection would inflate the stream, which in
/// a rare output could fall back to the header and detect as this file, but
/// that needs a metadata size of at least 0x1f8b0800 (about 529 MB).
fn reloads_as_serialized(bytes: &[u8]) -> Result<(), SaveError> {
    use crate::detect::{FileType, Signed, signature_only};
    let reason = match signature_only(bytes) {
        Ok(Signed::Type(FileType::AssetsFile)) => return Ok(()),
        Ok(Signed::Type(FileType::ResourceFile)) if bytes.len() < 128 => return Ok(()),
        Ok(Signed::Type(FileType::BundleFile)) => "SerializedFile bundle signature collision",
        Ok(Signed::Type(FileType::WebFile)) => "SerializedFile WebFile marker collision",
        Ok(Signed::Type(FileType::Zip)) => "SerializedFile zip signature collision",
        Ok(Signed::Type(FileType::ResourceFile)) => "SerializedFile header detected as a resource",
        Ok(Signed::Gzip) => "SerializedFile gzip magic collision",
        // A 20-byte output with no NUL, which detection raises for.
        Err(_) => "SerializedFile unterminated signature collision",
    };
    Err(SaveError::Range(reason))
}

impl SerializedType {
    fn write(
        &self,
        source: (&SerializedFile, &SerializedHeader),
        out: &mut Writer,
        trees: bool,
        is_ref: bool,
        index: usize,
        options: SaveOptions,
    ) -> Result<(), SaveError> {
        let (file, header) = source;
        let v = header.version;
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
                .write((file, header), out, options)?;
            } else {
                let nodes = required(self.nodes.as_ref(), "node")?;
                if crate::typetree::parse::is_blob_format(v) {
                    nodes.write_blob(
                        out,
                        v,
                        header.endian,
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

#[cfg(test)]
mod guard_tests {
    use super::reloads_as_serialized;
    use crate::files::save::{SaveError, SaveLayout, SaveOptions};
    use std::sync::Arc;

    /// A format-17 header (big-endian sizes, then the endian byte and three
    /// reserved bytes), zero-padded to `len`.
    fn header(metadata: u32, file: u32, len: usize) -> Vec<u8> {
        let mut out = Vec::new();
        for field in [metadata, file, 17, 0x40] {
            out.extend_from_slice(&field.to_be_bytes());
        }
        out.resize(len, 0);
        out
    }

    fn collision(reason: &'static str) -> Result<(), SaveError> {
        Err(SaveError::Range(reason))
    }

    #[test]
    fn only_outputs_detected_as_serialized_files_pass() {
        let guard = reloads_as_serialized;
        assert_eq!(guard(&header(0x10, 128, 128)), Ok(()));
        // Under 128 bytes detection says resource for any unsigned file.
        assert_eq!(guard(&header(0x10, 64, 64)), Ok(()));
        // Metadata sizes from 0x1f8b0000 open with gzip magic: refused
        // without inflating, a bad stream (method 0) and a plausible deflate
        // header (method 8) alike.
        for metadata in [0x1f8b_0000, 0x1f8b_0804] {
            assert_eq!(
                guard(&header(metadata, 128, 128)),
                collision("SerializedFile gzip magic collision")
            );
        }
        // Sizes 0x556e6974 and 0x79465300 spell `UnityFS\0`, and
        // 0x504b0304 then a zero byte `PK\x03\x04\0`.
        assert_eq!(
            guard(&header(0x556e_6974, 0x7946_5300, 128)),
            collision("SerializedFile bundle signature collision")
        );
        assert_eq!(
            guard(&header(0x504b_0304, 128, 128)),
            collision("SerializedFile zip signature collision")
        );
        let mut marked = header(0x10, 128, 128);
        marked[0x20..0x26].copy_from_slice(b"brotli");
        assert_eq!(
            guard(&marked),
            collision("SerializedFile WebFile marker collision")
        );
        // Sizes past the file fail the header check: a resource.
        assert_eq!(
            guard(&header(0x10, 4096, 128)),
            collision("SerializedFile header detected as a resource")
        );
    }

    /// The save path refuses with the guard's verdict on its own output. Only
    /// the brotli marker is reachable end to end: the gzip, bundle and zip
    /// sizes need metadata of hundreds of megabytes or more.
    #[test]
    fn semantic_save_refuses_with_the_guards_verdict_on_its_output() {
        use unityrs_testkit::blob::NodeSpec;
        use unityrs_testkit::serialized::{ObjectSpec, build, one_object};
        // Path 2's data, which opens with `brotli`, moves from 0x10 to 0x20
        // when the save writes path 1's 16 bytes first (the bundle test).
        let mut marked = b"brotli".to_vec();
        marked.resize(32, 1);
        let mut spec = one_object(49, vec![NodeSpec::new(0, "aa", "aa")], marked);
        spec.format = 8;
        spec.objects[0].type_id = 49;
        spec.objects[0].path_id = 2;
        spec.objects.push(ObjectSpec {
            path_id: 1,
            type_id: 49,
            class_id: 49,
            data: vec![0; 16],
            ..ObjectSpec::default()
        });
        let file = super::SerializedFile::parse(
            crate::RawStr::from_static(b"CAB-marker"),
            bytes::Bytes::from(build(&spec)),
            None,
            &crate::limits::Meter::unlimited(),
            &Arc::new(crate::options::FileContext::unitypy()),
        )
        .unwrap();
        let written = file
            .save(SaveOptions {
                layout: SaveLayout::UnityPy,
            })
            .unwrap();
        let verdict = reloads_as_serialized(&written);
        assert_eq!(
            verdict,
            collision("SerializedFile WebFile marker collision")
        );
        assert_eq!(file.save(SaveOptions::default()).map(|_| ()), verdict);
    }
}
