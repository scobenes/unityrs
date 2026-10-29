// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/CompressionHelper.py. See THIRD_PARTY_LICENSES.
//! Compression type 1: [`Lzma`], liblzma's raw LZMA1 decoder.

use bytes::Bytes;

use super::{BlockCodec, Codec, Decoded};
use crate::budget::DecodeBuf;
use crate::error::{Error, LimitKind};
use crate::limits::{ChargeError, heap_cost};

/// Compression type 1: liblzma's raw LZMA1 decoder, to the end of the input.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lzma;

const _: () = crate::send_sync::<Lzma>();

/// The dictionary the first LZMA attempt allocates at most, when the
/// declared size is larger (see [`Lzma`]'s `decode`).
const LZMA_FIRST_DICT_MAX: u64 = 8 << 20;

/// liblzma's smallest dictionary (`LZMA_DICT_SIZE_MIN`): a smaller size is
/// allocated at this, and the first attempt asks for no less.
const LZMA_DICT_SIZE_MIN: u32 = 4096;

impl BlockCodec for Lzma {
    /// `decompress_lzma` (`CompressionHelper.py:17-44`): the properties byte
    /// and the little-endian dictionary size, then a raw LZMA1
    /// `LZMADecompressor`'s `decompress(data[5:])`, whose loop is
    /// Python's `decompress_buf` (`Modules/_lzmamodule.c`): run until the
    /// stream ends, the input is used up with room left in the output, or an
    /// error; the declared size plays no part.
    ///
    /// **The dictionary.** liblzma allocates the dictionary the properties
    /// name (up to 4 GiB) on the C heap. The decoder only uses it to
    /// refuse a match distance at or beyond `min(output so far, dictionary)`,
    /// so a dictionary at least as large as the output decodes exactly what
    /// the named one does. The first attempt uses the declared size, between
    /// liblzma's 4 KiB minimum and 8 MiB, capped by the named size (a
    /// well-formed block fills exactly its declared size, so it decodes
    /// once); an attempt that fails on data
    /// after it outgrew its dictionary is decoded again with a larger one
    /// (at least twice the output so far, capped by the named size), so the
    /// dictionary stays within a constant plus twice the block's charged
    /// output. Each attempt's dictionary is charged to the input's meter
    /// ([`DecodeBuf::meter`]) before liblzma allocates it, and returned when
    /// the attempt ends, so the meter sees it beside the output (plan
    /// section 3.15); a refusal is the meter's, like a refused growth.
    fn decode(
        &self,
        input: &Bytes,
        uncompressed: u32,
        out: &mut DecodeBuf,
    ) -> Result<Decoded, Error> {
        // UnityPy helpers/CompressionHelper.py:27: `struct.unpack("<BI", data[:5])`.
        let (Some(&props), Some(dict)) = (input.first(), input.get(1..5)) else {
            return Err(lzma_error(&format!(
                "struct.error: unpack requires a buffer of 5 bytes (got {})",
                input.len()
            )));
        };
        let named = u32::from_le_bytes(<[u8; 4]>::try_from(dict).unwrap_or_default());
        let data = input.get(5..).unwrap_or_default();
        // UnityPy helpers/CompressionHelper.py:28-31
        let lc = u32::from(props % 9);
        let rest = u32::from(props / 9);
        let (pb, lp) = (rest / 5, rest % 5);
        let first =
            u64::from(uncompressed).clamp(u64::from(LZMA_DICT_SIZE_MIN), LZMA_FIRST_DICT_MAX);
        let mut dict = u32::try_from(first).unwrap_or(u32::MAX).min(named);
        let start = out.len();
        loop {
            match lzma_attempt([lc, lp, pb], dict, data, uncompressed, out) {
                Err(LzmaFail::Data(produced))
                    if dict < named
                        && u64::try_from(produced).unwrap_or(u64::MAX) >= u64::from(dict) =>
                {
                    out.truncate(start);
                    let wider = u64::try_from(produced)
                        .unwrap_or(u64::MAX)
                        .saturating_mul(2)
                        .max(u64::from(dict).saturating_mul(2));
                    dict = u32::try_from(wider).unwrap_or(u32::MAX).min(named);
                }
                Err(LzmaFail::Data(_)) => return Err(lzma_error("LZMAError: Corrupt input data")),
                Err(LzmaFail::Other(e)) => return Err(e),
                Ok(()) => return Ok(Decoded::Written),
            }
        }
    }
}

fn lzma_error(detail: &str) -> Error {
    Error::Decompress {
        codec: Codec::Lzma,
        detail: detail.into(),
    }
}

enum LzmaFail {
    /// `LZMA_DATA_ERROR` after this many bytes of output.
    Data(usize),
    /// Anything else.
    Other(Error),
}

impl From<ChargeError> for LzmaFail {
    fn from(e: ChargeError) -> Self {
        Self::Other(e.into())
    }
}

/// The window an LZMA decode grows its output by at most at once.
const LZMA_WINDOW_MAX: usize = 1 << 20;

/// One decode of an LZMA1 stream with dictionary `dict`, appended to `out`.
fn lzma_attempt(
    [lc, lp, pb]: [u32; 3],
    dict: u32,
    data: &[u8],
    declared: u32,
    out: &mut DecodeBuf,
) -> Result<(), LzmaFail> {
    use liblzma::stream::{Action, Filters, LzmaOptions, Status, Stream};

    let other = |what: &str| LzmaFail::Other(lzma_error(what));
    let map = |e: liblzma::stream::Error, produced: usize| match e {
        liblzma::stream::Error::Data => LzmaFail::Data(produced),
        liblzma::stream::Error::Options => other("LZMAError: Invalid or unsupported options"),
        liblzma::stream::Error::Mem => other("MemoryError"),
        _ => other("LZMAError: Internal error"),
    };
    // CPython's `parse_filter_spec_lzma` starts from the default preset and
    // sets what the filter spec names.
    let mut opts = LzmaOptions::new_preset(6).map_err(|e| map(e, 0))?;
    opts.dict_size(dict)
        .literal_context_bits(lc)
        .literal_position_bits(lp)
        .position_bits(pb);
    let mut filters = Filters::new();
    filters.lzma1(&opts);
    // liblzma allocates the whole dictionary (at least its 4 KiB minimum)
    // when the decoder starts: charged first, held for the attempt.
    let _dictionary = out
        .meter()
        .charge(heap_cost(u64::from(dict.max(LZMA_DICT_SIZE_MIN))))?;
    let mut stream = Stream::new_raw_decoder(&filters).map_err(|e| map(e, 0))?;
    let seg_start = out.len();
    // The first window is the declared size, which a well-formed block
    // fills exactly; a probe then finds the end without growing again.
    let mut want = match usize::try_from(declared) {
        Ok(0) | Err(_) => LZMA_WINDOW_MAX,
        Ok(n) => n.min(LZMA_WINDOW_MAX),
    };
    let input_done =
        |stream: &Stream| usize::try_from(stream.total_in()).unwrap_or(usize::MAX) >= data.len();
    let rest = |stream: &Stream| {
        let consumed = usize::try_from(stream.total_in()).unwrap_or(usize::MAX);
        data.get(consumed..).unwrap_or_default()
    };
    // Whether the last call left the output full (Python then grows its
    // buffer and calls again; here a one-byte probe asks first, so a
    // well-formed block is never given room it does not fill).
    let mut full = false;
    loop {
        let produced = out.len() - seg_start;
        if full {
            let mut probe = [0u8; 1];
            let before = stream.total_out();
            let status = stream
                .process(rest(&stream), &mut probe, Action::Run)
                .map_err(|e| map(e, produced))?;
            if stream.total_out() > before {
                out.extend_from_slice(&probe)?;
            }
            match status {
                Status::StreamEnd => return Ok(()),
                Status::MemNeeded if !input_done(&stream) => {
                    return Err(other("LZMAError: Insufficient buffer space"));
                }
                _ => {}
            }
            // Nothing more and no input left ends it. No progress with input
            // left: liblzma answers `LZMA_OK` once before `LZMA_BUF_ERROR`,
            // so ask again with a window.
            if stream.total_out() == before && input_done(&stream) {
                return Ok(());
            }
            full = false;
            continue;
        }
        // A window the meter or the block's cap refuses is retried smaller,
        // down to one byte; a refused byte goes to the probe, which fails
        // only if the stream has another byte to give.
        let base = out.len();
        let window = match out.grow_zeroed(want) {
            Ok(w) => w,
            Err(e) if want > 1 => {
                want = refused_window(&e, want, produced);
                continue;
            }
            Err(_) => {
                full = true;
                continue;
            }
        };
        let room = window.len();
        let before = stream.total_out();
        let status = stream.process(rest(&stream), window, Action::Run);
        let made = usize::try_from(stream.total_out() - before).unwrap_or(room);
        out.truncate(base + made);
        let status = status.map_err(|e| map(e, produced + made))?;
        match status {
            Status::StreamEnd => return Ok(()),
            // `LZMA_BUF_ERROR` with no input left and room in the output is
            // not an error in Python; with input left, it is.
            Status::MemNeeded if !input_done(&stream) && made < room => {
                return Err(other("LZMAError: Insufficient buffer space"));
            }
            _ => {}
        }
        if made == room {
            // `avail_out == 0`.
            full = true;
            want = want.saturating_mul(2).min(LZMA_WINDOW_MAX);
        } else if input_done(&stream) {
            // `avail_in == 0`: a truncated stream yields what decoded.
            return Ok(());
        }
    }
}

/// The next window after `want` bytes were refused with `e`, after
/// `produced` bytes of this block: the room left under the block's cap when
/// that is what refused, else half.
fn refused_window(e: &ChargeError, want: usize, produced: usize) -> usize {
    let half = (want / 2).max(1);
    match e {
        ChargeError::Limit(l) if l.kind == LimitKind::BlockOutput => {
            let room = usize::try_from(l.max)
                .unwrap_or(usize::MAX)
                .saturating_sub(produced);
            room.clamp(1, half)
        }
        _ => half,
    }
}
