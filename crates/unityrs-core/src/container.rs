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
//! them, is dropped after). The tables are reached by reference only, so
//! they cannot outlive the charge:
//!
//! ```compile_fail,E0616
//! fn take(c: unityrs_core::container::Container) -> Box<[unityrs_core::container::ContainerRow]> {
//!     c.rows
//! }
//! ```
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
use crate::limits::{Charge, ChargeError, Meter, MeteredVec};
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
    rows: Box<[ContainerRow]>,
    preload_table: Option<Box<[PPtr]>>,
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

    /// The rows and preload table a read grew on the input's meter. A table
    /// with spare capacity moves into an exact allocation that is charged
    /// first ([`MeteredVec::into_boxed_slice`]).
    pub fn new(
        rows: MeteredVec<ContainerRow>,
        preload_table: Option<MeteredVec<PPtr>>,
    ) -> Result<Self, ChargeError> {
        let (rows, mut charge) = rows.into_boxed_slice()?;
        let preload_table = match preload_table {
            Some(t) => {
                let (t, c) = t.into_boxed_slice()?;
                charge.absorb(c);
                Some(t)
            }
            None => None,
        };
        Ok(Self {
            rows,
            preload_table,
            charge,
        })
    }

    /// Rows in file order.
    pub fn rows(&self) -> &[ContainerRow] {
        &self.rows
    }

    /// `m_PreloadTable`, when the AssetBundle has one.
    pub fn preload_table(&self) -> Option<&[PPtr]> {
        self.preload_table.as_deref()
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{Container, ContainerRow};
    use crate::budget::{Counting, Reservation};
    use crate::env::PPtr;
    use crate::limits::{Meter, MeteredVec, heap_cost};

    fn row(i: i64) -> ContainerRow {
        ContainerRow {
            path: "a".into(),
            preload_index: 0,
            preload_size: 0,
            file_id: 0,
            path_id: i,
        }
    }

    #[test]
    fn the_tables_are_charged_at_their_exact_size() {
        // Round 3: `into_boxed_slice` moved a table with spare capacity into
        // a new allocation nobody charged.
        let budget = Arc::new(Counting::new(u64::MAX));
        let m = Meter::new(u64::MAX, budget.clone(), Reservation::Input);
        let mut rows = MeteredVec::new(&m);
        let mut preload = MeteredVec::new(&m);
        for i in 0..100 {
            rows.push(row(i)).unwrap();
            preload.push(PPtr { file_id: 0, path_id: i }).unwrap();
        }
        let c = Container::new(rows, Some(preload)).unwrap();
        let exact = |n: usize| heap_cost(u64::try_from(n).unwrap());
        assert_eq!(
            m.used(),
            exact(100 * std::mem::size_of::<ContainerRow>()) + exact(100 * std::mem::size_of::<PPtr>())
        );
        assert_eq!((c.rows().len(), c.preload_table().map(<[PPtr]>::len)), (100, Some(100)));
        assert_eq!(c.charge().records(), 0);
        drop(c);
        assert_eq!((m.used(), budget.used()), (0, 0));
    }
}
