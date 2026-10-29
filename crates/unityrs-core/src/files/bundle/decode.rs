// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/BundleFile.py. See THIRD_PARTY_LICENSES.
//! `decompress_data` (`BundleFile.py:509-521`): one block, or the
//! blocks-info section, through its registered codec, decrypted first
//! where UnityPy decrypts it.

use std::sync::Arc;

use bytes::Bytes;

use crate::budget::DecodeBuf;
use crate::codec::{Codec, decode_block};
use crate::error::{Error, LimitKind, Unsupported};
use crate::limits::Meter;
use crate::options::FileContext;

/// `decompress_data` (`BundleFile.py:509-521`) on one block or the
/// blocks-info section, as a whole buffer: a slice of the input for a stored
/// one, else a charged decode.
pub(super) fn decode_one(
    ctx: &FileContext,
    meter: &Arc<Meter>,
    input: Bytes,
    uncompressed: u32,
    flags: u32,
    index: usize,
) -> Result<Bytes, Error> {
    let mut out = DecodeBuf::new(meter, 0, u64::MAX, LimitKind::InputMemory)?;
    let (which, codec) = codec_for(ctx, flags)?;
    let input = decrypted(ctx, meter, input, flags, which, index)?;
    match decode_block(
        which,
        codec.as_ref(),
        &input,
        uncompressed,
        &mut out,
        ctx.limits.max_block_output,
    )
    .map_err(|e| with_codec(e, which))?
    {
        Some(slice) => Ok(slice),
        None => Ok(out.into_bytes()?),
    }
}

/// `decompress_data` on one block, appended to `out`.
pub(super) fn decode_block_into(
    ctx: &FileContext,
    meter: &Arc<Meter>,
    input: Bytes,
    uncompressed: u32,
    flags: u32,
    index: usize,
    out: &mut DecodeBuf,
) -> Result<(), Error> {
    let (which, codec) = codec_for(ctx, flags)?;
    let input = decrypted(ctx, meter, input, flags, which, index)?;
    if let Some(slice) = decode_block(
        which,
        codec.as_ref(),
        &input,
        uncompressed,
        out,
        ctx.limits.max_block_output,
    )
    .map_err(|e| with_codec(e, which))?
    {
        out.extend_from_slice(&slice)?;
    }
    Ok(())
}

/// The registered codec for `flags & 0x3F`: LZHAM and an unknown type
/// raise (`CompressionHelper.py:234-235`, `BundleFile.py:515-521`).
fn codec_for(
    ctx: &FileContext,
    flags: u32,
) -> Result<(Codec, Arc<dyn crate::codec::BlockCodec>), Error> {
    let which = Codec::from_flags(flags);
    match ctx.codecs.get(which) {
        Some(c) => Ok((which, c.clone())),
        None if which == Codec::Lzham => Err(Error::Unsupported(Unsupported::Lzham)),
        None => Err(Error::Unsupported(Unsupported::Codec(which.id()))),
    }
}

/// The block decrypted as UnityPy decrypts it (`flags & 0x100`, not
/// stored), in place in a charged copy; the input as it is otherwise.
// UnityPy files/BundleFile.py:512-513
fn decrypted(
    ctx: &FileContext,
    meter: &Arc<Meter>,
    input: Bytes,
    flags: u32,
    which: Codec,
    index: usize,
) -> Result<Bytes, Error> {
    let Some(d) = ctx.decryptor.as_ref() else {
        return Ok(input);
    };
    if flags & 0x100 == 0 || which == Codec::None {
        return Ok(input);
    }
    let mut copy = DecodeBuf::new(meter, 0, u64::MAX, LimitKind::InputMemory)?;
    let block = copy.grow_zeroed(input.len())?;
    block.copy_from_slice(&input);
    d.decrypt(block, index)?;
    Ok(copy.into_bytes()?)
}

/// A decode error named by the block's codec (one codec may serve two ids).
fn with_codec(e: Error, which: Codec) -> Error {
    match e {
        Error::Decompress { detail, .. } => Error::Decompress {
            codec: which,
            detail,
        },
        e => e,
    }
}
