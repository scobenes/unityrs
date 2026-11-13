//! One Input-only phase account, reusable by publication after scratch drops.
use crate::{
    attempt::ReasonCode,
    budget::{SealedBudget, Snapshot},
    job::RunFailure,
    spool,
};
use std::{
    ops::{Deref, DerefMut},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};
use unityrs_core::{
    budget::Reservation,
    limits::{Charge, ChargeError, Meter, heap_cost},
};

pub const WORKING_SET: u64 = 40 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Stopped,
    Run(RunFailure),
}
impl Error {
    #[must_use]
    pub const fn internal() -> Self {
        Self::Run(RunFailure::new(ReasonCode::Internal))
    }
    #[must_use]
    pub const fn memory() -> Self {
        Self::Run(RunFailure::new(ReasonCode::HostMemory))
    }
}
impl From<spool::Error> for Error {
    fn from(e: spool::Error) -> Self {
        match e {
            spool::Error::Stopped => Self::Stopped,
            e => Self::Run(e.into()),
        }
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::OutOfMemory {
            Self::memory()
        } else {
            Self::Run(RunFailure::new(ReasonCode::HostIo))
        }
    }
}
impl From<ChargeError> for Error {
    fn from(_: ChargeError) -> Self {
        Self::internal()
    }
}
pub fn check(stopped: &dyn Fn() -> bool) -> Result<(), Error> {
    if stopped() {
        Err(Error::Stopped)
    } else {
        Ok(())
    }
}

/// Run on the thread performing the blocking work, including pool callbacks.
/// Thread-local refusals cannot be audited by the receiving controller alone.
pub fn on_pool<T>(body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    let before = crate::alloc::refusals();
    let mut result =
        catch_unwind(AssertUnwindSafe(body)).unwrap_or_else(|_| Err(Error::internal()));
    let mut failure = match result.as_ref().err() {
        Some(Error::Run(f)) => Some(*f),
        _ => None,
    };
    if crate::alloc::refusals() != before {
        RunFailure::reduce(&mut failure, RunFailure::new(ReasonCode::HostMemory));
    }
    if let Some(failure) = failure {
        result = Err(Error::Run(failure));
    }
    result
}

pub struct Account {
    budget: Arc<SealedBudget>,
    meter: Arc<Meter>,
    limit: u64,
}
impl Account {
    /// `limit` is the run's `I`; small values support refusal tests.
    #[must_use]
    pub fn new(limit: u64) -> Self {
        let budget = Arc::new(SealedBudget::input_only(limit));
        let meter = Meter::new(limit, budget.clone(), Reservation::Input);
        Self {
            budget,
            meter,
            limit,
        }
    }
    #[must_use]
    pub fn meter(&self) -> &Arc<Meter> {
        &self.meter
    }
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        self.budget.snapshot()
    }

    /// Serial blocking-task seam; the caller retains this account and the
    /// workspace outside the unwind boundary. `T` must contain only a fixed
    /// handoff, never scratch allocations or permits. No thread is started.
    pub fn execute<T>(&self, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        self.audit(0, WORKING_SET, body)
    }
    /// A sort borrows a charged source that can remain live between passes.
    /// The complete phase driver uses `execute` for the final zero audit.
    pub(crate) fn scoped<T>(&self, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        self.audit(self.snapshot().input.used, WORKING_SET, body)
    }
    /// Publication is a serial blocking-task boundary. Invoke on the pool
    /// thread itself so its allocator refusals are included in this audit.
    pub fn publication<T>(&self, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        self.audit(0, self.limit, body)
    }
    /// Scheduler capacity check, before attempting a meter reservation. A
    /// busy account is not an allocation refusal. Caller retries after receipt.
    #[must_use]
    pub fn admits(&self, bytes: u64) -> bool {
        self.snapshot()
            .input
            .used
            .checked_add(bytes)
            .is_some_and(|n| n <= self.limit)
    }
    #[must_use]
    pub const fn limit(&self) -> u64 {
        self.limit
    }
    fn audit<T>(
        &self,
        expected: u64,
        bound: u64,
        body: impl FnOnce() -> Result<T, Error>,
    ) -> Result<T, Error> {
        let mut result = on_pool(body);
        let snapshot = self.snapshot();
        let mut failure = match result.as_ref().err() {
            Some(Error::Run(f)) => Some(*f),
            _ => None,
        };
        if snapshot.input.used != expected
            || snapshot.dependency.used != 0
            || snapshot.refusals() != 0
            || snapshot.input.peak > bound
        {
            RunFailure::reduce(&mut failure, RunFailure::new(ReasonCode::Internal));
        }
        if let Some(failure) = failure {
            result = Err(Error::Run(failure));
        }
        result
    }
}

/// Fixed-capacity fallible storage. No growth, including simultaneous old
/// and new allocations; the capacity's `heap_cost` is charged first.
pub struct Buffer<T> {
    values: Vec<T>,
    charge: Charge,
    limit: usize,
}
impl<T> Buffer<T> {
    #[must_use]
    pub fn charged(&self) -> u64 {
        self.charge.bytes()
    }
    pub fn new(meter: &Arc<Meter>, capacity: usize) -> Result<Self, Error> {
        let bytes = capacity
            .checked_mul(size_of::<T>())
            .ok_or_else(Error::internal)?;
        let charge = meter.charge(heap_cost(
            u64::try_from(bytes).map_err(|_| Error::internal())?,
        ))?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(capacity)
            .map_err(|_| Error::memory())?;
        if values.capacity() != capacity {
            return Err(Error::internal());
        }
        Ok(Self {
            values,
            charge,
            limit: capacity,
        })
    }
    pub fn push(&mut self, value: T) -> Result<(), Error> {
        if self.values.len() == self.limit {
            return Err(Error::internal());
        }
        self.values.push(value);
        Ok(())
    }
    pub fn clear(&mut self) {
        self.values.clear();
    }
}
impl<T: Copy> Buffer<T> {
    pub fn extend(&mut self, values: &[T]) -> Result<(), Error> {
        if values.len() > self.limit - self.values.len() {
            return Err(Error::internal());
        }
        self.values.extend_from_slice(values);
        Ok(())
    }
    pub fn fill(&mut self, value: T) {
        self.values.resize(self.limit, value);
    }
}
impl<T> Deref for Buffer<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.values
    }
}
impl<T> DerefMut for Buffer<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        &mut self.values
    }
}
