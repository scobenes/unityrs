//! One input's records, from `unityrs-core`'s public API: a port of
//! `tools/parity/dump_records.py`'s `_dump_input`, `emit_file`,
//! `object_record` and the L1 builders.
//!
//! **Order** (`docs/CANONICAL_DUMP.md` section 5): the input's `file`
//! record, then a SerializedFile's objects in ascending signed `path_id`
//! (`sorted(f.objects.values(), key=lambda o: o.path_id)`), or a
//! container's entries in `files` dict order (`f.files.items()`),
//! recursively, each entry's `l0` describing its own decoded bytes (the
//! oracle's `_read_files` hook detects every directory node and keys the
//! results by path, so a repeated path shows its last node, which is the
//! node `files` holds).
//!
//! **A failing entry fails its input** (section 4.2): UnityPy's
//! `Environment.load_file` reads a bundle's entries with no error handling,
//! parsing every directory entry, nested bundles' included. The dump does
//! the same before it writes anything ([`load_deep`]: `BundleFile::load_all`
//! on the bundle and on every bundle among its entries), and an input whose
//! load fails is one `file` record with `parsed_as` `"error"` and
//! `{"kind":"parse"}`. A core error met while the records are written (a
//! refusal of the meter, which the load did not meet) fails the input the
//! same way, so an input is dumped whole or as one error record.
//!
//! **Memory.** The records are held until the run writes them, and they can
//! be thousands of times the input (an object of 4 bytes read as 2^20 empty
//! structs; an object record repeating a long container path). Each line is
//! a [`Text`] charged to the input's meter as it grows, and the list of
//! lines is charged too, so the records count toward the input's
//! `max_input_memory` (and the caller's budget) with what its parse holds,
//! until the run has written them ([`InputDump::charge`]). A refusal fails
//! the input as any core error does: the meter's own bound is a `parse`
//! record whose note is the limit (`limit InputMemory: <v> > <max>`), and a
//! refusal of the caller's budget or of the allocator (an out-of-memory,
//! met by the records or by the core, an object's read included, which is
//! never that object's L2 outcome: `out_of_memory`) is a `crash` record,
//! the oracle's record for a `MemoryError` (`CANONICAL_DUMP.md` section
//! 4.2); either way the records made so far are dropped first, and the run
//! goes on.
//!
//! **Context**: the dump reads with the [`DumpOptions::context`] it is
//! given, `FileContext::unitypy()` by default, and nothing else.

use std::sync::Arc;

use unityrs_core::budget::{BudgetExceeded, Reservation};
use unityrs_core::detect::{FileType, ParsedAs, check_file_type};
use unityrs_core::env::DerefError;
use unityrs_core::files::{File, LayoutKind, WebPacker, parse_file};
use unityrs_core::limits::{Charge, ChargeError, Meter, MeteredVec};
use unityrs_core::provider::TreeSource;
use unityrs_core::stream::Endian;
use unityrs_core::typetree::TreeError;
use unityrs_core::walk::{L2Kind, ParseError, ReadDetail, WriteError, WriteRules};
use unityrs_core::{BundleFile, Bytes, Error, ObjectRef, Parsed, RawStr, SerializedFile, WebFile};

use crate::encode::encode_into;
use crate::json::sha256;
use crate::record::{
    BlockRow, BundleL1, ContainerRow, DirRow, ExternalRow, FileError, FileErrorKind, FileL1,
    FileRecord, L0, L2, L2Error, L3, ObjectL1, ObjectRecord, Resave, SerializedHeaderRow,
    SerializedL1, TypeRecord, WebL1, file_line, file_line_into, first_difference, note,
    object_line_into,
};
use crate::text::Text;
use crate::{DumpOptions, InputDump, Levels};

/// UnityPy's `SerializedFile.unity_version` below format 7, where the file
/// stores none (`files/SerializedFile.py:237`).
pub const DEFAULT_UNITY_VERSION: &str = "2.5.0f5";

/// One input's records (`dump_records.py` `_dump_input`). `rel` is the
/// input's dump name, as bytes. It is parsed on a meter of its own.
#[must_use]
pub fn dump_input(rel: &[u8], data: Bytes, opts: &DumpOptions) -> InputDump {
    let ctx = &opts.context;
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    dump_on(rel, data, &meter, opts)
}

/// [`dump_input`] on `meter`, the input's meter, which its bytes may
/// already be charged to ([`crate::read::read_input`]): everything the parse
/// keeps counts toward the same `max_input_memory`.
#[must_use]
pub fn dump_on(rel: &[u8], data: Bytes, meter: &Arc<Meter>, opts: &DumpOptions) -> InputDump {
    let name = RawStr::from(rel);
    let mut l0 = L0 {
        ty: None,
        parsed_as: ParsedAs::Error,
        size: u64::try_from(data.len()).unwrap_or(u64::MAX),
        sha256: sha256(&data),
    };
    let mut out = InputDump {
        sha256: l0.sha256,
        bytes_in: l0.size,
        ..InputDump::default()
    };
    let ctx = &opts.context;
    let ty = match check_file_type(&data, &ctx.limits) {
        Ok(ty) => ty,
        Err(e) => {
            // A file whose detection raises would raise again inside
            // `Environment.load_file` (dump_records.py `_dump_input`).
            let rec = FileRecord {
                file: vec![name],
                l0,
                l1: None,
                error: Some(FileError {
                    kind: FileErrorKind::Detect,
                    note: Some(note(&Error::Detect(e).to_string())),
                }),
            };
            out.push_file(file_line(&rec));
            return out;
        }
    };
    l0.ty = Some(ty);
    if ty == FileType::Zip {
        // Format 1 does not expand zip/apk members (section 8).
        l0.parsed_as = ParsedAs::Zip;
        out.push_file(file_line(&FileRecord {
            file: vec![name],
            l0,
            l1: None,
            error: None,
        }));
        return out;
    }
    let mut records = Records::on(meter);
    let result = parse_file(name.clone(), data, ty, None, meter, ctx).and_then(|file| {
        if opts.lazy_reverse {
            prepass_reverse(&file);
        }
        load_deep(&file)?;
        l0.parsed_as = parsed_as_of(&file);
        let mut w = Writer {
            levels: opts.levels,
            meter,
            records: &mut records,
        };
        w.emit(&file, std::slice::from_ref(&name), l0.clone())
    });
    match result {
        Ok(()) => {
            let Records {
                lines,
                mut charge,
                files,
                objects,
            } = records;
            let (lines, list) = lines.into_parts();
            charge.absorb(list);
            out.lines = lines;
            out.charge = Some(charge);
            out.files = files;
            out.objects = objects;
        }
        Err(e) => {
            // What was made is dropped before the one record is.
            drop(records);
            l0.parsed_as = ParsedAs::Error;
            let (kind, text) = match memory_refused(&e) {
                Some(b) => (
                    FileErrorKind::Crash,
                    format!("unityrs ran out of memory dumping this input ({b})"),
                ),
                None => (FileErrorKind::Parse, e.to_string()),
            };
            out.push_file(file_line(&FileRecord {
                file: vec![name],
                l0,
                l1: None,
                error: Some(FileError {
                    kind,
                    note: Some(note(&text)),
                }),
            }));
        }
    }
    out
}

/// One input's records as they are made: the lines, in a list charged to
/// the input's meter, and the charges of the lines themselves.
struct Records {
    lines: MeteredVec<String>,
    charge: Charge,
    files: u64,
    objects: u64,
}

impl Records {
    fn on(meter: &Arc<Meter>) -> Self {
        Self {
            lines: MeteredVec::new(meter),
            charge: Charge::zero(meter),
            files: 0,
            objects: 0,
        }
    }

    /// Appends a line [`Text::finish`] gave, with its charge.
    fn push(&mut self, (line, charge): (String, Charge)) -> Result<(), ChargeError> {
        self.lines.push(line)?;
        self.charge.absorb(charge);
        Ok(())
    }
}

/// UnityPy's eager load: every directory entry of every bundle parsed, in
/// order, nested bundles included (`files/File.py:76-90`, called from each
/// `BundleFile.__init__`); the first failure fails the input.
pub fn load_deep(file: &File) -> Result<(), Error> {
    if let File::Bundle(b) = file {
        b.load_all()?;
        for i in 0..b.directory().len() {
            let entry = b.entry(i)?;
            load_deep(&entry)?;
        }
    }
    Ok(())
}

/// G1c's pre-pass ([`DumpOptions::lazy_reverse`]): every directory entry of
/// every bundle parsed lazily, in reverse directory order, a nested bundle's
/// entries as soon as it is parsed, each read decoding only the blocks its
/// span needs. Failures are left where they are kept: [`load_deep`] meets
/// each again, or anew where it is not kept (a meter refusal), so the
/// dump's own pass decides the records.
pub fn prepass_reverse(file: &File) {
    if let File::Bundle(b) = file {
        for i in (0..b.directory().len()).rev() {
            if let Ok(entry) = b.entry(i) {
                prepass_reverse(&entry);
            }
        }
    }
}

/// What UnityPy built (`dump_records.py` `parsed_as`).
#[must_use]
pub fn parsed_as_of(file: &File) -> ParsedAs {
    match file {
        File::Serialized(_) => ParsedAs::SerializedFile,
        File::Bundle(_) => ParsedAs::BundleFile,
        File::Web(_) => ParsedAs::WebFile,
        File::Raw(_) => ParsedAs::Raw,
    }
}

struct Writer<'a> {
    levels: Levels,
    /// The input's meter: every line is charged to it.
    meter: &'a Arc<Meter>,
    records: &'a mut Records,
}

impl Writer<'_> {
    /// `emit_file`: one file record, its objects by path id, then its
    /// entries in `files` order. It recurses once a level of nesting, so
    /// its frame is kept to what the recursion needs: the records are made
    /// in frames of their own (`CONTAINER_LEVEL_STACK`, `run/mod.rs`).
    fn emit(&mut self, file: &File, path: &[RawStr], l0: L0) -> Result<(), Error> {
        self.file_record(file, path, l0)?;
        match file {
            File::Serialized(sf) => self.object_records(sf, path),
            File::Bundle(b) => {
                for slot in b.files() {
                    let (child, l0) = entry_l0(b, slot.dir_index)?;
                    let mut child_path = path.to_vec();
                    child_path.push(slot.name.clone());
                    self.emit(&child, &child_path, l0)?;
                }
                Ok(())
            }
            // A WebFile's entries: P3 (core refuses WebFiles in P1, plan section 1).
            File::Web(_) | File::Raw(_) => Ok(()),
        }
    }

    /// `file`'s own record.
    #[inline(never)]
    fn file_record(&mut self, file: &File, path: &[RawStr], l0: L0) -> Result<(), Error> {
        let l1 = if self.levels.l1 {
            match file {
                File::Serialized(sf) => Some(FileL1::Serialized(Box::new(serialized_l1(sf)?))),
                File::Bundle(b) => Some(FileL1::Bundle(bundle_l1(b))),
                File::Web(w) => Some(FileL1::Web(web_l1(w))),
                File::Raw(_) => None,
            }
        } else {
            None
        };
        let mut line = Text::on(self.meter);
        file_line_into(
            &mut line,
            &FileRecord {
                file: path.to_vec(),
                l0,
                l1,
                error: None,
            },
        );
        self.records.push(line.finish()?)?;
        self.records.files += 1;
        Ok(())
    }

    /// The records of `sf`'s objects, by path id.
    #[inline(never)]
    fn object_records(&mut self, sf: &Arc<SerializedFile>, path: &[RawStr]) -> Result<(), Error> {
        let mut objs: Vec<ObjectRef> = sf.objects().collect();
        objs.sort_by_key(ObjectRef::path_id);
        for obj in &objs {
            let (rec, value) = object_record(obj, self.levels, self.meter)?;
            let mut line = Text::on(self.meter);
            object_line_into(&mut line, path, &rec);
            // The value's text is in the line now.
            drop((rec, value));
            self.records.push(line.finish()?)?;
            self.records.objects += 1;
        }
        Ok(())
    }
}

/// A bundle entry, and its `l0`: its type is what the core's detection of
/// it gave (`BundleFile::entry`), counted as the open's work then; detecting
/// the bytes again would inflate a gzip entry a second time, uncounted.
#[inline(never)]
fn entry_l0(b: &BundleFile, dir_index: usize) -> Result<(File, L0), Error> {
    let bytes = b.entry_bytes(dir_index)?;
    let child = b.entry(dir_index)?;
    let l0 = L0 {
        ty: Some(child.file_type()),
        parsed_as: parsed_as_of(&child),
        size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        sha256: sha256(&bytes),
    };
    Ok((child, l0))
}

fn bundle_l1(b: &BundleFile) -> BundleL1 {
    let h = b.header();
    let fs = h.layout == LayoutKind::Fs;
    let row = |bi: &unityrs_core::files::BlockInfo| BlockRow {
        uncompressed: bi.uncompressed,
        compressed: bi.bytes_read,
        flags: bi.flags,
    };
    BundleL1 {
        signature: h.signature.clone(),
        format: h.format,
        version_player: h.version_player.clone(),
        version_engine: h.version_engine.clone(),
        engine_version_packed: match &h.engine_version {
            Some(Ok(v)) => Some(v.version.packed().clone()),
            _ => None,
        },
        flags: h.data_flags,
        flags_class: h.flags_class,
        block_alignment: h.block_alignment,
        encrypted: h.encrypted,
        hash: h.hash,
        crc: h.crc,
        blocks_info: h.blocks_info.as_ref().map(row),
        blocks: fs.then(|| b.blocks().iter().map(row).collect()),
        directory: dir_rows(b.directory()),
    }
}

fn dir_rows(dir: &[unityrs_core::files::DirEntry]) -> Vec<DirRow> {
    dir.iter()
        .map(|d| DirRow {
            offset: d.offset,
            size: d.size,
            flags: d.flags,
            path: d.path.clone(),
        })
        .collect()
}

fn web_l1(w: &WebFile) -> WebL1 {
    WebL1 {
        signature: w.signature().clone(),
        packer: match w.packer() {
            WebPacker::None => "none",
            WebPacker::Gzip => "gzip",
            WebPacker::Brotli => "brotli",
        },
        directory: dir_rows(w.directory()),
    }
}

fn type_record(t: &unityrs_core::files::SerializedType) -> TypeRecord {
    TypeRecord {
        class_id: t.class_id,
        is_stripped_type: t.is_stripped_type,
        script_type_index: t.script_type_index,
        script_id: t.script_id,
        old_type_hash: t.old_type_hash,
        nodes: t.nodes.clone(),
        type_dependencies: t.type_dependencies.as_ref().map(|d| d.to_vec()),
        class_name: t.class_name.clone(),
        namespace: t.namespace.clone(),
        assembly: t.assembly.clone(),
    }
}

/// `serialized_l1`.
fn serialized_l1(sf: &Arc<SerializedFile>) -> Result<SerializedL1, Error> {
    let h = sf.header();
    let infos = sf.object_infos();
    Ok(SerializedL1 {
        header: SerializedHeaderRow {
            metadata_size: h.metadata_size,
            file_size: h.file_size,
            version: h.version,
            data_offset: h.data_offset,
            big_endian: h.endian == Endian::Big,
            reserved: h.reserved,
            unknown: h.unknown,
        },
        unity_version: sf
            .unity_version()
            .cloned()
            .unwrap_or_else(|| RawStr::from(DEFAULT_UNITY_VERSION)),
        version_packed: sf.version().map(|v| v.version.packed().clone()),
        target_platform: sf.target_platform(),
        enable_type_tree: sf.enable_type_tree(),
        big_id_enabled: sf.big_id_enabled(),
        types: sf.types().iter().map(type_record).collect(),
        object_order: sf
            .object_order()
            .iter()
            .filter_map(|i| usize::try_from(*i).ok().and_then(|i| infos.get(i)))
            .map(|o| o.path_id)
            .collect(),
        script_types: sf
            .script_types()
            .iter()
            .map(|s| (s.file_index, s.identifier))
            .collect(),
        externals: sf
            .externals()
            .iter()
            .map(|e| ExternalRow {
                temp_empty: e.temp_empty.clone(),
                guid: e.guid,
                kind: e.kind,
                path: e.path.clone(),
            })
            .collect(),
        ref_types: sf.ref_types().map(|t| t.iter().map(type_record).collect()),
        user_information: sf.user_information().cloned(),
        container: sf
            .container()?
            .rows()
            .iter()
            .map(|r| ContainerRow {
                path: r.path.clone(),
                preload_index: r.preload_index,
                preload_size: r.preload_size,
                file_id: r.file_id,
                path_id: r.path_id,
            })
            .collect(),
    })
}

/// `error_kind` on a `check_read` failure (`dump_records.py`): its regex,
/// `Expected to read (\d+) bytes, but only read (\d+) bytes`, has no sign,
/// so a negative `bytes_read` (the pure-Python reader after a negative
/// `TypelessData` length) falls through every test to `other`, with no
/// `bytes_read`, and the oracle reads no value for it
/// (`tests/fixtures/dump/l2_kinds.json` records a real read that does this).
#[must_use]
pub fn check_read_kind(bytes_read: i64) -> (L2Kind, Option<i64>) {
    if bytes_read >= 0 {
        (L2Kind::CheckRead, Some(bytes_read))
    } else {
        (L2Kind::Other, None)
    }
}

/// The L2 outcome of a read: the error, if any, and the value to encode.
fn l2_outcome(result: Result<Parsed, ParseError>) -> (Option<L2Error>, Option<ParsedValue>) {
    match result {
        Ok(p) => (None, Some(ParsedValue::Owned(p))),
        Err(e) => {
            let text = note(&e.to_string());
            if let ParseError::CheckRead { parsed, .. } = e {
                let (kind, bytes_read) = check_read_kind(parsed.bytes_read);
                let error = L2Error {
                    kind,
                    bytes_read,
                    note: Some(text),
                };
                let value = (kind == L2Kind::CheckRead).then_some(ParsedValue::Shared(parsed));
                return (Some(error), value);
            }
            (
                Some(L2Error {
                    kind: e.l2_kind(),
                    bytes_read: None,
                    note: Some(text),
                }),
                None,
            )
        }
    }
}

/// A read value: owned on success, shared by a `check_read` error.
enum ParsedValue {
    Owned(Parsed),
    Shared(Arc<Parsed>),
}

impl ParsedValue {
    fn get(&self) -> &Parsed {
        match self {
            Self::Owned(p) => p,
            Self::Shared(p) => p,
        }
    }
}

/// `object_record`, its value's text charged to `meter`: the record, and
/// the charge for that text, which must live as long as the record does.
/// A refusal of the text's growth fails the input.
fn object_record(
    obj: &ObjectRef,
    levels: Levels,
    meter: &Arc<Meter>,
) -> Result<(ObjectRecord, Option<Charge>), Error> {
    let info = obj.info();
    let l1 = levels.l1.then_some(ObjectL1 {
        type_id: info.type_id,
        class_id: info.class_id,
        byte_start: info.byte_start,
        byte_size: info.byte_size,
        is_destroyed: info.is_destroyed,
        is_stripped: info.is_stripped,
    });
    let mut l2 = None;
    let mut l3 = None;
    // The value a full parse gave, kept for the resave (`parsed_ok`).
    let mut resavable: Option<Parsed> = None;
    let mut held = None;
    if levels.l2 {
        let mut out = L2 {
            root_type: None,
            source: None,
            error: None,
            value: None,
        };
        // `_get_typetree_node()`: every failure to find a tree is `no_typetree`.
        match obj.tree(None) {
            Ok(Some(t)) => {
                out.root_type = Some(RawStr::from(t.tree.root_type()));
                out.source = Some(match t.source {
                    TreeSource::Embedded => "embedded",
                    TreeSource::Provider(name) => name,
                    TreeSource::Explicit => "explicit",
                });
                let parsed = obj.parse_as_dict(true);
                if let Err(e) = &parsed {
                    out_of_memory(e)?;
                }
                let (error, value) = l2_outcome(parsed);
                out.error = error;
                if let Some(v) = value {
                    let mut text = Text::on(meter);
                    let encoded = encode_into(&mut text, v.get());
                    // A refused growth fails the input, whatever the encoder said.
                    let (text, charge) = text.finish()?;
                    match encoded {
                        Ok(()) => {
                            out.value = Some(text);
                            held = Some(charge);
                            if let ParsedValue::Owned(p) = v
                                && out.error.is_none()
                            {
                                resavable = Some(p);
                            }
                        }
                        Err(e) => {
                            // Only a producer under test writes `encode` (section 4.3).
                            out.error = Some(L2Error {
                                kind: L2Kind::Encode,
                                bytes_read: None,
                                note: Some(note(&e.to_string())),
                            });
                            out.value = None;
                        }
                    }
                }
            }
            Ok(None) => {
                out.error = Some(L2Error {
                    kind: L2Kind::NoTypetree,
                    bytes_read: None,
                    note: Some(format!("no typetree for class {}", info.class_id)),
                });
            }
            Err(e) => {
                out_of_memory(&e)?;
                out.error = Some(L2Error {
                    kind: L2Kind::NoTypetree,
                    bytes_read: None,
                    note: Some(note(&e.to_string())),
                });
            }
        }
        l2 = Some(out);
    }
    if levels.l3 {
        l3 = Some(l3_of(obj, resavable.as_ref())?);
    }
    Ok((
        ObjectRecord {
            path_id: info.path_id,
            l1,
            l2,
            l3,
        },
        held,
    ))
}

/// The object's `l3`: its raw bytes, and the resave of `resavable`, the
/// value a full parse gave. A resave the budget or the allocator refuses
/// fails the input ([`out_of_memory`]).
fn l3_of(obj: &ObjectRef, resavable: Option<&Parsed>) -> Result<L3, Error> {
    let raw = obj.raw_data();
    let resave = match resavable {
        None => Resave::Skipped,
        Some(p) => match obj.write_typetree(p, None, WriteRules::UnityPy) {
            Ok(bytes) if bytes == raw => Resave::Equal,
            Ok(bytes) => Resave::Differs {
                size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                sha256: sha256(&bytes),
                first_diff: first_difference(&bytes, &raw),
            },
            Err(WriteError::Charge(c)) if charge_refused(&c).is_some() => return Err(c.into()),
            Err(e) => Resave::Error {
                note: Some(note(&e.to_string())),
            },
        },
    };
    Ok(L3 {
        raw_size: u64::try_from(raw.len()).unwrap_or(u64::MAX),
        raw_sha256: sha256(&raw),
        resave,
    })
}

/// `Err(Error::Budget)` when `e` is the caller's budget or the allocator
/// refusing memory to an object's read (its value, its tree, or the load
/// `m_Script` names): the oracle's `MemoryError`, which is never an L2 or
/// L3 outcome but the input's crash record (`CANONICAL_DUMP.md` 4.2). It
/// depends on the machine and on what the other inputs hold, not on the
/// input. The input's own bound, `InputMemory`, stays the object's limit
/// refusal.
fn out_of_memory(e: &ParseError) -> Result<(), Error> {
    read_refused(e).map_or(Ok(()), |b| Err(Error::Budget(b.clone())))
}

/// The budget's or the allocator's refusal under `e`, through every error
/// that wraps one (an entry's, the load-time AssetBundle read's, a
/// dereference's load), or `None`: what [`dump_on`] makes a `crash` record.
fn memory_refused(e: &Error) -> Option<&BudgetExceeded> {
    match e {
        Error::Budget(b) => Some(b),
        Error::Tree(t) => tree_refused(t),
        Error::Entry { source, .. } => memory_refused(source),
        Error::AssetBundle(p) => read_refused(p),
        Error::Deref(d) => deref_refused(d),
        _ => None,
    }
}

fn read_refused(e: &ParseError) -> Option<&BudgetExceeded> {
    match e {
        ParseError::Read(r) => match &r.detail {
            ReadDetail::Charge(c) => charge_refused(c),
            ReadDetail::Tree(t) => tree_refused(t),
            _ => None,
        },
        ParseError::Tree(t) => tree_refused(t),
        ParseError::Deref(d) => deref_refused(d),
        _ => None,
    }
}

fn deref_refused(e: &DerefError) -> Option<&BudgetExceeded> {
    match e {
        DerefError::Load(e) => memory_refused(e),
        _ => None,
    }
}

fn tree_refused(e: &TreeError) -> Option<&BudgetExceeded> {
    match e {
        TreeError::Charge(c) => charge_refused(c),
        _ => None,
    }
}

fn charge_refused(e: &ChargeError) -> Option<&BudgetExceeded> {
    match e {
        ChargeError::Budget(b) => Some(b),
        ChargeError::Limit(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gzip stream of `n` zero bytes, as detection inflates it.
    fn gzip_of_zeros(n: usize) -> Vec<u8> {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&vec![0; n]).unwrap();
        let mut out = gz.finish().unwrap();
        // Detection reads a gzip stream only in a file of 128 bytes or more.
        out.resize(out.len().max(128), 0);
        out
    }

    #[test]
    fn a_detection_refused_at_its_limit_names_the_limit() {
        let mut ctx = unityrs_core::FileContext::unitypy();
        ctx.limits.max_inflate = 1000;
        let opts = DumpOptions {
            context: Arc::new(ctx),
            ..DumpOptions::default()
        };
        let lines = dump_input(b"big.gz", Bytes::from(gzip_of_zeros(5000)), &opts).lines;
        assert_eq!(lines.len(), 1);
        // Every limit refusal's text (`tests/limits.rs` reads it as
        // `tools/parity/g3b.py` does).
        assert!(
            lines[0].contains(r#""error":{"kind":"detect","note":"detection: limit Inflate: "#)
                && lines[0].ends_with(" > 1000\"}}"),
            "{}",
            lines[0]
        );
        // Under the bound the same stream is no limit refusal.
        let opts = DumpOptions::default();
        let lines = dump_input(b"big.gz", Bytes::from(gzip_of_zeros(5000)), &opts).lines;
        assert!(!lines[0].contains("limit Inflate"), "{}", lines[0]);
        // Any other detection failure keeps its own text.
        let mut twenty = vec![b'x'; 20];
        twenty[0] = b'y';
        let lines = dump_input(b"x", Bytes::from(twenty), &opts).lines;
        assert!(
            lines[0].contains(r#""note":"detection: unterminated"#),
            "{}",
            lines[0]
        );
    }

    #[test]
    fn a_negative_check_read_is_other_as_the_oracle_classifies_it() {
        assert_eq!(check_read_kind(8), (L2Kind::CheckRead, Some(8)));
        assert_eq!(check_read_kind(0), (L2Kind::CheckRead, Some(0)));
        assert_eq!(check_read_kind(-4), (L2Kind::Other, None));
    }
}
