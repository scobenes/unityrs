// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
//! `TpkFile` (`helpers/Tpk.py:145-197`): the header, the compressed bytes,
//! and `GetDataBlob`'s decompression, with unityrs-core's codecs in place of
//! python-lz4 and `decompress_lzma` (crate docs, "Loading").

use std::ops::Deref;
use std::sync::Arc;

use unityrs_core::budget::{DecodeBuf, Reservation, Unbounded, charged_bytes};
use unityrs_core::codec::{self, BlockCodec, Codec};
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_core::{Bytes, LimitKind};

use unityrs_core::limits::ChargeError;

use crate::blob::memory;
use crate::{TpkError, TpkProvider};

/// `TpkFile.Struct.size`: `<IbbbbIII`.
const HEADER: usize = 20;
/// `TpkFile.TpkMagicBytes`, `b"TPK*"`.
const MAGIC: u32 = 0x2A4B_5054;
/// `TpkFile.TpkVersionNumber`.
const VERSION: u8 = 1;
/// `TpkCompressionType` (`Tpk.py:98-102`).
const NONE: u8 = 0;
const LZ4: u8 = 1;
const LZMA: u8 = 2;
const BROTLI: u8 = 3;
/// `TpkDataType.TypeTreeInformation`; the members are 0-5 (`Tpk.py:105-111`).
const TYPE_TREE_INFORMATION: u8 = 0;
const LAST_DATA_TYPE: u8 = 5;

/// A TPK's payload: its stored bytes, or what a codec decoded from them.
pub(crate) enum Payload<'a> {
    Stored(&'a [u8]),
    Decoded(Bytes),
}

impl Deref for Payload<'_> {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            Payload::Stored(b) => b,
            Payload::Decoded(b) => b,
        }
    }
}

/// `TpkFile(stream)` and `GetDataBlob()` up to the blob (`Tpk.py:155-197`):
/// the header checked in UnityPy's order, then the payload, decoded; the
/// data type is checked last, as `init`'s `assert isinstance(blob,
/// TpkTypeTreeBlob)` is (`:37-38`). What decoding copies is charged to
/// `meter`, the provider's.
pub(crate) fn payload<'a>(bytes: &'a [u8], meter: &Arc<Meter>) -> Result<Payload<'a>, TpkError> {
    let bad = |what: &str| TpkError::Malformed(what.into());
    // `TpkFile.Struct.unpack(stream.read(20))`.
    let Some((head, rest)) = bytes.split_first_chunk::<HEADER>() else {
        return Err(bad("struct.error: unpack requires a buffer of 20 bytes"));
    };
    // magic, versionNumber, compressionType, dataType, _, _,
    // CompressedSize, UncompressedSize.
    let [
        m0,
        m1,
        m2,
        m3,
        version,
        compression,
        data_type,
        _,
        _,
        _,
        _,
        _,
        c0,
        c1,
        c2,
        c3,
        u0,
        u1,
        u2,
        u3,
    ] = *head;
    if u32::from_le_bytes([m0, m1, m2, m3]) != MAGIC {
        return Err(bad("Invalid TPK magic bytes"));
    }
    if version != VERSION {
        return Err(bad("Invalid TPK version number"));
    }
    // `TpkCompressionType(...)` and `TpkDataType(...)`: the signed bytes must
    // be members (0-3 and 0-5).
    if compression > BROTLI {
        return Err(bad("ValueError: not a TpkCompressionType"));
    }
    if data_type > LAST_DATA_TYPE {
        return Err(bad("ValueError: not a TpkDataType"));
    }
    // The sizes are `I`: unsigned.
    let compressed = u32::from_le_bytes([c0, c1, c2, c3]);
    let uncompressed = u32::from_le_bytes([u0, u1, u2, u3]);
    // `stream.read(self.CompressedSize)`, which must return them all.
    let data = usize::try_from(compressed)
        .ok()
        .and_then(|n| rest.get(..n))
        .ok_or_else(|| bad("Invalid compressed size"))?;
    let decoded = match compression {
        NONE => Payload::Stored(data),
        LZ4 => Payload::Decoded(decode(Codec::Lz4, &codec::Lz4, data, uncompressed, meter)?),
        LZMA => Payload::Decoded(decode(
            Codec::Lzma,
            &codec::Lzma,
            data,
            uncompressed,
            meter,
        )?),
        _ => return Err(TpkError::Unsupported("brotli")),
    };
    // `self.DataType.ToBlob(...)`, then `init`'s assertion: any other blob
    // fails the load. UnityPy reads a blob of data type 1-3 first and raises
    // its reader's exception where it does not read; this reads none, so
    // only the exception's name differs (`PARITY.md`, `TpkFile`'s row).
    if data_type != TYPE_TREE_INFORMATION {
        return Err(TpkError::NotTypeTrees);
    }
    Ok(decoded)
}

/// `data` decoded by `codec`, as one block of unityrs-core's: the output
/// capped at [`TpkProvider::MAX_DECODED_BYTES`], and the output and an LZMA
/// decoder (its dictionary and liblzma's state) charged to a meter of
/// [`TpkProvider::MAX_DECODER_MEMORY`]. An LZMA payload's output is
/// allocated once, at the cap, before the decoder starts: the stream runs
/// to its end whatever the header declares (`decompress_lzma`), so room
/// grown as it decodes would depend on the declared size, which UnityPy
/// ignores, and a retry with a larger dictionary would hold it beside the
/// room's last growth (review round 13: a payload of 16 MiB declared
/// 1 MiB - 1 held 32 MiB of room and a 32 MiB dictionary, and was refused
/// for decoder memory). LZ4's output is allocated once by the codec, at
/// the length its scan finds.
/// The codec reads its input as `Bytes`, so `data` is copied once, the copy
/// charged to `provider` (the provider's meter) before it is made (round 4:
/// it was not charged). The decoder's meter is the provider's reservation,
/// and a growth the allocator refuses is the load's `OutOfMemory`, as every
/// other growth of the provider's is ([`memory`]), not a decode failure
/// (round 5: it was `Decompress`, "refused ... for Input"). So is liblzma's
/// own allocation of the dictionary the decoder's meter granted, which the
/// codec reports as the meter's [`unityrs_core::Error::Budget`] (round 6:
/// it was `Decompress`, `"MemoryError"`).
fn decode(
    which: Codec,
    codec: &dyn BlockCodec,
    data: &[u8],
    uncompressed: u32,
    provider: &Arc<Meter>,
) -> Result<Bytes, TpkError> {
    let name = if which == Codec::Lz4 { "lz4" } else { "lzma" };
    let fail = |e: unityrs_core::Error| match e {
        unityrs_core::Error::Limit(l) => TpkError::TooLarge {
            what: if l.kind == LimitKind::InputMemory {
                "decoder memory"
            } else {
                "decoded payload"
            },
            value: l.value,
            max: l.max,
        },
        unityrs_core::Error::Budget(b) => memory(ChargeError::Budget(b)),
        unityrs_core::Error::Decompress { detail, .. } => TpkError::Decompress {
            codec: name,
            detail: Box::from(&*detail),
        },
        e => TpkError::Decompress {
            codec: name,
            detail: e.to_string().into(),
        },
    };
    let meter = Meter::new(
        TpkProvider::MAX_DECODER_MEMORY,
        Arc::new(Unbounded),
        Reservation::Provider,
    );
    let cap = TpkProvider::MAX_DECODED_BYTES;
    let room = if which == Codec::Lzma { cap } else { 0 };
    let mut out =
        DecodeBuf::new(&meter, room, cap, LimitKind::BlockOutput).map_err(|e| fail(e.into()))?;
    let mut copy = MeteredVec::with_capacity(provider, data.len()).map_err(memory)?;
    copy.extend_from_slice(data).map_err(memory)?;
    let (copy, charge) = copy.into_parts();
    let data = charged_bytes(copy, charge).map_err(memory)?;
    match codec::decode_block(which, codec, &data, uncompressed, &mut out, cap).map_err(fail)? {
        Some(input) => Ok(input),
        None => out.into_bytes().map_err(|e| fail(e.into())),
    }
}
