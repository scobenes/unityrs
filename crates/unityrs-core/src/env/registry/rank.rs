//! A registration's rank (module docs, "Ranks").

use std::mem::size_of;
use std::sync::Arc;

use crate::limits::heap_cost;

/// Where a registration falls in the order an eager load registers it
/// (module docs): the load's sequence number, then the directory indexes
/// from the load's top-level file to the entry.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Rank {
    pub(super) load: u64,
    pub(super) path: Arc<[usize]>,
}

impl Rank {
    /// The directory indexes from the load's top-level file.
    pub(crate) fn path(&self) -> &[usize] {
        &self.path
    }

    /// The heap a rank's path takes: its `Arc` in a glibc chunk.
    pub(super) fn heap(path_len: usize) -> u64 {
        let bytes = 2 * size_of::<usize>() + size_of::<usize>().saturating_mul(path_len);
        heap_cost(u64::try_from(bytes).unwrap_or(u64::MAX))
    }
}
