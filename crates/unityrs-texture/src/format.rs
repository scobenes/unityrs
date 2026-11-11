// Ported from UnityPy 1.25.2 (MIT): UnityPy/enums/TextureFormat.py (the member names and values), and the output modes of export/Texture2DConverter.py's CONV_TABLE for the five formats. See THIRD_PARTY_LICENSES.
//! Texture formats: every value UnityPy 1.25.2 names ([`texture_format_name`]),
//! the five this crate decodes ([`Format`]), and the image modes they decode
//! to ([`Mode`]).

/// UnityPy 1.25.2's `TextureFormat` members, value and name, in its order
/// (`enums/TextureFormat.py:4-82`). Values 37 to 40 are unassigned.
pub const TEXTURE_FORMATS: [(i32, &str); 78] = [
    (1, "Alpha8"),
    (2, "ARGB4444"),
    (3, "RGB24"),
    (4, "RGBA32"),
    (5, "ARGB32"),
    (6, "ARGBFloat"),
    (7, "RGB565"),
    (8, "BGR24"),
    (9, "R16"),
    (10, "DXT1"),
    (11, "DXT3"),
    (12, "DXT5"),
    (13, "RGBA4444"),
    (14, "BGRA32"),
    (15, "RHalf"),
    (16, "RGHalf"),
    (17, "RGBAHalf"),
    (18, "RFloat"),
    (19, "RGFloat"),
    (20, "RGBAFloat"),
    (21, "YUY2"),
    (22, "RGB9e5Float"),
    (23, "RGBFloat"),
    (24, "BC6H"),
    (25, "BC7"),
    (26, "BC4"),
    (27, "BC5"),
    (28, "DXT1Crunched"),
    (29, "DXT5Crunched"),
    (30, "PVRTC_RGB2"),
    (31, "PVRTC_RGBA2"),
    (32, "PVRTC_RGB4"),
    (33, "PVRTC_RGBA4"),
    (34, "ETC_RGB4"),
    (35, "ATC_RGB4"),
    (36, "ATC_RGBA8"),
    (41, "EAC_R"),
    (42, "EAC_R_SIGNED"),
    (43, "EAC_RG"),
    (44, "EAC_RG_SIGNED"),
    (45, "ETC2_RGB"),
    (46, "ETC2_RGBA1"),
    (47, "ETC2_RGBA8"),
    (48, "ASTC_RGB_4x4"),
    (49, "ASTC_RGB_5x5"),
    (50, "ASTC_RGB_6x6"),
    (51, "ASTC_RGB_8x8"),
    (52, "ASTC_RGB_10x10"),
    (53, "ASTC_RGB_12x12"),
    (54, "ASTC_RGBA_4x4"),
    (55, "ASTC_RGBA_5x5"),
    (56, "ASTC_RGBA_6x6"),
    (57, "ASTC_RGBA_8x8"),
    (58, "ASTC_RGBA_10x10"),
    (59, "ASTC_RGBA_12x12"),
    (60, "ETC_RGB4_3DS"),
    (61, "ETC_RGBA8_3DS"),
    (62, "RG16"),
    (63, "R8"),
    (64, "ETC_RGB4Crunched"),
    (65, "ETC2_RGBA8Crunched"),
    (66, "ASTC_HDR_4x4"),
    (67, "ASTC_HDR_5x5"),
    (68, "ASTC_HDR_6x6"),
    (69, "ASTC_HDR_8x8"),
    (70, "ASTC_HDR_10x10"),
    (71, "ASTC_HDR_12x12"),
    (72, "RG32"),
    (73, "RGB48"),
    (74, "RGBA64"),
    (75, "R8_SIGNED"),
    (76, "RG16_SIGNED"),
    (77, "RGB24_SIGNED"),
    (78, "RGBA32_SIGNED"),
    (79, "R16_SIGNED"),
    (80, "RG32_SIGNED"),
    (81, "RGB48_SIGNED"),
    (82, "RGBA64_SIGNED"),
];

/// UnityPy 1.25.2's name for a `TextureFormat` value, or `None` for a value
/// its enum lacks (where `TextureFormat(value)` raises `ValueError`).
#[must_use]
pub fn texture_format_name(value: i32) -> Option<&'static str> {
    TEXTURE_FORMATS
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(_, name)| *name)
}

/// The five formats this crate decodes (`docs/plans/extract.md` E15).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    /// `RGB24` (3): Pillow's raw `RGB` into `RGB`.
    Rgb24,
    /// `RGBA32` (4): Pillow's raw `RGBA` into `RGBA`.
    Rgba32,
    /// `DXT1` (10): Pillow's `bcn` 1 (BC1) into `RGBA`.
    Dxt1,
    /// `DXT5` (12): Pillow's `bcn` 3 (BC3) into `RGBA`.
    Dxt5,
    /// `BC7` (25): Pillow's `bcn` 7 into `RGBA`.
    Bc7,
}

const _: () = crate::send_sync::<Format>();

impl Format {
    /// The five, in `TextureFormat` order.
    pub const ALL: [Self; 5] = [Self::Rgb24, Self::Rgba32, Self::Dxt1, Self::Dxt5, Self::Bc7];

    /// The format of a `TextureFormat` value, when it is one of the five.
    #[must_use]
    pub const fn from_texture_format(value: i32) -> Option<Self> {
        match value {
            3 => Some(Self::Rgb24),
            4 => Some(Self::Rgba32),
            10 => Some(Self::Dxt1),
            12 => Some(Self::Dxt5),
            25 => Some(Self::Bc7),
            _ => None,
        }
    }

    /// Its `TextureFormat` value.
    #[must_use]
    pub const fn texture_format(self) -> i32 {
        match self {
            Self::Rgb24 => 3,
            Self::Rgba32 => 4,
            Self::Dxt1 => 10,
            Self::Dxt5 => 12,
            Self::Bc7 => 25,
        }
    }

    /// Its `TextureFormat` member name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rgb24 => "RGB24",
            Self::Rgba32 => "RGBA32",
            Self::Dxt1 => "DXT1",
            Self::Dxt5 => "DXT5",
            Self::Bc7 => "BC7",
        }
    }

    /// The mode UnityPy decodes it to (`CONV_TABLE`,
    /// `export/Texture2DConverter.py:491-516`): `RGB` for `RGB24`, `RGBA`
    /// for the other four. A texture with a side of 0 is an empty `RGBA`
    /// image whatever its format ([`crate::decode()`]).
    #[must_use]
    pub const fn mode(self) -> Mode {
        match self {
            Self::Rgb24 => Mode::Rgb,
            Self::Rgba32 | Self::Dxt1 | Self::Dxt5 | Self::Bc7 => Mode::Rgba,
        }
    }

    /// The bytes one 4x4 block holds, for a block format: 8 for DXT1, 16 for
    /// DXT5 and BC7; `None` for the raw formats, which UnityPy does not pad
    /// (`get_compressed_image_size`, `export/Texture2DConverter.py:36-50`:
    /// names starting `DXT` or `BC` are padded to 4x4 blocks).
    #[must_use]
    pub const fn block_bytes(self) -> Option<u64> {
        match self {
            Self::Dxt1 => Some(8),
            Self::Dxt5 | Self::Bc7 => Some(16),
            Self::Rgb24 | Self::Rgba32 => None,
        }
    }
}

/// An image mode, as Pillow names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
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
            Self::Rgb => "RGB",
            Self::Rgba => "RGBA",
        }
    }

    /// The bytes one pixel takes.
    #[must_use]
    pub const fn bytes_per_pixel(self) -> u64 {
        match self {
            Self::Rgb => 3,
            Self::Rgba => 4,
        }
    }
}
