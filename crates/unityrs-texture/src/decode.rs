// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/Texture2DConverter.py's parse_image_data for the five formats (the checks, the block padding, the crop and the flip), and how Pillow 9.5.0's decoders, as it calls them, treat short and long data. See THIRD_PARTY_LICENSES.
//! [`decode()`]: a texture's mip 0 to pixels, as `parse_image_data` gives them
//! for the five formats (`export/Texture2DConverter.py:254-320`), and
//! [`plan()`]: what a decode will need, before it runs.
//!
//! **What UnityPy does**, in order, and so what [`decode()`] does:
//! 1. A side of 0 gives an empty `RGBA` image, 0x0, before the data, the
//!    format or the other side is looked at (`:265-266`).
//! 2. Empty data raises `ValueError("Texture2D has no image data")`
//!    (`:268-269`): [`DecodeError::NoImageData`].
//! 3. A block format's size is padded to whole 4x4 blocks (`:291`,
//!    `get_compressed_image_size`), Pillow decodes the padded image
//!    (`Image.frombytes` with `CONV_TABLE`'s arguments, `:331-344`, `:491-516`),
//!    it is cropped back to size (`:314-315`) and flipped top to bottom
//!    (`:317-318`). The raw formats are not padded.
//! 4. A negative side raises `ValueError` in Pillow, whichever check meets
//!    it first ("Width and height must be >= 0", or, where padding made a
//!    side 0, a crop or tile error): [`DecodeError::NegativeSize`].
//! 5. Pillow decodes the data from its start and stops when the image is
//!    full: bytes past it (later mips, anything else) are ignored, and data
//!    that runs out first raises `ValueError` "not enough image data"
//!    (`Image.frombytes`; `RawDecode.c` reads whole rows, `BcnDecode.c`
//!    whole blocks): [`DecodeError::NotEnoughData`]. So a texture with mips
//!    decodes as its mip 0. The `BCn` decoder takes the data's length as a C
//!    `int` (`decode_bcn`'s `bytes`), so data of 2 GiB or more is read as
//!    that length's low 32 bits, signed: data whose low 32 bits are below
//!    what the image needs, or negative, raises too ([`bcn_length`]).
//!
//! The decoders write straight into the output, cropped and flipped as they
//! go, with no padded image between: each block is decoded into a 16-pixel
//! array on the stack, and only the pixels inside the texture are copied,
//! each to its flipped row. That is the padded decode, then the crop and
//! the flip, pixel for pixel.
//!
//! **Bounds, where UnityPy has none.** A texture of more than
//! [`MAX_PIXELS`] pixels, counted at its padded size, is refused before
//! anything is allocated ([`DecodeError::TooLarge`]): 16384 x 16384, the
//! largest size Unity's texture importer offers. [`decode()`] makes the
//! data checks above first, so a texture whose data is short is
//! [`DecodeError::NotEnoughData`] whatever its size, as in UnityPy;
//! [`plan()`], which has no data, refuses on the size alone. So the output
//! is at most 1 GiB and the work linear in it. Sizes are computed in `u64`,
//! from the `i32` sides, so none wraps.
//!
//! **Memory** (`docs/plans/extract.md` E59). The output is the only buffer a
//! decode allocates in proportion to the texture (no scratch: a block is 64
//! bytes on the stack). It is charged to the caller's [`Meter`] at
//! [`heap_cost`] of its length before it is allocated (through
//! [`MeteredVec`], whose refusal allocates nothing and whose allocation
//! failure is a refusal too), and the [`Charge`] lives in the [`Image`], so
//! it returns when the image drops, a panic's unwinding included. The
//! engine calls [`plan()`] first: [`Plan::output_bytes`] is the pixel bytes
//! the decode writes (export work, `docs/plans/batch.md` 2.6 step 10) and
//! [`Plan::charge`] the memory it charges.

use std::fmt;
use std::sync::Arc;

use unityrs_core::limits::{Charge, ChargeError, Meter, MeteredVec, heap_cost};

use crate::bcn;
use crate::format::{Format, Mode};

/// The most pixels a texture may have, at its padded size: 16384 x 16384.
pub const MAX_PIXELS: u64 = 1 << 28;

/// Why a texture did not decode. Every variant but [`Self::TooLarge`],
/// [`Self::Charge`] and [`Self::Panicked`] is a case where UnityPy 1.25.2
/// raises too.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
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
    /// The data ends before mip 0 does: Pillow's `ValueError` "not enough
    /// image data".
    #[error("not enough image data: {needed} bytes needed, {got} given")]
    NotEnoughData {
        /// What mip 0 holds, at its padded size.
        needed: u64,
        /// The data's length.
        got: u64,
    },
    /// The meter or its budget refused the output buffer, or the allocator
    /// did; nothing was allocated.
    #[error(transparent)]
    Charge(#[from] ChargeError),
    /// The decoder panicked ([`crate::contain()`]): the panic's payload, or
    /// [`crate::NON_STRING_PAYLOAD`].
    #[error("the decoder panicked: {payload}")]
    Panicked {
        /// The payload.
        payload: String,
    },
}

const _: () = crate::send_sync::<DecodeError>();

/// What decoding a texture of a format and a size will need and give,
/// worked out before any data is read ([`plan()`]).
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
}

/// What decoding `format` at `width` x `height` needs and gives: the steps
/// of the module docs that need no data. A side of 0 plans the empty `RGBA`
/// image; a negative side is [`DecodeError::NegativeSize`]; a texture past
/// [`MAX_PIXELS`] is [`DecodeError::TooLarge`].
pub fn plan(format: Format, width: i32, height: i32) -> Result<Plan, DecodeError> {
    let (plan, pixels) = sizes(format, width, height)?;
    bounded(pixels)?;
    Ok(plan)
}

/// [`plan()`] with no bound, and the pixels of the padded image (0 for a
/// side of 0).
fn sizes(format: Format, width: i32, height: i32) -> Result<(Plan, u64), DecodeError> {
    if width == 0 || height == 0 {
        let empty = Plan {
            mode: Mode::Rgba,
            width: 0,
            height: 0,
            input_bytes: 0,
            output_bytes: 0,
        };
        return Ok((empty, 0));
    }
    let (Ok(w), Ok(h)) = (u32::try_from(width), u32::try_from(height)) else {
        return Err(DecodeError::NegativeSize { width, height });
    };
    let (w64, h64) = (u64::from(w), u64::from(h));
    let (padded_w, padded_h) = match format.block_bytes() {
        Some(_) => (w64.div_ceil(4) * 4, h64.div_ceil(4) * 4),
        None => (w64, h64),
    };
    // Each side is at most 2^31, so the product fits a u64.
    let pixels = padded_w.saturating_mul(padded_h);
    let mode = format.mode();
    let input_bytes = match format.block_bytes() {
        Some(block) => (pixels / 16).saturating_mul(block),
        None => pixels.saturating_mul(mode.bytes_per_pixel()),
    };
    let plan = Plan {
        mode,
        width: w,
        height: h,
        input_bytes,
        output_bytes: w64
            .saturating_mul(h64)
            .saturating_mul(mode.bytes_per_pixel()),
    };
    Ok((plan, pixels))
}

/// [`DecodeError::TooLarge`] past [`MAX_PIXELS`].
fn bounded(pixels: u64) -> Result<(), DecodeError> {
    if pixels > MAX_PIXELS {
        return Err(DecodeError::TooLarge {
            pixels,
            max: MAX_PIXELS,
        });
    }
    Ok(())
}

/// What [`decode()`] checks before it allocates, in the module docs' order,
/// for data of `len` bytes: the image to decode, or why not.
fn admit(format: Format, len: usize, width: i32, height: i32) -> Result<Plan, DecodeError> {
    if width == 0 || height == 0 {
        return sizes(format, width, height).map(|(plan, _)| plan);
    }
    if len == 0 {
        return Err(DecodeError::NoImageData);
    }
    let (plan, pixels) = sizes(format, width, height)?;
    let got = u64::try_from(len).unwrap_or(u64::MAX);
    let seen = match format.block_bytes() {
        Some(_) => bcn_length(len),
        None => got,
    };
    if seen < plan.input_bytes {
        return Err(DecodeError::NotEnoughData {
            needed: plan.input_bytes,
            got,
        });
    }
    bounded(pixels)?;
    Ok(plan)
}

/// A decoded image: rows top to bottom, each pixel its mode's bytes, red
/// first. It holds the [`Charge`] for its pixels, returned when it drops.
pub struct Image {
    mode: Mode,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
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
    /// returns.
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
    /// allocation does.
    pub fn into_parts(self) -> (Vec<u8>, Charge) {
        (self.pixels, self.charge)
    }
}

/// Decodes a texture's mip 0, `data` being its image data from the start
/// (`image_data`, or what its `m_StreamData` names: `Texture2D.get_image_data`),
/// mips and all, as UnityPy 1.25.2 with Pillow 9.5.0 does (module docs),
/// charging the output to `meter` before it is allocated. It runs under
/// [`crate::contain()`], so a panic is [`DecodeError::Panicked`], with the
/// output's charge returned.
pub fn decode(
    format: Format,
    data: &[u8],
    width: i32,
    height: i32,
    meter: &Arc<Meter>,
) -> Result<Image, DecodeError> {
    crate::contain(|| decode_uncontained(format, data, width, height, meter))
}

fn decode_uncontained(
    format: Format,
    data: &[u8],
    width: i32,
    height: i32,
    meter: &Arc<Meter>,
) -> Result<Image, DecodeError> {
    let plan = admit(format, data.len(), width, height)?;
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
    let (mut pixels, charge) = MeteredVec::<u8>::with_capacity(meter, out_len)?.into_parts();
    // Within the capacity just reserved: no allocation.
    pixels.resize(out_len, 0);
    match format {
        _ if out_len == 0 => {}
        Format::Rgb24 | Format::Rgba32 => raw(data, w, h, &mut pixels, plan.mode),
        Format::Dxt1 => bcn::decode(bcn::Kind::Bc1, data, w, h, &mut pixels),
        Format::Dxt5 => bcn::decode(bcn::Kind::Bc3, data, w, h, &mut pixels),
        Format::Bc7 => bcn::decode(bcn::Kind::Bc7, data, w, h, &mut pixels),
    }
    Ok(Image {
        mode: plan.mode,
        width: plan.width,
        height: plan.height,
        pixels,
        charge,
    })
}

/// The length Pillow 9.5.0's `BCn` decoder sees for `len` bytes of data:
/// `ImagingBcnDecode` passes its `Py_ssize_t` length to `decode_bcn`'s `int`
/// parameter, which keeps its low 32 bits as a signed value (gcc's
/// conversion on x86-64), so the decode loop (`while (bytes >= SZ)`) runs
/// on that. Below 2 GiB it is the length itself.
#[must_use]
pub fn bcn_length(len: usize) -> u64 {
    let low = u64::try_from(len).unwrap_or(u64::MAX) & 0xFFFF_FFFF;
    // As a signed 32-bit value: negative when bit 31 is set, and then the
    // loop decodes no block.
    if low >= 1 << 31 { 0 } else { low }
}

/// RGB24 or RGBA32: Pillow's raw decoder copies whole rows in order, each
/// pixel as stored (its `RGB` unpacker adds an alpha of 255 that `tobytes`
/// drops again), and UnityPy flips them: output row `r` is stored row
/// `h - 1 - r`. `data` holds at least `h` rows.
fn raw(data: &[u8], w: usize, h: usize, out: &mut [u8], mode: Mode) {
    let row = w.saturating_mul(if mode == Mode::Rgb { 3 } else { 4 });
    if row == 0 {
        return;
    }
    for (r, dst) in out.chunks_exact_mut(row).enumerate() {
        let from = h.saturating_sub(1).saturating_sub(r).saturating_mul(row);
        if let Some(src) = data.get(from..from.saturating_add(row)) {
            dst.copy_from_slice(src);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DecodeError, Format, MAX_PIXELS, admit, plan};

    /// The data checks come before the bound, as UnityPy's come before any
    /// bound it could have: a texture past [`MAX_PIXELS`] with short data is
    /// short; with enough, too large.
    #[test]
    fn short_data_is_short_whatever_the_size() {
        let (w, h) = (16388, 16384);
        assert!(matches!(
            plan(Format::Dxt1, w, h),
            Err(DecodeError::TooLarge { .. })
        ));
        let needed = 4097 * 4096 * 8;
        assert_eq!(
            admit(Format::Dxt1, needed - 1, w, h),
            Err(DecodeError::NotEnoughData {
                needed: 4097 * 4096 * 8,
                got: 4097 * 4096 * 8 - 1,
            })
        );
        assert_eq!(
            admit(Format::Dxt1, needed, w, h),
            Err(DecodeError::TooLarge {
                pixels: 16388 * 16384,
                max: MAX_PIXELS
            })
        );
        assert!(matches!(
            admit(Format::Rgba32, 3, i32::MAX, i32::MAX),
            Err(DecodeError::NotEnoughData { got: 3, .. })
        ));
    }

    /// Pillow's `BCn` decoder sees the data's length as a C `int`.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn a_bcn_length_past_2_gib_is_read_as_its_low_32_bits() {
        // A 4x4 BC7 texture needs 16 bytes.
        assert!(admit(Format::Bc7, (1 << 32) + 16, 4, 4).is_ok());
        assert_eq!(
            admit(Format::Bc7, (1 << 32) + 15, 4, 4),
            Err(DecodeError::NotEnoughData {
                needed: 16,
                got: (1 << 32) + 15
            })
        );
        assert!(matches!(
            admit(Format::Bc7, (1 << 31) + 16, 4, 4),
            Err(DecodeError::NotEnoughData { .. })
        ));
        // The raw formats read the whole length.
        assert!(admit(Format::Rgba32, (1 << 31) + 16, 2, 2).is_ok());
    }
}
