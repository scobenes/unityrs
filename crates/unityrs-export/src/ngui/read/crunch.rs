// Ported from UnityPy 1.25.2 (MIT): export/Texture2DConverter.py's get_image_from_texture2d and parse_image_data, classes/legacy_patch/Texture2D.py's get_image_data, and helpers/UnityVersion.py's tuple conversion. See THIRD_PARTY_LICENSES.
//! Version selection after resource retrieval, shared by every texture output.

use unityrs_core::files::SerializedFile;
use unityrs_texture::Format;

use super::Texture;
use crate::ngui::classify::{self, Class};

/// Called after resource retrieval: `get_image_from_texture2d` evaluates
/// `get_image_data` first (export/Texture2DConverter.py:237-251), which returns
/// inline data, retrieves the stream, or raises for missing data
/// (classes/legacy_patch/Texture2D.py:63-75). `parse_image_data` returns for a
/// zero side before rejecting empty data (export/Texture2DConverter.py:265-269).
/// Only its Crunched branch indexes the version, including ETC (:297-306).
pub(in crate::ngui) fn data_format(
    t: &Texture,
    file: &SerializedFile,
    format: Format,
    data: &[u8],
) -> Result<Format, Class> {
    if t.width == 0 || t.height == 0 {
        return Ok(format);
    }
    if data.is_empty() {
        return Err(classify::decode(&unityrs_texture::DecodeError::NoImageData));
    }
    if !format.is_crunched() {
        return Ok(format);
    }
    // helpers/UnityVersion.py:99-107 constructs every tuple field, including
    // type's enum conversion (:41-42). from_list (:56-60) packs fields by OR,
    // allowing a wide type_number to spill into an invalid type byte.
    let version = file
        .version()
        .map(|v| {
            v.version
                .as_tuple()
                .map(|(major, minor, ..)| (major, minor))
                .map_err(|e| classify::error(&unityrs_core::Error::Version(e)))
        })
        .transpose()?
        .unwrap_or((u16::MAX, u16::MAX));
    // The caller already checked its allowlist; use the same selector with
    // the owning file's version now that the data checks have succeeded.
    super::formats(
        t,
        file.target_platform()
            .unwrap_or(crate::ngui::attempt::UNKNOWN_PLATFORM),
        unityrs_texture::Formats::DECODED,
        version,
    )
}
