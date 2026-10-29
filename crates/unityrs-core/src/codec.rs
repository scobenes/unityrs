//! Block codecs with UnityPy's semantics (`helpers/CompressionHelper.py:238-244`).
//!
//! - **None** returns the stored bytes as they are, whatever blocks-info says.
//! - **LZMA** (`:17-44`) is a raw LZMA1 decoder over the 5 property bytes and
//!   the rest, run to the end of the input: the declared size is ignored, a
//!   truncated stream yields what decoded, an over-long one yields all of it.
//!   Output is capped at [`Limits::max_block_output`] (a `PARITY.md` safety
//!   divergence). liblzma static, as `docs/spikes/lzma.md` decided.
//! - **LZ4/LZ4HC** (`:83-94`) is `lz4.block.decompress(data, uncompressed_size)`:
//!   the declared size is the *maximum* output. A shorter output is accepted;
//!   a longer one raises (python-lz4: `LZ4BlockError`).
//! - **LZHAM** raises (`:234-235`), and so does an unknown type
//!   (`files/BundleFile.py:515-521`).
//!
//! A block whose decoded length differs from blocks-info makes the bundle
//! fall back to UnityPy's joined layout (plan section 3.6).
//!
//! Owner: package C. The registry type is implemented by the scaffold.

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

use std::fmt;
use std::sync::Arc;

use crate::error::Error;
use crate::limits::Limits;

/// A block's compression, `flags & 0x3F`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Codec {
    /// 0: stored.
    None,
    /// 1.
    Lzma,
    /// 2.
    Lz4,
    /// 3.
    Lz4Hc,
    /// 4: unsupported, as in UnityPy.
    Lzham,
    /// 5-63: a custom codec, if one is registered.
    Other(u8),
}

impl Codec {
    /// The codec of a block or blocks-info `flags` value.
    #[must_use]
    pub fn from_flags(flags: u32) -> Self {
        match flags & 0x3F {
            0 => Self::None,
            1 => Self::Lzma,
            2 => Self::Lz4,
            3 => Self::Lz4Hc,
            4 => Self::Lzham,
            // `& 0x3F` keeps the value below 64, so the conversion never fails.
            n => Self::Other(u8::try_from(n).unwrap_or(u8::MAX)),
        }
    }

    /// The compression type number.
    #[must_use]
    pub fn id(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Lzma => 1,
            Self::Lz4 => 2,
            Self::Lz4Hc => 3,
            Self::Lzham => 4,
            Self::Other(n) => n,
        }
    }
}

/// Decodes one block.
pub trait BlockCodec: Send + Sync {
    /// Appends the decoded block to `out`. `uncompressed` is blocks-info's
    /// declared size; each codec treats it as UnityPy does (module docs).
    fn decode(
        &self,
        input: &[u8],
        uncompressed: u32,
        limits: &Limits,
        out: &mut Vec<u8>,
    ) -> Result<(), Error>;
}

/// Decrypts one block in place before it is decoded (UnityCN: P3). UnityPy
/// decrypts a block whose flags carry 0x100 and whose codec is not `None`
/// (`files/BundleFile.py:512-513`).
pub trait BlockDecryptor: Send + Sync {
    /// Decrypts block `index` in place.
    fn decrypt(&self, block: &mut [u8], index: usize) -> Result<(), Error>;
}

/// Codecs by compression type number (0-63).
#[derive(Clone)]
pub struct Codecs {
    slots: [Option<Arc<dyn BlockCodec>>; 64],
}

impl Codecs {
    /// No codec at all.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
        }
    }

    /// UnityPy's set: none, LZMA, LZ4 and LZ4HC.
    #[must_use]
    pub fn unitypy() -> Self {
        let mut c = Self::empty();
        c.set(Codec::None, Arc::new(Stored));
        c.set(Codec::Lzma, Arc::new(Lzma));
        c.set(Codec::Lz4, Arc::new(Lz4));
        c.set(Codec::Lz4Hc, Arc::new(Lz4));
        c
    }

    /// Registers `codec` for `which` (a custom codec is `Codec::Other(n)`).
    pub fn set(&mut self, which: Codec, codec: Arc<dyn BlockCodec>) {
        if let Some(slot) = self.slots.get_mut(usize::from(which.id())) {
            *slot = Some(codec);
        }
    }

    /// The codec for `which`, if registered.
    #[must_use]
    pub fn get(&self, which: Codec) -> Option<&Arc<dyn BlockCodec>> {
        self.slots.get(usize::from(which.id()))?.as_ref()
    }
}

impl Default for Codecs {
    fn default() -> Self {
        Self::unitypy()
    }
}

impl fmt::Debug for Codecs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ids: Vec<usize> = (0..self.slots.len())
            .filter(|i| self.slots.get(*i).is_some_and(Option::is_some))
            .collect();
        f.debug_struct("Codecs").field("registered", &ids).finish()
    }
}

/// Compression type 0: the stored bytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stored;

impl BlockCodec for Stored {
    fn decode(
        &self,
        input: &[u8],
        uncompressed: u32,
        limits: &Limits,
        out: &mut Vec<u8>,
    ) -> Result<(), Error> {
        todo!()
    }
}

/// Compression type 1: liblzma's raw LZMA1 decoder, to the end of the input.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lzma;

impl BlockCodec for Lzma {
    fn decode(
        &self,
        input: &[u8],
        uncompressed: u32,
        limits: &Limits,
        out: &mut Vec<u8>,
    ) -> Result<(), Error> {
        todo!()
    }
}

/// Compression types 2 and 3: an LZ4 block, `uncompressed` as the maximum.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lz4;

impl BlockCodec for Lz4 {
    fn decode(
        &self,
        input: &[u8],
        uncompressed: u32,
        limits: &Limits,
        out: &mut Vec<u8>,
    ) -> Result<(), Error> {
        todo!()
    }
}
