//! Typed, bounded NDJSON artifacts; no whole-line or JSON-tree allocation.
mod encode;
mod flat;
mod model;
pub mod read;
pub mod text;
mod write;
pub use model::*;
pub use text::Text;
pub use write::{Writer, copy_output, write_failure_record};

/// Fixed charged buffer size. Full buffers and the final partial buffer
/// are the only records-file writes that count as export work.
pub const BUFFER: usize = 64 * 1024;
/// Bound includes the terminating newline.
pub const LINE: usize = 32 * 1024;

#[derive(Debug)]
pub enum Error {
    Spool(crate::spool::Error),
    Work(crate::reason::Limit),
    Charge(unityrs_core::limits::ChargeError),
    /// Invalid line/model, counter overflow, or writer already failed.
    Internal,
}
impl From<crate::spool::Error> for Error {
    fn from(e: crate::spool::Error) -> Self {
        Self::Spool(e)
    }
}
impl From<crate::reason::Limit> for Error {
    fn from(e: crate::reason::Limit) -> Self {
        Self::Work(e)
    }
}
impl From<unityrs_core::limits::ChargeError> for Error {
    fn from(e: unityrs_core::limits::ChargeError) -> Self {
        Self::Charge(e)
    }
}
