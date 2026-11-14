//! Canonical stored ZIP stream. This component never acknowledges publication.
//! The caller drives it on bounded blocking work; Drop performs no cleanup.
//! Store transport must assemble parts outside request body reads/timers,
//! because a pull can write central scratch. At most one blocking operation
//! per stream is outstanding; scheduling and permits belong to the caller.
mod central;
mod headers;
mod stream;
use crate::{
    digest::Etag,
    phase::{Error, check},
    spool::workspace::{Sealed, Workspace},
};
use std::sync::Arc;
#[cfg(any(test, feature = "test-hooks"))]
pub use stream::open_at;
pub use stream::{Stream, open};
use unityrs_core::limits::Meter;

/// Admission envelope including a charged 64 KiB caller transport buffer,
/// at ordinary filesystem path bounds. Independent of payload/member counts.
pub const ENVELOPE: u64 = 1024 * 1024;

/// B7b's single committed numeric payload seam, shared with loose
/// publication. Implementations open only the selected immutable payload; no
/// member path is used as a filesystem path. Boxed-reader allocation must be
/// fallible and covered by the stream envelope.
pub use crate::publish::payload::Payloads;
#[derive(Clone, Copy)]
pub struct Input<'a> {
    pub key: &'a str,
    pub index: u32,
    pub members: u64,
    /// Complete logical file size (equal to emitted size at production origin zero).
    pub size: u64,
    pub list: &'a Sealed,
}
pub struct Context<'a> {
    pub workspace: &'a Workspace,
    pub payloads: &'a dyn Payloads,
    pub meter: &'a Arc<Meter>,
    pub hooks: &'a dyn Hooks,
}
/// Internal generation result, available only after scratch retirement.
/// Public successful metadata additionally requires publication acknowledgment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    pub size: u64,
    /// Bytes actually emitted; excludes a test-only virtual prefix.
    pub emitted_size: u64,
    /// Hashes cover emitted bytes. Production always has origin zero.
    pub md5: [u8; 16],
    pub etag: Etag,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Eligibility {
    Empty,
    Withheld,
    Oversized,
    Eligible,
}
/// Apply before payload opens, central scratch or destination temp creation.
#[must_use]
pub const fn eligibility(members: u64, blocked: bool, size: u64) -> Eligibility {
    if members == 0 {
        Eligibility::Empty
    } else if blocked {
        Eligibility::Withheld
    } else if size > crate::winners::layout::LIMIT {
        Eligibility::Oversized
    } else {
        Eligibility::Eligible
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    Member,
    PayloadRead,
    CentralWrite,
    CentralSeal,
    CentralRead,
    Retire,
    Complete,
}
/// Bounded observation/fault seam; no observer is stored per member.
pub trait Hooks: Send + Sync {
    fn at(&self, _: Point) -> Result<(), Error> {
        Ok(())
    }
}
pub struct NoHooks;
impl Hooks for NoHooks {}
pub(super) fn boundary(
    hooks: &dyn Hooks,
    point: Point,
    stopped: &dyn Fn() -> bool,
) -> Result<(), Error> {
    check(stopped)?;
    hooks.at(point)?;
    check(stopped)
}
