//! Block codecs with UnityPy's semantics (`helpers/CompressionHelper.py:238-244`).
//!
//! - **None** returns the stored bytes as they are, whatever blocks-info says.
//! - **LZMA** (`:17-44`) is a raw LZMA1 decoder over the 5 property bytes and
//!   the rest, run to the end of the input: the declared size is ignored, a
//!   truncated stream yields what decoded, an over-long one yields all of it.
//!   Output is capped at [`Limits::max_block_output`] (a `PARITY.md` safety
//!   divergence). Fewer than 5 input bytes raise (`struct.unpack("<BI", ...)`).
//!   liblzma static, as `docs/spikes/lzma.md` decided.
//! - **LZ4/LZ4HC** (`:83-94`) is `lz4.block.decompress(data, uncompressed_size)`:
//!   the declared size is the *maximum* output. A shorter output is accepted;
//!   a longer one raises (python-lz4: `LZ4BlockError`), and so does an empty
//!   input, whatever the declared size (`LZ4_decompress_safe` returns -1).
//! - **LZHAM** raises (`:234-235`), and so does an unknown type
//!   (`files/BundleFile.py:515-521`; `CompressionFlags` is an `IntFlag`, so the
//!   map lookup is what raises).
//!
//! A block's decoded length can differ from blocks-info; the bundle always
//! lays entries out by the actual lengths, as UnityPy's join does (plan
//! section 3.6), so a codec returns what it decoded and never pads or cuts.
//!
//! **LZ4 acceptance is liblz4's.** UnityPy's python-lz4 4.4.4 runs liblz4
//! 1.9.4's `LZ4_decompress_safe`, which refuses some blocks `lz4_flex` decodes:
//! its end-of-block conditions (a literal run that ends within the last 12
//! bytes of the declared size or the last 8 bytes of input must be the last
//! sequence and end exactly at the end of input; a match must end at least 5
//! bytes before the declared size; the length-extension guards). [`Lz4`]
//! checks the sequence stream against those conditions, and the oracle
//! fixture `tests/fixtures/scaffold/lz4_blocks.json` (python-lz4's verdict and
//! output for a grid of blocks around each boundary) is the acceptance test.
//!
//! **Output.** A codec writes into a [`DecodeBuf`], which checks the block's
//! cap and charges the input's meter before it grows (plan section 3.6), or
//! hands back a zero-copy slice of its input (the stored codec). A codec is
//! caller code when a user registers it: the core calls it with no lock held.
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

use bytes::Bytes;

use crate::budget::DecodeBuf;
use crate::error::Error;

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
    /// 5-63: a custom codec, if one is registered. Only these ids are
    /// canonical: `Other(1)` names no codec (it is not `Lzma`), and the
    /// registry refuses and never answers for it ([`Codec::is_canonical`]).
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

    /// Whether this is the value [`Codec::from_flags`] gives for its id: every
    /// named variant, and `Other(5..=63)`.
    #[must_use]
    pub fn is_canonical(self) -> bool {
        match self {
            Self::Other(n) => (5..64).contains(&n),
            _ => true,
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
    /// Decodes `input`, a block's stored bytes (a slice of the bundle).
    /// `uncompressed` is blocks-info's declared size; each codec treats it as
    /// UnityPy does (module docs). The output goes into `out`, which pays and
    /// checks the caps before it grows, and the codec returns `Ok(None)`; or,
    /// with `out` left empty, the codec returns `Ok(Some(slice))`, a zero-copy
    /// slice of `input` (the stored codec), which the caller checks against
    /// the block's cap as if it had been written (it holds no new memory).
    fn decode(
        &self,
        input: &Bytes,
        uncompressed: u32,
        out: &mut DecodeBuf,
    ) -> Result<Option<Bytes>, Error>;
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
        let lz4: Arc<dyn BlockCodec> = Arc::new(Lz4);
        for (which, codec) in [
            (Codec::None, Arc::new(Stored) as Arc<dyn BlockCodec>),
            (Codec::Lzma, Arc::new(Lzma)),
            (Codec::Lz4, lz4.clone()),
            (Codec::Lz4Hc, lz4),
        ] {
            if let Some(slot) = c.slots.get_mut(usize::from(which.id())) {
                *slot = Some(codec);
            }
        }
        c
    }

    /// Registers `codec` for `which` (a custom codec is `Codec::Other(5..=63)`).
    /// A non-canonical value (`Other(n)` with `n` below 5 or above 63) names no
    /// slot and is handed back.
    pub fn set(&mut self, which: Codec, codec: Arc<dyn BlockCodec>) -> Result<(), Codec> {
        if !which.is_canonical() {
            return Err(which);
        }
        match self.slots.get_mut(usize::from(which.id())) {
            Some(slot) => {
                *slot = Some(codec);
                Ok(())
            }
            None => Err(which),
        }
    }

    /// The codec for `which`, if registered (never for a non-canonical value).
    #[must_use]
    pub fn get(&self, which: Codec) -> Option<&Arc<dyn BlockCodec>> {
        if !which.is_canonical() {
            return None;
        }
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
        input: &Bytes,
        uncompressed: u32,
        out: &mut DecodeBuf,
    ) -> Result<Option<Bytes>, Error> {
        todo!()
    }
}

/// Compression type 1: liblzma's raw LZMA1 decoder, to the end of the input.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lzma;

impl BlockCodec for Lzma {
    fn decode(
        &self,
        input: &Bytes,
        uncompressed: u32,
        out: &mut DecodeBuf,
    ) -> Result<Option<Bytes>, Error> {
        todo!()
    }
}

/// Compression types 2 and 3: an LZ4 block, `uncompressed` as the maximum.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lz4;

impl BlockCodec for Lz4 {
    fn decode(
        &self,
        input: &Bytes,
        uncompressed: u32,
        out: &mut DecodeBuf,
    ) -> Result<Option<Bytes>, Error> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{Codec, Codecs, Lz4};

    #[test]
    fn every_id_has_one_canonical_codec() {
        for id in 0..64u32 {
            let c = Codec::from_flags(id);
            assert!(c.is_canonical(), "{c:?}");
            assert_eq!(u32::from(c.id()), id);
            assert_eq!(Codec::from_flags(u32::from(c.id())), c);
        }
        for n in [0u8, 1, 2, 3, 4, 64, 200, 255] {
            assert!(!Codec::Other(n).is_canonical(), "Other({n})");
        }
    }

    #[test]
    fn an_alias_neither_replaces_nor_finds_a_codec() {
        let mut c = Codecs::unitypy();
        let lzma = c.get(Codec::Lzma).unwrap().clone();
        // `Other(1)` is not LZMA: registering it must not replace slot 1.
        let _ = c.set(Codec::Other(1), Arc::new(Lz4));
        assert!(Arc::ptr_eq(c.get(Codec::Lzma).unwrap(), &lzma));
        assert!(c.get(Codec::Other(1)).is_none());
        assert_eq!(format!("{c:?}"), "Codecs { registered: [0, 1, 2, 3] }");
    }

    #[test]
    fn an_id_past_the_registry_is_handed_back() {
        let mut c = Codecs::empty();
        assert_eq!(
            c.set(Codec::Other(64), Arc::new(Lz4)),
            Err(Codec::Other(64))
        );
        assert_eq!(
            c.set(Codec::Other(200), Arc::new(Lz4)),
            Err(Codec::Other(200))
        );
        assert_eq!(c.set(Codec::Other(5), Arc::new(Lz4)), Ok(()));
        assert!(c.get(Codec::Other(5)).is_some());
    }
}
