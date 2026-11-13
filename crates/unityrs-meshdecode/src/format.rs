// Ported from UnityPy 1.25.2 (MIT): UnityPy/enums/VertexFormat.py's three format enums and their struct-code tables, UnityPy/enums/MeshTopology.py, UnityPy/enums/GfxPrimitiveType.py, and helpers/MeshHelper.py's get_channel_dtype and get_channel_component_size. See THIRD_PARTY_LICENSES.
//! The vertex formats, by version, as the struct codes UnityPy reads them
//! with, and the topology enums.
//!
//! **Which table** (`MeshHandler.get_channel_dtype`,
//! `helpers/MeshHelper.py:451-462`): [`VertexChannelFormat`] below Unity
//! 2017, [`VertexFormat2017`] below 2019, [`VertexFormat`] from 2019, by
//! the version's major alone. A format the table's enum lacks raises
//! `ValueError` (`12 is not a valid VertexFormat`), which
//! [`get_channel_dtype`] returns as [`MeshError::InvalidFormat`].
//!
//! **What a struct code reads** (`enums/VertexFormat.py:43-80`): UnityPy
//! reads `UNorm`, `SNorm` and the byte colour formats as the raw integers,
//! never scaled, and `Float16` through Python's `e` code
//! (`StructCode::F16`). [`get_channel_component_size`] is
//! `struct.Struct(code).size` (`helpers/MeshHelper.py:464-466`).
//!
//! **Normalised** (`PARITY.md` section 19.1, Tier C): unityrs reads a
//! Mesh's channel in a normalised format ([`Norm`]: each table's `UNorm8`,
//! `SNorm8`, `UNorm16` and `SNorm16`, and the byte colour formats
//! `kChannelFormatColor` and `kVertexFormatColor`) as a `float`, where
//! UnityPy leaves the integer (`enums/VertexFormat.py:46`, `:54-58`,
//! `:70-73`, read by `helpers/MeshHelper.py:392`). The reference is the
//! normalised fixed-point conversion of the Vulkan specification
//! ("Fixed-Point Data Conversions", its "Conversion from Normalized
//! Fixed-Point to Floating-Point") and of Direct3D 10 and later's data
//! conversion rules (`SNORM` and `UNORM` to `FLOAT`): an n-bit `UNorm` value
//! `v` is `v / (2^n - 1)`, in [0, 1], and an n-bit `SNorm` value
//! `max(v / (2^(n-1) - 1), -1)`, in [-1, 1], so the most negative value
//! and the one above it both give -1. A compressed mesh's packed colours
//! are already bytes over 255 in UnityPy (`helpers/MeshHelper.py:618-628`),
//! the `UNorm8` rule. `kChannelFormatByte` and the integer formats stay
//! integers, and so do the blend indices (channel 13 from 2018), which are
//! bone indices whatever their format; the blend weights are normalised.
//! A Sprite's render data is read as UnityPy reads it, its integers kept,
//! whatever its formats: a deliberate compatibility policy, not Unity's
//! meaning (a GPU would normalise a `UNorm` sprite UV), so that the sprite
//! renderer (`unityrs-texture`), which ports `get_image_from_sprite` at
//! Tier A on UnityPy's values, reads bit for bit what it reads there. A
//! Tier C correction of the sprite path is deferred (`PARITY.md` section
//! 19.1).

use crate::Version;
use crate::error::MeshError;

/// A `struct` code `get_channel_dtype` gives: how one component is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StructCode {
    /// `f`: a 32-bit float.
    F32,
    /// `e`: a 16-bit float.
    F16,
    /// `B`: an unsigned byte.
    U8,
    /// `b`: a signed byte.
    I8,
    /// `H`: an unsigned 16-bit integer.
    U16,
    /// `h`: a signed 16-bit integer.
    I16,
    /// `I`: an unsigned 32-bit integer.
    U32,
    /// `i`: a signed 32-bit integer.
    I32,
}

const _: () = crate::send_sync::<StructCode>();

impl StructCode {
    /// The code's character.
    #[must_use]
    pub const fn code(self) -> char {
        match self {
            Self::F32 => 'f',
            Self::F16 => 'e',
            Self::U8 => 'B',
            Self::I8 => 'b',
            Self::U16 => 'H',
            Self::I16 => 'h',
            Self::U32 => 'I',
            Self::I32 => 'i',
        }
    }

    /// `struct.Struct(code).size`: the component's bytes.
    #[must_use]
    pub const fn size(self) -> u32 {
        match self {
            Self::U8 | Self::I8 => 1,
            Self::F16 | Self::U16 | Self::I16 => 2,
            Self::F32 | Self::U32 | Self::I32 => 4,
        }
    }

    /// Whether Python reads it as a `float` (else an `int`).
    #[must_use]
    pub const fn is_float(self) -> bool {
        matches!(self, Self::F32 | Self::F16)
    }
}

/// A normalised format's rule (module docs, Tier C): how unityrs reads a
/// Mesh's integer of that format as a `float`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Norm {
    /// `UNorm`, and the byte colour formats: `v / (2^n - 1)`, in [0, 1].
    UNorm,
    /// `SNorm`: `max(v / (2^(n-1) - 1), -1)`, in [-1, 1].
    SNorm,
}

const _: () = crate::send_sync::<Norm>();

impl Norm {
    /// The `float` an integer `raw` of `code` normalises to, by the
    /// Vulkan and Direct3D rule (module docs): for an 8-bit `SNorm`, -128
    /// and -127 give -1.0, 0 gives 0.0 and 127 gives 1.0; for an 8-bit
    /// `UNorm`, 255 gives 1.0. Each value is the correctly rounded
    /// quotient of two integers an `f64` holds exactly.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // A struct code's integer: at most 32 bits, exact.
    pub fn value(self, code: StructCode, raw: i64) -> f64 {
        let bits = code.size() * 8;
        let v = raw as f64;
        match self {
            Self::UNorm => v / f64::from(u32::MAX >> (32 - bits)),
            Self::SNorm => (v / f64::from(u32::MAX >> (33 - bits))).max(-1.0),
        }
    }
}

/// `VertexChannelFormat`, the formats below Unity 2017
/// (`enums/VertexFormat.py:4-9`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(non_camel_case_types)]
pub enum VertexChannelFormat {
    /// 0, read as `f`.
    kChannelFormatFloat = 0,
    /// 1, read as `e`.
    kChannelFormatFloat16 = 1,
    /// 2, read as `B`.
    kChannelFormatColor = 2,
    /// 3, read as `B`.
    kChannelFormatByte = 3,
    /// 4, read as `I`.
    kChannelFormatUInt32 = 4,
}

const _: () = crate::send_sync::<VertexChannelFormat>();

impl VertexChannelFormat {
    /// Every member, in UnityPy's order.
    pub const ALL: [Self; 5] = [
        Self::kChannelFormatFloat,
        Self::kChannelFormatFloat16,
        Self::kChannelFormatColor,
        Self::kChannelFormatByte,
        Self::kChannelFormatUInt32,
    ];

    /// The member of this value, as `VertexChannelFormat(value)` finds it.
    #[must_use]
    pub fn from_value(value: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|m| *m as u8 == value)
    }

    /// Its code in `VERTEX_CHANNEL_FORMAT_STRUCT_TYPE_MAP`
    /// (`enums/VertexFormat.py:43-49`).
    #[must_use]
    pub const fn struct_code(self) -> StructCode {
        match self {
            Self::kChannelFormatFloat => StructCode::F32,
            Self::kChannelFormatFloat16 => StructCode::F16,
            Self::kChannelFormatColor | Self::kChannelFormatByte => StructCode::U8,
            Self::kChannelFormatUInt32 => StructCode::U32,
        }
    }

    /// Its rule, if unityrs normalises it (module docs): the colour bytes
    /// only.
    #[must_use]
    pub const fn norm(self) -> Option<Norm> {
        match self {
            Self::kChannelFormatColor => Some(Norm::UNorm),
            _ => None,
        }
    }
}

/// `VertexFormat2017`, the formats of Unity 2017 and 2018
/// (`enums/VertexFormat.py:12-25`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(non_camel_case_types)]
pub enum VertexFormat2017 {
    /// 0, read as `f`.
    kVertexFormatFloat = 0,
    /// 1, read as `e`.
    kVertexFormatFloat16 = 1,
    /// 2, read as `B`.
    kVertexFormatColor = 2,
    /// 3, read as `B`.
    kVertexFormatUNorm8 = 3,
    /// 4, read as `b`.
    kVertexFormatSNorm8 = 4,
    /// 5, read as `H`.
    kVertexFormatUNorm16 = 5,
    /// 6, read as `h`.
    kVertexFormatSNorm16 = 6,
    /// 7, read as `B`.
    kVertexFormatUInt8 = 7,
    /// 8, read as `b`.
    kVertexFormatSInt8 = 8,
    /// 9, read as `H`.
    kVertexFormatUInt16 = 9,
    /// 10, read as `h`.
    kVertexFormatSInt16 = 10,
    /// 11, read as `I`.
    kVertexFormatUInt32 = 11,
    /// 12, read as `i`.
    kVertexFormatSInt32 = 12,
}

const _: () = crate::send_sync::<VertexFormat2017>();

impl VertexFormat2017 {
    /// Every member, in UnityPy's order.
    pub const ALL: [Self; 13] = [
        Self::kVertexFormatFloat,
        Self::kVertexFormatFloat16,
        Self::kVertexFormatColor,
        Self::kVertexFormatUNorm8,
        Self::kVertexFormatSNorm8,
        Self::kVertexFormatUNorm16,
        Self::kVertexFormatSNorm16,
        Self::kVertexFormatUInt8,
        Self::kVertexFormatSInt8,
        Self::kVertexFormatUInt16,
        Self::kVertexFormatSInt16,
        Self::kVertexFormatUInt32,
        Self::kVertexFormatSInt32,
    ];

    /// The member of this value, as `VertexFormat2017(value)` finds it.
    #[must_use]
    pub fn from_value(value: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|m| *m as u8 == value)
    }

    /// Its code in `VERTEX_FORMAT_2017_STRUCT_TYPE_MAP`
    /// (`enums/VertexFormat.py:51-65`).
    #[must_use]
    pub const fn struct_code(self) -> StructCode {
        match self {
            Self::kVertexFormatFloat => StructCode::F32,
            Self::kVertexFormatFloat16 => StructCode::F16,
            Self::kVertexFormatColor | Self::kVertexFormatUNorm8 | Self::kVertexFormatUInt8 => {
                StructCode::U8
            }
            Self::kVertexFormatSNorm8 | Self::kVertexFormatSInt8 => StructCode::I8,
            Self::kVertexFormatUNorm16 | Self::kVertexFormatUInt16 => StructCode::U16,
            Self::kVertexFormatSNorm16 | Self::kVertexFormatSInt16 => StructCode::I16,
            Self::kVertexFormatUInt32 => StructCode::U32,
            Self::kVertexFormatSInt32 => StructCode::I32,
        }
    }

    /// Its rule, if unityrs normalises it (module docs): the colour
    /// bytes, `UNorm` and `SNorm`.
    #[must_use]
    pub const fn norm(self) -> Option<Norm> {
        match self {
            Self::kVertexFormatColor | Self::kVertexFormatUNorm8 | Self::kVertexFormatUNorm16 => {
                Some(Norm::UNorm)
            }
            Self::kVertexFormatSNorm8 | Self::kVertexFormatSNorm16 => Some(Norm::SNorm),
            _ => None,
        }
    }
}

/// `VertexFormat`, the formats from Unity 2019 (`enums/VertexFormat.py:28-40`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(non_camel_case_types)]
pub enum VertexFormat {
    /// 0, read as `f`.
    kVertexFormatFloat = 0,
    /// 1, read as `e`.
    kVertexFormatFloat16 = 1,
    /// 2, read as `B`.
    kVertexFormatUNorm8 = 2,
    /// 3, read as `b`.
    kVertexFormatSNorm8 = 3,
    /// 4, read as `H`.
    kVertexFormatUNorm16 = 4,
    /// 5, read as `h`.
    kVertexFormatSNorm16 = 5,
    /// 6, read as `B`.
    kVertexFormatUInt8 = 6,
    /// 7, read as `b`.
    kVertexFormatSInt8 = 7,
    /// 8, read as `H`.
    kVertexFormatUInt16 = 8,
    /// 9, read as `h`.
    kVertexFormatSInt16 = 9,
    /// 10, read as `I`.
    kVertexFormatUInt32 = 10,
    /// 11, read as `i`.
    kVertexFormatSInt32 = 11,
}

const _: () = crate::send_sync::<VertexFormat>();

impl VertexFormat {
    /// Every member, in UnityPy's order.
    pub const ALL: [Self; 12] = [
        Self::kVertexFormatFloat,
        Self::kVertexFormatFloat16,
        Self::kVertexFormatUNorm8,
        Self::kVertexFormatSNorm8,
        Self::kVertexFormatUNorm16,
        Self::kVertexFormatSNorm16,
        Self::kVertexFormatUInt8,
        Self::kVertexFormatSInt8,
        Self::kVertexFormatUInt16,
        Self::kVertexFormatSInt16,
        Self::kVertexFormatUInt32,
        Self::kVertexFormatSInt32,
    ];

    /// The member of this value, as `VertexFormat(value)` finds it.
    #[must_use]
    pub fn from_value(value: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|m| *m as u8 == value)
    }

    /// Its code in `VERTEX_FORMAT_STRUCT_TYPE_MAP`
    /// (`enums/VertexFormat.py:67-80`).
    #[must_use]
    pub const fn struct_code(self) -> StructCode {
        match self {
            Self::kVertexFormatFloat => StructCode::F32,
            Self::kVertexFormatFloat16 => StructCode::F16,
            Self::kVertexFormatUNorm8 | Self::kVertexFormatUInt8 => StructCode::U8,
            Self::kVertexFormatSNorm8 | Self::kVertexFormatSInt8 => StructCode::I8,
            Self::kVertexFormatUNorm16 | Self::kVertexFormatUInt16 => StructCode::U16,
            Self::kVertexFormatSNorm16 | Self::kVertexFormatSInt16 => StructCode::I16,
            Self::kVertexFormatUInt32 => StructCode::U32,
            Self::kVertexFormatSInt32 => StructCode::I32,
        }
    }

    /// Its rule, if unityrs normalises it (module docs): `UNorm` and
    /// `SNorm`.
    #[must_use]
    pub const fn norm(self) -> Option<Norm> {
        match self {
            Self::kVertexFormatUNorm8 | Self::kVertexFormatUNorm16 => Some(Norm::UNorm),
            Self::kVertexFormatSNorm8 | Self::kVertexFormatSNorm16 => Some(Norm::SNorm),
            _ => None,
        }
    }
}

/// The three tables, as `get_channel_dtype` picks one by version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FormatTable {
    /// [`VertexChannelFormat`]: a major below 2017.
    Channel,
    /// [`VertexFormat2017`]: a major of 2017 or 2018.
    V2017,
    /// [`VertexFormat`]: a major of 2019 or more.
    V2019,
}

const _: () = crate::send_sync::<FormatTable>();

impl FormatTable {
    /// The table a version's major reads (`helpers/MeshHelper.py:452-460`).
    #[must_use]
    pub const fn for_major(major: u16) -> Self {
        if major < 2017 {
            Self::Channel
        } else if major < 2019 {
            Self::V2017
        } else {
            Self::V2019
        }
    }

    /// The enum's name, as its `ValueError` gives it.
    #[must_use]
    pub const fn python_name(self) -> &'static str {
        match self {
            Self::Channel => "VertexChannelFormat",
            Self::V2017 => "VertexFormat2017",
            Self::V2019 => "VertexFormat",
        }
    }

    /// The rule unityrs normalises `format` of this table by, if any
    /// (module docs): `None` for a format the enum lacks, too.
    #[must_use]
    pub fn norm(self, format: u8) -> Option<Norm> {
        match self {
            Self::Channel => {
                VertexChannelFormat::from_value(format).and_then(VertexChannelFormat::norm)
            }
            Self::V2017 => VertexFormat2017::from_value(format).and_then(VertexFormat2017::norm),
            Self::V2019 => VertexFormat::from_value(format).and_then(VertexFormat::norm),
        }
    }

    /// The struct code of `format` in this table, if the enum has it.
    #[must_use]
    pub fn struct_code(self, format: u8) -> Option<StructCode> {
        match self {
            Self::Channel => {
                VertexChannelFormat::from_value(format).map(VertexChannelFormat::struct_code)
            }
            Self::V2017 => VertexFormat2017::from_value(format).map(VertexFormat2017::struct_code),
            Self::V2019 => VertexFormat::from_value(format).map(VertexFormat::struct_code),
        }
    }
}

/// `MeshHandler.get_channel_dtype` (`helpers/MeshHelper.py:451-462`): the
/// struct code of a channel's `format` under `version`'s table, or
/// UnityPy's `ValueError` for a format the table's enum lacks.
pub fn get_channel_dtype(version: Version, format: u8) -> Result<StructCode, MeshError> {
    let table = FormatTable::for_major(version.major()?);
    table
        .struct_code(format)
        .ok_or(MeshError::InvalidFormat { format, table })
}

/// `MeshHandler.get_channel_component_size`
/// (`helpers/MeshHelper.py:464-466`): the bytes of one component of a
/// channel in `format`.
pub fn get_channel_component_size(version: Version, format: u8) -> Result<u32, MeshError> {
    get_channel_dtype(version, format).map(StructCode::size)
}

/// `MeshTopology` (`enums/MeshTopology.py:4-10`), a `SubMesh`'s
/// `topology`, which `get_triangles` compares with
/// (`helpers/MeshHelper.py:646`, `:651`, `:668`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MeshTopology {
    /// 0.
    Triangles = 0,
    /// 1.
    TriangleStrip = 1,
    /// 2.
    Quads = 2,
    /// 3.
    Lines = 3,
    /// 4.
    LineStrip = 4,
    /// 5.
    Points = 5,
}

const _: () = crate::send_sync::<MeshTopology>();

impl MeshTopology {
    /// Every member, in UnityPy's order.
    pub const ALL: [Self; 6] = [
        Self::Triangles,
        Self::TriangleStrip,
        Self::Quads,
        Self::Lines,
        Self::LineStrip,
        Self::Points,
    ];

    /// The member's value.
    #[must_use]
    pub const fn value(self) -> i64 {
        self as i64
    }

    /// The member of this value.
    #[must_use]
    pub fn from_value(value: i64) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.value() == value)
    }
}

/// `GfxPrimitiveType` (`enums/GfxPrimitiveType.py:4-10`), the same values
/// under the engine's names; nothing in UnityPy reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(non_camel_case_types)]
pub enum GfxPrimitiveType {
    /// 0.
    kPrimitiveTriangles = 0,
    /// 1.
    kPrimitiveTriangleStrip = 1,
    /// 2.
    kPrimitiveQuads = 2,
    /// 3.
    kPrimitiveLines = 3,
    /// 4.
    kPrimitiveLineStrip = 4,
    /// 5.
    kPrimitivePoints = 5,
}

const _: () = crate::send_sync::<GfxPrimitiveType>();

impl GfxPrimitiveType {
    /// Every member, in UnityPy's order.
    pub const ALL: [Self; 6] = [
        Self::kPrimitiveTriangles,
        Self::kPrimitiveTriangleStrip,
        Self::kPrimitiveQuads,
        Self::kPrimitiveLines,
        Self::kPrimitiveLineStrip,
        Self::kPrimitivePoints,
    ];

    /// The member's value.
    #[must_use]
    pub const fn value(self) -> i64 {
        self as i64
    }

    /// The member of this value.
    #[must_use]
    pub fn from_value(value: i64) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.value() == value)
    }
}
