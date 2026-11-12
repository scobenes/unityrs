// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/SpriteHelper.py's SpriteSettings, get_image, get_image_from_sprite, mask_sprite, render_sprite_mesh and copy_triangle. See THIRD_PARTY_LICENSES.
//! Sprites, rendered as UnityPy 1.25.2's `SpriteHelper` renders them
//! (`docs/plans/extract.md` E17; `PARITY.md` section 15.2), pixel for pixel,
//! over a sheet [`crate::decode()`] gave.
//!
//! - [`Sheet::new`]: the sprite's texture, and its alpha texture merged in
//!   where it has one (`get_image`, `export/SpriteHelper.py:44-59`).
//! - [`select_render_data`]: the render data `get_image_from_sprite`
//!   renders from (`:63-86`), the Sprite's own or its atlas's.
//! - [`render()`]: one sprite's image from its sheet, its render data
//!   ([`SpriteData`]) and, for a tight sprite, its mesh
//!   (`get_image_from_sprite`, `:62-127`).
//!
//! **What `get_image_from_sprite` does, and the order here.** It crops the
//! sheet by `textureRect` (Pillow's crop: the box's edges compared, then
//! each rounded half to even, then the decompression-bomb check, then C
//! `int`s; [`canvas::crop_box`]); reads the settings (a rotation from 5 to
//! 15 raises, [`Settings::parse`]); when packed, flips or turns the crop
//! (`:100-113`); for tight packing, processes the sprite's own `m_RD` mesh
//! (`:116-118`) and, when its `m_UV0` holds a non-zero value, renders the
//! first submesh's triangles from the whole sheet (`render_sprite_mesh`,
//! `:173-224`), and otherwise masks the crop by every submesh's triangles
//! (`mask_sprite`, `:130-170`); and flips the result top to bottom
//! (`:127`). Every check UnityPy makes raises here, as [`SpriteError`],
//! in UnityPy's order, within the bounds below. One step is not taken: the
//! crop is made only where a later step reads it (rectangle packing, and
//! the mask), since the mesh path replaces it unread; its checks are made
//! wherever UnityPy makes them, Pillow's width cap on it and on its turned
//! size included ([`SpriteError::ImageTooWide`]).
//!
//! **Orientation.** UnityPy decodes the sheet unflipped
//! (`get_image_from_texture2d(texture, False)`, `:47-58`), and
//! [`crate::decode()`] returns a texture flipped, as `Texture2D.image`
//! gives it, so a [`Sheet`] reads its rows from the last.
//!
//! **Divergences** (`PARITY.md` sections 19.2 and 19.3). A triangle whose
//! system meets an exactly zero pivot in `numpy`'s LU on the pinned kernel
//! copies nothing, where `numpy` raises `LinAlgError` (Tier C, [`affine`]);
//! three collinear destination corners need not make one, and where they
//! do not, the triangle is copied as UnityPy copies it. A canvas of more
//! than [`crate::MAX_PIXELS`] pixels is [`SpriteError::TooLarge`], where
//! Pillow's behaviour is its memory's. A field the render reads that is
//! missing or of another type is [`SpriteError::Malformed`] (Tier U). Every
//! buffer is charged to the caller's meter before it is allocated, and
//! every loop's work counted on it before it runs (`docs/plans/extract.md`
//! E59): an image's bytes and its rows, an empty image's rows included,
//! and of the fields [`SpriteFields::read`] and [`select_render_data`]
//! read, each atlas, map entry, field walked and tag, a name's comparison
//! by its bytes ([`fields`]); a refusal is [`SpriteError::Charge`].

pub mod affine;
mod canvas;
mod exact;
pub mod fields;
mod raster;
mod render;

use std::sync::Arc;

use unityrs_core::limits::{ChargeError, Meter};
use unityrs_meshdecode::{MeshError, Processed};

use crate::contain::Payload;
use crate::decode::{DecodeError, Image};

pub use canvas::Sheet;
pub use fields::{
    AtlasKey, Guid, Pptr, RenderData, SpriteFields, atlas_render_data, select_render_data,
};

/// What [`render()`] reads of a sprite: its render data's rectangle and
/// settings (the Sprite's `m_RD`, or the atlas's `SpriteAtlasData` for
/// it), and the Sprite's `m_PixelsToUnits`, as Python `float`s and `int`s.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpriteData {
    /// `textureRect`: `x`, `y`, `width`, `height`.
    pub texture_rect: [f64; 4],
    /// `settingsRaw`.
    pub settings_raw: u64,
    /// `m_PixelsToUnits`.
    pub pixels_to_units: f64,
}

const _: () = crate::send_sync::<SpriteData>();

/// `SpritePackingMode` (`enums/SpritePackingMode.py`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PackingMode {
    /// `kSPMTight`, 0.
    Tight,
    /// `kSPMRectangle`, 1.
    Rectangle,
}

/// `SpritePackingRotation` (`enums/SpritePackingRotation.py`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rotation {
    /// `kSPRNone`, 0.
    None,
    /// `kSPRFlipHorizontal`, 1: Pillow's `FLIP_LEFT_RIGHT`.
    FlipHorizontal,
    /// `kSPRFlipVertical`, 2: `FLIP_TOP_BOTTOM`.
    FlipVertical,
    /// `kSPRRotate180`, 3: `ROTATE_180`.
    Rotate180,
    /// `kSPRRotate90`, 4: `ROTATE_270`, a quarter turn clockwise.
    Rotate90,
}

/// `SpriteSettings` (`export/SpriteHelper.py:29-40`): `settingsRaw`'s bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Settings {
    /// Bit 0.
    pub packed: bool,
    /// Bit 1.
    pub packing_mode: PackingMode,
    /// Bits 2 to 5.
    pub packing_rotation: Rotation,
    /// Bit 6, `SpriteMeshType`: 0 `kSMTFullRect`, 1 `kSMTTight`, which
    /// rendering does not read.
    pub mesh_type: u8,
}

const _: () = crate::send_sync::<Settings>();
const _: () = crate::send_sync::<PackingMode>();
const _: () = crate::send_sync::<Rotation>();

impl Settings {
    /// `SpriteSettings(settings_raw)`: a rotation from 5 to 15 is no
    /// `SpritePackingRotation`, [`SpriteError::Rotation`]; the bits past 6
    /// are ignored (`:35-40`).
    pub fn parse(raw: u64) -> Result<Self, SpriteError> {
        let packing_mode = if (raw >> 1) & 1 == 0 {
            PackingMode::Tight
        } else {
            PackingMode::Rectangle
        };
        let rotation = (raw >> 2) & 0xF;
        let packing_rotation = match rotation {
            0 => Rotation::None,
            1 => Rotation::FlipHorizontal,
            2 => Rotation::FlipVertical,
            3 => Rotation::Rotate180,
            4 => Rotation::Rotate90,
            // Masked to four bits: 5 to 15.
            other => return Err(SpriteError::Rotation(other)),
        };
        Ok(Self {
            packed: raw & 1 == 1,
            packing_mode,
            packing_rotation,
            mesh_type: u8::from((raw >> 6) & 1 == 1),
        })
    }
}

/// Why a sprite did not render. Each variant but [`Self::Malformed`],
/// [`Self::AtlasUnresolved`], [`Self::TooLarge`], [`Self::Charge`] and
/// [`Self::Panicked`] is a case where UnityPy 1.25.2 raises;
/// [`Self::python`] gives its exception's class and message.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SpriteError {
    /// Pillow's crop of `textureRect`: its right edge before its left
    /// (`ValueError`), checked before the lower edge.
    #[error("Coordinate 'right' is less than 'left'")]
    CropRightLeft,
    /// Its lower edge above its upper (`ValueError`).
    #[error("Coordinate 'lower' is less than 'upper'")]
    CropLowerUpper,
    /// `round()` of a NaN (`ValueError`).
    #[error("cannot convert float NaN to integer")]
    RoundNan,
    /// `round()` of an infinity (`OverflowError`).
    #[error("cannot convert float infinity to integer")]
    RoundInfinity,
    /// Pillow's crop of more than [`crate::MAX_CROP_PIXELS`] pixels
    /// (`DecompressionBombError`).
    #[error(
        "Image size ({pixels} pixels) exceeds limit of 178956970 pixels, could be decompression bomb DOS attack."
    )]
    DecompressionBomb {
        /// The crop's pixels, exactly, in decimal, as Python counts them.
        pixels: String,
    },
    /// A box edge that `PyArg_ParseTuple`'s `i` refuses within a C `long`
    /// (`OverflowError`): past `INT_MAX` when `above`, else below
    /// `INT_MIN`.
    #[error("signed integer is {}", if *above { "greater than maximum" } else { "less than minimum" })]
    CIntOverflow {
        /// Above `INT_MAX`, else below `INT_MIN`.
        above: bool,
    },
    /// A box edge past a C `long`, which `PyLong_AsLong` refuses before
    /// `i` sees it (`OverflowError`).
    #[error("Python int too large to convert to C long")]
    CLongOverflow,
    /// An image Pillow refuses to make: wider than `INT_MAX / 4 - 1`
    /// pixels, whatever its height (`ImagingNewPrologueSubtype`'s
    /// `MemoryError`, which has no message): a crop, its turned size, a
    /// mesh's canvas, a translated triangle's crop.
    #[error("Pillow refuses an image wider than 536870910 pixels")]
    ImageTooWide,
    /// `numpy.linalg.solve` of a source corner outside `[-2^63, 2^64)`,
    /// which `numpy` holds in an `object` array that its `solve` cannot
    /// cast (`UFuncTypeError`).
    #[error(
        "Cannot cast ufunc 'solve1' input 1 from dtype('O') to dtype('float64') with casting rule 'same_kind'"
    )]
    SolveCast,
    /// `SpritePackingRotation(n)` of 5 to 15 (`ValueError`).
    #[error("{0} is not a valid SpritePackingRotation")]
    Rotation(u64),
    /// The alpha texture's decoded size is not the texture's: `Image.merge`'s
    /// `ValueError`.
    #[error("size mismatch")]
    AlphaSizeMismatch,
    /// `mask_sprite`'s `assert positions` (`AssertionError`).
    #[error("No vertices found in sprite mesh!")]
    NoVertices,
    /// A tuple unpacked into `expected` names (`ValueError`): `got` fewer,
    /// or `None` for more.
    #[error("{}", unpack_message(*expected, *got))]
    Unpack {
        /// The names.
        expected: usize,
        /// The tuple's length, when shorter.
        got: Option<usize>,
    },
    /// An index past a list (`IndexError` "list index out of range"), or
    /// past a tuple when `tuple` ("tuple index out of range").
    #[error("{} index out of range", if *tuple { "tuple" } else { "list" })]
    Index {
        /// A tuple's, else a list's.
        tuple: bool,
    },
    /// `render_sprite_mesh`: every axis of the vertices varies
    /// (`ValueError`).
    #[error("Can't process 3d sprites!")]
    Sprite3d,
    /// `render_sprite_mesh`: no submesh with vertices (`ValueError`).
    #[error("No triangles found in mesh!")]
    NoTriangles,
    /// `Image.new` of a negative side (`ValueError`).
    #[error("Width and height must be >= 0")]
    NegativeSize,
    /// The atlas holds no render data for the sprite's key: the
    /// `StopIteration` of `next()` (`:78`).
    #[error("the atlas has no render data for the sprite's m_RenderDataKey")]
    NoAtlasData,
    /// The Sprite's `m_SpriteAtlas` does not resolve: the caller found no
    /// object for it, where UnityPy's `deref_parse_as_object` raises its
    /// own error (Tier U).
    #[error("the sprite's m_SpriteAtlas does not resolve")]
    AtlasUnresolved,
    /// A field the render reads is missing, or not of the type its
    /// typetree gives it, where UnityPy's behaviour is not characterised
    /// (Tier U): named by its path.
    #[error("the sprite's {0} is missing or of another type")]
    Malformed(&'static str),
    /// The mesh's own error (`unityrs_meshdecode`).
    #[error(transparent)]
    Mesh(#[from] MeshError),
    /// A canvas of more than [`crate::MAX_PIXELS`] pixels: the bound
    /// UnityPy does not have (`PARITY.md` section 19.3).
    #[error("the sprite's {what} is past the renderer's bound")]
    TooLarge {
        /// What: `"canvas"`, every image the render makes.
        what: &'static str,
    },
    /// A buffer's charge or a loop's work, refused by the caller's meter.
    #[error(transparent)]
    Charge(#[from] ChargeError),
    /// A panic in the render, caught ([`crate::contain()`]).
    #[error("the sprite's render panicked: {}", .0.message())]
    Panicked(Payload),
}

const _: () = crate::send_sync::<SpriteError>();

fn unpack_message(expected: usize, got: Option<usize>) -> String {
    match got {
        Some(n) => format!("not enough values to unpack (expected {expected}, got {n})"),
        None => format!("too many values to unpack (expected {expected})"),
    }
}

impl SpriteError {
    /// The class and message of the exception UnityPy raises where this
    /// error is returned, or `None` where it raises none (a bound, a
    /// charge, a panic, a Tier U refusal) or the mesh's error has none.
    #[must_use]
    pub fn python(&self) -> Option<(&'static str, String)> {
        let class = match self {
            Self::CropRightLeft
            | Self::CropLowerUpper
            | Self::RoundNan
            | Self::Rotation(_)
            | Self::Unpack { .. }
            | Self::Sprite3d
            | Self::NoTriangles
            | Self::NegativeSize
            | Self::AlphaSizeMismatch => "ValueError",
            Self::RoundInfinity | Self::CIntOverflow { .. } | Self::CLongOverflow => {
                "OverflowError"
            }
            Self::DecompressionBomb { .. } => "DecompressionBombError",
            Self::NoVertices => "AssertionError",
            Self::Index { .. } => "IndexError",
            Self::SolveCast => "UFuncTypeError",
            // `PyErr_NoMemory()`'s, which has no message.
            Self::ImageTooWide => return Some(("MemoryError", String::new())),
            // `next()`'s `StopIteration`, which has no message.
            Self::NoAtlasData => return Some(("StopIteration", String::new())),
            Self::Mesh(e) => return e.python(),
            Self::Malformed(_)
            | Self::AtlasUnresolved
            | Self::TooLarge { .. }
            | Self::Charge(_)
            | Self::Panicked(_) => {
                return None;
            }
        };
        Some((class, self.to_string()))
    }
}

impl From<DecodeError> for SpriteError {
    /// [`crate::contain()`]'s errors: a panic, or a charge.
    fn from(e: DecodeError) -> Self {
        match e {
            DecodeError::Panicked { payload } => Self::Panicked(payload),
            DecodeError::Charge(c) => Self::Charge(c),
            // `contain` returns no other; a check of the decode's is no
            // render's.
            _ => Self::Malformed("decode"),
        }
    }
}

/// One sprite's image (module docs): `sheet` is the render data's texture
/// (and alpha texture), `data` its rectangle and settings with the
/// Sprite's `m_PixelsToUnits`, and `mesh` processes the Sprite's own
/// `m_RD` (`MeshHandler(m_Sprite.m_RD, version).process()`), called only
/// for a tight sprite, where UnityPy calls it. The image is charged to
/// `meter`, and every buffer and loop on the way; the pixel work runs
/// under [`crate::contain()`].
pub fn render<'p>(
    sheet: &Sheet<'_>,
    data: &SpriteData,
    mesh: impl FnOnce() -> Result<Processed<'p>, MeshError>,
    meter: &Arc<Meter>,
) -> Result<Image, SpriteError> {
    render::render(sheet, data, mesh, meter)
}

/// Runs a pixel loop over buffers already charged under
/// [`crate::contain()`]: a panic in it is [`SpriteError::Panicked`].
pub(crate) fn contained<T>(f: impl FnOnce() -> T) -> Result<T, SpriteError> {
    crate::contain(std::panic::AssertUnwindSafe(|| Ok(f()))).map_err(SpriteError::from)
}
