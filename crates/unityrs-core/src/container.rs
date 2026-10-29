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
//! **Memory.** The rows and the preload table grow in
//! [`crate::limits::MeteredVec`]s on the file's input meter, and the
//! [`Container`] keeps their charge (the AssetBundle's value, read to get
//! them, is dropped after).
//!
//! Owner: package D. [`Container`]'s constructors are the scaffold's.

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
use crate::limits::{Charge, Meter, MeteredVec};
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

/// A file's container, with the charge for its tables.
#[derive(Debug)]
pub struct Container {
    /// Rows in file order.
    pub rows: Box<[ContainerRow]>,
    /// `m_PreloadTable`, when the AssetBundle has one.
    pub preload_table: Option<Box<[PPtr]>>,
    charge: Charge,
}

impl PartialEq for Container {
    fn eq(&self, other: &Self) -> bool {
        self.rows == other.rows && self.preload_table == other.preload_table
    }
}

impl Eq for Container {}

impl Container {
    /// No rows, no preload table.
    pub fn empty(meter: &Arc<Meter>) -> Self {
        Self {
            rows: Box::new([]),
            preload_table: None,
            charge: Charge::zero(meter),
        }
    }

    /// The rows and preload table a read grew on the input's meter.
    pub fn new(rows: MeteredVec<ContainerRow>, preload_table: Option<MeteredVec<PPtr>>) -> Self {
        let (rows, mut charge) = rows.into_parts();
        let preload_table = preload_table.map(|t| {
            let (t, c) = t.into_parts();
            charge.absorb(c);
            t.into_boxed_slice()
        });
        // `into_boxed_slice` reallocates when there is spare capacity, so the
        // reader reserves exactly (its count is known before it reads rows).
        Self {
            rows: rows.into_boxed_slice(),
            preload_table,
            charge,
        }
    }

    /// The charge for the tables.
    pub fn charge(&self) -> &Charge {
        &self.charge
    }

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
