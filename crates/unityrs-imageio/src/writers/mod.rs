// Ported from Pillow 9.5.0 (MIT-CMU): src/PIL/WebPImagePlugin.py's _save and src/_webp.c's WebPEncode_wrapper (default WebP dispatch). See THIRD_PARTY_LICENSES.
//! Default Pillow 9.5.0 image writers (PARITY section 15.8, W2/W1c).
//! Images have no metadata beyond their optional RGB palette. Output and
//! working buffers are charged before fallible allocation.
mod dispatch;
mod icons;
mod raster;
mod resample;
mod sink;
mod structured;
use crate::{Buffer, Charger, EncodeError, Image};
pub use dispatch::{Format, Outcome, save_by_extension, select_extension};
pub(crate) use sink::Sink;

/// Pillow's exception class, preserved independently of its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exception {
    /// Python `OSError`.
    OSError,
    /// Python `ValueError`.
    ValueError,
    /// Python `SystemError`.
    SystemError,
    /// Python `MemoryError`.
    MemoryError,
    /// Python struct.error.
    StructError,
    /// Python `UnicodeEncodeError`.
    UnicodeEncodeError,
    /// Python `KeyError`.
    KeyError,
}
impl Exception {
    /// Python's spelling of the exception class.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::OSError => "OSError",
            Self::ValueError => "ValueError",
            Self::SystemError => "SystemError",
            Self::MemoryError => "MemoryError",
            Self::StructError => "error",
            Self::UnicodeEncodeError => "UnicodeEncodeError",
            Self::KeyError => "KeyError",
        }
    }
}
/// A writer's refusal, before output is allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{class:?}: {message}")]
pub struct Refusal {
    /// Original exception class.
    pub class: Exception,
    /// Original message (Unicode errors use a stable description).
    pub message: &'static str,
}
impl Refusal {
    /// Builds a typed Pillow refusal.
    #[must_use]
    pub const fn new(class: Exception, message: &'static str) -> Self {
        Self { class, message }
    }
}
/// Why saving failed, including temporary P4 refusals.
#[derive(Debug, thiserror::Error)]
pub enum SaveError<R> {
    /// Pillow refuses the image.
    #[error(transparent)]
    Pillow(#[from] Refusal),
    /// A named writer deferred by decision 7.
    #[error("unsupported image writer {format:?}")]
    Unsupported {
        /// The writer.
        format: Format,
    },
    /// Memory charging, allocation, PNG, or address-space failure.
    #[error(transparent)]
    Encode(#[from] EncodeError<R>),
    /// WebP resource or encoder failure. Never triggers a PNG fallback.
    #[error(transparent)]
    Webp(#[from] crate::WebpError<R>),
}
/// Owned file bytes and the live charge for their allocation.
pub struct Saved<C> {
    bytes: Vec<u8>,
    charge: C,
}
impl<C> Saved<C> {
    pub(crate) fn new(bytes: Vec<u8>, charge: C) -> Self {
        Self { bytes, charge }
    }
    /// File bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    /// Output allocation charge.
    #[must_use]
    pub fn charge(&self) -> &C {
        &self.charge
    }
    /// Transfers bytes and charge to the caller, which keeps the charge alive.
    #[must_use]
    pub fn into_parts(self) -> (Vec<u8>, C) {
        (self.bytes, self.charge)
    }
}
impl<C> std::fmt::Debug for Saved<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Saved")
            .field("len", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// Saves a format explicitly, like Image.save(fp, format), with a filename
/// for IM and SGI. Filename parsing uses POSIX separators, as the oracle does.
pub fn save<C: Charger>(
    image: &Image<'_>,
    format: Format,
    filename: &str,
    charger: &mut C,
) -> Result<Saved<C::Charge>, SaveError<C::Refusal>> {
    if format == Format::Png {
        let (bytes, charge) = crate::encode(image, charger)?.into_parts();
        return Ok(Saved::new(bytes, charge));
    }
    if format == Format::Webp {
        let (bytes, charge) = crate::encode_webp(image, crate::WebpOptions::default(), charger)
            .map_err(|error| match error {
                // _webp.c's WebPEncode_wrapper surfaces invalid picture
                // dimensions as ValueError, VP8_ENC_ERROR_BAD_DIMENSION (5).
                crate::WebpError::Dimensions { .. } => {
                    SaveError::Pillow(Refusal::new(Exception::ValueError, "encoding error 5"))
                }
                error => SaveError::Webp(error),
            })?
            .into_parts();
        return Ok(Saved::new(bytes, charge));
    }
    if matches!(format, Format::Ico | Format::Icns) {
        return icons::encode(image, format, charger);
    }
    if matches!(
        format,
        Format::Gif | Format::Jpeg | Format::Mpo | Format::Pdf | Format::Jpeg2000
    ) {
        return Err(SaveError::Unsupported { format });
    }
    let write = |out: &mut Sink| match format {
        Format::Bmp => raster::bmp(image, out, false),
        Format::Dib => raster::bmp(image, out, true),
        Format::Tga => raster::tga(image, out),
        Format::Ppm => raster::ppm(image, out),
        Format::Pcx => raster::pcx(image, out),
        Format::Dds => raster::dds(image, out),
        Format::Sgi => structured::sgi(image, out, filename),
        Format::Im => structured::im(image, out, filename),
        Format::Tiff => structured::tiff(image, out),
        Format::Eps => structured::eps(image, out),
        _ => Err(Refusal::new(
            Exception::OSError,
            "writer cannot save this image",
        )),
    };
    let mut counter = Sink::count();
    write(&mut counter)?;
    let len = counter.finish()?;
    let charge =
        charger
            .charge(Buffer::Output, &[len])
            .map_err(|refusal| EncodeError::Refused {
                buffer: Buffer::Output,
                refusal,
            })?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(len)
        .map_err(|_| EncodeError::Alloc {
            buffer: Buffer::Output,
            request: len,
        })?;
    bytes.resize(len, 0);
    let mut out = Sink::write(&mut bytes);
    write(&mut out)?;
    out.finish()?;
    Ok(Saved::new(bytes, charge))
}
