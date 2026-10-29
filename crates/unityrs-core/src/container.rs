//! The AssetBundle container (`files/SerializedFile.py:316-324`,
//! `classes/generated.py` `AssetBundle`, `environment.py:207-224`).
//!
//! A `SerializedFile`'s container is the `m_Container` of the first
//! AssetBundle object in dict order, as `(path, AssetInfo)` pairs in file
//! order, repeated paths kept (the dump's L1 `container` rows). Each row
//! points at its asset with `asset.m_FileID`/`m_PathID`; `preload_index` and
//! `preload_size` index `m_PreloadTable`, which the environment's container
//! preload walks (plan section 3.13).
//!
//! Owner: package D.

// STUB(D): delete this allow when the module is implemented.
#![allow(
    clippy::todo,
    clippy::needless_pass_by_value,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    unused_variables
)]

use std::sync::Arc;

use crate::env::PPtr;
use crate::error::Error;
use crate::files::SerializedFile;
use crate::raw::RawStr;

/// One container row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerRow {
    /// The container path, as stored.
    pub path: RawStr,
    /// `AssetInfo.preloadIndex`.
    pub preload_index: i32,
    /// `AssetInfo.preloadSize`.
    pub preload_size: i32,
    /// `AssetInfo.asset.m_FileID`.
    pub file_id: i32,
    /// `AssetInfo.asset.m_PathID`.
    pub path_id: i64,
}

impl ContainerRow {
    /// The row's asset pointer.
    pub fn asset(&self) -> PPtr {
        PPtr {
            file_id: self.file_id,
            path_id: self.path_id,
        }
    }
}

/// A file's container.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Container {
    /// Rows in file order.
    pub rows: Box<[ContainerRow]>,
    /// `m_PreloadTable`, when the AssetBundle has one.
    pub preload_table: Option<Box<[PPtr]>>,
}

impl Container {
    /// Whether the file has no container rows.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Reads the container of the first AssetBundle in dict order with
/// `check_read`. A file without one has an empty container; a failing read
/// is an error (it already failed the file at load under
/// [`crate::FileContext::eager_asset_bundle`]).
pub fn read_container(file: &Arc<SerializedFile>) -> Result<Container, Error> {
    todo!()
}
