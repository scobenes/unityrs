use super::{AttemptEnd, classify};
use crate::{budget::SealedBudget, work::Work};
use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};
use unityrs_core::{RawStr, limits::Meter};
use unityrs_export::ngui::{self, Class, Failure, Reason};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    Opened,
    Metadata,
    Registered,
    Copied(u64),
    BeforeSeal,
    BeforeFinish,
}
/// Observers may request stops or inject faults; they cannot replace internal
/// budget or work accounting. Callbacks run inside the unwind boundary.
pub trait Hooks: ngui::Hooks {
    fn usage(&self) -> Option<std::sync::Arc<std::sync::atomic::AtomicU64>> {
        None
    }
    fn point(&self, _point: Point) {}
    fn sheets(&self, _sheets: &[ngui::SheetRow]) {}
    fn request_stop(&self, _end: &AttemptEnd) {}
}
impl Hooks for ngui::NoHooks {}

pub(super) struct Engine<'a> {
    pub memory: &'a crate::job_memory::Memory,
    pub budget: Arc<SealedBudget>,
    pub stop: &'a dyn crate::control::Control,
    pub observer: &'a dyn Hooks,
    pub work: RefCell<Work>,
    pub decode_peak: Cell<u64>,
    pub evictions: Cell<u64>,
    pub fault: Cell<bool>,
}
pub(super) struct MeterSample<'a, 'b>(&'a Engine<'b>, &'a Meter);
impl Drop for MeterSample<'_, '_> {
    fn drop(&mut self) {
        self.0.sample(self.1);
    }
}
impl<'b> Engine<'b> {
    pub fn track<'a>(&'a self, meter: &'a Meter) -> MeterSample<'a, 'b> {
        MeterSample(self, meter)
    }
    pub fn check(&self) -> Result<(), Failure> {
        if self.fault.get() {
            return Err(classify::internal());
        }
        if ngui::Hooks::stopped(self) {
            Err(Failure::Stopped)
        } else {
            Ok(())
        }
    }
    pub fn point(&self, point: Point) -> Result<(), Failure> {
        self.observer.point(point);
        self.check()
    }
    pub fn sample(&self, meter: &Meter) {
        self.decode_peak
            .set(self.decode_peak.get().max(meter.decoded()));
    }
    pub fn records<T>(
        &self,
        f: impl FnOnce(&mut Work) -> Result<T, crate::records::Error>,
    ) -> Result<T, Failure> {
        let before = self.budget.snapshot().refusals();
        f(&mut self.work.borrow_mut()).map_err(|e| {
            let failure = classify::record(e);
            if self.budget.snapshot().refusals() == before
                && matches!(&failure,
                Failure::Input(r) if r.reason.starts_with("budget("))
            {
                Failure::Run(classify::plain("host_memory"))
            } else {
                failure
            }
        })
    }
}
impl ngui::Hooks for Engine<'_> {
    fn stopped(&self) -> bool {
        self.fault.get() || self.stop.stopped() || self.observer.stopped()
    }
    fn refusals(&self) -> Option<u64> {
        Some(self.budget.snapshot().refusals())
    }
    fn export_work(&self, bytes: u64) -> Result<(), Reason> {
        self.work
            .borrow_mut()
            .pixels(bytes)
            .map_err(classify::limit_reason)
    }
    fn meter(&self, meter: &Meter) {
        self.sample(meter);
        // Meter observations run from RAII guards, possibly during unwind.
        // Latch a callback panic instead of permitting a second unwind.
        if !self.fault.get()
            && std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.observer.meter(meter)))
                .is_err()
        {
            self.fault.set(true);
        }
    }
    fn environment(&self, env: &unityrs_core::env::Environment) {
        self.evictions.set(env.stats().dependencies_evicted);
        self.observer.environment(env);
    }
    fn sprite_group(&self, group: usize, file: &RawStr, path_id: i64) {
        self.observer.sprite_group(group, file, path_id);
    }
    #[cfg(feature = "test-hooks")]
    fn environment_fault(
        &self,
        env: &unityrs_core::env::Environment,
    ) -> Option<unityrs_core::Error> {
        self.observer.environment_fault(env)
    }
    #[cfg(feature = "test-hooks")]
    fn skipped(&self, exporting: bool) {
        crate::work::tap::tell(crate::work::tap::Event::Skip(exporting));
        self.observer.skipped(exporting);
    }
    #[cfg(feature = "test-hooks")]
    fn decoder_fault(&self) {
        self.observer.decoder_fault();
    }
}
/// Same refusal probe as extraction, for individual batch-side core calls.
pub(crate) fn probe<T>(
    hooks: &dyn ngui::Hooks,
    f: impl FnOnce() -> Result<T, Class>,
) -> Result<T, Failure> {
    let before = hooks.refusals();
    f().map_err(|c| {
        let (Class::Skip(r) | Class::Input(r) | Class::Run(r)) = &c;
        if before.is_some() && before == hooks.refusals() && r.reason.starts_with("budget(") {
            Failure::Run(classify::plain("host_memory"))
        } else {
            c.into()
        }
    })
}
