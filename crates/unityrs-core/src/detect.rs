// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/ImportHelper.py. See THIRD_PARTY_LICENSES.
// Ported from CPython 3.11 (PSF-2.0): Lib/gzip.py, _read_exact, _read_gzip_header, _GzipReader.read and _read_eof. See THIRD_PARTY_LICENSES.
//! `ImportHelper.check_file_type` and `parse_file`'s naming rule
//! (`helpers/ImportHelper.py:44-156`), quirks included: every rule of
//! `docs/CANONICAL_DUMP.md` section 4.2 (`l0`) and section 7 (L0 detection).
//!
//! Notable rules: eight U+00FA characters as a signature (the UTF-8 bytes
//! `C3 BA` x 8, never `FA` x 8); a zip only when byte 4 is NUL; a 20-byte file
//! with no NUL raises from bytes, and from a path, whose stream reader drops
//! the last byte it read, is a resource ([`check_file_type_with_reader`]);
//! a gzip file must decompress in full, every member and
//! trailer checked (the P0 spike inflated only 20 bytes, which this fixes).
//! That is UnityPy's own behaviour, not a unityrs policy: its streaming
//! `read_string_to_null` tests `Position != Length` per byte
//! (`streams/EndianBinaryReader.py:136-144`), and `Length` seeks the
//! `GzipFile` to its end (`streams/EndianBinaryReader.py:307-312`), which inflates every member. The
//! output is streamed through a scratch buffer and only its first 20 bytes
//! and its length are kept, so detection charges nothing: the scratch (at
//! most 64 KiB) is part of plan section 3.15's constant, as are the
//! inflater's state and 32 KiB window, one member's at a time. Also:
//! `brotli` at 0x20; the SerializedFile header read big-endian.
//!
//! **The inflater is zlib's.** flate2 runs `zlib-rs` (`unityrs-core`'s
//! manifest), a port of zlib that keeps its rules, so a stream inflates
//! or raises as under `CPython`'s zlib, which `gzip._GzipReader` runs. One
//! rule differs in flate2's default backend, `miniz_oxide`: a match reaching
//! before its member's first output byte is zlib's "invalid distance too
//! far back", where `miniz_oxide` copies zeros from its window (round 9:
//! a corrupt stream detected as `ResourceFile` or `WebFile`, and a bundle
//! UnityPy fails loaded). `tests/fixtures/c/detect.json` pins it.
//!
//! **Work.** One stream inflates at most [`Limits::max_inflate`] bytes. A
//! container detects each of its entries ([`check_entry_type`]), and many
//! directory rows can name one small stream that inflates to the bound, so
//! there every inflated byte is also counted as the open's work, the
//! allowance block decodes draw on ([`Limits::max_decode_work`],
//! [`Meter::count_decode`]): the open's detections together inflate no more
//! than that, and the one that passes it is refused
//! ([`DetectError::Work`], which the container reports as the open's
//! `LimitKind::DecodeWork` limit), as is every one after it. The bytes of
//! the stream a detection reads count too, inflated or not: a header's file
//! name or comment searched for its NUL, deflate input that inflates to
//! nothing (empty blocks), each member's header and trailer, and the zero
//! padding after it, all of which each row naming the stream reads again
//! (round 4's sweep: a stream of one tiny member and a gigabyte of padding
//! counted a few bytes per row). A detection that fails counts what it read
//! and inflated before the failure, and returns the stream's own error
//! (round 5: a malformed deflate stream returned before its consumed input
//! was counted, so every row read it again uncounted).
//!
//! Owner: package C.

use std::sync::Arc;

use flate2::{Crc, Decompress, FlushDecompress, Status};

use crate::error::{LimitError, LimitKind};
use crate::limits::{Limits, Meter};
use crate::stream::ReaderKind;

/// UnityPy's `FileType` verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileType {
    /// A `SerializedFile`.
    AssetsFile,
    /// UnityFS, UnityWeb, UnityRaw or the U+00FA signature.
    BundleFile,
    /// `UnityWebData`/`TuanjieWebData`, plain, gzip or brotli.
    WebFile,
    /// Anything else.
    ResourceFile,
    /// `PK\x03\x04` followed by NUL.
    Zip,
}

/// Why detection raised (the dump's `"detect"` error kind).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DetectError {
    /// A file of exactly 20 bytes with no NUL in them, read by the memory
    /// reader (bytes; from a path the stream reader's rule never raises
    /// here, [`check_file_type_with_reader`]).
    #[error("unterminated signature in a 20-byte file")]
    Unterminated,
    /// A `1f 8b` file whose stream does not decompress in full.
    #[error("gzip: {0}")]
    Gzip(Arc<str>),
    /// The gzip stream inflates past [`Limits::max_inflate`]: a
    /// [`LimitKind::Inflate`] refusal, whose value is what the stream had
    /// inflated to when it was refused, so its text is every limit
    /// refusal's, `limit Inflate: <value> > <max>`.
    #[error(transparent)]
    InflateLimit(LimitError),
    /// [`check_entry_type`]: the open's work allowance
    /// ([`Limits::max_decode_work`]) ran out while a gzip stream inflated,
    /// or had run out before it; a `LimitKind::DecodeWork` limit, whose text
    /// is `limit DecodeWork: <value> > <max>`.
    #[error(transparent)]
    Work(LimitError),
}

/// `check_file_type` on the bytes (UnityPy's memory reader).
pub fn check_file_type(data: &[u8], limits: &Limits) -> Result<FileType, DetectError> {
    detect(data, limits, None, ReaderKind::Memory)
}

/// [`check_file_type`] by the rules of the reader UnityPy reads `data`
/// with: the stream reader for a file loaded from a path, whose
/// `read_string_to_null(20)` drops the last byte it read where no NUL is in
/// reach and never raises from the start of a file, so a 20-byte file with
/// no NUL is a `ResourceFile` (or a `WebFile` by its first 19 bytes) where
/// the memory reader raises ([`DetectError::Unterminated`]).
pub fn check_file_type_with_reader(
    data: &[u8],
    limits: &Limits,
    reader: ReaderKind,
) -> Result<FileType, DetectError> {
    detect(data, limits, None, reader)
}

/// [`check_file_type`] on a container's entry, with every byte a gzip
/// stream inflates counted as work of the open `meter` belongs to (module
/// docs, "Work"): the verdict is the same until the open's allowance runs
/// out, and [`DetectError::Work`] from then on.
pub fn check_entry_type(
    data: &[u8],
    limits: &Limits,
    meter: &Meter,
) -> Result<FileType, DetectError> {
    detect(data, limits, Some(meter), ReaderKind::Memory)
}

fn detect(
    data: &[u8],
    limits: &Limits,
    work: Option<&Meter>,
    reader: ReaderKind,
) -> Result<FileType, DetectError> {
    // UnityPy helpers/ImportHelper.py:54-55
    if data.len() < 20 {
        return Ok(FileType::ResourceFile);
    }
    // UnityPy helpers/ImportHelper.py:57: the reader's
    // `read_string_to_null(20)` at 0, decoded with `surrogateescape`, which
    // is injective, so comparing the bytes compares the strings.
    let (range, _) = reader
        .string_to_null(data, 0, 20)
        .ok_or(DetectError::Unterminated)?;
    let signature = data.get(range).unwrap_or_default();
    // UnityPy helpers/ImportHelper.py:60-70
    if BUNDLE_SIGNATURES.contains(&signature) {
        return Ok(FileType::BundleFile);
    }
    if is_web_data(signature) {
        return Ok(FileType::WebFile);
    }
    if signature == b"PK\x03\x04" {
        return Ok(FileType::Zip);
    }
    // UnityPy helpers/ImportHelper.py:72-73
    if data.len() < 128 {
        return Ok(FileType::ResourceFile);
    }
    // UnityPy helpers/ImportHelper.py:75-83
    if data.get(..2) == Some(GZIP_MAGIC.as_slice())
        && is_web_data(&gzip_signature(data, limits.max_inflate, work)?)
    {
        return Ok(FileType::WebFile);
    }
    // UnityPy helpers/ImportHelper.py:84-88
    if data.get(0x20..0x26) == Some(b"brotli".as_slice()) {
        return Ok(FileType::WebFile);
    }
    // UnityPy helpers/ImportHelper.py:90-124: the header read big-endian
    // (the reader's default) whatever the byte at 16 says; the file holds at
    // least 128 bytes here, so every read succeeds.
    let mut metadata_size = be_u32(data, 0);
    let mut file_size = be_u32(data, 4);
    let version = be_u32(data, 8);
    let mut data_offset = be_u32(data, 12);
    if version >= 22 {
        metadata_size = be_u32(data, 20);
        file_size = be_i64(data, 24);
        data_offset = be_i64(data, 32);
    }
    let len = i64::try_from(data.len()).unwrap_or(i64::MAX);
    let bad = version > 100
        || [file_size, metadata_size, version, data_offset]
            .iter()
            .any(|x| *x < 0 || *x > len)
        || file_size < metadata_size
        || file_size < data_offset;
    Ok(if bad {
        FileType::ResourceFile
    } else {
        FileType::AssetsFile
    })
}

/// The signatures that make a bundle (`ImportHelper.py:60-65`), as the UTF-8
/// of the strings UnityPy compares.
const BUNDLE_SIGNATURES: [&[u8]; 4] = [
    b"UnityWeb",
    b"UnityRaw",
    // Eight U+00FA: the byte 0xFA itself decodes to a surrogate escape and
    // never matches (CANONICAL_DUMP.md section 7).
    "\u{fa}\u{fa}\u{fa}\u{fa}\u{fa}\u{fa}\u{fa}\u{fa}".as_bytes(),
    b"UnityFS",
];

const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// `signature.startswith(("UnityWebData", "TuanjieWebData"))`.
fn is_web_data(signature: &[u8]) -> bool {
    signature.starts_with(b"UnityWebData") || signature.starts_with(b"TuanjieWebData")
}

fn be_u32(data: &[u8], at: usize) -> i64 {
    data.get(at..at.saturating_add(4))
        .and_then(|b| <[u8; 4]>::try_from(b).ok())
        .map_or(0, |b| i64::from(u32::from_be_bytes(b)))
}

fn be_i64(data: &[u8], at: usize) -> i64 {
    data.get(at..at.saturating_add(8))
        .and_then(|b| <[u8; 8]>::try_from(b).ok())
        .map_or(0, i64::from_be_bytes)
}

/// The scratch gzip detection inflates into (part of plan section 3.15's
/// constant).
const SCRATCH: usize = 64 << 10;

/// `EndianBinaryReader(GzipFile(fileobj=reader)).read_string_to_null(20)`
/// (`ImportHelper.py:78-80`). Its first `Length` seeks the `GzipFile` to its
/// end (`streams/EndianBinaryReader.py:307-312`), which inflates and checks
/// every member as Python 3.11's `gzip._GzipReader` does. The streaming loop
/// (`streams/EndianBinaryReader.py:136-144`) then keeps each byte it read except the last: byte `i` is kept
/// when it and every byte before it are not NUL, `i < 19`, and `i + 1` is
/// below the decompressed length.
fn gzip_signature(
    data: &[u8],
    max_inflate: u64,
    work: Option<&Meter>,
) -> Result<Vec<u8>, DetectError> {
    let mut first = [0u8; 20];
    let mut first_len = 0usize;
    let total = gzip_inflate(data, max_inflate, work, |chunk| {
        let n = chunk.len().min(first.len().saturating_sub(first_len));
        if let (Some(dst), Some(src)) = (first.get_mut(first_len..first_len + n), chunk.get(..n)) {
            dst.copy_from_slice(src);
        }
        first_len += n;
    })?;
    let limit = usize::try_from(total.saturating_sub(1))
        .unwrap_or(usize::MAX)
        .min(19)
        .min(first_len);
    let head = first.get(..limit).unwrap_or_default();
    let keep = head.iter().position(|b| *b == 0).unwrap_or(head.len());
    Ok(head.get(..keep).unwrap_or_default().to_vec())
}

fn gzip_error(what: &str) -> DetectError {
    DetectError::Gzip(what.into())
}

const EOF_MESSAGE: &str =
    "EOFError: Compressed file ended before the end-of-stream marker was reached";

/// `_read_exact`: `n` bytes at `*pos`, or `EOFError`.
fn read_exact<'a>(data: &'a [u8], pos: &mut usize, n: usize) -> Result<&'a [u8], DetectError> {
    let end = pos.saturating_add(n);
    let got = data.get(*pos..end).ok_or_else(|| gzip_error(EOF_MESSAGE))?;
    *pos = end;
    Ok(got)
}

/// `bytes` inflated, counted as the open's work: when that passes the
/// allowance, they are counted anyway (they were inflated), so every later
/// detection of the open is refused before it inflates anything.
fn count_work(meter: &Meter, bytes: u64) -> Result<(), DetectError> {
    if meter.count_decode(bytes).is_ok() {
        return Ok(());
    }
    meter.add_work(bytes);
    meter
        .work_used_up()
        .map_or(Ok(()), |l| Err(DetectError::Work(l)))
}

/// `got`, after the `bytes` read or inflated for it are counted as `open`'s
/// work: a success as [`count_work`] counts it (refused once the allowance
/// is passed), a failure unchecked, so it returns the stream's own error and
/// the work after it is refused.
fn counted<T>(
    open: Option<&Meter>,
    bytes: u64,
    got: Result<T, DetectError>,
) -> Result<T, DetectError> {
    let Some(meter) = open else {
        return got;
    };
    match got {
        Ok(v) => count_work(meter, bytes).map(|()| v),
        Err(e) => {
            meter.add_work(bytes);
            Err(e)
        }
    }
}

/// The stream inflated to `total`, past `max_inflate`.
fn inflate_limit(total: u64, max_inflate: u64) -> DetectError {
    DetectError::InflateLimit(LimitError {
        kind: LimitKind::Inflate,
        value: total,
        max: max_inflate,
    })
}

/// The bytes from `from` to `to`, as work.
fn read_from(from: usize, to: usize) -> u64 {
    u64::try_from(to.saturating_sub(from)).unwrap_or(u64::MAX)
}

/// `_read_gzip_header` from the magic at `*pos`, which it moves past the
/// header, every byte of which, the file name's and comment's searches
/// included, is counted as `open`'s work when there is one, whether the
/// header reads or fails.
fn member_header(data: &[u8], pos: &mut usize, open: Option<&Meter>) -> Result<(), DetectError> {
    let member = *pos;
    let got = read_member_header(data, pos);
    counted(open, read_from(member, *pos), got)
}

/// [`member_header`]'s reads: `*pos` moves past each byte read, a failing
/// read's excepted.
fn read_member_header(data: &[u8], pos: &mut usize) -> Result<(), DetectError> {
    const FHCRC: u8 = 2;
    const FEXTRA: u8 = 4;
    const FNAME: u8 = 8;
    const FCOMMENT: u8 = 16;
    *pos += 2;
    let head = read_exact(data, pos, 8)?;
    let (method, flag) = (
        head.first().copied().unwrap_or(0),
        head.get(1).copied().unwrap_or(0),
    );
    if method != 8 {
        return Err(gzip_error("BadGzipFile: Unknown compression method"));
    }
    if flag & FEXTRA != 0 {
        let n = read_exact(data, pos, 2)?;
        let n = u16::from_le_bytes([
            n.first().copied().unwrap_or(0),
            n.get(1).copied().unwrap_or(0),
        ]);
        read_exact(data, pos, usize::from(n))?;
    }
    for bit in [FNAME, FCOMMENT] {
        if flag & bit != 0 {
            // Up to and including a NUL, or to the end.
            let rest = data.get(*pos..).unwrap_or_default();
            *pos += rest
                .iter()
                .position(|b| *b == 0)
                .map_or(rest.len(), |i| i + 1);
        }
    }
    if flag & FHCRC != 0 {
        read_exact(data, pos, 2)?;
    }
    Ok(())
}

/// Inflates every member of a gzip file as Python 3.11's `gzip._GzipReader`
/// reads it (`_read_gzip_header`, `read`, `_read_eof`), handing each output
/// chunk to `sink`; returns the decompressed length. The header's FHCRC is
/// skipped, not checked, as Python skips it. Each chunk, and every byte of
/// `data` read for it, is counted as `open`'s work when there is one
/// (module docs, "Work").
fn gzip_inflate(
    data: &[u8],
    max_inflate: u64,
    open: Option<&Meter>,
    mut sink: impl FnMut(&[u8]),
) -> Result<u64, DetectError> {
    if let Some(l) = open.and_then(Meter::work_used_up) {
        return Err(DetectError::Work(l));
    }
    let mut scratch = vec![0u8; SCRATCH];
    let mut pos = 0usize;
    let mut total = 0u64;
    loop {
        // `_read_gzip_header`: `fp.read(2)` empty ends the file; anything
        // else must be the magic.
        let rest = data.get(pos..).unwrap_or_default();
        if rest.is_empty() {
            return Ok(total);
        }
        if !rest.starts_with(&GZIP_MAGIC) {
            return Err(gzip_error("BadGzipFile: Not a gzipped file"));
        }
        member_header(data, &mut pos, open)?;
        // The member's raw deflate stream.
        let input = data.get(pos..).unwrap_or_default();
        let mut z = Decompress::new(false);
        let mut crc = Crc::new();
        loop {
            let (in_before, out_before) = (z.total_in(), z.total_out());
            let at = usize::try_from(in_before).unwrap_or(usize::MAX);
            let status = z
                .decompress(
                    input.get(at..).unwrap_or_default(),
                    &mut scratch,
                    FlushDecompress::None,
                )
                .map_err(|e| DetectError::Gzip(format!("zlib.error: {e}").into()));
            // What the call consumed and made counts whether it failed or
            // not (zlib-rs advances flate2's totals by what it read and
            // wrote before it reports a failure), and before the stream's
            // own bound is checked, so the chunk that passes `max_inflate`
            // is the open's work too.
            let made = usize::try_from(z.total_out() - out_before).unwrap_or(0);
            let consumed = z.total_in() - in_before;
            let made_bytes = u64::try_from(made).unwrap_or(u64::MAX);
            let status = counted(open, made_bytes.saturating_add(consumed), status)?;
            let out = scratch.get(..made).unwrap_or_default();
            crc.update(out);
            total = total.saturating_add(made_bytes);
            if total > max_inflate {
                return Err(inflate_limit(total, max_inflate));
            }
            sink(out);
            if status == Status::StreamEnd {
                break;
            }
            if z.total_in() == in_before && made == 0 {
                return Err(gzip_error(EOF_MESSAGE));
            }
        }
        pos = pos.saturating_add(usize::try_from(z.total_in()).unwrap_or(usize::MAX));
        let eof = pos;
        let got = member_trailer(data, &mut pos, &crc);
        counted(open, read_from(eof, pos), got)?;
    }
}

/// `_read_eof` from `*pos`: the CRC-32 and the size modulo 2^32, checked,
/// then the zero bytes after them; `*pos` moves past each byte read.
fn member_trailer(data: &[u8], pos: &mut usize, crc: &Crc) -> Result<(), DetectError> {
    let trailer = read_exact(data, pos, 8)?;
    let word = |i: usize| {
        trailer
            .get(i..i + 4)
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map_or(0, u32::from_le_bytes)
    };
    if word(0) != crc.sum() {
        return Err(gzip_error("BadGzipFile: CRC check failed"));
    }
    if word(4) != crc.amount() {
        return Err(gzip_error("BadGzipFile: Incorrect length of data produced"));
    }
    while data.get(*pos) == Some(&0) {
        *pos += 1;
    }
    Ok(())
}

/// What UnityPy builds from a detected file (`l0.parsed_as`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParsedAs {
    /// A `SerializedFile`.
    SerializedFile,
    /// A `BundleFile`.
    BundleFile,
    /// A `WebFile`.
    WebFile,
    /// UnityPy kept a reader: a resource, a name-rule file, or a zip entry.
    Raw,
    /// An input detected as a zip.
    Zip,
    /// Detection or parsing raised.
    Error,
}

/// `parse_file`'s choice. An `AssetsFile` whose name ends in `.resS`,
/// `.resource`, `.config`, `.xml` or `.dat` (a case-sensitive byte suffix) stays
/// raw; a zip is `Zip` only for an input (`is_entry` false), else raw.
#[must_use]
pub fn parsed_as(ty: FileType, name: &[u8], is_entry: bool) -> ParsedAs {
    // UnityPy helpers/ImportHelper.py:138-151; a zip input is refused before
    // `parse_file` (CANONICAL_DUMP.md section 4.2), an entry stays a reader.
    match ty {
        FileType::AssetsFile if raw_by_name(name) => ParsedAs::Raw,
        FileType::AssetsFile => ParsedAs::SerializedFile,
        FileType::BundleFile => ParsedAs::BundleFile,
        FileType::WebFile => ParsedAs::WebFile,
        FileType::Zip if !is_entry => ParsedAs::Zip,
        FileType::Zip | FileType::ResourceFile => ParsedAs::Raw,
    }
}

/// Whether `name` ends in one of the suffixes that keep an `AssetsFile` raw
/// ([`parsed_as`]): the only part of a name that changes what `parse_file`
/// builds from given bytes.
pub(crate) fn raw_by_name(name: &[u8]) -> bool {
    const RAW_SUFFIXES: [&[u8]; 5] = [b".resS", b".resource", b".config", b".xml", b".dat"];
    RAW_SUFFIXES.iter().any(|s| name.ends_with(s))
}
