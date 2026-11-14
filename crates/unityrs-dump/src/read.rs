//! One input's bytes, as the run reads them: `dump_records.py`
//! `_dump_input`'s `fh.read()`, bounded and charged.
//!
//! The bytes go through the core's [`read_path`], the reader of
//! [`unityrs_core::env::Source::Path`]: a file longer than
//! [`Limits::max_input_bytes`](unityrs_core::limits::Limits::max_input_bytes)
//! is refused from its length, unread (`LimitKind::InputBytes`), and the
//! buffer is charged to the input's meter before it is allocated, the meter
//! the input is then parsed on, so its bytes count toward its
//! `max_input_memory` with everything its parse keeps.
//!
//! **A refusal costs only its input.** An input refused at a limit is one
//! `file` record of kind `detect` (it was refused before its detection),
//! whose note is the limit's own text, `limit <Kind>: <value> > <max>`, so
//! `tools/parity/g3b.py` reads it as every other limit refusal; one whose
//! buffer the allocator or the caller's memory budget refused is one record
//! of kind `crash` (the input ran its dumper out of memory: the oracle's
//! record for a `MemoryError`, `CANONICAL_DUMP.md` section 4.2). Either way
//! the run goes on, as the oracle's does past an input whose process ran
//! out of memory. Any other failure to read an input (one that cannot be
//! opened, an I/O error) is no outcome of the input and stops the run, as
//! the oracle's `InputUnreadable` does.
//!
//! **The refused input's `l0`** is `"error"` with `parsed_as` `"error"`:
//! its `size` is its length as the file system gives it, and its `sha256`
//! that of its first `size` bytes, read in chunks of [`CHUNK`] bytes and
//! kept nowhere. Nothing bounds that length (the input was refused for
//! it), so each chunk is counted as the open's work on the input's meter
//! before it is hashed, against the `max_decode_work` every other byte the
//! dump hashes counts against ([`crate::input`], "Work"). Within the
//! allowance a corpus index's pin still names the file
//! (`--allow-index-mismatch`). Past it the digest cannot be had: the chunk
//! the count refuses is not hashed and nothing past it is read, `sha256` is
//! [`NOT_TAKEN`], `size` is the length the file system gives, and the note
//! adds [`not_taken`]'s text, `its sha256 was not taken: limit DecodeWork:
//! <value> > <max>`. The input's sha256 is then `None`
//! ([`InputDump::sha256`]), which matches no pin.

use std::fmt;
use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use unityrs_core::budget::Reservation;
use unityrs_core::detect::ParsedAs;
use unityrs_core::env::read_path;
use unityrs_core::limits::{ChargeError, Meter};
use unityrs_core::{Bytes, Error, FileContext, RawStr};

use crate::InputDump;
use crate::record::{FileError, FileErrorKind, FileRecord, L0, file_line, note, note_keeping};

/// An input as the run read it.
#[derive(Debug)]
pub enum ReadInput {
    /// Its bytes, charged to `meter`, the meter it is parsed on.
    Bytes {
        /// The bytes.
        data: Bytes,
        /// The input's meter.
        meter: Arc<Meter>,
    },
    /// Refused: its one record.
    Refused(InputDump),
}

const _: () = crate::send_sync::<ReadInput>();

/// The bytes a refused input is hashed in, each chunk counted before it is
/// hashed (module docs).
pub const CHUNK: usize = 1 << 16;

/// The `l0.sha256` of an input whose digest was not taken: hashing its
/// bytes would pass its open's `max_decode_work` (module docs;
/// `CANONICAL_DUMP.md` section 4.2). It keeps the oracle's format, 64 hex
/// digits, and no input is known to hash to it.
pub const NOT_TAKEN: [u8; 32] = [0; 32];

/// The text a note gives an input whose sha256 was not taken, `refusal`
/// being the work allowance's refusal (`limit DecodeWork: <v> > <max>`).
#[must_use]
pub fn not_taken(refusal: &impl fmt::Display) -> String {
    format!("its sha256 was not taken: {refusal}")
}

/// Reads the input at `path`, named `name` in the dump, under `ctx`'s
/// limits and budget, on a meter of its own ([`read_on`]).
///
/// # Errors
/// A failure that is no outcome of the input: it cannot be opened, or an
/// I/O error.
pub fn read_input(name: &[u8], path: &Path, ctx: &FileContext) -> std::io::Result<ReadInput> {
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    read_on(name, path, ctx, meter)
}

/// [`read_input`] on `meter`, the input's meter: the bytes are charged to
/// it, and a refused input's hashing is counted on it (module docs).
///
/// # Errors
/// As [`read_input`].
pub fn read_on(
    name: &[u8],
    path: &Path,
    ctx: &FileContext,
    meter: Arc<Meter>,
) -> std::io::Result<ReadInput> {
    let (kind, text) = match read_path(path, &ctx.limits, &meter) {
        Ok(data) => return Ok(ReadInput::Bytes { data, meter }),
        Err(Error::Limit(l)) => (FileErrorKind::Detect, l.to_string()),
        Err(e @ (Error::Budget(_) | Error::Allocator(_))) => (
            FileErrorKind::Crash,
            format!("unityrs ran out of memory reading this input ({e})"),
        ),
        Err(Error::Io(e)) => return Err(std::io::Error::new(e.0.kind(), e.0.to_string())),
        // read_path only reads bytes; it cannot produce UnityCN errors.
        // Those arise in dump_on's parse and become per-input parse records.
        Err(e) => return Err(std::io::Error::other(e.to_string())),
    };
    let (size, digest) = prefix_sha256(path, &meter)?;
    let (sha256, note) = match digest {
        Ok(sha) => (Some(sha), note(&text)),
        Err(e) => {
            let limit = match &e {
                ChargeError::Limit(l) => Some(l),
                _ => None,
            };
            let text = format!("{text}; {}", not_taken(&e));
            (None, note_keeping(&text, limit))
        }
    };
    Ok(ReadInput::Refused(refused_before_detection(
        name, size, sha256, kind, note,
    )))
}

/// The one record of an input refused before it was detected: `l0` of type
/// and `parsed_as` `"error"`, its `size`, and its `sha256` ([`NOT_TAKEN`]
/// when it is `None`, not taken), with an error of `kind` and `note`.
#[must_use]
pub fn refused_before_detection(
    name: &[u8],
    size: u64,
    sha256: Option<[u8; 32]>,
    kind: FileErrorKind,
    note: String,
) -> InputDump {
    let rec = FileRecord {
        file: vec![RawStr::from(name)],
        l0: L0 {
            ty: None,
            parsed_as: ParsedAs::Error,
            size,
            sha256: sha256.unwrap_or(NOT_TAKEN),
        },
        l1: None,
        error: Some(FileError {
            kind,
            note: Some(note),
        }),
    };
    let mut out = InputDump {
        sha256,
        bytes_in: size,
        crashes: u64::from(kind == FileErrorKind::Crash),
        ..InputDump::default()
    };
    out.push_file(file_line(&rec));
    out
}

/// The file's length as `stat` gives it, and the sha256 of its first that
/// many bytes (fewer, if it is shorter by now), read in chunks of
/// [`CHUNK`] bytes, each counted as work on `meter` before it is hashed.
/// When the count refuses one, it is not hashed and nothing past it is
/// read: the length is the stat'd one and the digest the refusal.
fn prefix_sha256(
    path: &Path,
    meter: &Meter,
) -> std::io::Result<(u64, Result<[u8; 32], ChargeError>)> {
    let file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let mut hasher = Sha256::new();
    let mut chunk = vec![0u8; CHUNK];
    let mut rest = file.take(len);
    let mut size = 0u64;
    loop {
        match rest.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let n64 = u64::try_from(n).unwrap_or(u64::MAX);
                if let Err(e) = meter.count_decode(n64) {
                    return Ok((len, Err(e)));
                }
                hasher.update(chunk.get(..n).unwrap_or_default());
                size = size.saturating_add(n64);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok((size, Ok(hasher.finalize().into())))
}
