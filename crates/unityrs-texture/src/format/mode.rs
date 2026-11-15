// Ported from UnityPy 1.25.2 (MIT): export/Texture2DConverter.py's CONV_TABLE output modes; closed forms follow PARITY.md sections 17.1 and 19. See THIRD_PARTY_LICENSES.
use super::Format;

/// An image mode, as Pillow names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// `L`: one byte a pixel.
    L,
    /// `RGB`: three bytes a pixel, red first.
    Rgb,
    /// `RGBA`: four bytes a pixel, red first, alpha last.
    Rgba,
}

const _: () = crate::send_sync::<Mode>();

impl Mode {
    /// Pillow's name for it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::L => "L",
            Self::Rgb => "RGB",
            Self::Rgba => "RGBA",
        }
    }

    /// The bytes one pixel takes.
    #[must_use]
    pub const fn bytes_per_pixel(self) -> u64 {
        match self {
            Self::L => 1,
            Self::Rgb => 3,
            Self::Rgba => 4,
        }
    }
}

impl Format {
    /// The output mode: for closed forms, PARITY.md sections 17.1 and 19;
    /// otherwise the mode UnityPy decodes it to (`CONV_TABLE`,
    /// `export/Texture2DConverter.py:488-566`): `L` for `BC4` (`:513`);
    /// `RGB` for `RGB24` (`:491`), `RGB565` (`:495`), `BGR24` (`:496`), `R8`
    /// (`:497`) and `BC5` (`:514`); `RGBA` for the rest. A texture with a
    /// side of 0 is an empty `RGBA` image whatever its format
    /// ([`crate::decode()`]).
    #[must_use]
    pub const fn mode(self) -> Mode {
        if let Some(layout) = crate::tierb::layout(self) {
            return if layout.channels == 4 {
                Mode::Rgba
            } else {
                Mode::Rgb
            };
        }
        match self {
            Self::Bc4 => Mode::L,
            Self::Rgb24 | Self::Rgb565 | Self::Bgr24 | Self::R8 | Self::Bc5 | Self::Bc6h => {
                Mode::Rgb
            }
            _ => Mode::Rgba,
        }
    }
}
