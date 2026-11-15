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
