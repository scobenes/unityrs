// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/BundleFile.py. See THIRD_PARTY_LICENSES.
//! `decompress_data` (`files/BundleFile.py:489-521`): one block, or the
//! blocks-info section, through its registered codec, decrypted first
//! where UnityPy decrypts it.
//!
//! **The input is work.** A block's stored bytes count as the open's work
//! ([`crate::limits::Limits::max_decode_work`]) before a decryptor or a codec
//! reads them, whatever the decode then gives (round 6): LZ4 scans the whole
//! block before it writes a byte ([`crate::codec::Lz4`]), so a block that
//! fails at its end, or whose output the meter then refuses, wrote nothing
//! and counted nothing, and every directory row over a nested bundle opened
//! it again and scanned again, uncounted. The count is credit for what the
//! decode writes ([`DecodeBuf::prepay`]), so a decode counts the larger of
//! its block's stored bytes and its output: a compressed block that decodes
//! counts its output, as before. The built-in stored codec reads nothing
//! (it names its input), so a stored block counts what is copied from it,
//! as before. The count is one checked add before the read
//! ([`crate::limits::Meter::count_decode`]): a block the allowance cannot
//! pay for is refused with [`LimitKind::DecodeWork`] unread, and a decode the
//! budget or the allocator refuses gives back what its writes did not use
//! (`reading`).

use std::ops::Deref;
use std::sync::Arc;

use bytes::Bytes;

use crate::budget::DecodeBuf;
use crate::codec::{Codec, decode_block};
use crate::error::{Error, LimitKind, Unsupported};
use crate::limits::Meter;
use crate::options::FileContext;

/// What a bundle's blocks decode with: the caller's context, and whether
/// the bundle has a decryptor. UnityPy's `BundleFile.decryptor` is `None`
/// unless the header's encryption flag built one (`files/BundleFile.py:25`,
/// `:123-124`), and `decompress_data` decrypts only with one (`:512`), so a
/// decryptor the caller set reaches only the blocks of a bundle whose flag
/// is set (round 5: it decrypted a flagged block of any bundle). It
/// dereferences to the caller's context.
pub(super) struct BlockContext {
    ctx: Arc<FileContext>,
    encrypted: bool,
}

impl BlockContext {
    /// The context of a bundle whose header's encryption flag is `encrypted`.
    pub(super) fn new(ctx: &Arc<FileContext>, encrypted: bool) -> Self {
        Self {
            ctx: ctx.clone(),
            encrypted,
        }
    }

    /// The caller's context, which the bundle's entries parse with.
    pub(super) fn file(&self) -> &Arc<FileContext> {
        &self.ctx
    }
}

impl Deref for BlockContext {
    type Target = FileContext;

    fn deref(&self) -> &FileContext {
        &self.ctx
    }
}

/// `decompress_data` (`files/BundleFile.py:489-521`) on one block or the
/// blocks-info section, as a whole buffer: a slice of the input for a stored
/// one, else a charged decode.
pub(super) fn decode_one(
    ctx: &BlockContext,
    meter: &Arc<Meter>,
    input: Bytes,
    uncompressed: u32,
    flags: u32,
    index: usize,
) -> Result<Bytes, Error> {
    let mut out = DecodeBuf::new(meter, 0, u64::MAX, LimitKind::InputMemory)?;
    let (which, codec) = codec_for(ctx, flags)?;
    let decoded = reading(ctx, which, input.len(), &mut out, |out| {
        let input = decrypted(ctx, meter, input, flags, which, index)?;
        decode_block(
            which,
            codec.as_ref(),
            &input,
            uncompressed,
            out,
            ctx.limits.max_block_output,
        )
        .map_err(|e| with_codec(e, which))
    })?;
    match decoded {
        Some(slice) => Ok(slice),
        None => Ok(out.into_bytes()?),
    }
}

/// `decompress_data` on one block, appended to `out`.
pub(super) fn decode_block_into(
    ctx: &BlockContext,
    meter: &Arc<Meter>,
    input: Bytes,
    uncompressed: u32,
    flags: u32,
    index: usize,
    out: &mut DecodeBuf,
) -> Result<(), Error> {
    let (which, codec) = codec_for(ctx, flags)?;
    reading(ctx, which, input.len(), out, |out| {
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
    })
}

/// `decode`, which decrypts and decodes a block's `input` stored bytes
/// into `out`, with the bytes counted as the open's work before it runs
/// (module docs), as credit for what it writes ([`DecodeBuf::prepay`]): for
/// every codec but the built-in stored one
/// ([`crate::codec::Codecs::stores_as_is`]), which reads none. A decode
/// that ends in the budget's or the allocator's refusal
/// ([`Error::Budget`]) gives back the credit its writes did not use, as a
/// refused growth counts nothing: that depends on what else the process
/// held, and contention must not use up an open's allowance. Anything else
/// the decode ends in keeps it, the input's own meter refusing included, as
/// [`crate::once`] counts what such a refusal was granted.
fn reading<T>(
    ctx: &BlockContext,
    which: Codec,
    input: usize,
    out: &mut DecodeBuf,
    decode: impl FnOnce(&mut DecodeBuf) -> Result<T, Error>,
) -> Result<T, Error> {
    let n = if which == Codec::None && ctx.codecs.stores_as_is() {
        0
    } else {
        u64::try_from(input).unwrap_or(u64::MAX)
    };
    out.prepay(n)?;
    let got = decode(out);
    let unused = out.end_prepaid();
    if matches!(got, Err(Error::Budget(_))) {
        out.meter().uncount_decode(unused);
    }
    got
}

/// The registered codec for `flags & 0x3F`: LZHAM and an unknown type
/// raise (`CompressionHelper.py:234-235`, `files/BundleFile.py:515-521`).
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

/// The block decrypted as UnityPy decrypts it (a bundle whose header built
/// a decryptor, `flags & 0x100`, not stored), in place in a charged copy;
/// the input as it is otherwise.
// UnityPy files/BundleFile.py:512-513
fn decrypted(
    ctx: &BlockContext,
    meter: &Arc<Meter>,
    input: Bytes,
    flags: u32,
    which: Codec,
    index: usize,
) -> Result<Bytes, Error> {
    let Some(d) = ctx.decryptor.as_ref().filter(|_| ctx.encrypted) else {
        return Ok(input);
    };
    if flags & 0x100 == 0 || which == Codec::None {
        return Ok(input);
    }
    let mut copy = DecodeBuf::new(meter, 0, u64::MAX, LimitKind::InputMemory)?;
    let block = copy.grow_zeroed(input.len())?;
    block.copy_from_slice(&input);
    // A pool refusal the decryptor returns is counted for the
    // Environment's retry (`env::registry::from_callers`).
    crate::env::registry::from_callers(d.decrypt(block, index))?;
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
