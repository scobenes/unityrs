// Ported from UnityPy 1.25.2 (MIT): UnityPy/enums/TextureFormat.py (the member names and values), and the output modes and block sizes of export/Texture2DConverter.py's CONV_TABLE and TEXTURE_FORMAT_BLOCK_SIZE_TABLE for the formats decoded here. See THIRD_PARTY_LICENSES.
//! Texture formats: every value UnityPy 1.25.2 names ([`texture_format_name`]),
//! the ones this crate decodes ([`Format`]), of which version 1 of the batch
//! engine exports five ([`Format::exported`]), and the image modes they
//! decode to ([`Mode`]).

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

/// The formats this crate decodes: the five of `docs/plans/extract.md` E15,
/// which Pillow decodes for UnityPy, and the eight ETC and EAC formats
/// texture2ddecoder decodes for it (`etc`'s module docs).
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
    /// `ETC_RGB4` (34): texture2ddecoder's ETC1 into `RGBA`.
    EtcRgb4,
    /// `EAC_R` (41): texture2ddecoder's EAC R into `RGBA`.
    EacR,
    /// `EAC_R_SIGNED` (42): texture2ddecoder's signed EAC R into `RGBA`.
    EacRSigned,
    /// `EAC_RG` (43): texture2ddecoder's EAC RG into `RGBA`.
    EacRg,
    /// `EAC_RG_SIGNED` (44): texture2ddecoder's signed EAC RG into `RGBA`.
    EacRgSigned,
    /// `ETC2_RGB` (45): texture2ddecoder's ETC2 into `RGBA`.
    Etc2Rgb,
    /// `ETC2_RGBA1` (46): texture2ddecoder's ETC2 A1 into `RGBA`, with
    /// its transparent pixels (0, 0, 0, 0) (Tier C, `PARITY.md` section
    /// 19.1).
    Etc2Rgba1,
    /// `ETC2_RGBA8` (47): texture2ddecoder's ETC2 A8 into `RGBA`.
    Etc2Rgba8,
}

const _: () = crate::send_sync::<Format>();

/// The library that decodes a [`Format`] for UnityPy, which decides how the
/// data's length is read and which of Pillow's limits apply
/// (`decode`'s module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Library {
    /// Pillow's raw decoder, on the texture's own size.
    PillowRaw,
    /// Pillow's `bcn` decoder, on the size padded to whole 4x4 blocks.
    PillowBcn,
    /// texture2ddecoder, on the size padded to whole 4x4 blocks, its
    /// output then read by Pillow's raw decoder as `BGRA`.
    Texture2DDecoder,
}

impl Format {
    /// Every format, in `TextureFormat` order.
    pub const ALL: [Self; 13] = [
        Self::Rgb24,
        Self::Rgba32,
        Self::Dxt1,
        Self::Dxt5,
        Self::Bc7,
        Self::EtcRgb4,
        Self::EacR,
        Self::EacRSigned,
        Self::EacRg,
        Self::EacRgSigned,
        Self::Etc2Rgb,
        Self::Etc2Rgba1,
        Self::Etc2Rgba8,
    ];

    /// The five version 1 of the batch engine exports
    /// (`docs/BATCH_SPEC.md` section 3), in `TextureFormat` order.
    pub const EXPORTED: [Self; 5] = [Self::Rgb24, Self::Rgba32, Self::Dxt1, Self::Dxt5, Self::Bc7];

    /// The format of a `TextureFormat` value, when this crate decodes it.
    #[must_use]
    pub const fn from_texture_format(value: i32) -> Option<Self> {
        match value {
            3 => Some(Self::Rgb24),
            4 => Some(Self::Rgba32),
            10 => Some(Self::Dxt1),
            12 => Some(Self::Dxt5),
            25 => Some(Self::Bc7),
            34 => Some(Self::EtcRgb4),
            41 => Some(Self::EacR),
            42 => Some(Self::EacRSigned),
            43 => Some(Self::EacRg),
            44 => Some(Self::EacRgSigned),
            45 => Some(Self::Etc2Rgb),
            46 => Some(Self::Etc2Rgba1),
            47 => Some(Self::Etc2Rgba8),
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
            Self::EtcRgb4 => 34,
            Self::EacR => 41,
            Self::EacRSigned => 42,
            Self::EacRg => 43,
            Self::EacRgSigned => 44,
            Self::Etc2Rgb => 45,
            Self::Etc2Rgba1 => 46,
            Self::Etc2Rgba8 => 47,
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
            Self::EtcRgb4 => "ETC_RGB4",
            Self::EacR => "EAC_R",
            Self::EacRSigned => "EAC_R_SIGNED",
            Self::EacRg => "EAC_RG",
            Self::EacRgSigned => "EAC_RG_SIGNED",
            Self::Etc2Rgb => "ETC2_RGB",
            Self::Etc2Rgba1 => "ETC2_RGBA1",
            Self::Etc2Rgba8 => "ETC2_RGBA8",
        }
    }

    /// Whether version 1 of the batch engine exports it: one of
    /// [`Self::EXPORTED`], the five of `docs/plans/extract.md` E15
    /// ([`crate::select()`]).
    #[must_use]
    pub const fn exported(self) -> bool {
        matches!(
            self,
            Self::Rgb24 | Self::Rgba32 | Self::Dxt1 | Self::Dxt5 | Self::Bc7
        )
    }

    /// The mode UnityPy decodes it to (`CONV_TABLE`,
    /// `export/Texture2DConverter.py:491-532`): `RGB` for `RGB24`, `RGBA`
    /// for the rest. A texture with a side of 0 is an empty `RGBA` image
    /// whatever its format ([`crate::decode()`]).
    #[must_use]
    pub const fn mode(self) -> Mode {
        match self {
            Self::Rgb24 => Mode::Rgb,
            _ => Mode::Rgba,
        }
    }

    /// The bytes one 4x4 block holds, for a block format: 8 for DXT1,
    /// `ETC_RGB4`, `EAC_R`, `EAC_R_SIGNED`, `ETC2_RGB` and `ETC2_RGBA1`, 16 for the
    /// rest; `None` for the raw formats, which UnityPy does not pad
    /// (`get_compressed_image_size`, `export/Texture2DConverter.py:36-50`:
    /// names starting `DXT`, `BC`, `ETC` or `EAC` are padded to 4x4 blocks).
    #[must_use]
    pub const fn block_bytes(self) -> Option<u64> {
        match self {
            Self::Dxt1
            | Self::EtcRgb4
            | Self::EacR
            | Self::EacRSigned
            | Self::Etc2Rgb
            | Self::Etc2Rgba1 => Some(8),
            Self::Dxt5 | Self::Bc7 | Self::EacRg | Self::EacRgSigned | Self::Etc2Rgba8 => Some(16),
            Self::Rgb24 | Self::Rgba32 => None,
        }
    }

    /// The library that decodes it for UnityPy (`CONV_TABLE`).
    pub(crate) const fn library(self) -> Library {
        match self {
            Self::Rgb24 | Self::Rgba32 => Library::PillowRaw,
            Self::Dxt1 | Self::Dxt5 | Self::Bc7 => Library::PillowBcn,
            _ => Library::Texture2DDecoder,
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
