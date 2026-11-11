//! An image to encode or hash: its size, its mode and its pixels, borrowed.

use std::fmt;

/// An image mode Pillow 9.5.0 writes as an 8-bit PNG: the modes of a
/// decoded texture (`RGBA`, `RGB`, `L`) and of a sprite masked from an `L`
/// texture (`LA`), `PARITY.md` section 15.8.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// One byte a pixel, grey.
    L,
    /// Two bytes a pixel: grey, then alpha.
    La,
    /// Three bytes a pixel: red, green, blue.
    Rgb,
    /// Four bytes a pixel: red, green, blue, alpha.
    Rgba,
}

impl Mode {
    /// Every mode.
    pub const ALL: [Self; 4] = [Self::L, Self::La, Self::Rgb, Self::Rgba];

    /// Bytes a pixel (Pillow's raw mode's bits over 8: `bpp` in
    /// `ZipEncode.c`).
    #[must_use]
    pub const fn channels(self) -> usize {
        match self {
            Self::L => 1,
            Self::La => 2,
            Self::Rgb => 3,
            Self::Rgba => 4,
        }
    }

    /// The PNG colour type Pillow writes for the mode (`_OUTMODES` in
    /// `PngImagePlugin.py`): 0, 4, 2 and 6, each at bit depth 8.
    #[must_use]
    pub const fn color_type(self) -> u8 {
        match self {
            Self::L => 0,
            Self::La => 4,
            Self::Rgb => 2,
            Self::Rgba => 6,
        }
    }

    /// Pillow's name of the mode.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::L => "L",
            Self::La => "LA",
            Self::Rgb => "RGB",
            Self::Rgba => "RGBA",
        }
    }

    /// The mode Pillow names `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// An image whose pixels are rows top to bottom, each `width` pixels of
/// [`Mode::channels`] bytes, with no padding: what Pillow's `tobytes()`
/// gives for the mode.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Image<'a> {
    width: u32,
    height: u32,
    mode: Mode,
    pixels: &'a [u8],
}

impl fmt::Debug for Image<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Image")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl<'a> Image<'a> {
    /// The image of `pixels`, `width` x `height` in `mode`, when they hold
    /// exactly its bytes. A side of 0 is an image too: the hash takes it,
    /// and the encoder refuses it as Pillow does.
    pub fn new(width: u32, height: u32, mode: Mode, pixels: &'a [u8]) -> Result<Self, ImageError> {
        let expected = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|n| n.checked_mul(mode.channels() as u64));
        if expected != Some(pixels.len() as u64) {
            return Err(ImageError::Length {
                width,
                height,
                mode,
                actual: pixels.len(),
            });
        }
        Ok(Self {
            width,
            height,
            mode,
            pixels,
        })
    }

    /// Its width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Its height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Its mode.
    #[must_use]
    pub const fn mode(&self) -> Mode {
        self.mode
    }

    /// Its pixel bytes.
    #[must_use]
    pub const fn pixels(&self) -> &'a [u8] {
        self.pixels
    }

    /// The bytes of one row.
    pub(crate) fn row_bytes(&self) -> usize {
        // Exact: `new` checked that the whole image's bytes fit a usize.
        (self.width as usize).saturating_mul(self.mode.channels())
    }
}

/// Why an image cannot be built, or written as Pillow 9.5.0 writes a PNG.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ImageError {
    /// The pixels are not `width x height x channels` bytes.
    #[error("{width}x{height} {mode} needs {} bytes, not {actual}", u128::from(*width) * u128::from(*height) * (mode.channels() as u128))]
    Length {
        /// The width given.
        width: u32,
        /// The height given.
        height: u32,
        /// The mode given.
        mode: Mode,
        /// The bytes given.
        actual: usize,
    },
    /// A side of 0. Pillow 9.5.0 writes no PNG of it: `_setimage` raises
    /// `SystemError` "tile cannot extend outside image" (Pillow 9.5.0,
    /// `src/encode.c:253-257`; `PARITY.md` section 15.8), before it checks
    /// the width ([`ImageError::TooWide`]).
    ///
    /// Pillow raises after writing the PNG signature and IHDR, 33 bytes.
    /// Written to a file object, they stay; `Image.save` to a path removes
    /// the file if it created it, and leaves a file that was there before
    /// truncated to those 33 bytes (Pillow 9.5.0, `src/PIL/Image.py:2421-2441`),
    /// a case for P3's extractor tree. This crate writes nothing.
    #[error("a {width}x{height} image has a side of 0, which Pillow writes no PNG of")]
    ZeroSide {
        /// The width.
        width: u32,
        /// The height.
        height: u32,
    },
    /// Wider than Pillow 9.5.0's encoder takes in the mode: `_setimage`
    /// raises `MemoryError` for a tile wider than `INT_MAX / bits - 7`
    /// pixels, `bits` being the raw mode's bits a pixel, 8 x
    /// [`Mode::channels`] (Pillow 9.5.0, `src/encode.c:261`; the packers'
    /// bits, `src/libImaging/Pack.c:553-588`, which `get_packer` sets,
    /// `src/encode.c:372`). `widest` is 268,435,448 for `L`, 134,217,720
    /// for `LA`, 89,478,478 for `RGB` and 67,108,856 for `RGBA`, each below
    /// the widest image Pillow holds, 536,870,910
    /// (`src/libImaging/Storage.c:51`), so this is where Pillow stops.
    ///
    /// Pillow raises after writing the PNG signature and IHDR, 33 bytes.
    /// Written to a file object, they stay; `Image.save` to a path removes
    /// the file if it created it, and leaves a file that was there before
    /// truncated to those 33 bytes (Pillow 9.5.0, `src/PIL/Image.py:2421-2441`),
    /// a case for P3's extractor tree. This crate writes nothing.
    #[error(
        "a {width}-pixel-wide {mode} image is wider than Pillow 9.5.0's encoder takes, {widest}: its MemoryError"
    )]
    TooWide {
        /// The width.
        width: u32,
        /// The mode.
        mode: Mode,
        /// The widest Pillow's encoder takes in the mode.
        widest: u32,
    },
    /// Past this crate's own bounds, which no image Pillow 9.5.0 holds
    /// meets: a height over 2,147,483,647 (Pillow's sides are C `int`s),
    /// or an output larger than this machine's address space.
    #[error("a {width}x{height} image is past what this crate encodes")]
    TooLarge {
        /// The width.
        width: u32,
        /// The height.
        height: u32,
    },
}
