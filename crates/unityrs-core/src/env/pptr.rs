// Ported from UnityPy 1.25.2 (MIT): UnityPy/classes/PPtr.py, UnityPy/environment.py, UnityPy/files/File.py, UnityPy/helpers/ContainerHelper.py. See THIRD_PARTY_LICENSES.
//! `PPtr` and its dereference (`classes/PPtr.py:15-107`).
//!
//! The rules, in UnityPy's order:
//! 1. `m_PathID == 0` raises `ValueError`.
//! 2. `m_FileID == 0` is the pointer's own file.
//! 3. Else `externals[m_FileID - 1]` ([`external_index`]): computed in `i64`
//!    so `i32::MIN - 1` does not wrap; the guard is only `external_id >= len`
//!    (`FileNotFoundError`, [`DerefError::FileIdPastEnd`]), then Python
//!    indexing: a negative `m_FileID` counts from the end, and one below
//!    `-len` is an `IndexError` ([`DerefError::FileIdBelowStart`]). The two
//!    are different exceptions. The container preload
//!    (`helpers/ContainerHelper.py:43-46`, `except (FileNotFoundError,
//!    KeyError): continue`) swallows [`DerefError::FileIdPastEnd`],
//!    [`DerefError::NotFound`] and [`DerefError::MissingObject`], and
//!    propagates [`DerefError::FileIdBelowStart`], [`DerefError::RawTarget`]
//!    (an `AttributeError`), [`DerefError::Null`] (never reached: the
//!    preload skips null pointers first), [`DerefError::ContainerDropped`]
//!    and [`DerefError::Load`].
//! 4. The file's container (`assetsfile.parent`, `PPtr.py:62-66`): a bundle
//!    entry's is its bundle; a file loaded at top level has the
//!    `Environment` that loaded it as its parent (`environment.py:154`,
//!    `files/File.py:36`), so its container is that Environment. UnityPy's
//!    "no container is set" is raised only for a `SerializedFile` built
//!    outside any Environment, which [`super::Environment::deref`] never
//!    sees: the core has no such refusal. (Probe: two loose files `a/CAB-A`
//!    and `b/CAB-B`, A's external `archive:/CAB-B/CAB-B`, resolve to B's
//!    object in UnityPy 1.25.2.) A bundle entry whose bundle has been
//!    dropped (the `Parent`'s weak handle no longer upgrades) has no
//!    container to scan: UnityPy cannot reach that state (an object keeps
//!    `assets_file.parent` alive), so the core does not guess between "no
//!    sibling" and `find_file`, and fails with
//!    [`DerefError::ContainerDropped`].
//! 5. The external path is cleaned ([`crate::pystr::clean_external_path`]) and
//!    compared with each key of the container's `files` dict lower-cased with
//!    `str.lower` (no basename): a bundle's directory paths, or the
//!    Environment's `files` keys, the names given to `load`, dependencies
//!    included. The first equal key wins. Else `find_file` (the CAB registry,
//!    then the resolver), whose miss is `FileNotFoundError`: its `None`, and
//!    the walk's own `FileNotFoundError` (a resolver's
//!    [`super::CabResolver::find_file`] error naming
//!    [`crate::error::NotFound::Cab`]), are [`DerefError::NotFound`].
//! 6. The target's `objects[m_PathID]`: a missing id is a `KeyError`; a raw
//!    target or a container (a bundle under an Environment `files` key) has no
//!    `objects` (`AttributeError`).
//!
//! Owner: package D. The types and [`external_index`] are frozen (the
//! scaffold implements [`external_index`]; `tests/fixtures/scaffold/pptr_external.json`
//! pins it).

use crate::error::Error;
use crate::once::CellError;
use crate::raw::RawStr;

/// A pointer: `m_FileID` and `m_PathID`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PPtr {
    /// `m_FileID`: 0 is the pointer's own file, `n` is `externals[n - 1]`.
    pub file_id: i32,
    /// `m_PathID`: 0 is null.
    pub path_id: i64,
}

impl PPtr {
    /// `__bool__`: whether the pointer is non-null.
    #[must_use]
    pub fn is_set(&self) -> bool {
        self.path_id != 0
    }
}

/// Why `externals[m_FileID - 1]` fails (rule 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExternalIndexError {
    /// `m_FileID - 1 >= len`: `FileNotFoundError` (`"invalid m_FileID"`).
    PastEnd,
    /// `m_FileID - 1 < -len`: Python's `IndexError`.
    BelowStart,
}

/// Rule 3: the index into `externals` that `m_FileID` names, as UnityPy
/// computes it (`PPtr.py:56-60`): `external_id = m_FileID - 1` in a wide
/// integer, refused when `>= len`, then indexed as a Python list (a negative
/// id counts from the end). Callers handle `m_FileID == 0` (rule 2) first.
/// Implemented by the scaffold.
pub fn external_index(file_id: i32, len: usize) -> Result<usize, ExternalIndexError> {
    let id = i64::from(file_id) - 1;
    let len = i64::try_from(len).unwrap_or(i64::MAX);
    if id >= len {
        return Err(ExternalIndexError::PastEnd);
    }
    let i = if id < 0 { id + len } else { id };
    usize::try_from(i).map_err(|_| ExternalIndexError::BelowStart)
}

/// Why a dereference failed, with the exception UnityPy raises.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DerefError {
    /// `ValueError`: `m_PathID == 0`.
    #[error("PPtr can't deref with m_PathID == 0!")]
    Null,
    /// `FileNotFoundError`: `m_FileID - 1 >= len(externals)`.
    #[error("Failed to resolve pointer - invalid m_FileID {file_id}")]
    FileIdPastEnd {
        /// `m_FileID`.
        file_id: i32,
    },
    /// `IndexError`: `m_FileID - 1 < -len(externals)`.
    #[error("list index out of range (m_FileID {file_id})")]
    FileIdBelowStart {
        /// `m_FileID`.
        file_id: i32,
    },
    /// `FileNotFoundError`: no sibling, CAB or resolver hit.
    #[error("Failed to resolve pointer - {path} not found!")]
    NotFound {
        /// The external's path, as stored.
        path: RawStr,
    },
    /// `AttributeError`: the target is a raw file, or a container under an
    /// Environment `files` key, neither of which has `objects`.
    #[error("pointer target {name} is not a SerializedFile")]
    RawTarget {
        /// The target's name.
        name: RawStr,
    },
    /// `KeyError`: no such object in the target.
    #[error("no object {path_id} in {file}")]
    MissingObject {
        /// The target file's name.
        file: RawStr,
        /// `m_PathID`.
        path_id: i64,
    },
    /// The pointer's file is a bundle entry whose bundle has been dropped,
    /// so its siblings cannot be scanned (rule 4; UnityPy cannot reach this:
    /// a caller that hands out a file keeps its bundle).
    #[error("the bundle {name} of the pointer's file was dropped")]
    ContainerDropped {
        /// The bundle's name ([`crate::files::Parent::name`]).
        name: RawStr,
    },
    /// Loading the target failed.
    #[error(transparent)]
    Load(#[from] Error),
}

impl DerefError {
    /// Whether it depends on what else is held ([`crate::once::CellError`]):
    /// only a load's own transient failure does.
    pub(crate) fn is_transient(&self) -> bool {
        matches!(self, Self::Load(e) if e.is_transient())
    }

    /// Whether the input's own meter refused the load.
    pub(crate) fn is_meter_refusal(&self) -> bool {
        matches!(self, Self::Load(e) if e.is_meter_refusal())
    }

    /// The heap its names (or its load's error) hold.
    pub(crate) fn heap_bytes(&self) -> u64 {
        match self {
            Self::NotFound { path: n }
            | Self::RawTarget { name: n }
            | Self::MissingObject { file: n, .. }
            | Self::ContainerDropped { name: n } => crate::error::heap_of(n.len()),
            Self::Load(e) => e.heap_bytes(),
            Self::Null | Self::FileIdPastEnd { .. } | Self::FileIdBelowStart { .. } => 0,
        }
    }
}
