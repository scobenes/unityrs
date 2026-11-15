//! Charges shared by mutable-tree walks, parsers and language-boundary staging.
use super::{Charge, ChargeError, Meter};
use crate::{LimitError, LimitKind};
use std::sync::Arc;

/// A bounded tree operation. Its staging stays charged until handoff or refusal.
pub struct TreeTraversal {
    charge: Charge,
    depth_limit: u32,
}

const _: () = crate::send_sync::<TreeTraversal>();

impl TreeTraversal {
    /// Use the input's existing memory and cumulative work account.
    #[must_use]
    pub fn new(meter: &Arc<Meter>, depth_limit: u32) -> Self {
        Self {
            charge: Charge::zero(meter),
            depth_limit,
        }
    }

    /// Check depth, then charge work and staging before an expansion.
    /// Core-owned buffers additionally carry their own `MeteredVec` charges.
    pub fn step(&mut self, depth: u64, memory: u64, work: u64) -> Result<(), ChargeError> {
        if depth > u64::from(self.depth_limit) {
            return Err(
                LimitError::refused(LimitKind::TreeDepth, depth, self.depth_limit.into()).into(),
            );
        }
        self.charge.meter().count_decode(work)?;
        if memory > 0 {
            self.charge.grow(memory)?;
        }
        Ok(())
    }
}
