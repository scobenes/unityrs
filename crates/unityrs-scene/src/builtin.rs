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
impl BuiltinFile {
    /// Recognise the `ntpath` basename and ASCII case, shared with exporters.
    #[must_use]
    pub fn from_path(path: &[u8]) -> Option<Self> {
        let base = unityrs_core::pystr::ntpath_basename(path);
        [Self::DefaultResources, Self::BuiltinExtra]
            .into_iter()
            .find(|file| base.eq_ignore_ascii_case(file.name().as_bytes()))
    }
    /// Canonical resource name for lookup and metadata.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::DefaultResources => "unity default resources",
            Self::BuiltinExtra => "unity_builtin_extra",
        }
    }
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
impl BuiltinKind {
    /// Unity's conventional built-in name; ids remain provisional.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Cube => "Cube",
            Self::Cylinder => "Cylinder",
            Self::Sphere => "Sphere",
            Self::Capsule => "Capsule",
            Self::Plane => "Plane",
            Self::Quad => "Quad",
            Self::DefaultMaterial => "Default-Material",
        }
    }
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
    let Some(file) = BuiltinFile::from_path(ext.path.as_bytes()) else {
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

#[cfg(test)]
mod tests {
    use super::BuiltinFile;

    // Basenames checked with ntpath.basename in unityrs-dev (UnityPy 1.25.2).
    #[test]
    fn drive_relative_resource_names_are_recognized() {
        for (path, expected) in [
            (
                b"C:unity default resources".as_slice(),
                BuiltinFile::DefaultResources,
            ),
            (b"c:UNITY_BUILTIN_EXTRA", BuiltinFile::BuiltinExtra),
        ] {
            assert_eq!(BuiltinFile::from_path(path), Some(expected), "{path:?}");
        }
    }

    #[test]
    fn unc_share_roots_are_not_resource_basenames() {
        // Both share roots have an empty basename; a file below a share does not.
        for path in [
            b"//server/unity default resources".as_slice(),
            b"\\\\server\\unity_builtin_extra",
        ] {
            assert_eq!(BuiltinFile::from_path(path), None, "{path:?}");
        }
        assert_eq!(
            BuiltinFile::from_path(b"//server/share/unity default resources"),
            Some(BuiltinFile::DefaultResources)
        );
    }
}
