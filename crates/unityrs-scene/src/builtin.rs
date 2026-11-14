//! Recognition only. The ids in the accepted plan remain provisional
//! until Editor fixtures confirm them; resolution/generation modes are
//! separate export work (`PARITY.md` section 24).
use crate::SceneError;
use std::sync::Arc;
use unityrs_core::RawStr;
use unityrs_core::env::{PPtr, pptr::external_index};
use unityrs_core::files::SerializedFile;
use unityrs_core::limits::Meter;

/// The two recognised Unity resource files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinFile {
    DefaultResources,
    BuiltinExtra,
}
/// Known built-in ids; unknown ids in these files are still recognised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinKind {
    Cube,
    Cylinder,
    Sphere,
    Capsule,
    Plane,
    Quad,
    DefaultMaterial,
}
/// A built-in pointer, with its original external path and id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltinRef {
    pub file: BuiltinFile,
    pub path: RawStr,
    pub path_id: i64,
    pub kind: Option<BuiltinKind>,
}
/// Match the external's basename, case-insensitively, accepting either
/// slash as `ntpath` does. Negative file ids retain core's indexing rule.
pub fn recognize(
    from: &SerializedFile,
    ptr: PPtr,
    meter: &Arc<Meter>,
) -> Result<Option<BuiltinRef>, SceneError> {
    if ptr.file_id == 0 || ptr.path_id == 0 {
        return Ok(None);
    }
    let Some(ext) = external_index(ptr.file_id, from.externals().len())
        .ok()
        .and_then(|i| from.externals().get(i))
    else {
        return Ok(None);
    };
    meter.count_decode((ext.path.len() as u64).saturating_mul(3))?;
    let base = ext
        .path
        .as_bytes()
        .rsplit(|b| matches!(b, b'/' | b'\\'))
        .next()
        .unwrap_or_default();
    let file = if base.eq_ignore_ascii_case(b"unity default resources") {
        BuiltinFile::DefaultResources
    } else if base.eq_ignore_ascii_case(b"unity_builtin_extra") {
        BuiltinFile::BuiltinExtra
    } else {
        return Ok(None);
    };
    let kind = match (file, ptr.path_id) {
        (BuiltinFile::DefaultResources, 10202) => Some(BuiltinKind::Cube),
        (BuiltinFile::DefaultResources, 10206) => Some(BuiltinKind::Cylinder),
        (BuiltinFile::DefaultResources, 10207) => Some(BuiltinKind::Sphere),
        (BuiltinFile::DefaultResources, 10208) => Some(BuiltinKind::Capsule),
        (BuiltinFile::DefaultResources, 10209) => Some(BuiltinKind::Plane),
        (BuiltinFile::DefaultResources, 10210) => Some(BuiltinKind::Quad),
        (BuiltinFile::BuiltinExtra, 10303) => Some(BuiltinKind::DefaultMaterial),
        _ => None,
    };
    Ok(Some(BuiltinRef {
        file,
        path: ext.path.clone(),
        path_id: ptr.path_id,
        kind,
    }))
}
const _: () = crate::send_sync::<BuiltinFile>();
const _: () = crate::send_sync::<BuiltinKind>();
const _: () = crate::send_sync::<BuiltinRef>();
