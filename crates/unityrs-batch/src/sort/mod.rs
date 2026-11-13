//! Bounded external sorting of canonical projections. Decision semantics are
//! supplied by the caller; no candidate groups or inventories live in memory.
//! Allocation, framing and regression contracts: `docs/BATCH_SORT.md`.
pub mod catalog;
pub mod format;
pub mod merge;
pub mod run;
use crate::{
    phase::{Account, Error, check},
    spool::workspace::{Sealed, Workspace},
};
pub use format::{Frame, Reader};
use std::cmp::Ordering;

pub const RUN_BYTES: usize = 32 * 1024 * 1024;
pub const FAN_IN: usize = 16;

/// Projection-specific validation and a total semantic order over decoded
/// fields. Comparisons allocate nothing. Integer fields compare numerically,
/// never by their little-endian encoding. Equal comparisons are completed by
/// canonical payload bytes, so only identical records can use a run-index tie.
pub trait Order {
    fn tag(&self) -> u32;
    fn validate(&self, _record: &[u8]) -> Result<(), Error> {
        Ok(())
    }
    fn compare(&self, a: &[u8], b: &[u8]) -> Ordering;
}
pub(crate) fn compare(order: &dyn Order, a: &[u8], b: &[u8]) -> Ordering {
    order.compare(a, b).then_with(|| a.cmp(b))
}
/// Reuse `out`; return false only at clean source EOF. Sources check stop at
/// every bounded read. This adapter may scan committed records or a prior pass.
pub trait Source {
    fn next(&mut self, out: &mut Frame, stopped: &dyn Fn() -> bool) -> Result<bool, Error>;
}
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Allocation envelope including the packed arena and fixed-width index.
    pub run_bytes: usize,
    /// Earlier cuts for tests. Production uses `u64::MAX`.
    pub run_records: u64,
    pub fan_in: usize,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            run_bytes: RUN_BYTES,
            run_records: u64::MAX,
            fan_in: FAN_IN,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    Entry,
    Pass,
    BeforeRead,
    BeforeSort,
    AfterSort,
    RunSealed,
    MergeGroup,
    BeforeWrite,
    BeforeRetire,
    BeforeReturn,
}
pub trait Hooks {
    fn point(&self, _point: Point) {}
    fn run_storage(&self, _charged: u64) {}
    fn request_stop(&self, _failure: crate::job::RunFailure) {}
}
pub struct NoHooks;
impl Hooks for NoHooks {}
pub(crate) struct Control<'a> {
    pub stopped: &'a dyn Fn() -> bool,
    pub hooks: &'a dyn Hooks,
}
impl Control<'_> {
    pub fn at(&self, point: Point) -> Result<(), Error> {
        check(self.stopped)?;
        self.hooks.point(point);
        check(self.stopped)
    }
}
#[derive(Debug)]
pub struct Report {
    pub file: Option<Sealed>,
    pub runs: u64,
    pub generations: u32,
}

/// Directly driven blocking seam, audited after scratch drops. The caller
/// holds execution/workspace ownership through cancellation and performs the
/// canonical stop check before advancing to publication. No cleanup on stop.
pub fn sort(
    source: &mut dyn Source,
    order: &dyn Order,
    workspace: &mut Workspace,
    account: &Account,
    options: Options,
    stopped: &dyn Fn() -> bool,
    hooks: &dyn Hooks,
) -> Result<Report, Error> {
    let result = account.scoped(|| {
        let control = Control { stopped, hooks };
        control.at(Point::Entry)?;
        if options.run_bytes > RUN_BYTES
            || options.run_bytes < 256
            || options.run_records == 0
            || !(2..=FAN_IN).contains(&options.fan_in)
            || order.tag() == catalog::TAG
        {
            return Err(Error::internal());
        }
        // Covers fixed account/Arc, path, boxed file and bounded OS-adapter
        // allocations. Data buffers below have separate exact charges.
        let _overhead = account.meter().charge(64 * 1024)?;
        let catalog = run::generate(source, order, workspace, account.meter(), options, &control)?;
        let runs = catalog.count;
        let (file, generations) = merge::generations(
            catalog,
            order,
            workspace,
            account.meter(),
            options.fan_in,
            &control,
        )?;
        control.at(Point::BeforeReturn)?;
        Ok(Report {
            file,
            runs,
            generations,
        })
    });
    if let Err(Error::Run(failure)) = result.as_ref()
        && std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            hooks.request_stop(*failure);
        }))
        .is_err()
    {
        return Err(Error::internal());
    }
    result
}

/// `/` compares directly at the first differing byte, even against NUL.
#[must_use]
pub fn slash_first(a: &[u8], b: &[u8]) -> Ordering {
    for (&a, &b) in a.iter().zip(b) {
        if a != b {
            return match (a == b'/', b == b'/') {
                (true, _) => Ordering::Less,
                (_, true) => Ordering::Greater,
                _ => a.cmp(&b),
            };
        }
    }
    a.len().cmp(&b.len())
}
