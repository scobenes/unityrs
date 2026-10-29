// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/CompressionHelper.py. See THIRD_PARTY_LICENSES.
//! Block codecs with UnityPy's semantics (`helpers/CompressionHelper.py:238-244`).
//!
//! - **None** returns the stored bytes as they are, whatever blocks-info says.
//! - **LZMA** (`:17-44`) is a raw LZMA1 decoder over the 5 property bytes and
//!   the rest, run to the end of the input: the declared size is ignored, a
//!   truncated stream yields what decoded, an over-long one yields all of it.
//!   Output is capped at [`crate::limits::Limits::max_block_output`] (a `PARITY.md` safety
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
//! names a range of its input (the stored codec: [`Decoded::Input`]), which
//! the caller slices. It cannot hand back bytes of its own: a buffer a
//! registered codec allocated (a Python decompressor's `bytes`) would be
//! input-sized memory no meter charged (round 5), so its output goes through
//! `out` like any other. Every block is decoded through [`decode_block`],
//! which starts the block's segment and checks what the codec returned. A
//! codec is caller code when a user registers it: the core calls it with no
//! lock held.
//!
//! Owner: package C. The registry type and [`decode_block`] are implemented
//! by the scaffold.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use bytes::Bytes;

use crate::budget::DecodeBuf;
use crate::error::{Error, LimitError, LimitKind};
use crate::limits::ChargeError;

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

/// Where a codec put a block's output ([`BlockCodec::decode`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decoded {
    /// In `out`: what the codec wrote into the block's segment.
    Written,
    /// In the input: the output is `input[range]`, a zero-copy slice of the
    /// stored bytes (the stored codec), with nothing written into `out`. A
    /// range, not `Bytes`, so a codec cannot hand back a buffer of its own.
    Input(Range<usize>),
}

/// Decodes one block.
pub trait BlockCodec: Send + Sync {
    /// Decodes `input`, a block's stored bytes (a slice of the bundle).
    /// `uncompressed` is blocks-info's declared size; each codec treats it as
    /// UnityPy does (module docs). The output goes into `out`, which pays and
    /// checks the caps before it grows, and the codec returns
    /// [`Decoded::Written`]; or, with `out` left empty, the codec returns
    /// [`Decoded::Input`], a range of `input` (the stored codec), which
    /// [`decode_block`] checks and slices.
    ///
    /// ```compile_fail,E0053
    /// // A codec cannot return bytes of its own (round 5: a buffer it
    /// // allocated was kept in the bundle's runs, charged to nothing).
    /// use unityrs_core::budget::DecodeBuf;
    /// use unityrs_core::codec::BlockCodec;
    /// use unityrs_core::{Bytes, Error};
    /// struct Own;
    /// impl BlockCodec for Own {
    ///     fn decode(&self, _: &Bytes, _: u32, _: &mut DecodeBuf) -> Result<Option<Bytes>, Error> {
    ///         Ok(Some(Bytes::from(vec![0u8; 1 << 20])))
    ///     }
    /// }
    /// ```
    fn decode(
        &self,
        input: &Bytes,
        uncompressed: u32,
        out: &mut DecodeBuf,
    ) -> Result<Decoded, Error>;
}

/// Decodes one block of a bundle with `codec` (the codec of `which`), as
/// every caller does (package C's bundle; implemented by the scaffold).
///
/// Starts the block's segment in `out`, capped at `cap` bytes
/// ([`DecodeBuf::begin_block`] with [`LimitKind::BlockOutput`]), runs the
/// codec, and checks what it returned: `Ok(None)` when the output is in the
/// segment; `Ok(Some(bytes))`, the slice of `input`, when the codec named a
/// range of its input, which must lie within the input, leave the segment
/// empty, and fit `cap` as if it had been written. A range outside the
/// input, or one returned with bytes written, is the codec's error
/// ([`Error::Decompress`]); a range longer than `cap` is
/// [`LimitKind::BlockOutput`].
pub fn decode_block(
    which: Codec,
    codec: &dyn BlockCodec,
    input: &Bytes,
    uncompressed: u32,
    out: &mut DecodeBuf,
    cap: u64,
) -> Result<Option<Bytes>, Error> {
    out.begin_block(cap, LimitKind::BlockOutput);
    let range = match codec.decode(input, uncompressed, out)? {
        Decoded::Written => return Ok(None),
        Decoded::Input(range) => range,
    };
    let bad = |detail: String| Error::Decompress {
        codec: which,
        detail: detail.into(),
    };
    if out.segment_len() != 0 {
        return Err(bad(format!(
            "the codec wrote {} bytes and returned a range of its input",
            out.segment_len()
        )));
    }
    if range.start > range.end || range.end > input.len() {
        return Err(bad(format!(
            "the codec returned the range {range:?} of a {}-byte input",
            input.len()
        )));
    }
    let n = u64::try_from(range.len()).unwrap_or(u64::MAX);
    if n > cap {
        return Err(Error::Limit(LimitError {
            kind: LimitKind::BlockOutput,
            value: n,
            max: cap,
        }));
    }
    Ok(Some(input.slice(range)))
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
    fn decode(&self, input: &Bytes, _: u32, _: &mut DecodeBuf) -> Result<Decoded, Error> {
        // UnityPy helpers/CompressionHelper.py:239: `lambda cd, _ucs: cd`.
        Ok(Decoded::Input(0..input.len()))
    }
}

/// Compression type 1: liblzma's raw LZMA1 decoder, to the end of the input.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lzma;

/// The dictionary the first LZMA attempt allocates at most, when the
/// declared size is larger (see [`Lzma`]'s `decode`).
const LZMA_FIRST_DICT_MAX: u64 = 8 << 20;

/// The dictionary the first LZMA attempt allocates at least.
const LZMA_FIRST_DICT_MIN: u64 = 1 << 20;

impl BlockCodec for Lzma {
    /// `decompress_lzma` (`CompressionHelper.py:17-44`): the properties byte
    /// and the little-endian dictionary size, then a raw LZMA1
    /// `LZMADecompressor`'s `decompress(data[5:])`, whose loop is
    /// Python's `decompress_buf` (`Modules/_lzmamodule.c`): run until the
    /// stream ends, the input is used up with room left in the output, or an
    /// error; the declared size plays no part.
    ///
    /// **The dictionary.** liblzma allocates the dictionary the properties
    /// name (up to 4 GiB) outside every meter. The decoder only uses it to
    /// refuse a match distance at or beyond `min(output so far, dictionary)`,
    /// so a dictionary at least as large as the output decodes exactly what
    /// the named one does. The first attempt uses the declared size, between
    /// 1 and 8 MiB, capped by the named size; an attempt that fails on data
    /// after it outgrew its dictionary is decoded again with a larger one
    /// (at least twice the output so far, capped by the named size), so the
    /// dictionary stays within a constant plus twice the block's charged
    /// output. Plan section 3.6 charges the named dictionary instead, which
    /// needs the block's meter and cap from [`DecodeBuf`] (REQUESTS-c.md 1).
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
        let first = u64::from(uncompressed).clamp(LZMA_FIRST_DICT_MIN, LZMA_FIRST_DICT_MAX);
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

/// Compression types 2 and 3: an LZ4 block, `uncompressed` as the maximum.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lz4;

impl BlockCodec for Lz4 {
    /// `lz4.block.decompress(data, uncompressed_size)` (python-lz4 4.4.4
    /// over liblz4 1.9.4's `LZ4_decompress_safe`): a declared size above
    /// `i32::MAX` is python-lz4's `OverflowError`; otherwise the block is
    /// scanned with liblz4's exact control flow ([`lz4_run`]), which gives
    /// its verdict and the output's length, and only then is the output
    /// allocated (that length, charged) and decoded.
    fn decode(
        &self,
        input: &Bytes,
        uncompressed: u32,
        out: &mut DecodeBuf,
    ) -> Result<Decoded, Error> {
        let fail = |detail: String| Error::Decompress {
            codec: Codec::Lz4,
            detail: detail.into(),
        };
        let Ok(capacity) = i32::try_from(uncompressed) else {
            return Err(fail(
                "OverflowError: signed integer is greater than maximum".to_owned(),
            ));
        };
        let capacity = usize::try_from(capacity).unwrap_or(0);
        let lz4_error = |at: usize| {
            fail(format!(
                "LZ4BlockError: Decompression failed: corrupt input or insufficient space in \
                 destination buffer. Error code: {}",
                at.saturating_add(1)
            ))
        };
        let n = lz4_run(input, capacity, None).map_err(lz4_error)?;
        let dst = out.grow_zeroed(n)?;
        lz4_run(input, capacity, Some(dst)).map_err(lz4_error)?;
        Ok(Decoded::Written)
    }
}

// liblz4 1.9.4 (lz4.c) constants.
const MINMATCH: usize = 4;
const LASTLITERALS: usize = 5;
const MFLIMIT: usize = 12;
const FASTLOOP_SAFE_DISTANCE: usize = 64;
const ML_MASK: usize = 15;
const RUN_MASK: usize = 15;

/// `read_variable_length` (lz4.c 1.9.4): the bytes at `*ip` summed while each
/// is 255; an error (`rvl_error`) once `*ip` passes `limit`, or, with
/// `initial_check`, when it starts at or past it. `limit` is signed: a
/// limit below the input's start fails the first check.
fn read_variable_length(
    src: &[u8],
    ip: &mut usize,
    limit: isize,
    initial_check: bool,
) -> Option<usize> {
    let past = |ip: usize| isize::try_from(ip).map_or(true, |i| i > limit);
    if initial_check && isize::try_from(*ip).map_or(true, |i| i >= limit) {
        return None;
    }
    let mut length = 0usize;
    loop {
        let s = *src.get(*ip)?;
        *ip += 1;
        length = length.saturating_add(usize::from(s));
        if past(*ip) {
            return None;
        }
        if s != 255 {
            return Some(length);
        }
    }
}

/// Copies a match of `length` bytes at `offset` behind `op` within `dst`,
/// byte by byte in LZ77 order; offset 0 writes zeros, as liblz4 does (it
/// zeroes four bytes, then copies them onto themselves).
fn copy_match(dst: &mut [u8], op: usize, offset: usize, length: usize) {
    let end = op.saturating_add(length).min(dst.len());
    if offset == 0 {
        if let Some(d) = dst.get_mut(op..end) {
            d.fill(0);
        }
        return;
    }
    let Some(src) = op.checked_sub(offset) else {
        return;
    };
    let mut at = op;
    while at < end {
        // Each pass copies what is already final: up to `offset` bytes, then
        // twice that, as the copied run lengthens.
        let step = (at - src).min(end - at);
        dst.copy_within(src..src + step, at);
        at += step;
    }
}

/// The LZ4 decoder's state: liblz4's `ip` and `op` as offsets, and the
/// output when this pass writes one.
struct Lz4State<'a, 'o> {
    src: &'a [u8],
    dst: Option<&'o mut [u8]>,
    ip: usize,
    op: usize,
    /// `iend` and `oend`, signed as liblz4's pointer comparisons are.
    iend: isize,
    oend: isize,
}

/// Where the safe loop is entered.
enum Lz4Entry {
    /// The loop's top.
    Top,
    /// `safe_literal_copy`, with the token and the literal length.
    Literals(usize, usize),
    /// `safe_match_copy`, with the offset and the length (MINMATCH added).
    Match(usize, usize),
}

fn signed(n: usize) -> isize {
    isize::try_from(n).unwrap_or(isize::MAX)
}

impl Lz4State<'_, '_> {
    fn token(&mut self) -> Result<usize, usize> {
        let t = self.src.get(self.ip).copied().ok_or(self.ip)?;
        self.ip += 1;
        Ok(usize::from(t))
    }

    fn le16(&mut self) -> Result<usize, usize> {
        let b = self
            .src
            .get(self.ip..self.ip.saturating_add(2))
            .and_then(|b| <[u8; 2]>::try_from(b).ok())
            .ok_or(self.ip)?;
        self.ip += 2;
        Ok(usize::from(u16::from_le_bytes(b)))
    }

    fn varlen(&mut self, limit: isize, initial_check: bool) -> Result<usize, usize> {
        read_variable_length(self.src, &mut self.ip, limit, initial_check).ok_or(self.ip)
    }

    /// Copies `n` literals and advances both positions.
    fn literals(&mut self, n: usize) {
        if let (Some(d), Some(from)) = (
            self.dst.as_deref_mut(),
            self.src.get(self.ip..self.ip.saturating_add(n)),
        ) && let Some(to) = d.get_mut(self.op..self.op.saturating_add(n))
        {
            to.copy_from_slice(from);
        }
        self.ip = self.ip.saturating_add(n);
        self.op = self.op.saturating_add(n);
    }

    /// Copies a match and advances the output.
    fn copy(&mut self, offset: usize, n: usize) {
        if let Some(d) = self.dst.as_deref_mut() {
            copy_match(d, self.op, offset, n);
        }
        self.op = self.op.saturating_add(n);
    }

    /// The match length at `_copy_match` from the token's `mlen`, MINMATCH
    /// added.
    fn match_length(&mut self, mlen: usize) -> Result<usize, usize> {
        let mut length = mlen;
        if length == ML_MASK {
            length += self.varlen(self.iend - signed(LASTLITERALS) + 1, false)?;
        }
        Ok(length + MINMATCH)
    }

    /// From the safe loop's literal copy (`cpy = op + length`, then
    /// `safe_literal_copy`) through the match's offset and length; `None`
    /// when this was the last sequence.
    fn safe_literals(
        &mut self,
        token: usize,
        length: usize,
    ) -> Result<Option<(usize, usize)>, usize> {
        let cpy = signed(self.op).saturating_add(signed(length));
        let lit_end = signed(self.ip).saturating_add(signed(length));
        if cpy > self.oend - signed(MFLIMIT) || lit_end > self.iend - signed(2 + 1 + LASTLITERALS) {
            // The last sequence: it must end the input exactly and fit.
            if lit_end != self.iend || cpy > self.oend {
                return Err(self.ip);
            }
            self.literals(length);
            return Ok(None);
        }
        self.literals(length);
        let offset = self.le16()?;
        let length = self.match_length(token & ML_MASK)?;
        Ok(Some((offset, length)))
    }

    /// The fast loop, run while 64 bytes of output remain; where the safe
    /// loop takes over.
    fn fast_loop(&mut self) -> Result<Lz4Entry, usize> {
        loop {
            let token = self.token()?;
            let mut length = token >> 4;
            if length == RUN_MASK {
                length += self.varlen(self.iend - signed(RUN_MASK), true)?;
                if signed(self.op).saturating_add(signed(length)) > self.oend - 32
                    || signed(self.ip).saturating_add(signed(length)) > self.iend - 32
                {
                    return Ok(Lz4Entry::Literals(token, length));
                }
            } else if signed(self.ip) > self.iend - (16 + 1) {
                return Ok(Lz4Entry::Literals(token, length));
            }
            self.literals(length);
            let offset = self.le16()?;
            let mut length = token & ML_MASK;
            if length == ML_MASK {
                length += self.varlen(self.iend - signed(LASTLITERALS) + 1, false)? + MINMATCH;
                if offset > self.op {
                    return Err(self.ip);
                }
            } else {
                length += MINMATCH;
            }
            if signed(self.op).saturating_add(signed(length))
                >= self.oend - signed(FASTLOOP_SAFE_DISTANCE)
            {
                return Ok(Lz4Entry::Match(offset, length));
            }
            if offset > self.op {
                return Err(self.ip);
            }
            self.copy(offset, length);
        }
    }

    /// The safe loop with its two-stage shortcut, from `entry`; the decoded
    /// length.
    fn safe_loop(&mut self, mut entry: Lz4Entry) -> Result<usize, usize> {
        // liblz4's `shortiend` and `shortoend`.
        let short_in = self.iend - 14 - 2;
        let short_out = self.oend - 14 - 18;
        loop {
            let (offset, length) = match std::mem::replace(&mut entry, Lz4Entry::Top) {
                Lz4Entry::Match(offset, length) => (offset, length),
                Lz4Entry::Literals(token, length) => match self.safe_literals(token, length)? {
                    Some(m) => m,
                    None => return Ok(self.op),
                },
                Lz4Entry::Top => {
                    let token = self.token()?;
                    let mut length = token >> 4;
                    if length != RUN_MASK
                        && signed(self.ip) < short_in
                        && signed(self.op) <= short_out
                    {
                        self.literals(length);
                        let mlen = token & ML_MASK;
                        let offset = self.le16()?;
                        if mlen != ML_MASK && offset >= 8 && offset <= self.op {
                            self.copy(offset, mlen + MINMATCH);
                            continue;
                        }
                        (offset, self.match_length(mlen)?)
                    } else {
                        if length == RUN_MASK {
                            length += self.varlen(self.iend - signed(RUN_MASK), true)?;
                        }
                        match self.safe_literals(token, length)? {
                            Some(m) => m,
                            None => return Ok(self.op),
                        }
                    }
                }
            };
            // `safe_match_copy`: the offset within the output, and the last
            // LASTLITERALS bytes left to literals.
            if offset > self.op {
                return Err(self.ip);
            }
            if signed(self.op).saturating_add(signed(length)) > self.oend - signed(LASTLITERALS) {
                return Err(self.ip);
            }
            self.copy(offset, length);
        }
    }
}

/// liblz4 1.9.4's `LZ4_decompress_generic` as `LZ4_decompress_safe` calls
/// it (a full block, no dictionary), on x86-64, where `LZ4_FAST_DEC_LOOP`
/// is on: the fast loop while 64 bytes of output remain, then the safe loop
/// with its two-stage shortcut. Every check is liblz4's at liblz4's place, so
/// the verdict is liblz4's; the copies are exact (liblz4's wild copies write
/// past a copy's end, but only bytes a later copy overwrites or the result
/// leaves out).
///
/// Returns the decoded length, or the input offset at which liblz4 fails (it
/// returns `-(offset) - 1`). With `dst` (exactly the decoded length, from an
/// earlier call without it), the output is written there.
fn lz4_run(src: &[u8], output_size: usize, dst: Option<&mut [u8]>) -> Result<usize, usize> {
    if output_size == 0 {
        return if src == [0] { Ok(0) } else { Err(0) };
    }
    if src.is_empty() {
        return Err(0);
    }
    let mut st = Lz4State {
        src,
        dst,
        ip: 0,
        op: 0,
        iend: signed(src.len()),
        oend: signed(output_size),
    };
    let entry = if st.oend >= signed(FASTLOOP_SAFE_DISTANCE) {
        st.fast_loop()?
    } else {
        Lz4Entry::Top
    };
    st.safe_loop(entry)
}
#[cfg(test)]
mod tests {
    use std::ops::Range;
    use std::sync::Arc;

    use bytes::Bytes;

    use super::{BlockCodec, Codec, Codecs, Decoded, Lz4, decode_block};
    use crate::budget::DecodeBuf;
    use crate::error::{Error, LimitKind};
    use crate::limits::Meter;

    /// A codec that writes `write` bytes, then returns `range` (or
    /// `Written` when there is none).
    struct Fake {
        write: usize,
        range: Option<Range<usize>>,
    }

    impl BlockCodec for Fake {
        fn decode(&self, _: &Bytes, _: u32, out: &mut DecodeBuf) -> Result<Decoded, Error> {
            out.grow_zeroed(self.write)?;
            Ok(self.range.clone().map_or(Decoded::Written, Decoded::Input))
        }
    }

    #[test]
    fn a_codecs_range_is_checked_and_sliced_from_its_input() {
        // Round 5: a codec's returned bytes were trusted to be a slice of its
        // input. Now it can only name a range, which must lie in the input,
        // with nothing written, within the block's cap.
        let m = Meter::unlimited();
        let input = Bytes::from(vec![7u8; 100]);
        let run = |codec: Fake, cap: u64| {
            let mut out = DecodeBuf::new(&m, 0, u64::MAX, LimitKind::BlockOutput).unwrap();
            out.extend_from_slice(&[1, 2, 3]).unwrap(); // an earlier block of the run
            let got = decode_block(Codec::Other(9), &codec, &input, 100, &mut out, cap);
            (got, out.len())
        };
        let (got, len) = run(
            Fake {
                write: 0,
                range: Some(10..60),
            },
            100,
        );
        let slice = got.unwrap().unwrap();
        assert_eq!(slice.len(), 50);
        assert_eq!(slice.as_ptr(), input[10..].as_ptr(), "a zero-copy slice");
        assert_eq!(len, 3);
        let (got, len) = run(
            Fake {
                write: 40,
                range: None,
            },
            100,
        );
        assert!(got.unwrap().is_none());
        assert_eq!(len, 43);
        for (codec, cap, what) in [
            (
                Fake {
                    write: 0,
                    range: Some(90..101),
                },
                1000,
                "past the end",
            ),
            (
                Fake {
                    write: 0,
                    range: Some(1000..2000),
                },
                1 << 20,
                "outside",
            ),
            (
                Fake {
                    write: 0,
                    range: Some(Range { start: 60, end: 10 }),
                },
                100,
                "reversed",
            ),
            (
                Fake {
                    write: 5,
                    range: Some(0..10),
                },
                100,
                "written and a range",
            ),
        ] {
            let (got, _) = run(codec, cap);
            assert!(
                matches!(
                    got,
                    Err(Error::Decompress {
                        codec: Codec::Other(9),
                        ..
                    })
                ),
                "{what}: {got:?}"
            );
        }
        let (got, _) = run(
            Fake {
                write: 0,
                range: Some(0..100),
            },
            99,
        );
        assert!(
            matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::BlockOutput && l.value == 100),
            "{got:?}"
        );
        // The segment was begun at the block's cap: a codec writing past it
        // is refused.
        let (got, _) = run(
            Fake {
                write: 50,
                range: None,
            },
            49,
        );
        assert!(
            matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::BlockOutput),
            "{got:?}"
        );
    }

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
