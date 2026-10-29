//! `PPtr` and its dereference (`classes/PPtr.py:15-107`).
//!
//! The rules, in UnityPy's order:
//! 1. `m_PathID == 0` raises `ValueError`.
//! 2. `m_FileID == 0` is the pointer's own file.
//! 3. Else `externals[m_FileID - 1]`, with Python indexing: the guard is only
//!    `external_id >= len`, so a negative `m_FileID` counts from the end, and
//!    below `-len` is an `IndexError`.
//! 4. A file with no container (`parent is None`) raises `FileNotFoundError`
//!    ("no container is set") even when the target is loaded: UnityPy does
//!    not consult the environment first.
//! 5. The external path is cleaned ([`crate::pystr::clean_external_path`]),
//!    compared with each sibling entry's name lower-cased (no basename), then
//!    looked up with `find_file`.
//! 6. The target's `objects[m_PathID]`: a missing id is a `KeyError`; a raw
//!    target has no `objects` (`AttributeError`).
//!
//! Owner: package D. The types are frozen.

use crate::error::Error;
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

/// Why a dereference failed, with the exception UnityPy raises.
#[derive(Clone, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DerefError {
    /// `ValueError`: `m_PathID == 0`.
    #[error("PPtr can't deref with m_PathID == 0!")]
    Null,
    /// `FileNotFoundError` or `IndexError`: no external for `m_FileID`.
    #[error("Failed to resolve pointer - invalid m_FileID {file_id}")]
    InvalidFileId {
        /// `m_FileID`.
        file_id: i32,
    },
    /// `FileNotFoundError`: the file is not in a container.
    #[error("PPtr points to {path} but no container is set!")]
    NoContainer {
        /// The external's path, as stored.
        path: RawStr,
    },
    /// `FileNotFoundError`: no sibling, CAB or resolver hit.
    #[error("Failed to resolve pointer - {path} not found!")]
    NotFound {
        /// The external's path, as stored.
        path: RawStr,
    },
    /// `AttributeError`: the target is a raw file.
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
    /// Loading the target failed.
    #[error(transparent)]
    Load(#[from] Error),
}
