// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TextureSwizzler.py's is_switch_swizzled, and export/Texture2DConverter.py's XBOX_SWAP_FORMATS and the order parse_image_data reads a texture's fields in. See THIRD_PARTY_LICENSES.
//! Which textures are exported, and which are skipped with which reason
//! (`docs/plans/extract.md` E66, `docs/BATCH_SPEC.md` section 3): a pure
//! function of a `Texture2D`'s fields, [`select()`].
//!
//! It lives here, beside the decoders, because its rules are UnityPy's
//! texture rules (`TextureSwizzler.is_switch_swizzled`,
//! `Texture2DConverter.XBOX_SWAP_FORMATS`, the `TextureFormat` names) and
//! must agree with what the decoders accept: a texture [`select()`] exports is
//! one [`crate::decode()`] decodes as UnityPy does, and a skipped one is never
//! decoded as another format. It exports the five formats version 1 exports
//! ([`Format::EXPORTED`]): the other formats [`crate::decode()`] also
//! decodes, the rest of Pillow's raw and `bcn` formats and the ETC and EAC
//! formats, are `unsupported_format` to it, as `docs/BATCH_SPEC.md` section
//! 3 says, until a spec version exports them. The engine reads the fields
//! (the object's `m_TextureFormat`, `m_Width`, `m_Height` and
//! `m_PlatformBlob`, and the platform of the SerializedFile that holds it,
//! as `get_image_from_texture2d` reads them,
//! `export/Texture2DConverter.py:237-251`) and records the skip.
//!
//! **Order.** Where more than one skip applies, the first of: a side of 0
//! (`empty_image`), because `parse_image_data` returns its empty image
//! before it looks at the format or the platform
//! (`export/Texture2DConverter.py:265-266`), so the oracle records
//! `empty_image` for such a texture in any format; then the format
//! (`unsupported_format`); then the platform (`unsupported_platform`). A
//! texture is on one platform, so the Switch and Xbox 360 skips never meet.
//! A negative side is no skip: UnityPy raises for it while decoding
//! (Pillow's "Width and height must be >= 0", or a crop or tile error), and
//! [`crate::decode()`] returns [`crate::DecodeError::NegativeSize`].
//!
//! **What it does not read.** `Texture2D.image` fetches the data before
//! `parse_image_data` sees the size (`get_image_data`,
//! `classes/legacy_patch/Texture2D.py:63-75`: the object's `image_data`, else
//! what its `m_StreamData` names), and raises there for a texture with
//! neither, or whose resource file is not loaded. A side of 0 is
//! `empty_image` without any fetch (`docs/BATCH_SPEC.md` section 3): the
//! extractor skips a zero width before it reads the image, and the oracle
//! records a zero height's raise as `empty_image` (`docs/plans/extract.md`
//! section 5). [`select()`] reads no data, so for any other texture the
//! fetch, and its failure, are the engine's to record.

use std::fmt;

use crate::format::{Format, texture_format_name};

/// `BuildTarget.XBOX360` (`enums/BuildTarget.py:16`).
pub const XBOX360: i32 = 11;
/// `BuildTarget.Switch` (`enums/BuildTarget.py:39`).
pub const SWITCH: i32 = 38;

/// The formats UnityPy byte-swaps on Xbox 360: `RGB565`, `DXT1`,
/// `DXT1Crunched`, `DXT5`, `DXT5Crunched`
/// (`export/Texture2DConverter.py:569`). Of the five exported, DXT1 and
/// DXT5.
pub const XBOX_SWAP_FORMATS: [i32; 5] = [7, 10, 28, 12, 29];

/// What [`select()`] reads of a `Texture2D`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureFields<'a> {
    /// `m_TextureFormat`.
    pub texture_format: i32,
    /// `m_Width`.
    pub width: i32,
    /// `m_Height`.
    pub height: i32,
    /// The target platform of the SerializedFile that holds the texture
    /// (`object_reader.platform`, a `BuildTarget` value).
    pub platform: i32,
    /// `m_PlatformBlob`'s bytes, or `None` where the texture has no such
    /// field (UnityPy's `getattr(..., None)`); an empty blob reads as `None`
    /// does.
    pub platform_blob: Option<&'a [u8]>,
}

const _: () = crate::send_sync::<TextureFields<'static>>();

/// What [`select()`] decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    /// Decode it as this format.
    Export(Format),
    /// Skip it for this reason.
    Skip(Skip),
}

const _: () = crate::send_sync::<Selection>();

/// Why a texture is skipped (`docs/BATCH_SPEC.md` section 3). Its
/// [`Display`](fmt::Display) form is the manifest's reason
/// (`docs/BATCH_SPEC.md` 9.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// A side of 0: `empty_image`.
    EmptyImage,
    /// A format other than the five ([`Format::EXPORTED`]), by its
    /// `TextureFormat` value:
    /// `unsupported_format(<name>)`, UnityPy's member name, or the decimal
    /// value where its enum has none.
    UnsupportedFormat(i32),
    /// A texture UnityPy would deswizzle or byte-swap:
    /// `unsupported_platform(switch | xbox360)`.
    UnsupportedPlatform(SkippedPlatform),
}

const _: () = crate::send_sync::<Skip>();

/// The platform an [`Skip::UnsupportedPlatform`] names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkippedPlatform {
    /// A Switch texture `is_switch_swizzled` holds for.
    Switch,
    /// An Xbox 360 texture of a format in [`XBOX_SWAP_FORMATS`].
    Xbox360,
}

const _: () = crate::send_sync::<SkippedPlatform>();

impl fmt::Display for Skip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyImage => f.write_str("empty_image"),
            Self::UnsupportedFormat(value) => match texture_format_name(*value) {
                Some(name) => write!(f, "unsupported_format({name})"),
                None => write!(f, "unsupported_format({value})"),
            },
            Self::UnsupportedPlatform(SkippedPlatform::Switch) => {
                f.write_str("unsupported_platform(switch)")
            }
            Self::UnsupportedPlatform(SkippedPlatform::Xbox360) => {
                f.write_str("unsupported_platform(xbox360)")
            }
        }
    }
}

/// Whether UnityPy deswizzles a texture as a Switch texture
/// (`TextureSwizzler.is_switch_swizzled`,
/// `helpers/TextureSwizzler.py:121-127`): the platform is Switch, the
/// platform blob holds at least 12 bytes, and `1 << n` is more than one GOB
/// per block, `n` being the little-endian `u32` at bytes 8 to 12 of the blob
/// (`get_switch_gobs_per_block`, `helpers/TextureSwizzler.py:117-118`), so
/// whenever `n` is not 0.
#[must_use]
pub fn is_switch_swizzled(platform: i32, platform_blob: Option<&[u8]>) -> bool {
    if platform != SWITCH {
        return false;
    }
    match platform_blob.and_then(|blob| blob.get(8..12)) {
        Some(gobs) => gobs.iter().any(|&b| b != 0),
        None => false,
    }
}

/// Whether a texture is exported, and as what, or skipped, and why
/// (module docs).
#[must_use]
pub fn select(texture: &TextureFields<'_>) -> Selection {
    if texture.width == 0 || texture.height == 0 {
        return Selection::Skip(Skip::EmptyImage);
    }
    // Version 1 exports the five of E15; the other formats this crate also
    // decodes are `unsupported_format` to it (`docs/BATCH_SPEC.md` section
    // 3).
    let Some(format) = Format::from_texture_format(texture.texture_format).filter(|f| f.exported())
    else {
        return Selection::Skip(Skip::UnsupportedFormat(texture.texture_format));
    };
    if is_switch_swizzled(texture.platform, texture.platform_blob) {
        return Selection::Skip(Skip::UnsupportedPlatform(SkippedPlatform::Switch));
    }
    if texture.platform == XBOX360 && XBOX_SWAP_FORMATS.contains(&texture.texture_format) {
        return Selection::Skip(Skip::UnsupportedPlatform(SkippedPlatform::Xbox360));
    }
    Selection::Export(format)
}
