// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/Texture2DConverter.py's parse_image_data for the formats decoded here (the checks, the block padding, the crop and the flip) and its astc's data check, and how Pillow 9.5.0, texture2ddecoder 1.0.5 and astc-encoder-py 0.1.12, as it calls them, treat short and long data and the sizes they refuse. See THIRD_PARTY_LICENSES.
// Ported from Pillow 9.5.0 (MIT-CMU): its raw decoder as UnityPy calls it (src/libImaging/RawDecode.c's row copy, and the width cap of src/decode.c's _setimage), and the decompression-bomb check of src/PIL/Image.py's crop, with its message. See THIRD_PARTY_LICENSES.
//! [`decode()`]: a texture's mip 0 to pixels, as `parse_image_data` gives them
//! for the formats of [`Format`] (`export/Texture2DConverter.py:254-320`), and
//! [`plan()`]: what a decode of data of a given length will need and give,
//! or why it fails, before the data is read.
//!
//! **What UnityPy does.** `parse_image_data` returns an empty image for a
//! side of 0, raises for empty data, pads a block format's size to whole
//! 4x4 blocks (`:291`, `get_compressed_image_size`; the raw formats are not
//! padded; an ASTC format to whole blocks of its footprint, `:36-50`), has
//! Pillow decode the padded image (`Image.frombytes` with
//! `CONV_TABLE`'s arguments, `:331-344`, `:488-566`), or for an ETC or EAC
//! format texture2ddecoder, whose `BGRA` output Pillow's raw decoder then
//! reads (`:405-432`, `:523-532`), or for an ASTC format astcenc (the
//! `astc` module), whose `RGBA` output it reads (`:356-366`, `:533-544`),
//! crops it back to size
//! (`:314-315`) and flips it top to bottom (`:317-318`). Pillow decodes the
//! data from its start and stops when the image is full: bytes past it
//! (later mips, anything else) are ignored, so a texture with mips decodes
//! as its mip 0.
//!
//! **The order.** [`plan()`] and [`decode()`] make one series of checks,
//! the first that fails naming the error, so a plan's answer is the
//! decode's:
//! 1. A side of 0 gives an empty `RGBA` image, 0x0, before the data, the
//!    format or the other side is looked at (`:265-266`).
//! 2. Empty data is [`DecodeError::NoImageData`], UnityPy's
//!    `ValueError("Texture2D has no image data")` (`:268-269`).
//! 3. A negative side is [`DecodeError::NegativeSize`]: Pillow's
//!    `ValueError`, whichever of its checks meets it first ("Width and
//!    height must be >= 0", or, where padding made a side 0, a crop or tile
//!    error). For an ETC or EAC format, where padding made a side 0
//!    texture2ddecoder decodes nothing and the crop raises, but a side
//!    padded below 0 (-4 and below) reaches it as a width or height near
//!    2^32, which it multiplies in 32 bits and then reads and writes past
//!    its buffers with: the error is unityrs's own refusal there
//!    (`PARITY.md` section 19.3). So for an ASTC format: where padding made a
//!    side 0 the crop raises, and a side padded below 0 reaches
//!    astc-encoder-py as a negative size, which it refuses with `ASTCError`
//!    when one side is, and with both reads and writes past astcenc's
//!    buffers once the data holds the blocks the two negative counts'
//!    product names.
//! 4. A texture of more than [`MAX_PIXELS`] pixels at its padded size is
//!    [`DecodeError::TooLarge`], whatever the data's length once it is not
//!    empty: the bound UnityPy does not have (below).
//! 5. A raw texture wider than `INT_MAX / bits - 7` pixels is
//!    [`DecodeError::PillowMemoryError`]: Pillow's raw decoder refuses such
//!    a row with `MemoryError` when it is set up, before it reads any data
//!    (`_setimage`, `src/decode.c:197-198`), `bits` being its raw mode's
//!    unpacker's (`Format::raw_bits`): 268,435,448 pixels for Alpha8 and R8
//!    (8 bits), 134,217,720 for ARGB4444, RGBA4444 and RGB565 (16),
//!    89,478,478 for RGB24 and BGR24 (24), 67,108,856 for RGBA32, BGRA32
//!    and ARGB32 (32).
//!    So is an ETC, EAC or ASTC texture whose padded width passes
//!    67,108,856 (67,108,857 and up, padded to 67,108,860 at 4x4): Pillow's
//!    raw decoder reads texture2ddecoder's output as `BGRA`, or astcenc's
//!    as `RGBA`, at that width, and refuses it after the decode, which
//!    changes no pixel. The `bcn` decoder has no such cap. For an ASTC
//!    format step 6 comes first: `astc` checks the data's length before
//!    astcenc decodes and Pillow reads (`:356-366`).
//! 6. Data that runs out before mip 0 does is [`DecodeError::NotEnoughData`],
//!    Pillow's `ValueError` "not enough image data" (`Image.frombytes`;
//!    `RawDecode.c` reads whole rows, of `bits / 8` bytes a pixel,
//!    `BcnDecode.c` whole blocks). The `BCn`
//!    decoder takes the data's length as a C `int` (`decode_bcn`'s
//!    `bytes`), so data of 2 GiB or more is read as that length's low 32
//!    bits, signed: data whose low 32 bits are below what the image needs,
//!    or negative, is short too ([`bcn_length`]). texture2ddecoder checks
//!    no length, and reads on past the data's end where it is short (its
//!    `decode` template, `src/pylink.cpp`, ignores the length it parses):
//!    for an ETC or EAC format the error is unityrs's own refusal, which
//!    reads nothing that is not there (`PARITY.md` section 19.3). For an
//!    ASTC format it is `astc`'s
//!    `ValueError("Invalid ASTC data size: {len} < {size}")`, mip 0's blocks
//!    at 16 bytes each (`calculate_astc_compressed_size`, `:390-397`).
//! 7. The decode.
//! 8. A block-format texture (DXT1, DXT3, DXT5, BC4, BC5, BC7, ETC, EAC or
//!    ASTC) with a side that
//!    is no multiple of its block's and
//!    more than [`MAX_CROP_PIXELS`] (178,956,970) pixels is
//!    [`DecodeError::DecompressionBomb`]: UnityPy's crop of the padded image
//!    runs Pillow's decompression-bomb check on the size it crops to, which
//!    raises `DecompressionBombError` past twice `Image.MAX_IMAGE_PIXELS`
//!    (`Image._crop`, `src/PIL/Image.py:1232-1250`; `:3162-3180`), after the
//!    decode. No pixel depends on it, so it is decided before the decode,
//!    after the data check, as in Pillow. An aligned texture is not cropped,
//!    nor is a raw one, and neither is checked. Past `MAX_IMAGE_PIXELS`
//!    alone the crop only warns, which changes no pixel.
//!
//! The decoders write straight into the output, cropped and flipped as they
//! go, with no padded image between: each block is decoded into a 16-pixel
//! array on the stack (an ASTC block into one of up to 144 pixels), and only
//! the pixels inside the texture are copied,
//! each to its flipped row. That is the padded decode, then the crop and
//! the flip, pixel for pixel.
//!
//! **The bound, where UnityPy has none.** [`MAX_PIXELS`] is 16384 x 16384
//! at the padded size, the largest size Unity's texture importer offers,
//! checked before the data's length (step 4), so the output is at most
//! 1 GiB, the work linear in it, and [`plan()`] needs only the data's
//! length. It is
//! the stated divergence (`PARITY.md` section 19.3), and every limit Pillow
//! has that a texture could meet beyond steps 5 and 8 lies past it:
//! `Image.new`'s `MemoryError` for a row wider than 536,870,910 pixels, the
//! `OverflowError` of a `BCn` side padded to 2^31, and the raw decoder's
//! count of the bytes it read, which wraps through an `int` at 2 GiB
//! (Pillow then returns the image with its unread rows zero). A texture
//! with a negative side and data is [`DecodeError::NegativeSize`] whatever
//! its other side, where Pillow, given a padded side past those limits,
//! raises theirs. With step 5 in, no width that passes overflows the `int`
//! Pillow's raw decoder computes a row's bytes in
//! (`src/libImaging/RawDecode.c:31`, `state->xsize * state->bits + 7`): a
//! width of at most `INT_MAX / bits - 7` makes it at most
//! `INT_MAX - 7 * bits + 7`. Sizes are computed here in `u64`, from the
//! `i32` sides, so none wraps.
//!
//! **Memory** (`docs/plans/extract.md` E59). The output is the only buffer a
//! decode allocates in proportion to the texture (no scratch: a block is 64
//! bytes on the stack). An ASTC format's decode also builds its footprint's
//! tables (the `astc` module, at most about 0.55 MB, at 12x12, whatever the
//! texture's size), charged to the same meter after the output and dropped
//! before the decode returns: [`Plan::scratch`]. The output is charged to the caller's [`Meter`] at
//! [`heap_cost`] of its length before it is allocated (through
//! [`MeteredVec`], whose refusal allocates nothing and whose allocation
//! failure is a refusal too), once every check above has passed, and the
//! [`Charge`] lives in the [`Image`], declared after its pixels, so it is
//! returned after their buffer is freed, on every path, a panic's unwinding
//! included. Only the pixel loop runs under [`crate::contain()`]. The
//! engine calls [`plan()`] first: an `Ok` is a decode that will charge
//! [`Plan::charge`], at most [`Plan::peak_charge`] while it runs, and write
//! [`Plan::output_bytes`] (export work,
//! `docs/plans/batch.md` 2.6 step 10); an `Err` is the decode's error, which
//! charges nothing.

use std::fmt;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use unityrs_core::limits::{Charge, ChargeError, Meter, MeteredVec, heap_cost};

use crate::contain::Payload;
use crate::format::{Format, Library, Mode};
use crate::{astc, bcn, etc, raw};

/// The most pixels a texture may have, at its padded size: 16384 x 16384.
/// UnityPy has no such bound (module docs; `PARITY.md` section 19.3).
pub const MAX_PIXELS: u64 = 1 << 28;

/// The most pixels UnityPy's crop of a padded block-format image takes: twice
/// Pillow 9.5.0's `Image.MAX_IMAGE_PIXELS`, `1024 * 1024 * 1024 // 4 // 3`
/// (module docs, step 8).
pub const MAX_CROP_PIXELS: u64 = 2 * 89_478_485;

/// C's `INT_MAX`, the bound of Pillow's `int` sizes.
const INT_MAX: u32 = 0x7FFF_FFFF;

/// Why a texture did not decode, in the order of the module docs' checks.
/// Every variant but [`Self::TooLarge`], [`Self::Charge`] and
/// [`Self::Panicked`] is a case where UnityPy 1.25.2 raises too, but
/// for an ETC or EAC format with short data or a side padded below 0,
/// where texture2ddecoder reads past the data (module docs, steps 3 and
/// 6). [`plan()`]
/// returns the checks' variants only, never `Charge` or `Panicked`, which
/// [`decode()`] alone gives, after a plan that passed.
///
/// Not `Clone`, by design, unlike the core's errors: `Panicked` owns the
/// panic's payload, the box the unwinding carried ([`Payload`]), which
/// cannot be cloned, and a copy of its text would allocate on the panic
/// path.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DecodeError {
    /// The data is empty: UnityPy's `ValueError("Texture2D has no
    /// image data")`.
    #[error("the texture has no image data")]
    NoImageData,
    /// A side is negative (and neither is 0): Pillow's `ValueError`.
    #[error("the texture's size {width}x{height} has a negative side")]
    NegativeSize {
        /// `m_Width`.
        width: i32,
        /// `m_Height`.
        height: i32,
    },
    /// The texture passes [`MAX_PIXELS`] at its padded size, a bound
    /// UnityPy does not have.
    #[error("the texture's {pixels} pixels pass the decoders' bound of {max}")]
    TooLarge {
        /// Its pixels, padded to whole blocks.
        pixels: u64,
        /// [`MAX_PIXELS`].
        max: u64,
    },
    /// A raw texture's row, or an ETC, EAC or ASTC texture's padded row,
    /// wider than Pillow's raw decoder takes: Pillow's `MemoryError`
    /// (module docs, step 5).
    #[error("Pillow's raw decoder refuses a row of {width} pixels, past {max} (MemoryError)")]
    PillowMemoryError {
        /// The row's width as Pillow's raw decoder gets it: `m_Width`,
        /// padded to whole blocks for an ETC, EAC or ASTC format.
        width: u32,
        /// The widest row it takes: `INT_MAX / bits - 7`.
        max: u32,
    },
    /// The data ends before mip 0 does: Pillow's `ValueError` "not enough
    /// image data", for an ASTC format UnityPy's `ValueError` "Invalid ASTC
    /// data size", or for an ETC or EAC format, where texture2ddecoder
    /// reads on past the data's end, unityrs's own refusal (module docs,
    /// step 6).
    #[error("not enough image data: {needed} bytes needed, {got} given")]
    NotEnoughData {
        /// What mip 0 holds, at its padded size.
        needed: u64,
        /// The data's length.
        got: u64,
    },
    /// UnityPy's crop of a padded `BCn` image meets Pillow's
    /// decompression-bomb check: its `DecompressionBombError`, whose message
    /// this one is (module docs, step 8).
    #[error(
        "Image size ({pixels} pixels) exceeds limit of {max} pixels, could be decompression bomb DOS attack."
    )]
    DecompressionBomb {
        /// `m_Width` x `m_Height`.
        pixels: u64,
        /// [`MAX_CROP_PIXELS`].
        max: u64,
    },
    /// The meter or its budget refused the output buffer, or the allocator
    /// did; nothing was allocated.
    #[error(transparent)]
    Charge(#[from] ChargeError),
    /// The decoder panicked ([`crate::contain()`]): its payload, kept whole.
    #[error("the decoder panicked: {payload}")]
    Panicked {
        /// The payload; [`Payload::message`] is the manifest's detail.
        payload: Payload,
    },
}

const _: () = crate::send_sync::<DecodeError>();

/// What decoding a texture of a format and a size, from data of a given
/// length, will need and give, worked out before the data is read
/// ([`plan()`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The image's mode: the format's, or `RGBA` for a side of 0.
    pub mode: Mode,
    /// The image's width: the texture's, or 0 for a side of 0.
    pub width: u32,
    /// The image's height: the texture's, or 0 for a side of 0.
    pub height: u32,
    /// The bytes of data mip 0 takes, at its padded size; 0 for a side of
    /// 0.
    pub input_bytes: u64,
    /// The image's bytes: width x height x the mode's bytes per pixel.
    pub output_bytes: u64,
    /// What the decode charges beside the output while it runs, and returns
    /// before it ends: an ASTC format's tables (the `astc` module), 0 for
    /// the rest.
    pub scratch: u64,
}

const _: () = crate::send_sync::<Plan>();

impl Plan {
    /// What [`decode()`] charges the meter for the output: [`heap_cost`] of
    /// [`Self::output_bytes`] (0 for the empty image). The allocator may
    /// round the allocation up past it, in which case [`MeteredVec`]
    /// charges the difference too.
    #[must_use]
    pub const fn charge(&self) -> u64 {
        heap_cost(self.output_bytes)
    }

    /// The most [`decode()`] has charged at once: [`Self::charge`] and
    /// [`Self::scratch`].
    #[must_use]
    pub const fn peak_charge(&self) -> u64 {
        self.charge().saturating_add(self.scratch)
    }
}

/// What decoding `format` at `width` x `height` from `data_len` bytes of
/// data needs and gives, or the error [`decode()`] returns for it: the
/// module docs' checks, all of them but the decode itself, in their order.
pub fn plan(format: Format, data_len: usize, width: i32, height: i32) -> Result<Plan, DecodeError> {
    if width == 0 || height == 0 {
        return Ok(Plan {
            mode: Mode::Rgba,
            width: 0,
            height: 0,
            input_bytes: 0,
            output_bytes: 0,
            scratch: 0,
        });
    }
    if data_len == 0 {
        return Err(DecodeError::NoImageData);
    }
    let (Ok(w), Ok(h)) = (u32::try_from(width), u32::try_from(height)) else {
        return Err(DecodeError::NegativeSize { width, height });
    };
    let (w64, h64) = (u64::from(w), u64::from(h));
    let block = format.block_bytes();
    let (padded_w, padded_h) = match format.block_size() {
        Some((bw, bh)) => {
            let (bw, bh) = (u64::from(bw), u64::from(bh));
            (w64.div_ceil(bw) * bw, h64.div_ceil(bh) * bh)
        }
        None => (w64, h64),
    };
    // Each side is at most 2^31 + 11, so the product fits a u64.
    let pixels = padded_w * padded_h;
    if pixels > MAX_PIXELS {
        return Err(DecodeError::TooLarge {
            pixels,
            max: MAX_PIXELS,
        });
    }
    let mode = format.mode();
    let library = format.library();
    // From here every size is at most 4 x MAX_PIXELS.
    let len = u64::try_from(data_len).unwrap_or(u64::MAX);
    let (input_bytes, seen) = match (block, format.block_size(), library) {
        (Some(b), Some((bw, bh)), _) => {
            let blocks = padded_w / u64::from(bw) * (padded_h / u64::from(bh));
            let seen = if library == Library::PillowBcn {
                bcn_length(data_len)
            } else {
                len
            };
            (blocks * b, seen)
        }
        // Pillow's raw decoder reads whole rows of `bits` a pixel, each a
        // whole number of bytes here.
        _ => (pixels * u64::from(format.raw_bits().unwrap_or(0) / 8), len),
    };
    let short = seen < input_bytes;
    let not_enough = || DecodeError::NotEnoughData {
        needed: input_bytes,
        got: len,
    };
    // UnityPy checks an ASTC texture's data before astcenc decodes it and
    // Pillow reads its output (`export/Texture2DConverter.py:356-366`); for
    // the rest Pillow's width cap comes first (module docs, step 5).
    if library == Library::Astcenc && short {
        return Err(not_enough());
    }
    if let Some(bits) = format.raw_bits() {
        // The width Pillow's raw decoder is set up with: the texture's, or
        // for texture2ddecoder's and astcenc's output the padded one, at
        // most 2^28 here.
        let row = u32::try_from(padded_w).unwrap_or(u32::MAX);
        let max = INT_MAX / bits - 7;
        if row > max {
            return Err(DecodeError::PillowMemoryError { width: row, max });
        }
    }
    if short {
        return Err(not_enough());
    }
    let shown = w64 * h64;
    if (padded_w, padded_h) != (w64, h64) && shown > MAX_CROP_PIXELS {
        return Err(DecodeError::DecompressionBomb {
            pixels: shown,
            max: MAX_CROP_PIXELS,
        });
    }
    Ok(Plan {
        mode,
        width: w,
        height: h,
        input_bytes,
        output_bytes: shown * mode.bytes_per_pixel(),
        scratch: astc_footprint(format).map_or(0, astc::Tables::charge),
    })
}

/// An ASTC format's footprint.
fn astc_footprint(format: Format) -> Option<astc::Footprint> {
    if format.library() != Library::Astcenc {
        return None;
    }
    let (w, h) = format.block_size()?;
    astc::Footprint::new(w, h)
}

/// A decoded image: rows top to bottom, each pixel its mode's bytes, red
/// first. It holds the [`Charge`] for its pixels, returned when it drops,
/// after the pixels' buffer is freed.
pub struct Image {
    mode: Mode,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    // After `pixels`: fields drop in order, so the charge outlives the
    // buffer it pays for.
    charge: Charge,
}

const _: () = crate::send_sync::<Image>();

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Image")
            .field("mode", &self.mode)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.pixels.len())
            .field("charge", &self.charge)
            .finish()
    }
}

impl Image {
    /// An image of pixels already charged by `charge` (the sprite renderer's
    /// canvases).
    pub(crate) fn from_parts(
        mode: Mode,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
        charge: Charge,
    ) -> Self {
        Self {
            mode,
            width,
            height,
            pixels,
            charge,
        }
    }

    /// The mode.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixels: what Pillow's `tobytes()` gives for the image UnityPy
    /// returns. One exception: `tobytes()`, and so a PNG save, raises
    /// `MemoryError` for an image wider than `INT_MAX / bits - 7` pixels,
    /// `bits` its mode's (its raw encoder's cap, `src/encode.c:260-262`):
    /// 67,108,856 for `RGBA`, 89,478,478 for `RGB`. The images that wide
    /// that decode, in UnityPy as here, are a `BCn` texture's in `RGBA`:
    /// 67,108,857 to 67,108,864 wide at a height of 1 or 2, and 67,108,860
    /// or 67,108,864 wide at a height of 4 (a height of 3, or another width
    /// at 4, is a [`DecodeError::DecompressionBomb`]); and a raw texture's
    /// whose raw mode takes fewer bits a pixel than its image's mode, up to
    /// that raw mode's cap (step 5): Alpha8's, ARGB4444's and RGBA4444's in
    /// `RGBA`, R8's and RGB565's in `RGB`. A PNG writer mirrors that cap.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// The charge the pixels hold.
    #[must_use]
    pub fn charge(&self) -> &Charge {
        &self.charge
    }

    /// The pixels and their charge, which must live as long as the pixels'
    /// allocation does: drop the charge last. Beware `let (pixels, charge) =
    /// image.into_parts();`: a pattern's bindings drop in reverse order, so
    /// that drops `charge` first, returning its bytes while the buffer still
    /// holds them. Drop the pixels first (`drop(pixels)`), or keep the
    /// charge in an owner that outlives them.
    pub fn into_parts(self) -> (Vec<u8>, Charge) {
        (self.pixels, self.charge)
    }
}

/// Decodes a texture's mip 0, `data` being its image data from the start
/// (`image_data`, or what its `m_StreamData` names: `Texture2D.get_image_data`),
/// mips and all, as UnityPy 1.25.2 with Pillow 9.5.0 does (module docs),
/// charging the output to `meter` before it is allocated. Its pixel loop
/// runs under [`crate::contain()`], so a panic there is
/// [`DecodeError::Panicked`], with the output's charge returned.
pub fn decode(
    format: Format,
    data: &[u8],
    width: i32,
    height: i32,
    meter: &Arc<Meter>,
) -> Result<Image, DecodeError> {
    decode_with(format, data, width, height, meter, fill)
}

/// The pixels the loop writes: `out` is `w` x `h` pixels of the format's
/// mode, `data` holds at least mip 0 ([`plan()`]), and an ASTC format has
/// its tables.
type Fill = fn(Format, &[u8], usize, usize, &mut [u8], Option<&mut astc::Tables>);

/// [`decode()`] with its pixel loop, which a test may replace.
fn decode_with(
    format: Format,
    data: &[u8],
    width: i32,
    height: i32,
    meter: &Arc<Meter>,
    fill: Fill,
) -> Result<Image, DecodeError> {
    let plan = plan(format, data.len(), width, height)?;
    let (Ok(out_len), Ok(w), Ok(h)) = (
        usize::try_from(plan.output_bytes),
        usize::try_from(plan.width),
        usize::try_from(plan.height),
    ) else {
        // Only on a target whose `usize` cannot hold 1 GiB and more.
        return Err(DecodeError::TooLarge {
            pixels: u64::from(plan.width) * u64::from(plan.height),
            max: MAX_PIXELS,
        });
    };
    // The empty image of a side of 0 charges and allocates nothing.
    let (pixels, charge) = MeteredVec::<u8>::with_capacity(meter, out_len)?.into_parts();
    let mut image = Image {
        mode: plan.mode,
        width: plan.width,
        height: plan.height,
        pixels,
        charge,
    };
    // Within the capacity just reserved: no allocation.
    image.pixels.resize(out_len, 0);
    if out_len > 0 {
        // An ASTC format's tables, charged after the output (`Plan::scratch`)
        // and dropped before the decode returns.
        let mut tables = match astc_footprint(format) {
            Some(fp) => Some(astc::Tables::new(fp, meter)?),
            None => None,
        };
        let out = &mut image.pixels;
        // Unwind-safe: on a panic `image` and the tables are dropped, never
        // read, so their half-written bytes are seen by no one; `data` is
        // only read.
        crate::contain(AssertUnwindSafe(|| {
            fill(format, data, w, h, out, tables.as_mut());
            Ok(())
        }))?;
    }
    Ok(image)
}

/// The format's decoder.
fn fill(
    format: Format,
    data: &[u8],
    w: usize,
    h: usize,
    out: &mut [u8],
    tables: Option<&mut astc::Tables>,
) {
    if let Some(tables) = tables {
        // Every ASTC format, and only they, has tables. UnityPy flips the
        // padded image after astcenc's decode and its crop.
        astc::decode_image(tables, data, (w, h), true, astc::ERROR, out);
        return;
    }
    match format {
        Format::Alpha8 => raw::decode(raw::Kind::A, data, w, h, out),
        Format::Argb4444 => raw::decode(raw::Kind::Rgba4BArgb, data, w, h, out),
        Format::Rgb24 => raw::decode(raw::Kind::Rgb, data, w, h, out),
        Format::Rgba32 => raw::decode(raw::Kind::Rgba, data, w, h, out),
        Format::Argb32 => raw::decode(raw::Kind::Argb, data, w, h, out),
        Format::Rgb565 => raw::decode(raw::Kind::Bgr16, data, w, h, out),
        Format::Bgr24 => raw::decode(raw::Kind::Bgr, data, w, h, out),
        Format::Rgba4444 => raw::decode(raw::Kind::Rgba4BRgba, data, w, h, out),
        Format::Bgra32 => raw::decode(raw::Kind::Bgra, data, w, h, out),
        Format::R8 => raw::decode(raw::Kind::R, data, w, h, out),
        Format::Dxt1 => bcn::decode(bcn::Kind::Bc1, data, w, h, out),
        Format::Dxt3 => bcn::decode(bcn::Kind::Bc2, data, w, h, out),
        Format::Dxt5 => bcn::decode(bcn::Kind::Bc3, data, w, h, out),
        Format::Bc4 => bcn::decode(bcn::Kind::Bc4, data, w, h, out),
        Format::Bc5 => bcn::decode(bcn::Kind::Bc5, data, w, h, out),
        Format::Bc7 => bcn::decode(bcn::Kind::Bc7, data, w, h, out),
        Format::EtcRgb4 => etc::decode(etc::Kind::Etc1, data, w, h, out),
        Format::EacR => etc::decode(etc::Kind::EacR, data, w, h, out),
        Format::EacRSigned => etc::decode(etc::Kind::EacRSigned, data, w, h, out),
        Format::EacRg => etc::decode(etc::Kind::EacRg, data, w, h, out),
        Format::EacRgSigned => etc::decode(etc::Kind::EacRgSigned, data, w, h, out),
        Format::Etc2Rgb => etc::decode(etc::Kind::Etc2, data, w, h, out),
        Format::Etc2Rgba1 => etc::decode(etc::Kind::Etc2A1, data, w, h, out),
        Format::Etc2Rgba8 => etc::decode(etc::Kind::Etc2A8, data, w, h, out),
        // Decoded above, with their tables.
        Format::AstcRgb4x4
        | Format::AstcRgb5x5
        | Format::AstcRgb6x6
        | Format::AstcRgb8x8
        | Format::AstcRgb10x10
        | Format::AstcRgb12x12
        | Format::AstcRgba4x4
        | Format::AstcRgba5x5
        | Format::AstcRgba6x6
        | Format::AstcRgba8x8
        | Format::AstcRgba10x10
        | Format::AstcRgba12x12 => {}
    }
}

/// The length Pillow 9.5.0's `BCn` decoder sees for `len` bytes of data:
/// `ImagingBcnDecode` passes its `Py_ssize_t` length to `decode_bcn`'s `int`
/// parameter, a conversion GCC and Clang both make modulo 2^32, then as a
/// signed value (C leaves an out-of-range conversion to the
/// implementation), so the decode loop (`while (bytes >= SZ)`) runs on the
/// length's low 32 bits, none when bit 31 is set. Below 2 GiB it is the
/// length itself.
#[must_use]
pub fn bcn_length(len: usize) -> u64 {
    let low = u64::try_from(len).unwrap_or(u64::MAX) & 0xFFFF_FFFF;
    // As a signed 32-bit value: negative when bit 31 is set, and then the
    // loop decodes no block.
    if low >= 1 << 31 { 0 } else { low }
}

#[cfg(test)]
mod tests;
