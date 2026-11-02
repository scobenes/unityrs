// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/CompressionHelper.py. See THIRD_PARTY_LICENSES.
//! Compression type 1: [`Lzma`], liblzma's raw LZMA1 decoder.

use bytes::Bytes;

use super::{BlockCodec, Codec, Decoded};
use crate::budget::{DecodeBuf, Reservation};
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

/// The room liblzma's LZ decoder keeps on each side of its dictionary
/// (`2 * LZ_DICT_REPEAT_MAX`, `lz/lz_decoder.h:75` of the xz 5.8.4 that
/// `liblzma-sys` 0.4.9 builds).
const LZ_DICT_REPEAT_ROOM: u64 = 2 * 288;

impl Lzma {
    /// What one liblzma raw LZMA1 decoder holds beside its dictionary, in
    /// glibc chunks ([`heap_cost`]): liblzma's own state, 32,784 bytes (the
    /// stream's `lzma_internal`, `common/common.c:183`; the LZ decoder's
    /// coder, `lz/lz_decoder.c:229`; the LZMA1 decoder and its probability
    /// tables, `lzma/lzma_decoder.c:1122`, of the xz 5.8.4 that
    /// `liblzma-sys` 0.4.9 builds), and the 256 bytes the `liblzma` crate's
    /// filter chain takes on the Rust heap while it is built (a one-entry
    /// array grown to four, 16 bytes then 64, both held while it grows, and
    /// a 128-byte options node: `src/stream.rs:709-735` and `:1128-1131` of
    /// `liblzma` 0.4.9). Each attempt charges it beside the dictionary
    /// ([`Self::decoder_charge`]; review round 12: only the dictionary was
    /// charged, so about 32 KiB a decode reached no meter).
    ///
    /// `tests/lzma_decoder_heap.rs` measures both on glibc, in a child
    /// process, and fails unless liblzma's allocations and the filter chain
    /// are exactly [`Self::decoder_charge`], and one decode's charge covers
    /// the heap it holds with at most 32 bytes to spare (the filter array's
    /// first chunk, freed once it grows); `cargo test -p unityrs-core --test
    /// lzma_decoder_heap -- --nocapture` prints what it measured.
    pub const DECODER_STATE: u64 = 32_784 + 256;

    /// What one decode attempt with a dictionary of `dict` bytes charges
    /// before liblzma allocates anything: the dictionary buffer liblzma
    /// allocates (`lz/lz_decoder.c:253-289`: at least 4 KiB, rounded up to
    /// a multiple of 16, and `2 * LZ_DICT_REPEAT_MAX`, 576 bytes, more; its
    /// `LZ_DICT_EXTRA` is 0 in `liblzma-sys`'s build, whose `config.h`
    /// defines no `HAVE_IMMINTRIN_H`, `lz/lz_decoder.h:22-46`), in a glibc
    /// chunk, and [`Self::DECODER_STATE`].
    /// A `const fn`, so a bound sized from it is checked when it compiles
    /// (`unityrs-tpk`'s `TpkProvider::MAX_DECODER_MEMORY`).
    #[must_use]
    pub const fn decoder_charge(dict: u32) -> u64 {
        let dict = if dict < LZMA_DICT_SIZE_MIN {
            LZMA_DICT_SIZE_MIN
        } else {
            dict
        };
        let buffer = (dict as u64).next_multiple_of(16) + LZ_DICT_REPEAT_ROOM;
        heap_cost(buffer) + Self::DECODER_STATE
    }
}

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
    /// output. Each attempt's dictionary buffer and the decoder's state
    /// ([`Lzma::DECODER_STATE`]) are charged to the input's meter
    /// ([`DecodeBuf::meter`]) before liblzma allocates them, and returned
    /// when the attempt ends, so the meter sees them beside the output (plan
    /// section 3.15); a refusal is the meter's, like a refused growth. The
    /// output's room is `out`'s: it grows geometrically, never past the
    /// block's cap, and a retry keeps the room the failed attempt grew, so an
    /// attempt holds its room (and, while the room grows, the block it
    /// replaces) beside its dictionary. A caller that must bound that sum by
    /// the output's cap alone allocates the room whole first, as unityrs-tpk
    /// does (`TpkProvider::MAX_DECODER_MEMORY`; review round 13).
    fn decode(
        &self,
        input: &Bytes,
        uncompressed: u32,
        out: &mut DecodeBuf,
    ) -> Result<Decoded, Error> {
        // UnityPy helpers/CompressionHelper.py:26: `struct.unpack("<BI", data[:5])`.
        let (Some(&props), Some(dict)) = (input.first(), input.get(1..5)) else {
            return Err(lzma_error(&format!(
                "struct.error: unpack requires a buffer of 5 bytes (got {})",
                input.len()
            )));
        };
        let named = u32::from_le_bytes(<[u8; 4]>::try_from(dict).unwrap_or_default());
        let data = input.get(5..).unwrap_or_default();
        // UnityPy helpers/CompressionHelper.py:27-30
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

/// A liblzma failure after `produced` bytes of the block's output, in an
/// attempt that holds `held` bytes of dictionary and decoder state on a
/// meter booked as `what`. `LZMA_MEM_ERROR` is the allocator refusing what
/// liblzma allocates itself, the dictionary above all, which the meter had granted:
/// a refusal of memory, as a growth the Rust allocator refuses is
/// ([`crate::limits::MeteredVec`]), so it is the meter's reservation's
/// [`Error::Budget`] (transient: nothing keeps it, and the same block decodes
/// once the memory is there), never a decode failure (review round 6: it
/// was `Decompress`, `"MemoryError"`, so a TPK loaded while the process was
/// short of memory read as a corrupt one). `tests/codecs.rs`
/// `liblzmas_refused_dictionary_is_the_meters_reservation_refused` makes the
/// C allocator refuse it, in a child process under an address-space cap
/// (`unityrs_testkit::address_space`), on a meter of each reservation
/// (review round 7: only this function's own test covered it).
fn failure(e: liblzma::stream::Error, produced: usize, held: u64, what: Reservation) -> LzmaFail {
    let other = |detail: &str| LzmaFail::Other(lzma_error(detail));
    match e {
        liblzma::stream::Error::Data => LzmaFail::Data(produced),
        liblzma::stream::Error::Options => other("LZMAError: Invalid or unsupported options"),
        liblzma::stream::Error::Mem => {
            LzmaFail::Other(Error::Budget(crate::limits::refusal(held, what)))
        }
        _ => other("LZMAError: Internal error"),
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
    // liblzma allocates the whole dictionary (at least its 4 KiB minimum)
    // and its state when the decoder starts, and the filter chain allocates
    // on the Rust heap: all charged first, held for the attempt.
    let held = Lzma::decoder_charge(dict);
    let _decoder = out.meter().charge(held)?;
    let what = out.meter().reservation();
    let map = |e: liblzma::stream::Error, produced: usize| failure(e, produced, held, what);
    // CPython's `parse_filter_spec_lzma` starts from the default preset and
    // sets what the filter spec names.
    let mut opts = LzmaOptions::new_preset(6).map_err(|e| map(e, 0))?;
    opts.dict_size(dict)
        .literal_context_bits(lc)
        .literal_position_bits(lp)
        .position_bits(pb);
    let mut filters = Filters::new();
    filters.lzma1(&opts);
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

#[cfg(test)]
mod tests {
    use super::{LzmaFail, failure};
    use crate::budget::{BudgetExceeded, Reservation};
    use crate::error::Error;
    use crate::once::CellError;

    /// Round 6: liblzma's `LZMA_MEM_ERROR` (the allocator refusing the
    /// dictionary liblzma allocates itself) was `Decompress`, `"MemoryError"`,
    /// a decode failure. It is the meter's reservation refused the memory it
    /// had granted, as a growth the Rust allocator refuses is: a caller tells
    /// it from a corrupt block, and nothing keeps it.
    #[test]
    fn liblzmas_memory_error_is_a_refusal_of_memory_not_a_decode_failure() {
        for what in [Reservation::Input, Reservation::Provider] {
            match failure(liblzma::stream::Error::Mem, 3, 8 << 20, what) {
                LzmaFail::Other(e @ Error::Budget(_)) => {
                    assert!(e.is_transient());
                    let Error::Budget(b) = e else { unreachable!() };
                    assert_eq!(
                        b,
                        BudgetExceeded {
                            requested: 8 << 20,
                            what
                        }
                    );
                }
                LzmaFail::Other(e) => panic!("{what:?}: {e:?}"),
                LzmaFail::Data(n) => panic!("{what:?}: a data error after {n} bytes"),
            }
        }
        // A data error is still the stream's, after the bytes it made.
        assert!(matches!(
            failure(liblzma::stream::Error::Data, 3, 4096, Reservation::Input),
            LzmaFail::Data(3)
        ));
    }

    /// On a dependency's meter, liblzma's memory error is a refusal of the
    /// dependency pool, counted as every other one is
    /// (`crate::limits::pool_refusals`), which the Environment's retry tells
    /// a refusal it already retried from a later one by (review round 8).
    #[test]
    fn liblzmas_memory_error_on_a_dependency_counts_as_a_pool_refusal() {
        let before = crate::limits::pool_refusals();
        let fail = failure(
            liblzma::stream::Error::Mem,
            3,
            8 << 20,
            Reservation::Dependency,
        );
        assert!(
            matches!(&fail, LzmaFail::Other(Error::Budget(b)) if b.what == Reservation::Dependency)
        );
        assert_eq!(crate::limits::pool_refusals() - before, 1);
    }
}
