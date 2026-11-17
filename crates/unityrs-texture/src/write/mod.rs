// Ported from UnityPy 1.25.2 (MIT): export/Texture2DConverter.py and classes/legacy_patch/Texture2D.py. See LICENSE-UNITYPY.
//! Raw texture writes, format promotion and mip chains. The converter is
//! `export/Texture2DConverter.py:118-234`; field edits and mip termination are
//! `classes/legacy_patch/Texture2D.py:14-60`. The `encode` feature supplies
//! etcpak and astcenc; without it compressed writes return a typed refusal.
use std::sync::Arc;
use unityrs_core::Bytes;
use unityrs_core::budget::DecodeBuf;
use unityrs_core::error::LimitKind;
use unityrs_core::limits::{Charge, ChargeError, Meter, heap_cost};
use unityrs_imageio::writers::resample::{self, Filter};
use unityrs_imageio::{Buffer, Charger, Image};

#[cfg(feature = "encode")]
mod encoders;
#[cfg(feature = "encode")]
mod ffi;
mod lab;
mod object;
mod source;
mod ycbcr;
pub use object::set_image;
pub use source::Source;

/// Errors leave the installed object data unchanged.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    /// This build does not include the optional encoders.
    #[error("encoder unavailable for TextureFormat {0}")]
    EncoderUnavailable(i32),
    /// A vendored encoder refused a checked request; no bytes are installed.
    #[error("texture encoder failed with code {0}")]
    Encoder(i32),
    /// A named Tier U refusal.
    #[error("unsupported set_image: {0}")]
    Unsupported(&'static str),
    /// Invalid image storage or object fields.
    #[error("invalid set_image input: {0}")]
    Invalid(&'static str),
    /// Bounds, allocation or work refusal.
    #[error(transparent)]
    Charge(#[from] ChargeError),
    /// Platform padding/layout refusal.
    #[error(transparent)]
    Decode(#[from] crate::DecodeError),
    /// Pillow resize refusal.
    #[error(transparent)]
    Resize(#[from] unityrs_imageio::writers::SaveError<ChargeError>),
    /// Object read failure.
    #[error(transparent)]
    Parse(#[from] unityrs_core::walk::ParseError),
    /// Object re-save failure.
    #[error(transparent)]
    Save(#[from] unityrs_core::files::SaveError),
}
const _: () = crate::send_sync::<WriteError>();

/// Converter options. Platform blobs are preserved verbatim by object writes.
#[derive(Clone, Copy, Debug)]
pub struct Options<'a> {
    /// Target `TextureFormat`, before UnityPy's promotion.
    pub format: i32,
    /// `BuildTarget`; zero for a detached texture.
    pub platform: i32,
    /// Existing platform blob, interpreted as little-endian GOB metadata.
    pub platform_blob: Option<&'a [u8]>,
    /// Flip before padding and packing (UnityPy's default).
    pub flip: bool,
    /// Requested mip count; zero and negative counts are retained by UnityPy.
    pub mipmap_count: i32,
}
const _: () = crate::send_sync::<Options<'static>>();
impl Options<'_> {
    /// One flipped level on platform zero.
    #[must_use]
    pub const fn new(format: i32) -> Self {
        Self {
            format,
            platform: 0,
            platform_blob: None,
            flip: true,
            mipmap_count: 1,
        }
    }
}

/// Inline image bytes and the fields computed by the writer.
#[derive(Debug)]
pub struct Encoded {
    /// Charged bytes, top level followed by smaller levels.
    pub data: Bytes,
    /// The written (possibly promoted) `TextureFormat`.
    pub format: i32,
    /// Actual mip count, including UnityPy's retained nonpositive counts.
    pub mipmap_count: i32,
}
const _: () = crate::send_sync::<Encoded>();

struct Budget<'a>(&'a Arc<Meter>);
impl Charger for Budget<'_> {
    type Charge = Charge;
    type Refusal = ChargeError;
    fn charge(&mut self, _: Buffer, requests: &[usize]) -> Result<Charge, ChargeError> {
        self.0.charge(
            requests
                .iter()
                .fold(0u64, |n, &r| n.saturating_add(heap_cost(r as u64))),
        )
    }
}

// R/RGB are byte layouts, not the requested texture's stored channel widths.
fn mapped(format: i32, rgb: bool) -> Result<(i32, usize, bool), WriteError> {
    if crate::texture_format_name(format).is_none() {
        return Err(WriteError::Invalid("unknown TextureFormat"));
    }
    let (format, channels, encoder) = match format {
        10 | 28 => (10, 4, true),
        12 | 29 => (12, 4, true),
        25..=27 | 48..=59 => (format, 4, true),
        66..=71 => (format - if rgb { 18 } else { 12 }, 4, true),
        34 | 64 | 60 => (if format == 60 { 60 } else { 34 }, 4, true),
        45 => (45, 4, true),
        46 | 47 | 65 => (47, 4, true),
        1 => (1, 1, false),
        63 | 9 | 15 | 18 | 41 | 42 => (63, 1, false),
        7 | 3 | 8 | 22 | 30 | 32 | 35 => (3, 3, false),
        _ => (4, 4, false),
    };
    Ok((format, channels, encoder))
}

/// `image_to_texture2d`, including repeated BICUBIC mip resizes when requested.
/// Zero sides write empty data for every known target (PARITY 19.1). Tier U
/// gates precede the encoder availability check; no encoder is needed to
/// represent an empty linear texture.
pub fn image_to_texture2d(
    source: &Source<'_>,
    options: Options<'_>,
    meter: &Arc<Meter>,
) -> Result<Encoded, WriteError> {
    let (format, mut channels, encoder) = mapped(options.format, source.mode() == "RGB")?;
    let switch = crate::is_switch_swizzled(options.platform, options.platform_blob);
    let empty = source.width() == 0 || source.height() == 0;
    if switch && empty {
        return Err(WriteError::Unsupported("zero-size Switch texture"));
    }
    if switch && options.mipmap_count > 1 && source.width() >= 8 && source.height() >= 8 {
        return Err(WriteError::Unsupported("Switch mip chain"));
    }
    let switch_format = if format == 3 { 4 } else { format };
    if switch && crate::platform::switch_block(switch_format).is_none() {
        return Err(WriteError::Unsupported("Switch texel block"));
    }
    if !empty && options.platform == crate::XBOX360 && crate::XBOX_SWAP_FORMATS.contains(&format) {
        return Err(WriteError::Unsupported("Xbox byte-swapped write"));
    }
    if empty {
        return Ok(Encoded {
            data: Bytes::new(),
            format,
            mipmap_count: if options.mipmap_count > 1 {
                1
            } else {
                options.mipmap_count
            },
        });
    }
    source.check_convertible()?;
    if encoder && !cfg!(feature = "encode") {
        return Err(WriteError::EncoderUnavailable(format));
    }
    if switch && format == 3 {
        channels = 4;
    }
    let prepared = source.prepare(channels, format == 1, meter)?;
    let mut image = prepared.image()?;
    let mut out = DecodeBuf::new(meter, 0, i32::MAX as u64, LimitKind::BlockOutput)?;
    write_level(
        &image,
        options,
        format,
        channels,
        (prepared.alpha_band, source.auxiliary()),
        meter,
        &mut out,
    )?;
    let mut mipmap_count = options.mipmap_count;
    let mut resized = None;
    let mut width = image.width();
    let mut height = image.height();
    for i in 1..options.mipmap_count {
        width /= 2;
        height /= 2;
        if width < 4 || height < 4 {
            mipmap_count = i;
            break;
        }
        meter.count_decode((image.pixels().len() as u64).saturating_mul(8))?;
        let next = resample::resize(&image, width, height, Filter::Bicubic, &mut Budget(meter))?;
        // Drop the previous view before replacing its storage.
        resized = Some(next);
        let Some(level) = &resized else {
            return Err(WriteError::Invalid("missing mip storage"));
        };
        image = Image::new(width, height, prepared.mode, &level.bytes)
            .map_err(|_| WriteError::Invalid("mip size"))?;
        write_level(
            &image,
            Options {
                platform: 0,
                platform_blob: None,
                ..options
            },
            format,
            channels,
            (prepared.alpha_band, Some(&[])),
            meter,
            &mut out,
        )?;
    }
    // Keep the last resize charge alive through its view's final use.
    drop(resized);
    Ok(Encoded {
        data: out.into_bytes()?,
        format,
        mipmap_count,
    })
}

fn write_level(
    image: &Image<'_>,
    options: Options<'_>,
    format: i32,
    channels: usize,
    (alpha_band, auxiliary): (usize, Option<&[u8]>),
    meter: &Arc<Meter>,
    out: &mut DecodeBuf,
) -> Result<(), WriteError> {
    let layout = if crate::is_switch_swizzled(options.platform, options.platform_blob) {
        let exponent = options
            .platform_blob
            .and_then(|b| b.get(8..12))
            .and_then(|b| <[u8; 4]>::try_from(b).ok())
            .map(u32::from_le_bytes)
            .ok_or(WriteError::Invalid("Switch blob"))?;
        Some(crate::platform::switch_layout(
            if format == 3 { 4 } else { format },
            i32::try_from(image.width()).map_err(|_| WriteError::Invalid("width"))?,
            i32::try_from(image.height()).map_err(|_| WriteError::Invalid("height"))?,
            exponent,
        )?)
    } else {
        None
    };
    let (width, height) = layout.as_ref().map_or(
        (image.width(), image.height()),
        crate::platform::SwitchLayout::padded_size,
    );
    #[cfg(feature = "encode")]
    let (width, height) = if encoders::is_encoded(format) {
        encoders::padded_size(format, width, height)?
    } else {
        (width, height)
    };
    let len = u64::from(width) * u64::from(height) * channels as u64;
    meter.count_decode(len)?;
    let mut linear = DecodeBuf::new(meter, len, len, LimitKind::BlockOutput)?;
    for y in 0..height {
        let bottom = y >= image.height();
        let y = y.min(image.height() - 1);
        let y = if options.flip {
            image.height() - 1 - y
        } else {
            y
        };
        for x in 0..width {
            let at = (y as usize * image.width() as usize + x.min(image.width() - 1) as usize)
                * image.mode().channels();
            let pixel = image
                .pixels()
                .get(at..at + image.mode().channels())
                .ok_or(WriteError::Invalid("pixel span"))?;
            if channels == 1 {
                linear.extend_from_slice(
                    pixel
                        .get(if format == 1 { alpha_band } else { 0 }..)
                        .and_then(|b| b.get(..1))
                        .ok_or(WriteError::Invalid("band"))?,
                )?;
            } else if channels == 4 && image.mode() == unityrs_imageio::Mode::Rgb {
                // Pillow's RGB resampler leaves the unused fourth storage
                // byte zero. Its raw RGBA packer copies that byte verbatim.
                linear.extend_from_slice(pixel)?;
                // pad_image copies strips but constructs its corner with
                // Image.new(mode, size, getpixel(...)). RGB's three-band
                // colour initializes that new pixel's hidden byte to 255.
                let padding = if bottom && x >= image.width() {
                    255
                } else {
                    auxiliary.map_or(255, |b| b.get(at / 3).copied().unwrap_or(0))
                };
                linear.extend_from_slice(&[padding])?;
            } else {
                linear.extend_from_slice(
                    pixel
                        .get(..channels)
                        .ok_or(WriteError::Invalid("channels"))?,
                )?;
            }
        }
    }
    #[cfg(feature = "encode")]
    let compressed = if encoders::is_encoded(format) {
        Some(encoders::encode(
            linear.as_slice(),
            width,
            height,
            format,
            meter,
        )?)
    } else {
        None
    };
    #[cfg(feature = "encode")]
    let data = compressed
        .as_ref()
        .map_or(linear.as_slice(), |v| v.as_slice());
    #[cfg(not(feature = "encode"))]
    let data = linear.as_slice();
    if let Some(layout) = layout {
        let swizzled = crate::platform::swizzle(data, &layout, meter)?;
        out.extend_from_slice(swizzled.as_slice())?;
    } else {
        out.extend_from_slice(data)?;
    }
    Ok(())
}

/// Compresses standalone U8 RGBA context storage at a legal 2D ASTC footprint.
pub fn astc_context(
    rgba: &[u8],
    width: u32,
    height: u32,
    block: (u8, u8),
    meter: &Arc<Meter>,
) -> Result<unityrs_core::limits::MeteredVec<u8>, WriteError> {
    #[cfg(feature = "encode")]
    {
        encoders::astc_context(rgba, width, height, block, meter)
    }
    #[cfg(not(feature = "encode"))]
    {
        let _ = (rgba, width, height, block, meter);
        Err(WriteError::EncoderUnavailable(54))
    }
}
