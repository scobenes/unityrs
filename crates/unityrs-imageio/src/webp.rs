// Ported from Pillow 9.5.0 (MIT-CMU): src/PIL/WebPImagePlugin.py's _save and src/_webp.c's WebPEncode_wrapper. See THIRD_PARTY_LICENSES.
//! Still images without metadata. Pillow's _save converts L to RGB and LA
//! to RGBA; RGB/RGBA pass through. Defaults: quality 80, method 4, exact
//! false, lossless off (WebPImagePlugin.py _save, _webp.c:586-624).
//! Alpha uses libwebp's default `alpha_quality=100` and `alpha_compression=1`;
//! even lossy WebP needs the lossless encoder for that alpha plane.
use crate::webp_ffi;
use crate::{Buffer, Charger, Image, Mode};

/// Pillow's still-image WebP options. Metadata and animation belong to the
/// later writer/API work; lossless is deliberately off on this path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WebpOptions {
    /// A finite value from 0 to 100, inclusive.
    pub quality: f32,
    /// Encoder effort, 0 through 6.
    pub method: u8,
    /// Preserve invisible RGB as Pillow's `exact` option requests.
    pub exact: bool,
}
impl Default for WebpOptions {
    fn default() -> Self {
        Self {
            quality: 80.0,
            method: 4,
            exact: false,
        }
    }
}

/// Typed refusals, before any charge or allocation where possible.
#[derive(Debug, thiserror::Error)]
pub enum WebpError<R> {
    /// Palette (`P`) images are refused until the encoder takes Pillow's
    /// palette conversion; the extractor never produces them.
    #[error("WebP does not take palette images yet")]
    Palette,
    /// WebP sides must be in 1..=16383.
    #[error("WebP refuses dimensions {width}x{height}")]
    Dimensions { width: u32, height: u32 },
    /// Quality is not finite or outside 0..=100, or method exceeds 6.
    #[error("invalid WebP quality or method")]
    Options,
    /// The caller refused the charge.
    #[error("the WebP {buffer:?} charge was refused")]
    Refused { buffer: Buffer, refusal: R },
    /// A fallible allocation failed, including inside libwebp.
    #[error("the WebP allocator refused memory")]
    Memory,
    /// The enforced workspace allowance was exhausted.
    #[error("WebP exceeded its charged workspace")]
    Allowance,
    /// An encoder error (`VP8EncodingError`, or a negative wrapper failure).
    #[error("WebP encoder error {0}")]
    Encoder(i32),
}

/// Memory offered to the charger before the encode. The workspace bound
/// includes block headers and conservative heap overhead, and is enforced
/// inside the C allocator. See `tests/webp_charges.rs` and `vendor/WEBP.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WebpPlan {
    /// Constant plus per-pixel allowance, including L/LA mode conversion.
    pub workspace: usize,
    /// One bounded output allocation; no growth callback can exceed it.
    pub output: usize,
}

/// Plan default settings without allocating or needing pixel data.
/// Use [`webp_plan_with_options`] when selecting a different method.
pub fn webp_plan(width: u32, height: u32) -> Result<WebpPlan, WebpError<std::convert::Infallible>> {
    webp_plan_with_options(width, height, WebpOptions::default())
}

/// Plan a configured encoder; methods 5/6 need a larger alpha workspace.
pub fn webp_plan_with_options(
    width: u32,
    height: u32,
    options: WebpOptions,
) -> Result<WebpPlan, WebpError<std::convert::Infallible>> {
    if !options.quality.is_finite()
        || !(0.0..=100.0).contains(&options.quality)
        || options.method > 6
    {
        return Err(WebpError::Options);
    }
    if width == 0 || height == 0 || width > 16383 || height > 16383 {
        return Err(WebpError::Dimensions { width, height });
    }
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or(WebpError::Memory)?;
    let (constant, slope) = if options.method > 4 {
        (32 << 20, 256)
    } else {
        (2 << 20, 256)
    };
    let workspace = pixels
        .checked_mul(slope)
        .and_then(|n| n.checked_add(constant))
        .ok_or(WebpError::Memory)?;
    let output = pixels
        .checked_mul(8)
        .and_then(|n| n.checked_add(1 << 20))
        .ok_or(WebpError::Memory)?;
    Ok(WebpPlan { workspace, output })
}

/// Encoded WebP and its output charge. Bytes are freed before the charge.
#[derive(Debug)]
pub struct Webp<C> {
    bytes: Vec<u8>,
    charge: C,
}
impl<C> Webp<C> {
    /// File bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    /// Transfer bytes and their charge, which must outlive the bytes.
    pub fn into_parts(self) -> (Vec<u8>, C) {
        (self.bytes, self.charge)
    }
}

/// Encode as Pillow 9.5.0 saves an extractor image with no metadata.
pub fn encode_webp<C: Charger>(
    image: &Image<'_>,
    options: WebpOptions,
    charger: &mut C,
) -> Result<Webp<C::Charge>, WebpError<C::Refusal>> {
    encode_webp_impl(image, options, charger, false)
}

/// Test-only Pillow lossless configuration using the same allocator and charges.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn test_encode_webp_lossless<C: Charger>(
    image: &Image<'_>,
    options: WebpOptions,
    charger: &mut C,
) -> Result<Webp<C::Charge>, WebpError<C::Refusal>> {
    encode_webp_impl(image, options, charger, true)
}

fn encode_webp_impl<C: Charger>(
    image: &Image<'_>,
    options: WebpOptions,
    charger: &mut C,
    lossless: bool,
) -> Result<Webp<C::Charge>, WebpError<C::Refusal>> {
    if image.mode() == Mode::P {
        return Err(WebpError::Palette);
    }
    if !options.quality.is_finite()
        || !(0.0..=100.0).contains(&options.quality)
        || options.method > 6
    {
        return Err(WebpError::Options);
    }
    // Map only the dimension/size errors that planning can return.
    // The lossless test path reserves the larger, checked workspace plan.
    let plan_options = if lossless {
        WebpOptions {
            method: 6,
            ..options
        }
    } else {
        options
    };
    let plan = webp_plan_with_options(image.width(), image.height(), plan_options).map_err(
        |e| match e {
            WebpError::Dimensions { width, height } => WebpError::Dimensions { width, height },
            _ => WebpError::Memory,
        },
    )?;
    let _workspace = charger
        .charge(Buffer::WebpState, &[plan.workspace])
        .map_err(|refusal| WebpError::Refused {
            buffer: Buffer::WebpState,
            refusal,
        })?;
    let output_charge = charger
        .charge(Buffer::Output, &[plan.output])
        .map_err(|refusal| WebpError::Refused {
            buffer: Buffer::Output,
            refusal,
        })?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(plan.output)
        .map_err(|_| WebpError::Memory)?;
    output.resize(plan.output, 0);
    let mut converted = Vec::new();
    let (pixels, alpha) = match image.mode() {
        Mode::Rgb => (image.pixels(), false),
        Mode::Rgba => (image.pixels(), true),
        Mode::L | Mode::La => {
            let n = image.pixels().len() / image.mode().channels()
                * if image.mode() == Mode::L { 3 } else { 4 };
            converted
                .try_reserve_exact(n)
                .map_err(|_| WebpError::Memory)?;
            for pixel in image.pixels().chunks_exact(image.mode().channels()) {
                if let Some(&l) = pixel.first() {
                    converted.extend_from_slice(&[l, l, l]);
                    if let Some(&a) = pixel.get(1) {
                        converted.push(a);
                    }
                }
            }
            (converted.as_slice(), image.mode() == Mode::La)
        }
        Mode::P => return Err(WebpError::Palette),
    };
    let written = webp_ffi::encode(
        pixels,
        image.width(),
        image.height(),
        alpha,
        options,
        lossless,
        &mut output,
        plan.workspace.saturating_sub(converted.capacity()),
    )?;
    output.truncate(written);
    Ok(Webp {
        bytes: output,
        charge: output_charge,
    })
}
