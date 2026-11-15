// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/CompressionHelper.py. See THIRD_PARTY_LICENSES.
// Ported from UnityPy git 417d595 (MIT): UnityPy/helpers/CompressionHelper.py. See THIRD_PARTY_LICENSES.
//! Bundle packers following `helpers/CompressionHelper.py:158-231`.
//! LZ4 and LZ4HC both use pristine liblz4 1.9.4 HC at level 9.

use std::sync::Arc;

use bytes::Bytes;

use super::Codec;
use crate::budget::DecodeBuf;
use crate::error::LimitKind;
use crate::files::save::{SaveError, Writer, narrow};
use crate::limits::{Meter, MeteredVec};
use crate::stream::Endian;

/// One UnityFS block-table row, before it is serialized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockInfo {
    /// Input length of this block.
    pub uncompressed: u32,
    /// Stored length of this block.
    pub compressed: u32,
    /// The caller's flags, with the codec cleared for a stored chunk.
    pub flags: u16,
    /// HEAD format 9's offset in the padded compressed payload.
    pub offset: Option<u64>,
}

/// Packed bundle payload and its metered block table.
#[derive(Debug)]
pub struct PackedBlocks {
    /// Concatenated stored blocks, padded to 16 bytes for HEAD format 9.
    pub data: Bytes,
    /// Block rows, in payload order.
    pub blocks: MeteredVec<BlockInfo>,
}

const _: () = crate::send_sync::<BlockInfo>();
const _: () = crate::send_sync::<PackedBlocks>();

/// Compress one block, including Unity's five-byte LZMA properties prefix.
/// Both LZ4 codec IDs select `compress_lz4`
/// (`helpers/CompressionHelper.py:97-105`).
pub fn compress(data: &[u8], codec: Codec, meter: &Arc<Meter>) -> Result<Bytes, SaveError> {
    match codec {
        Codec::None => {
            let mut out = Writer::new(meter, Endian::Big)?;
            out.bytes(data)?;
            out.finish()
        }
        Codec::Lzma => compress_lzma(data, meter),
        Codec::Lz4 | Codec::Lz4Hc => compress_lz4(data, meter),
        _ => Err(SaveError::UnsupportedCodec(codec)),
    }
}

/// UnityPy 1.25.2's `chunk_based_compress`
/// (`helpers/CompressionHelper.py:158-231`): 128 KiB for LZ4, at most `u32::MAX`
/// for LZMA. A chunk is stored only when compression strictly increases its
/// size; all other flag bits survive. Stored empty data has one empty row;
/// a compressing packer has no row for empty data.
pub fn chunk_based_compress(
    data: &[u8],
    flags: u16,
    meter: &Arc<Meter>,
) -> Result<PackedBlocks, SaveError> {
    chunks(data, flags, None, meter)
}

/// HEAD 417d595's `chunk_based_compress`: the bytes already written, with
/// padding, determine whether the next block is stored. From format 9 each
/// compressed-packer block has an offset and trailing 16-byte alignment.
pub fn head_chunk_based_compress(
    data: &[u8],
    flags: u16,
    version: u32,
    meter: &Arc<Meter>,
) -> Result<PackedBlocks, SaveError> {
    chunks(data, flags, Some(version), meter)
}

fn chunks(
    data: &[u8],
    flags: u16,
    head: Option<u32>,
    meter: &Arc<Meter>,
) -> Result<PackedBlocks, SaveError> {
    let codec = Codec::from_flags(u32::from(flags));
    let chunk = match codec {
        Codec::None => data.len().max(1),
        Codec::Lzma => usize::try_from(u32::MAX).unwrap_or(usize::MAX),
        Codec::Lz4 | Codec::Lz4Hc => 0x20000,
        _ => return Err(SaveError::UnsupportedCodec(codec)),
    };
    let mut blocks = MeteredVec::new(meter);
    if codec == Codec::None {
        let len = narrow(data.len(), "stored block length")?;
        blocks.push(BlockInfo {
            uncompressed: len,
            compressed: len,
            flags,
            offset: head.filter(|version| *version >= 9).map(|_| 0),
        })?;
        return Ok(PackedBlocks {
            data: compress(data, codec, meter)?,
            blocks,
        });
    }
    let mut out = Writer::new(meter, Endian::Big)?;
    for part in data.chunks(chunk) {
        let packed = compress(part, codec, meter)?;
        let store = if head.is_some() {
            out.len() >= chunk
        } else {
            packed.len() > part.len()
        };
        let (stored, block_flags) = if store {
            (part, flags ^ u16::from(codec.id()))
        } else {
            (packed.as_ref(), flags)
        };
        blocks.push(BlockInfo {
            uncompressed: narrow(part.len(), "uncompressed block length")?,
            compressed: narrow(stored.len(), "compressed block length")?,
            flags: block_flags,
            offset: if head.is_some_and(|version| version >= 9) {
                Some(narrow(out.len(), "block offset")?)
            } else {
                None
            },
        })?;
        out.bytes(stored)?;
        if head.is_some_and(|version| version >= 9) {
            out.align(16)?;
        }
    }
    Ok(PackedBlocks {
        data: out.finish()?,
        blocks,
    })
}

/// `compress_lz4` (`helpers/CompressionHelper.py:97-105`). liblz4's external
/// state entry point runs exactly the same compressor as its allocating
/// convenience function, with a caller-provided eight-byte-aligned state.
fn compress_lz4(data: &[u8], meter: &Arc<Meter>) -> Result<Bytes, SaveError> {
    let capacity = unityrs_compress_sys::lz4_bound(data.len())
        .map_err(|e| native_error(e, Codec::Lz4Hc, meter))?;
    let words = unityrs_compress_sys::lz4_state_words()
        .map_err(|e| native_error(e, Codec::Lz4Hc, meter))?;
    // Count the Rust-side state zeroing as well as the input scan before
    // allocating or touching either compressor buffer.
    meter.count_decode(
        u64::try_from(data.len())
            .unwrap_or(u64::MAX)
            .saturating_add(u64::try_from(words).unwrap_or(u64::MAX).saturating_mul(8)),
    )?;
    let mut state = MeteredVec::with_capacity(meter, words)?;
    for _ in 0..words {
        state.push(0u64)?;
    }
    let mut out = DecodeBuf::counted(meter, u64::MAX, LimitKind::InputMemory)?;
    let output = out.grow_zeroed(capacity)?;
    let written = unityrs_compress_sys::lz4_compress(data, output, state.as_mut_slice())
        .map_err(|e| native_error(e, Codec::Lz4Hc, meter))?;
    out.truncate(written);
    Ok(out.into_bytes()?)
}

fn native_error(error: unityrs_compress_sys::Error, codec: Codec, meter: &Meter) -> SaveError {
    use unityrs_compress_sys::Error;
    match error {
        Error::Range => SaveError::Range("compressor length"),
        Error::Allocator(bytes) => {
            crate::limits::ChargeError::from(meter.allocator_refused(bytes)).into()
        }
        Error::Allowance => SaveError::Compress {
            codec,
            detail: "native encoder exceeded its charged allowance",
        },
        Error::Compress => SaveError::Compress {
            codec,
            detail: "native encoder failed",
        },
    }
}

/// `compress_lzma` (`helpers/CompressionHelper.py:47-79`). Filters and
/// liblzma allocations are owned by the safe native bridge; its complete
/// memory allowance is charged before the encoder starts.
fn compress_lzma(data: &[u8], meter: &Arc<Meter>) -> Result<Bytes, SaveError> {
    meter.count_decode(u64::try_from(data.len()).unwrap_or(u64::MAX))?;
    let allowance = unityrs_compress_sys::lzma_encoder_memory()
        .map_err(|e| native_error(e, Codec::Lzma, meter))?;
    let _state = meter.charge(allowance)?;
    let mut out = DecodeBuf::counted(meter, u64::MAX, LimitKind::InputMemory)?;
    out.extend_from_slice(&[0x5d, 0, 0, 0x80, 0])?;
    unityrs_compress_sys::lzma_compress(data, allowance, |part| out.extend_from_slice(part))
        .map_err(|error| match error {
            unityrs_compress_sys::LzmaError::Native(e) => native_error(e, Codec::Lzma, meter),
            unityrs_compress_sys::LzmaError::Output(e) => SaveError::Charge(e),
        })?;
    Ok(out.into_bytes()?)
}

#[cfg(test)]
#[path = "encode_tests.rs"]
mod tests;
