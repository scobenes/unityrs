// Ported from UnityPy 1.25.2 (MIT): UnityPy/enums/TextureFormat.py (the member names and values), and the output modes and block sizes of export/Texture2DConverter.py's CONV_TABLE and TEXTURE_FORMAT_BLOCK_SIZE_TABLE for the formats decoded here. See THIRD_PARTY_LICENSES.
// Ported from Pillow 9.5.0 (MIT-CMU): the bits a pixel of each raw mode UnityPy names takes (src/libImaging/Unpack.c's unpackers table). See THIRD_PARTY_LICENSES.
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

/// The formats this crate decodes: the five of `docs/plans/extract.md` E15
/// and the rest of the formats UnityPy decodes with Pillow's raw and `bcn`
/// decoders (`raw`'s and `bcn`'s module docs), and the eight ETC and EAC
/// formats texture2ddecoder decodes for it (`etc`'s module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    /// `Alpha8` (1): Pillow's raw `A` into `RGBA`.
    Alpha8,
    /// `ARGB4444` (2): Pillow's raw `RGBA;4B` into `RGBA`, its bands
    /// reordered (2, 1, 0, 3).
    Argb4444,
    /// `RGB24` (3): Pillow's raw `RGB` into `RGB`.
    Rgb24,
    /// `RGBA32` (4): Pillow's raw `RGBA` into `RGBA`.
    Rgba32,
    /// `ARGB32` (5): Pillow's raw `ARGB` into `RGBA`.
    Argb32,
    /// `RGB565` (7): Pillow's raw `BGR;16` into `RGB`.
    Rgb565,
    /// `BGR24` (8): Pillow's raw `BGR` into `RGB`.
    Bgr24,
    /// `DXT1` (10): Pillow's `bcn` 1 (BC1) into `RGBA`.
    Dxt1,
    /// `DXT3` (11): Pillow's `bcn` 2 (BC2) into `RGBA`.
    Dxt3,
    /// `DXT5` (12): Pillow's `bcn` 3 (BC3) into `RGBA`.
    Dxt5,
    /// `RGBA4444` (13): Pillow's raw `RGBA;4B` into `RGBA`, its bands
    /// reordered (3, 2, 1, 0).
    Rgba4444,
    /// `BGRA32` (14): Pillow's raw `BGRA` into `RGBA`.
    Bgra32,
    /// `BC7` (25): Pillow's `bcn` 7 into `RGBA`.
    Bc7,
    /// `BC4` (26): Pillow's `bcn` 4 into `L`.
    Bc4,
    /// `BC5` (27): Pillow's `bcn` 5 into `RGB`.
    Bc5,
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
    /// `R8` (63): Pillow's raw `R` into `RGB`.
    R8,
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
    pub const ALL: [Self; 24] = [
        Self::Alpha8,
        Self::Argb4444,
        Self::Rgb24,
        Self::Rgba32,
        Self::Argb32,
        Self::Rgb565,
        Self::Bgr24,
        Self::Dxt1,
        Self::Dxt3,
        Self::Dxt5,
        Self::Rgba4444,
        Self::Bgra32,
        Self::Bc7,
        Self::Bc4,
        Self::Bc5,
        Self::EtcRgb4,
        Self::EacR,
        Self::EacRSigned,
        Self::EacRg,
        Self::EacRgSigned,
        Self::Etc2Rgb,
        Self::Etc2Rgba1,
        Self::Etc2Rgba8,
        Self::R8,
    ];

    /// The five version 1 of the batch engine exports
    /// (`docs/BATCH_SPEC.md` section 3), in `TextureFormat` order.
    pub const EXPORTED: [Self; 5] = [Self::Rgb24, Self::Rgba32, Self::Dxt1, Self::Dxt5, Self::Bc7];

    /// The format of a `TextureFormat` value, when this crate decodes it.
    #[must_use]
    pub const fn from_texture_format(value: i32) -> Option<Self> {
        match value {
            1 => Some(Self::Alpha8),
            2 => Some(Self::Argb4444),
            3 => Some(Self::Rgb24),
            4 => Some(Self::Rgba32),
            5 => Some(Self::Argb32),
            7 => Some(Self::Rgb565),
            8 => Some(Self::Bgr24),
            10 => Some(Self::Dxt1),
            11 => Some(Self::Dxt3),
            12 => Some(Self::Dxt5),
            13 => Some(Self::Rgba4444),
            14 => Some(Self::Bgra32),
            25 => Some(Self::Bc7),
            26 => Some(Self::Bc4),
            27 => Some(Self::Bc5),
            34 => Some(Self::EtcRgb4),
            41 => Some(Self::EacR),
            42 => Some(Self::EacRSigned),
            43 => Some(Self::EacRg),
            44 => Some(Self::EacRgSigned),
            45 => Some(Self::Etc2Rgb),
            46 => Some(Self::Etc2Rgba1),
            47 => Some(Self::Etc2Rgba8),
            63 => Some(Self::R8),
            _ => None,
        }
    }

    /// Its `TextureFormat` value.
    #[must_use]
    pub const fn texture_format(self) -> i32 {
        match self {
            Self::Alpha8 => 1,
            Self::Argb4444 => 2,
            Self::Rgb24 => 3,
            Self::Rgba32 => 4,
            Self::Argb32 => 5,
            Self::Rgb565 => 7,
            Self::Bgr24 => 8,
            Self::Dxt1 => 10,
            Self::Dxt3 => 11,
            Self::Dxt5 => 12,
            Self::Rgba4444 => 13,
            Self::Bgra32 => 14,
            Self::Bc7 => 25,
            Self::Bc4 => 26,
            Self::Bc5 => 27,
            Self::EtcRgb4 => 34,
            Self::EacR => 41,
            Self::EacRSigned => 42,
            Self::EacRg => 43,
            Self::EacRgSigned => 44,
            Self::Etc2Rgb => 45,
            Self::Etc2Rgba1 => 46,
            Self::Etc2Rgba8 => 47,
            Self::R8 => 63,
        }
    }

    /// Its `TextureFormat` member name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Alpha8 => "Alpha8",
            Self::Argb4444 => "ARGB4444",
            Self::Rgb24 => "RGB24",
            Self::Rgba32 => "RGBA32",
            Self::Argb32 => "ARGB32",
            Self::Rgb565 => "RGB565",
            Self::Bgr24 => "BGR24",
            Self::Dxt1 => "DXT1",
            Self::Dxt3 => "DXT3",
            Self::Dxt5 => "DXT5",
            Self::Rgba4444 => "RGBA4444",
            Self::Bgra32 => "BGRA32",
            Self::Bc7 => "BC7",
            Self::Bc4 => "BC4",
            Self::Bc5 => "BC5",
            Self::EtcRgb4 => "ETC_RGB4",
            Self::EacR => "EAC_R",
            Self::EacRSigned => "EAC_R_SIGNED",
            Self::EacRg => "EAC_RG",
            Self::EacRgSigned => "EAC_RG_SIGNED",
            Self::Etc2Rgb => "ETC2_RGB",
            Self::Etc2Rgba1 => "ETC2_RGBA1",
            Self::Etc2Rgba8 => "ETC2_RGBA8",
            Self::R8 => "R8",
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
    /// `export/Texture2DConverter.py:488-566`): `L` for `BC4` (`:513`);
    /// `RGB` for `RGB24` (`:491`), `RGB565` (`:495`), `BGR24` (`:496`), `R8`
    /// (`:497`) and `BC5` (`:514`); `RGBA` for the rest. A texture with a
    /// side of 0 is an empty `RGBA` image whatever its format
    /// ([`crate::decode()`]).
    #[must_use]
    pub const fn mode(self) -> Mode {
        match self {
            Self::Bc4 => Mode::L,
            Self::Rgb24 | Self::Rgb565 | Self::Bgr24 | Self::R8 | Self::Bc5 => Mode::Rgb,
            _ => Mode::Rgba,
        }
    }

    /// The bytes one 4x4 block holds, for a block format: 8 for DXT1, BC4,
    /// `ETC_RGB4`, `EAC_R`, `EAC_R_SIGNED`, `ETC2_RGB` and `ETC2_RGBA1`, 16
    /// for the rest; `None` for the raw formats, which UnityPy does not pad
    /// (`get_compressed_image_size`, `export/Texture2DConverter.py:36-50`:
    /// names starting `DXT`, `BC`, `ETC` or `EAC` are padded to 4x4 blocks,
    /// `:22-33`).
    #[must_use]
    pub const fn block_bytes(self) -> Option<u64> {
        match self {
            Self::Dxt1
            | Self::Bc4
            | Self::EtcRgb4
            | Self::EacR
            | Self::EacRSigned
            | Self::Etc2Rgb
            | Self::Etc2Rgba1 => Some(8),
            Self::Dxt3
            | Self::Dxt5
            | Self::Bc7
            | Self::Bc5
            | Self::EacRg
            | Self::EacRgSigned
            | Self::Etc2Rgba8 => Some(16),
            Self::Alpha8
            | Self::Argb4444
            | Self::Rgb24
            | Self::Rgba32
            | Self::Argb32
            | Self::Rgb565
            | Self::Bgr24
            | Self::Rgba4444
            | Self::Bgra32
            | Self::R8 => None,
        }
    }

    /// The bits a pixel takes where Pillow's raw decoder reads the data:
    /// its unpacker's for the raw mode UnityPy names (`CONV_TABLE`;
    /// Pillow's `src/libImaging/Unpack.c`): 8 for `A` and `R`, 16 for
    /// `RGBA;4B` and `BGR;16`, 24 for `RGB` and `BGR`, 32 for `RGBA`,
    /// `BGRA` and `ARGB`; 32 for texture2ddecoder's output, which Pillow
    /// reads as `BGRA`; `None` for the `bcn` decoder, which has no such
    /// reader. Pillow's raw decoder refuses a row of more than
    /// `INT_MAX / bits - 7` pixels (`decode`'s module docs).
    pub(crate) const fn raw_bits(self) -> Option<u32> {
        match self.library() {
            Library::PillowBcn => None,
            Library::Texture2DDecoder => Some(32),
            Library::PillowRaw => Some(match self {
                Self::Alpha8 | Self::R8 => 8,
                Self::Argb4444 | Self::Rgba4444 | Self::Rgb565 => 16,
                Self::Rgb24 | Self::Bgr24 => 24,
                _ => 32,
            }),
        }
    }

    /// The library that decodes it for UnityPy (`CONV_TABLE`).
    pub(crate) const fn library(self) -> Library {
        match self {
            Self::Dxt1 | Self::Dxt3 | Self::Dxt5 | Self::Bc4 | Self::Bc5 | Self::Bc7 => {
                Library::PillowBcn
            }
            Self::EtcRgb4
            | Self::EacR
            | Self::EacRSigned
            | Self::EacRg
            | Self::EacRgSigned
            | Self::Etc2Rgb
            | Self::Etc2Rgba1
            | Self::Etc2Rgba8 => Library::Texture2DDecoder,
            _ => Library::PillowRaw,
        }
    }
}

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
