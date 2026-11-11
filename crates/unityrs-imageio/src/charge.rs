//! How the encoder's memory is charged before it is allocated
//! (`docs/plans/extract.md` E59, `docs/plans/batch.md` 2.6 step 10).
//!
//! Every block the encoder takes in proportion to the image, and zlib's
//! deflate state, is first offered to the caller's [`Charger`] as the
//! requests it will make of Rust's global allocator, one [`Buffer`] at a
//! time; a refusal allocates nothing and ends the encode. [`plan`] gives the
//! same requests ahead of time. The engine's charger charges the sum of each
//! request's `heap_cost` (`unityrs_core::limits::heap_cost`) to the job's
//! meter (`Meter::charge`) and holds the `Charge` until the buffer is freed:
//!
//! ```ignore
//! impl imageio::Charger for Attempt {
//!     type Charge = unityrs_core::limits::Charge;
//!     type Refusal = unityrs_core::limits::ChargeError;
//!     fn charge(&mut self, _: imageio::Buffer, requests: &[usize]) -> Result<Charge, ChargeError> {
//!         let cost = requests.iter().map(|&n| heap_cost(n as u64)).fold(0, u64::saturating_add);
//!         self.meter.charge(cost)
//!     }
//! }
//! ```
//!
//! The encoder holds the charges of the deflate state and of the rows until
//! it has freed them, and hands the output's back with the PNG
//! ([`crate::Png`]), dropped after the bytes. What the encoder takes beside
//! these is a constant: the `z_stream` and its allowance, on the stack.

use crate::image::{ImageError, Mode};
use crate::zlib;

/// A buffer the encoder charges before it allocates it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Buffer {
    /// zlib's deflate state: the five blocks `deflateInit2` allocates at
    /// Pillow's settings, through `zalloc` ([`DEFLATE_STATE_REQUESTS`]),
    /// charged as a constant before `deflateInit2`.
    DeflateState,
    /// The PNG file, allocated once at its final capacity
    /// ([`Plan::output`]).
    Output,
    /// The five rows the filter works in ([`Plan::rows`]): the row, the one
    /// before it, and the up, sub and Paeth candidates, each a filter byte
    /// and the row's bytes, as Pillow's `ZipEncode.c` keeps them (less the
    /// average row, which only `optimize` uses).
    Rows,
}

/// Charges the encoder's buffers before it allocates them (module docs).
pub trait Charger {
    /// A granted charge, held while its buffer lives; dropping it gives the
    /// bytes back.
    type Charge;
    /// Why a charge was refused.
    type Refusal;

    /// Charges `buffer`, whose allocations ask Rust's global allocator for
    /// `requests` bytes each (a `Vec<u8>`'s capacity, or a `zalloc` block
    /// with its header). A refusal ends the encode with nothing allocated
    /// for `buffer`.
    fn charge(&mut self, buffer: Buffer, requests: &[usize])
    -> Result<Self::Charge, Self::Refusal>;
}

/// A [`Charger`] that grants every charge: for callers that meter nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Unmetered;

impl Charger for Unmetered {
    type Charge = ();
    type Refusal = std::convert::Infallible;

    fn charge(&mut self, _: Buffer, _: &[usize]) -> Result<(), Self::Refusal> {
        Ok(())
    }
}

/// The requests of zlib's deflate state, which [`Buffer::DeflateState`]
/// charges: each block `deflateInit2` asks `zalloc` for at Pillow's
/// settings, rounded up to 8 bytes, plus the 8-byte header `zalloc` puts
/// before it. The first, the state itself, is a bound above its size on
/// every target (5,952 bytes on 64-bit Linux and macOS); `zalloc` refuses a
/// block past its request, so zlib holds no more than this. At glibc's
/// `heap_cost` the five come to 407,600 bytes, about 398 KiB
/// (`docs/plans/batch.md` 5.5).
pub const DEFLATE_STATE_REQUESTS: [usize; 5] = [
    zlib::request(zlib::STATE_SLOTS[0]),
    zlib::request(zlib::STATE_SLOTS[1]),
    zlib::request(zlib::STATE_SLOTS[2]),
    zlib::request(zlib::STATE_SLOTS[3]),
    zlib::request(zlib::STATE_SLOTS[4]),
];

/// The PNG signature, IHDR and IEND: what a PNG holds beside its IDAT
/// chunks.
const FRAMING: u64 = 8 + (12 + 13) + 12;
/// One chunk's length, type and CRC.
const CHUNK: u64 = 12;

/// What encoding an image of a size and mode charges, before it starts:
/// the requests of each [`Buffer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// [`Buffer::Rows`]: five rows of the row's bytes and a filter byte.
    pub rows: [usize; 5],
    /// [`Buffer::Output`]: the PNG's one allocation. zlib's `deflateBound`
    /// of the filtered bytes (`height x (row + 1)`) at Pillow's settings,
    /// one byte more, and the framing: the signature, IHDR, IEND and 12
    /// bytes for each IDAT chunk there can be (each but the last holds
    /// [`Plan::idat_size`] bytes).
    pub output: usize,
    /// [`Buffer::DeflateState`]: [`DEFLATE_STATE_REQUESTS`].
    pub deflate_state: [usize; 5],
    /// The data bytes of each IDAT chunk but the last: Pillow 9.5.0's
    /// encoder buffer, `max(65536, 4 x width)` bytes (`ImageFile._save`).
    pub idat_size: usize,
    /// The filtered bytes zlib deflates: `height x (row + 1)`.
    pub filtered: u64,
}

/// The [`Plan`] of encoding a `width` x `height` image in `mode`, or why
/// Pillow 9.5.0 writes no PNG of it ([`ImageError::ZeroSide`], then
/// [`ImageError::TooWide`], in the order Pillow's `_setimage` checks), or
/// why this crate does not ([`ImageError::TooLarge`]).
pub fn plan(width: u32, height: u32, mode: Mode) -> Result<Plan, ImageError> {
    if width == 0 || height == 0 {
        return Err(ImageError::ZeroSide { width, height });
    }
    let widest = widest(mode);
    if width > widest {
        return Err(ImageError::TooWide {
            width,
            mode,
            widest,
        });
    }
    // Within `widest`, the row (at most 268,435,448 bytes) and the encoder
    // buffer, `max(65536, 4 x width)` bytes (at most 1,073,741,792), are C
    // ints, so neither the encoder's check of the row (Pillow 9.5.0,
    // `src/libImaging/ZipEncode.c:37-40`) nor a buffer size past a C int can
    // refuse. The rest is the crate's own.
    let too_large = ImageError::TooLarge { width, height };
    if u64::from(height) > u64::from(i32::MAX.unsigned_abs()) {
        return Err(too_large);
    }
    let row = u64::from(width) * mode.channels() as u64;
    let idat_size = (u64::from(width) * 4).max(65536);
    let filtered = u64::from(height) * (row + 1);
    let bound = deflate_bound(filtered).ok_or(too_large.clone())?;
    let chunks = bound / idat_size + 1;
    let output = chunks
        .checked_mul(CHUNK)
        .and_then(|c| c.checked_add(FRAMING))
        .and_then(|c| c.checked_add(bound))
        .and_then(|c| c.checked_add(1))
        .and_then(|c| usize::try_from(c).ok())
        .ok_or(too_large.clone())?;
    let row = usize::try_from(row + 1).map_err(|_| too_large.clone())?;
    let idat_size = usize::try_from(idat_size).map_err(|_| too_large)?;
    Ok(Plan {
        rows: [row; 5],
        output,
        deflate_state: DEFLATE_STATE_REQUESTS,
        idat_size,
        filtered,
    })
}

/// The widest image Pillow 9.5.0's encoder takes in `mode`,
/// `INT_MAX / bits - 7` (`src/encode.c:261`), with the raw mode's bits a
/// pixel from the packers' table (`src/libImaging/Pack.c:553-588`):
/// [`ImageError::TooWide`].
const fn widest(mode: Mode) -> u32 {
    let bits = match mode {
        Mode::L => 8,
        Mode::La => 16,
        Mode::Rgb => 24,
        Mode::Rgba => 32,
    };
    i32::MAX.unsigned_abs() / bits - 7
}

/// zlib 1.2.13's `deflateBound` of `len` bytes for a zlib stream at
/// Pillow's settings (`deflate.c`): memLevel 9 makes `hash_bits` 16, not
/// 15, so zlib returns its conservative bound for fixed blocks, `len +
/// len/8 + len/256 + len/512 + 4`, plus the zlib wrapper's 6 bytes (no
/// preset dictionary). `None` when it passes a `u64`. The tests hold it to
/// zlib's own.
pub(crate) fn deflate_bound(len: u64) -> Option<u64> {
    len.checked_add(len >> 3)?
        .checked_add(len >> 8)?
        .checked_add(len >> 9)?
        .checked_add(4 + 6)
}
