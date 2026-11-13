//! B9's socket-table seam. The descriptor owner always closes before removal.
use super::{Detail, Error};
use std::{os::fd::RawFd, sync::Arc};

/// B9 supplies a fixed-capacity table. Registration must publish the descriptor
/// before returning. Exit sets `closing` before resetting entries; a racing
/// registration therefore either sees closing or is included in that reset.
/// Removal must clear only this registration's entry, never a reused slot.
/// A failed registration publishes nothing and retains no descriptor.
/// Implementations must not wait for IO or allocate on these operations.
pub trait SocketTable: Send + Sync {
    fn register(&self, descriptor: RawFd) -> Result<usize, Error>;
    fn closing(&self) -> bool;
    fn remove(&self, entry: usize);
}
/// This guard is constructed before connect and survives TCP and TLS setup.
/// The connecting future must be dropped before this guard on every failure.
pub(crate) struct Registration {
    table: Arc<dyn SocketTable>,
    entry: usize,
}
impl Registration {
    pub(crate) fn new(table: &Arc<dyn SocketTable>, descriptor: RawFd) -> Result<Self, Error> {
        let entry = table.register(descriptor)?;
        Ok(Self {
            table: table.clone(),
            entry,
        })
    }
    pub(crate) fn check(&self) -> Result<(), Error> {
        if self.table.closing() {
            Err(Error::transport(Detail::Connect))
        } else {
            Ok(())
        }
    }
    pub(crate) fn wrap<T>(self, value: T) -> Registered<T> {
        Registered {
            value: Some(value),
            registration: Some(self),
        }
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.table.remove(self.entry);
    }
}
/// `Drop` is explicit so field reordering cannot clear the table too early.
pub struct Registered<T> {
    pub(crate) value: Option<T>,
    registration: Option<Registration>,
}
impl<T> Registered<T> {
    pub(crate) fn close(&mut self) {
        drop(self.value.take());
        drop(self.registration.take());
    }
}
impl<T> Drop for Registered<T> {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
#[path = "socket_tests.rs"]
mod tests;
