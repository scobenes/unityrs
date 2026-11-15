//! Extension selection and the Tier C PNG policy, separate from file I/O.
use super::{SaveError, Saved, save};
use crate::{Charger, EncodeError, Image, ImageError};

/// A Pillow writer. P4 writers remain explicitly named refusals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Windows bitmap.
    Bmp,
    /// Headerless Windows bitmap.
    Dib,
    /// Truevision TGA.
    Tga,
    /// Netpbm.
    Ppm,
    /// `ZSoft` PCX.
    Pcx,
    /// SGI raster.
    Sgi,
    /// IFUNC IM.
    Im,
    /// Raw uncompressed TIFF.
    Tiff,
    /// `DirectDraw` surface.
    Dds,
    /// PNG-based Windows icon.
    Ico,
    /// PNG-based icon family.
    Icns,
    /// Encapsulated PostScript.
    Eps,
    /// PNG.
    Png,
    /// Deferred GIF writer.
    Gif,
    /// Deferred JPEG writer.
    Jpeg,
    /// Deferred MPO writer.
    Mpo,
    /// Deferred PDF writer.
    Pdf,
    /// Deferred JPEG2000 writer.
    Jpeg2000,
    /// Lossy WebP with Pillow's defaults.
    Webp,
}
impl Format {
    /// Recognises Pillow's format identifier, case-insensitively.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        [
            Self::Bmp,
            Self::Dib,
            Self::Tga,
            Self::Ppm,
            Self::Pcx,
            Self::Sgi,
            Self::Im,
            Self::Tiff,
            Self::Dds,
            Self::Ico,
            Self::Icns,
            Self::Eps,
            Self::Png,
            Self::Gif,
            Self::Jpeg,
            Self::Mpo,
            Self::Pdf,
            Self::Jpeg2000,
            Self::Webp,
        ]
        .into_iter()
        .find(|f| name.eq_ignore_ascii_case(f.name()))
    }
    /// Pillow format identifier.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bmp => "BMP",
            Self::Dib => "DIB",
            Self::Tga => "TGA",
            Self::Ppm => "PPM",
            Self::Pcx => "PCX",
            Self::Sgi => "SGI",
            Self::Im => "IM",
            Self::Tiff => "TIFF",
            Self::Dds => "DDS",
            Self::Ico => "ICO",
            Self::Icns => "ICNS",
            Self::Eps => "EPS",
            Self::Png => "PNG",
            Self::Gif => "GIF",
            Self::Jpeg => "JPEG",
            Self::Mpo => "MPO",
            Self::Pdf => "PDF",
            Self::Jpeg2000 => "JPEG2000",
            Self::Webp => "WEBP",
        }
    }
}
/// Selects the writer from an extension including its dot.
#[must_use]
pub fn select_extension(extension: &str) -> Option<Format> {
    const EXTENSIONS: [(&str, Format); 39] = [
        (".bmp", Format::Bmp),
        (".dib", Format::Dib),
        (".tga", Format::Tga),
        (".icb", Format::Tga),
        (".vda", Format::Tga),
        (".vst", Format::Tga),
        (".pbm", Format::Ppm),
        (".pgm", Format::Ppm),
        (".pnm", Format::Ppm),
        (".ppm", Format::Ppm),
        (".pcx", Format::Pcx),
        (".bw", Format::Sgi),
        (".rgb", Format::Sgi),
        (".rgba", Format::Sgi),
        (".sgi", Format::Sgi),
        (".im", Format::Im),
        (".tif", Format::Tiff),
        (".tiff", Format::Tiff),
        (".dds", Format::Dds),
        (".ico", Format::Ico),
        (".icns", Format::Icns),
        (".eps", Format::Eps),
        (".ps", Format::Eps),
        (".png", Format::Png),
        (".apng", Format::Png),
        (".gif", Format::Gif),
        (".jfif", Format::Jpeg),
        (".jpe", Format::Jpeg),
        (".jpeg", Format::Jpeg),
        (".jpg", Format::Jpeg),
        (".mpo", Format::Mpo),
        (".pdf", Format::Pdf),
        (".j2c", Format::Jpeg2000),
        (".j2k", Format::Jpeg2000),
        (".jp2", Format::Jpeg2000),
        (".jpc", Format::Jpeg2000),
        (".jpf", Format::Jpeg2000),
        (".jpx", Format::Jpeg2000),
        (".webp", Format::Webp),
    ];
    EXTENSIONS
        .iter()
        .find(|(e, _)| e.eq_ignore_ascii_case(extension))
        .map(|(_, f)| *f)
}
/// What the extractor should write under its already selected name.
#[derive(Debug)]
pub enum Outcome<C> {
    /// Save these bytes under the supplied name.
    Original(Saved<C>),
    /// Append `.png` to the supplied name and save these bytes.
    PngFallback(Saved<C>),
    /// PNG cannot represent this size; create no file.
    NoFile,
}
/// Applies section 19.2's policy without allocating a filename or touching disk.
/// Budget and allocator failures propagate, and temporary U writers stay U.
pub fn save_by_extension<C: Charger>(
    image: &Image<'_>,
    extension: &str,
    filename: &str,
    charger: &mut C,
) -> Result<Outcome<C::Charge>, SaveError<C::Refusal>> {
    if let Some(format) = select_extension(extension) {
        let empty_icon = format == Format::Ico && (image.width() < 16 || image.height() < 16);
        if !empty_icon {
            match save(image, format, filename, charger) {
                Ok(out) => return Ok(Outcome::Original(out)),
                Err(SaveError::Pillow(_)) => {}
                Err(SaveError::Encode(EncodeError::Image(
                    ImageError::ZeroSide { .. } | ImageError::TooWide { .. },
                ))) => return Ok(Outcome::NoFile),
                Err(error) => return Err(error),
            }
        }
    }
    match save(image, Format::Png, filename, charger) {
        Ok(out) => Ok(Outcome::PngFallback(out)),
        Err(SaveError::Encode(EncodeError::Image(
            ImageError::ZeroSide { .. } | ImageError::TooWide { .. },
        ))) => Ok(Outcome::NoFile),
        Err(error) => Err(error),
    }
}
