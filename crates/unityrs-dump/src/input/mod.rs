//! One input's records, from `unityrs-core`'s public API: a port of
//! `tools/parity/dump_records.py`'s `_dump_input`, `emit_file`,
//! `object_record` (`object`) and the L1 builders (`l1`).
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
//! until the run has written them ([`InputDump::charge`]). So is everything
//! else the dump holds in proportion to the input while it writes a record:
//! the copies of the core's tables a file record's `l1` is built from
//! (`l1`), its path, and the stack of names the recursion walks
//! (`Writer::path`); a SerializedFile's objects are taken in the order the
//! core keeps by path id, so ordering them costs nothing. Notes are formatted
//! no further than they are kept ([`crate::record::note_of`]), and a limit
//! refusal's text is kept whole however long the names before it
//! ([`crate::record::note_keeping`]: `tools/parity/g3b.py` reads it). A refusal fails
//! the input as any core error does: the meter's own bound is a `parse`
//! record whose note is the limit (`limit InputMemory: <v> > <max>`), and a
//! refusal of the caller's budget or of the allocator (an out-of-memory,
//! met by the records or by the core, an object's read or tree included, a
//! TPK provider's walk among them, which is never that object's L2 outcome:
//! `refused`) is a `crash` record, the oracle's record for a `MemoryError`
//! (`CANONICAL_DUMP.md` section 4.2); either way the records made so far are
//! dropped first, and the run goes on.
//!
//! **Work.** Every byte the dump hashes or compares (the input's own bytes,
//! each bundle entry's bytes, each object's raw bytes and its resave's,
//! each value it writes as a length and a sha256:
//! [`crate::text::Sink::count_work`]) is counted as the open's work on the
//! input's meter first ([`Meter::count_decode`], the `max_decode_work` the
//! core's block decodes count against), because directory rows, objects
//! and values may overlap: K rows over the same S decoded bytes cost the
//! core a slice each, and would cost the dump K x S of hashing. Past the
//! allowance the input is its `limit DecodeWork: <v> > <max>` refusal
//! (`PARITY.md` section 19.3, "the dump's hashing"). The input's own bytes
//! are hashed first, before its detection: an input they alone take past
//! the allowance is refused before it is detected, a `detect` record whose
//! digest was not taken, as [`crate::read`] writes for a refused input
//! whose hashing passes it. The input's detection counts on the same meter
//! every byte a gzip stream's inflation reads or writes
//! ([`check_entry_type`], as the core detects an entry), and so does the
//! conversion of a packed version past 128 bits to decimal
//! ([`crate::json::write_packed`]): an input that passes the allowance
//! there is its `detect` record (`detection: limit DecodeWork: <v> >
//! <max>`), or its `parse` record.
//!
//! **Context**: the dump reads with the [`DumpOptions::context`] it is
//! given, `FileContext::unitypy()` by default, and nothing else.

use std::sync::Arc;

use unityrs_core::budget::Reservation;
use unityrs_core::detect::{FileType, ParsedAs, check_entry_type};
use unityrs_core::files::{File, parse_file};
use unityrs_core::limits::{Charge, ChargeError, Meter, MeteredVec};
use unityrs_core::{BundleFile, Bytes, Error, RawStr, SerializedFile};

use crate::record::{
    FileError, FileErrorKind, FileL1, FileRecord, L0, file_line, file_line_into, note,
    object_line_into,
};
use crate::text::Text;
use crate::{DumpOptions, InputDump, Levels};

mod l1;
mod object;
mod refused;

pub use l1::DEFAULT_UNITY_VERSION;
pub use object::check_read_kind;

use refused::memory_refused;

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
    let size = u64::try_from(data.len()).unwrap_or(u64::MAX);
    let sha256 = match hashed(meter, &data) {
        Ok(sha) => sha,
        Err(e) => return not_taken(rel, size, &e),
    };
    let mut l0 = L0 {
        ty: None,
        parsed_as: ParsedAs::Error,
        size,
        sha256,
    };
    let mut out = InputDump {
        sha256: Some(sha256),
        bytes_in: size,
        ..InputDump::default()
    };
    let ctx = &opts.context;
    // A gzip stream's detection counts as an entry's does (module docs).
    let ty = match check_entry_type(&data, &ctx.limits, meter) {
        Ok(ty) => ty,
        Err(e) => {
            // A file whose detection raises would raise again inside
            // `Environment.load_file` (dump_records.py `_dump_input`).
            let error = FileError {
                kind: FileErrorKind::Detect,
                note: Some(refused::note(&Error::Detect(e))),
            };
            return with_record(out, name, l0, Some(error));
        }
    };
    l0.ty = Some(ty);
    if ty == FileType::Zip {
        // Format 1 does not expand zip/apk members (section 8).
        l0.parsed_as = ParsedAs::Zip;
        return with_record(out, name, l0, None);
    }
    let mut records = Records::on(meter);
    let result = parse_file(name.clone(), data, ty, None, meter, ctx).and_then(|file| {
        if opts.lazy_reverse {
            prepass_reverse(&file);
        }
        load_deep(&file)?;
        l0.parsed_as = parsed_as_of(&file);
        let mut path = MeteredVec::new(meter);
        path.push(name.clone())?;
        let mut w = Writer {
            levels: opts.levels,
            meter,
            records: &mut records,
            path,
        };
        w.emit(&file, l0.clone())
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
                    note(&format!(
                        "unityrs ran out of memory dumping this input ({b})"
                    )),
                ),
                None => (FileErrorKind::Parse, refused::note(&e)),
            };
            out.crashes = u64::from(kind == FileErrorKind::Crash);
            out.push_file(file_line(&FileRecord {
                file: vec![name],
                l0,
                l1: None,
                error: Some(FileError {
                    kind,
                    note: Some(text),
                }),
            }));
        }
    }
    out
}

/// `out` with its input's one `file` record: `l0`, no `l1`, and `error`.
fn with_record(mut out: InputDump, name: RawStr, l0: L0, error: Option<FileError>) -> InputDump {
    out.push_file(file_line(&FileRecord {
        file: vec![name],
        l0,
        l1: None,
        error,
    }));
    out
}

/// The one record of an input whose own bytes the allowance refused
/// (`refusal`): refused before its detection, its sha256 not taken
/// ([`crate::read::refused_before_detection`]).
fn not_taken(rel: &[u8], size: u64, refusal: &ChargeError) -> InputDump {
    let text = crate::read::not_taken(refusal);
    crate::read::refused_before_detection(rel, size, None, FileErrorKind::Detect, note(&text))
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

/// Counts `bytes` as the open's work before the dump hashes or compares
/// them (module docs, "Work"): refused past `max_decode_work`, with nothing
/// done.
fn count_hashed(meter: &Meter, bytes: &[u8]) -> Result<(), ChargeError> {
    meter.count_decode(u64::try_from(bytes.len()).unwrap_or(u64::MAX))
}

/// The sha256 of `bytes`, counted first ([`count_hashed`]).
fn hashed(meter: &Meter, bytes: &[u8]) -> Result<[u8; 32], ChargeError> {
    count_hashed(meter, bytes)?;
    Ok(crate::json::sha256(bytes))
}

struct Writer<'a> {
    levels: Levels,
    /// The input's meter: every line is charged to it.
    meter: &'a Arc<Meter>,
    records: &'a mut Records,
    /// The identity of the file being written (`[input, entry, ...]`): one
    /// stack the recursion pushes each entry's name onto, charged to the
    /// meter, not a copy a level.
    path: MeteredVec<RawStr>,
}

impl Writer<'_> {
    /// `emit_file`: one file record, its objects by path id, then its
    /// entries in `files` order. It recurses once a level of nesting, so
    /// its frame is kept to what the recursion needs: the records are made
    /// in frames of their own (`CONTAINER_LEVEL_STACK`, `run/mod.rs`).
    fn emit(&mut self, file: &File, l0: L0) -> Result<(), Error> {
        self.file_record(file, l0)?;
        match file {
            File::Serialized(sf) => self.object_records(sf),
            File::Bundle(b) => {
                for slot in b.files() {
                    let (child, l0) = entry_l0(b, slot.dir_index, self.meter)?;
                    let depth = self.path.len();
                    self.path.push(slot.name.clone())?;
                    self.emit(&child, l0)?;
                    self.path.truncate(depth);
                }
                Ok(())
            }
            // A WebFile's entries: P3 (core refuses WebFiles in P1, plan section 1).
            File::Web(_) | File::Raw(_) => Ok(()),
        }
    }

    /// `file`'s own record. Its `l1` and its path are copies of the core's
    /// tables, charged to `held` until its line is written (`l1`).
    #[inline(never)]
    fn file_record(&mut self, file: &File, l0: L0) -> Result<(), Error> {
        let mut held = Charge::zero(self.meter);
        let l1 = if self.levels.l1 {
            match file {
                File::Serialized(sf) => Some(FileL1::Serialized(Box::new(l1::serialized_l1(
                    sf, &mut held,
                )?))),
                File::Bundle(b) => Some(FileL1::Bundle(l1::bundle_l1(b, &mut held)?)),
                File::Web(w) => Some(FileL1::Web(l1::web_l1(w, &mut held)?)),
                File::Raw(_) => None,
            }
        } else {
            None
        };
        let path = self.path.as_slice();
        let rec = FileRecord {
            file: l1::copy(&mut held, path.len(), path.iter().cloned())?,
            l0,
            l1,
            error: None,
        };
        let mut line = Text::on(self.meter);
        file_line_into(&mut line, &rec);
        // The copies go before their charge does.
        drop(rec);
        drop(held);
        self.records.push(line.finish()?)?;
        self.records.files += 1;
        Ok(())
    }

    /// The records of `sf`'s objects, by path id: the order the core keeps
    /// ([`SerializedFile::objects_by_path`]), so nothing is copied or
    /// sorted.
    #[inline(never)]
    fn object_records(&mut self, sf: &Arc<SerializedFile>) -> Result<(), Error> {
        for obj in sf.objects_by_path() {
            let (rec, value) = object::object_record(&obj, self.levels, self.meter)?;
            let mut line = Text::on(self.meter);
            object_line_into(&mut line, self.path.as_slice(), &rec);
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
/// the bytes again would inflate a gzip entry a second time, uncounted. Its
/// bytes are counted as the open's work before they are hashed
/// ([`hashed`]): directory rows may overlap.
#[inline(never)]
fn entry_l0(b: &BundleFile, dir_index: usize, meter: &Meter) -> Result<(File, L0), Error> {
    let bytes = b.entry_bytes(dir_index)?;
    let child = b.entry(dir_index)?;
    let l0 = L0 {
        ty: Some(child.file_type()),
        parsed_as: parsed_as_of(&child),
        size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        sha256: hashed(meter, &bytes)?,
    };
    Ok((child, l0))
}

#[cfg(test)]
mod tests {
    use unityrs_core::walk::L2Kind;

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
