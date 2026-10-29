//! `UnityFS` bundles (formats 6 to 8) and `UnityWeb`/`UnityRaw` (P3), built
//! from a spec, with every block codec the core reads.
//!
//! Owner: package C.

// STUB(C): delete this allow when the builder is implemented.
#![allow(
    clippy::todo,
    clippy::must_use_candidate,
    clippy::needless_pass_by_value,
    unused_variables
)]

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
        }
    }
}

/// The bundle's bytes.
pub fn build(spec: &BundleSpec) -> Vec<u8> {
    todo!()
}
