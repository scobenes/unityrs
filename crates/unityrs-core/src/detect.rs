//! `ImportHelper.check_file_type` and `parse_file`'s naming rule
//! (`helpers/ImportHelper.py:44-156`), quirks included: every rule of
//! `docs/CANONICAL_DUMP.md` section 4.2 (`l0`) and section 7 (L0 detection).
//!
//! Notable rules: eight U+00FA characters as a signature (the UTF-8 bytes
//! `C3 BA` x 8, never `FA` x 8); a zip only when byte 4 is NUL; a 20-byte file
//! with no NUL raises; a gzip file must decompress in full, every member and
//! trailer checked (the P0 spike inflated only 20 bytes, which this fixes).
//! That is UnityPy's own behaviour, not a unityrs policy: its streaming
//! `read_string_to_null` tests `Position != Length` per byte
//! (`streams/EndianBinaryReader.py:136-144`), and `Length` seeks the
//! `GzipFile` to its end (`:303-308`), which inflates every member. The
//! output is streamed through a scratch buffer and only its first 20 bytes
//! and its length are kept, so detection charges nothing: the scratch (at
//! most 64 KiB) is part of plan section 3.15's constant. Also: `brotli`
//! at 0x20; the SerializedFile header read big-endian.
//!
//! Owner: package C.

// STUB(C): delete this allow when the module is implemented.
#![allow(
    clippy::todo,
    clippy::unused_self,
    clippy::needless_pass_by_value,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    unused_variables,
    dead_code
)]

use crate::limits::Limits;

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
    /// A file of exactly 20 bytes with no NUL in them.
    #[error("unterminated signature in a 20-byte file")]
    Unterminated,
    /// A `1f 8b` file whose stream does not decompress in full.
    #[error("gzip: {0}")]
    Gzip(Box<str>),
    /// The gzip stream inflates past [`Limits::max_inflate`].
    #[error("gzip stream larger than the inflate limit")]
    InflateLimit,
}

/// `check_file_type` on the bytes.
pub fn check_file_type(data: &[u8], limits: &Limits) -> Result<FileType, DetectError> {
    todo!()
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
pub fn parsed_as(ty: FileType, name: &[u8], is_entry: bool) -> ParsedAs {
    todo!()
}
