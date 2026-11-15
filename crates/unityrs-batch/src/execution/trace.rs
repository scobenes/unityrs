//! Fixed process-test accounting shared by local and HTTP blocking work.
use crate::{run::hooks::Log, sys::Lock};
use std::sync::{Arc, OnceLock};
use tokio::runtime::{Handle, Id};

struct Pools {
    ids: [Id; 2],
    counts: Lock<[usize; 2]>,
    log: Arc<Log>,
}
static POOLS: OnceLock<Pools> = OnceLock::new();

/// Install after prewarming and before controller launch. Process-local
/// storage stays alive until `_exit`; library runs install no callback.
pub(crate) fn arm_pools(controller: &Handle, finisher: &Handle, log: Option<Arc<Log>>) {
    if let Some(log) = log {
        let _ = POOLS.set(Pools {
            ids: [controller.id(), finisher.id()],
            counts: Lock::new([0; 2]),
            log,
        });
    }
}

/// A spawn's counter, captured by its closure so a panic also returns it.
/// Lookup callers drop this before returning the quarantined token.
pub(crate) struct Task {
    index: Option<usize>,
    kind: &'static str,
}
pub(crate) fn task(handle: &Handle, kind: &'static str) -> Task {
    let index = POOLS.get().and_then(|pools| {
        let index = pools.ids.iter().position(|id| *id == handle.id())?;
        let mut counts = pools.counts.lock();
        let count = counts.get_mut(index)?;
        *count = count.saturating_add(1);
        pools.log.line(format_args!(
            "pool {} spawn {} {kind}",
            runtime(index),
            *count
        ));
        Some(index)
    });
    Task { index, kind }
}
impl Drop for Task {
    fn drop(&mut self) {
        if let (Some(pools), Some(index)) = (POOLS.get(), self.index) {
            let mut counts = pools.counts.lock();
            if let Some(count) = counts.get_mut(index) {
                *count = count.saturating_sub(1);
                pools.log.line(format_args!(
                    "pool {} end {} {}",
                    runtime(index),
                    *count,
                    self.kind
                ));
            }
        }
    }
}
fn runtime(index: usize) -> &'static str {
    if index == 0 { "controller" } else { "finisher" }
}
