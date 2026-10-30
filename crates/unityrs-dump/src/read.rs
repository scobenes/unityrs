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
//! the oracle's `DumperError` does.
//!
//! **The refused input's `l0`** is `"error"` with `parsed_as` `"error"`:
//! its `size` is its length as the file system gives it, and its `sha256`
//! that of its first `size` bytes, read in fixed chunks and kept nowhere, so
//! a corpus index's pin still names the file (`--allow-index-mismatch`).

use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use unityrs_core::budget::Reservation;
use unityrs_core::detect::ParsedAs;
use unityrs_core::env::read_path;
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, Error, FileContext, RawStr};

use crate::InputDump;
use crate::record::{FileError, FileErrorKind, FileRecord, L0, file_line, note};

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

/// Reads the input at `path`, named `name` in the dump, under `ctx`'s
/// limits and budget.
///
/// # Errors
/// A failure that is no outcome of the input: it cannot be opened, or an
/// I/O error.
pub fn read_input(name: &[u8], path: &Path, ctx: &FileContext) -> std::io::Result<ReadInput> {
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let (kind, text) = match read_path(path, &ctx.limits, &meter) {
        Ok(data) => return Ok(ReadInput::Bytes { data, meter }),
        Err(Error::Limit(l)) => (FileErrorKind::Detect, l.to_string()),
        Err(e @ Error::Budget(_)) => (
            FileErrorKind::Crash,
            format!("unityrs ran out of memory reading this input ({e})"),
        ),
        Err(Error::Io(e)) => return Err(std::io::Error::new(e.0.kind(), e.0.to_string())),
        Err(e) => return Err(std::io::Error::other(e.to_string())),
    };
    drop(meter);
    let (size, sha256) = prefix_sha256(path)?;
    let rec = FileRecord {
        file: vec![RawStr::from(name)],
        l0: L0 {
            ty: None,
            parsed_as: ParsedAs::Error,
            size,
            sha256,
        },
        l1: None,
        error: Some(FileError {
            kind,
            note: Some(note(&text)),
        }),
    };
    let mut out = InputDump {
        sha256,
        bytes_in: size,
        ..InputDump::default()
    };
    out.push_file(file_line(&rec));
    Ok(ReadInput::Refused(out))
}

/// The file's length as `stat` gives it, and the sha256 of its first that
/// many bytes (fewer, if it is shorter by now), read in fixed chunks.
fn prefix_sha256(path: &Path) -> std::io::Result<(u64, [u8; 32])> {
    let file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let mut hasher = Sha256::new();
    let mut chunk = vec![0u8; 1 << 16];
    let mut rest = file.take(len);
    let mut size = 0u64;
    loop {
        match rest.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(chunk.get(..n).unwrap_or_default());
                size += u64::try_from(n).unwrap_or(u64::MAX);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok((size, hasher.finalize().into()))
}
