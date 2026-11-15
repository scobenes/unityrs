//! X12's library-only observation seam. No history, allocation or progress
//! sampling: the test's coherent ledger receives each granted charge and
//! return, including returns on another thread. Install before preparation
//! in a fresh child. Native-only charges need separate component evidence;
//! these events must not be used to fit a Rust-only base coefficient.
use crate::progress::Phase;
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Copy, Debug)]
pub enum Point {
    Phase(Phase),
    /// Before the normal process exit, with frozen owners still retained.
    Exit,
    /// A real phase-6 merge group, including later generations.
    SortMerge,
}

/// Callbacks must not allocate, panic or reenter account operations. Point
/// callbacks may acknowledge a deterministic hold outside allocator events.
#[derive(Clone, Copy, Debug)]
pub struct Observer {
    pub charge: fn(u64, bool),
    /// Network reservations are reported separately. They must not also be
    /// deducted from a residual whose allowance already includes `C_net`.
    pub network: fn(u64, bool),
    pub point: fn(Point),
}
static OBSERVER: OnceLock<Observer> = OnceLock::new();
static PHASES: AtomicU64 = AtomicU64::new(0);

/// One observer per child, fixed before any account exists. It cannot be
/// replaced while outstanding permits retain the original observation.
pub fn install(observer: Observer) -> Result<(), Observer> {
    OBSERVER.set(observer)
}
pub(crate) fn charge(bytes: u64, held: bool) {
    if let Some(observer) = OBSERVER.get() {
        (observer.charge)(bytes, held);
    }
}
pub(crate) fn network(bytes: u64, held: bool) {
    if let Some(observer) = OBSERVER.get() {
        (observer.network)(bytes, held);
    }
}
pub fn at(point: Point) {
    if let Some(observer) = OBSERVER.get() {
        if let Point::Phase(phase) = point {
            let bit = 1 << phase as u32;
            if PHASES.fetch_or(bit, Ordering::AcqRel) & bit != 0 {
                return;
            }
        }
        (observer.point)(point);
    }
}
