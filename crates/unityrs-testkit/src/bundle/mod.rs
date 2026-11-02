// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/BundleFile.py. See THIRD_PARTY_LICENSES.
//! `UnityFS` bundles (formats 6 to 8) and `UnityWeb`/`UnityRaw` (P3), built
//! from a spec, with every block codec the core reads.
//!
//! The layout is the one UnityPy 1.25.2's `BundleFile.read_fs` reads
//! (`files/BundleFile.py:96-186`), big-endian throughout: the signature, the
//! format, both version strings, the total size, blocks-info's stored and
//! decoded sizes and the data flags; the 16-byte alignment where UnityPy
//! applies it (format 7 and up, or a 2019 engine from 2019.4.15); blocks-info
//! (hash, blocks, directory) right there or at the end of the file (flag
//! 0x80); the 16-byte padding before the data under the new flag set with
//! 0x200; and the data blocks, each compressed on its own.
//!
//! Owner: package C.

use crate::writer::ByteWriter;

mod matrix;

pub use matrix::matrix;

/// A block codec, as the flags' low six bits name it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TestCodec {
    /// 0.
    #[default]
    None,
    /// 1: LZMA "alone" props + data, as Unity writes it (no size field).
    Lzma,
    /// 2: LZ4 block.
    Lz4,
    /// 3: written as plain LZ4 (the decoder is the same).
    Lz4Hc,
    /// Any id, with the data stored as is (for refusal tests).
    Raw(u8),
}

impl TestCodec {
    /// The compression type number.
    #[must_use]
    pub fn id(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Lzma => 1,
            Self::Lz4 => 2,
            Self::Lz4Hc => 3,
            Self::Raw(n) => n,
        }
    }
}

/// Overrides for one block, so a test can place a malformed block before the
/// entry it reads (plan section 7.4, "one layout").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockOverride {
    /// Which block, in file order.
    pub block: usize,
    /// This block's codec instead of [`BundleSpec::data_codec`].
    pub codec: Option<TestCodec>,
    /// The declared decoded size written to blocks-info (an LZ4 block that
    /// decodes short, an LZMA block that outgrows it, a stored block whose
    /// declaration differs from its bytes).
    pub uncompressed: Option<u32>,
    /// The declared stored size written to blocks-info.
    pub compressed: Option<u32>,
}

/// One directory entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntrySpec {
    /// The path.
    pub name: Vec<u8>,
    /// The bytes.
    pub data: Vec<u8>,
    /// The flags (4 marks a `SerializedFile`).
    pub flags: u32,
}

/// A `UnityFS` bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleSpec {
    /// The format (6, 7 or 8).
    pub format: u32,
    /// `version_player`.
    pub version_player: Vec<u8>,
    /// `version_engine`.
    pub version_engine: Vec<u8>,
    /// The blocks-info codec.
    pub info_codec: TestCodec,
    /// The data codec.
    pub data_codec: TestCodec,
    /// Uncompressed block size; the data is cut into blocks of this size.
    pub block_size: usize,
    /// Blocks info at the end of the file (flag 0x80).
    pub info_at_end: bool,
    /// Entries, in directory order.
    pub entries: Vec<EntrySpec>,
    /// Extra data flags, combined with a bitwise or (0x200 block-info padding, 0x400 the old
    /// encryption bit, ...).
    pub extra_flags: u32,
    /// Per-block overrides.
    pub block_overrides: Vec<BlockOverride>,
    /// Cut the finished file to this length (a truncated last block).
    pub truncate_to: Option<usize>,
    /// Sets the encryption flag of the engine's flag set (`0x200` under
    /// `ArchiveFlagsOld`, `0x1000` under `ArchiveFlags`) and writes what
    /// `ArchiveStorageDecryptor(reader)` reads after the data flags
    /// (`helpers/ArchiveStorageManager.py`): a `u32`, then two vectors of 16
    /// and 16 bytes and a skipped byte, all zeros. A UnityCN key cannot open
    /// it (its signature check fails); a test's own decryptor can.
    pub encrypted: bool,
}

impl Default for BundleSpec {
    fn default() -> Self {
        Self {
            format: 7,
            version_player: b"5.x.x".to_vec(),
            version_engine: b"2022.3.0f1".to_vec(),
            info_codec: TestCodec::Lz4,
            data_codec: TestCodec::Lz4,
            block_size: 128 << 10,
            info_at_end: false,
            entries: Vec::new(),
            extra_flags: 0,
            block_overrides: Vec::new(),
            truncate_to: None,
            encrypted: false,
        }
    }
}

/// `data` compressed by `codec`: LZMA as Unity stores it (the property byte
/// `0x5D`, the 4-byte dictionary size, the raw LZMA1 stream), LZ4 and LZ4HC
/// as one LZ4 block, anything else stored as it is.
#[must_use]
pub fn compress(codec: TestCodec, data: &[u8]) -> Vec<u8> {
    match codec {
        TestCodec::None | TestCodec::Raw(_) => data.to_vec(),
        TestCodec::Lz4 | TestCodec::Lz4Hc => lz4_flex::block::compress(data),
        TestCodec::Lzma => lzma(data),
    }
}

/// The dictionary size the LZMA blocks declare (Unity's 512 KiB).
pub const LZMA_DICT: u32 = 1 << 19;

thread_local! {
    /// Each LZMA stream by its input. G3b's mutator builds every output again
    /// for each field it replaces (`mutate`), so most blocks repeat, and the
    /// encoder's set-up (its dictionary and match finder) costs more than the
    /// rest of a build.
    static LZMA_MEMO: std::cell::RefCell<std::collections::HashMap<Vec<u8>, Vec<u8>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// An LZMA block as Unity stores it: the properties byte, the dictionary size
/// (a writer field, written on every call, so a memo hit numbers the same
/// fields), then the raw stream.
fn lzma(data: &[u8]) -> Vec<u8> {
    // lc 3, lp 0, pb 2: (2 * 5 + 0) * 9 + 3.
    let mut out = ByteWriter::le();
    out.u8(0x5D).u32(LZMA_DICT);
    let mut out = out.into_inner();
    if let Some(hit) = LZMA_MEMO.with(|m| m.borrow().get(data).cloned()) {
        out.extend_from_slice(&hit);
        return out;
    }
    let stream = lzma_encode(data);
    LZMA_MEMO.with(|m| {
        let mut m = m.borrow_mut();
        // A bound, so a long-lived thread does not keep every block forever.
        if m.len() >= 4096 {
            m.clear();
        }
        m.insert(data.to_vec(), stream.clone());
    });
    out.extend_from_slice(&stream);
    out
}

/// The raw LZMA stream of `data` (no header), empty when the encoder fails.
fn lzma_encode(data: &[u8]) -> Vec<u8> {
    use liblzma::stream::{Action, Filters, LzmaOptions, Status, Stream};
    let mut opts = LzmaOptions::new_preset(1).unwrap_or_else(|_| LzmaOptions::new());
    opts.dict_size(LZMA_DICT)
        .literal_context_bits(3)
        .literal_position_bits(0)
        .position_bits(2);
    let mut filters = Filters::new();
    filters.lzma1(&opts);
    let Ok(mut stream) = Stream::new_raw_encoder(&filters) else {
        return Vec::new();
    };
    let mut buf = vec![0u8; data.len() + data.len() / 2 + 1024];
    loop {
        let (read, written) = (stream.total_in(), stream.total_out());
        let input = data.get(as_usize(read)..).unwrap_or_default();
        let window = buf.get_mut(as_usize(written)..).unwrap_or_default();
        match stream.process(input, window, Action::Finish) {
            Ok(Status::StreamEnd) | Err(_) => break,
            Ok(_) if window_full(&stream, buf.len()) => buf.resize(buf.len() * 2, 0),
            Ok(_) => {}
        }
    }
    buf.truncate(as_usize(stream.total_out()));
    buf
}

fn window_full(stream: &liblzma::stream::Stream, len: usize) -> bool {
    as_usize(stream.total_out()) >= len
}

fn as_usize(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// `(major, minor, build)` of an ASCII engine string, as far as it parses.
fn engine_triple(engine: &[u8]) -> Option<(u32, u32, u32)> {
    let s = std::str::from_utf8(engine).ok()?;
    let mut parts = s.splitn(3, '.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let rest = parts.next()?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some((major, minor, digits.parse().ok()?))
}

/// Whether UnityPy reads these flags as `ArchiveFlags` (the new set) for
/// this engine (`files/BundleFile.py:113-121`), on the engine's ASCII digits.
fn new_flags(engine: &[u8]) -> bool {
    let Some(v) = engine_triple(engine) else {
        return false;
    };
    !(v < (2020, 0, 0)
        || (v.0 == 2020 && v < (2020, 3, 34))
        || (v.0 == 2021 && v < (2021, 3, 2))
        || (v.0 == 2022 && v < (2022, 1, 1)))
}

/// Whether UnityPy aligns to 16 after the header (`files/BundleFile.py:129-131`).
fn aligned(format: u32, engine: &[u8]) -> bool {
    format >= 7 || engine_triple(engine).is_some_and(|v| v.0 == 2019 && v >= (2019, 4, 15))
}

/// The bundle's bytes.
#[must_use]
pub fn build(spec: &BundleSpec) -> Vec<u8> {
    build_with_signature(spec, b"UnityFS")
}

/// The bundle's bytes under another signature: `UnityWeb` or `UnityRaw` at
/// format 6 take the UnityFS layout with one more byte after the data flags
/// (`files/BundleFile.py:104-106`), which this writes for any signature but
/// `UnityFS`.
#[must_use]
pub fn build_with_signature(spec: &BundleSpec, signature: &[u8]) -> Vec<u8> {
    // The decoded data and the directory.
    let mut data = Vec::new();
    let mut directory = ByteWriter::be();
    directory.i32(i32::try_from(spec.entries.len()).unwrap_or(i32::MAX));
    for e in &spec.entries {
        directory
            .i64(i64::try_from(data.len()).unwrap_or(i64::MAX))
            .i64(i64::try_from(e.data.len()).unwrap_or(i64::MAX))
            .u32(e.flags)
            .cstr(&e.name);
        data.extend_from_slice(&e.data);
    }

    // The blocks.
    let size = spec.block_size.max(1);
    let mut stored = Vec::new();
    let mut rows = ByteWriter::be();
    let chunks: Vec<&[u8]> = data.chunks(size).collect();
    let count = chunks.len().max(
        spec.block_overrides
            .iter()
            .map(|o| o.block + 1)
            .max()
            .unwrap_or(0),
    );
    rows.bytes(&[0; 16]);
    rows.i32(i32::try_from(count).unwrap_or(i32::MAX));
    for i in 0..count {
        let chunk = chunks.get(i).copied().unwrap_or_default();
        let over = spec.block_overrides.iter().find(|o| o.block == i);
        let codec = over.and_then(|o| o.codec).unwrap_or(spec.data_codec);
        let bytes = compress(codec, chunk);
        let uncompressed = over
            .and_then(|o| o.uncompressed)
            .unwrap_or_else(|| u32::try_from(chunk.len()).unwrap_or(u32::MAX));
        let compressed = over
            .and_then(|o| o.compressed)
            .unwrap_or_else(|| u32::try_from(bytes.len()).unwrap_or(u32::MAX));
        rows.u32(uncompressed)
            .u32(compressed)
            .u16(u16::from(codec.id()));
        stored.extend_from_slice(&bytes);
    }
    rows.bytes(directory.as_slice());
    let info = rows.into_inner();
    let info_stored = compress(spec.info_codec, &info);

    let mut flags = u32::from(spec.info_codec.id()) | 0x40 | spec.extra_flags;
    if spec.info_at_end {
        flags |= 0x80;
    }
    if spec.encrypted {
        flags |= if new_flags(&spec.version_engine) {
            0x1000
        } else {
            0x200
        };
    }

    let mut w = ByteWriter::be();
    w.cstr(signature)
        .u32(spec.format)
        .cstr(&spec.version_player)
        .cstr(&spec.version_engine);
    let size_at = w.len();
    w.i64(0)
        .u32(u32::try_from(info_stored.len()).unwrap_or(u32::MAX))
        .u32(u32::try_from(info.len()).unwrap_or(u32::MAX))
        .u32(flags);
    if signature != b"UnityFS" {
        w.u8(0);
    }
    if spec.encrypted {
        w.u32(0).bytes(&[0; 2 * (16 + 16 + 1)]);
    }
    if aligned(spec.format, &spec.version_engine) {
        w.align(16);
    }
    let pad = flags & 0x200 != 0 && new_flags(&spec.version_engine);
    if !spec.info_at_end {
        w.bytes(&info_stored);
    }
    if pad {
        w.align(16);
    }
    w.bytes(&stored);
    if spec.info_at_end {
        w.bytes(&info_stored);
    }
    let mut file = w.into_inner();
    // Through a writer, so the total is a field (`writer::with_fields`).
    let mut total = ByteWriter::be();
    total.i64(i64::try_from(file.len()).unwrap_or(i64::MAX));
    if let Some(at) = file.get_mut(size_at..size_at + 8) {
        at.copy_from_slice(total.as_slice());
    }
    if let Some(n) = spec.truncate_to {
        file.truncate(n);
    }
    file
}

#[cfg(test)]
mod tests;
