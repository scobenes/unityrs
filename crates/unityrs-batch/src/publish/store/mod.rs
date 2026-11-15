//! Store publication components only; scheduling and run activation are separate.
//! Expected rows are streamed in key order. No per-key in-memory inventory.
pub mod resume;
mod staged;
pub mod verify;
use crate::{
    outcomes::Code,
    phase::{Account, Error},
    spool::workspace::Workspace,
    store::{etag::Etag, put::Request},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    Loose(u64),
    Archive(u64),
}
#[derive(Clone, Copy)]
pub struct Expected<'a> {
    /// Complete store key, including the sink prefix.
    pub key: &'a str,
    pub target: Target,
    pub code: Code,
    /// Resume: fresh expected digest, including a multipart part count.
    /// Verification of outcome 7: the returned write `ETag`, empty if absent.
    pub etag: &'a [u8],
    pub size: u64,
}
/// A charged cursor over winners and archives merged in strict bytewise order.
/// Resume archive rows need no expected tag: their hashing pass runs later.
pub trait Source {
    fn current(&self) -> Result<Option<Expected<'_>>, Error>;
    fn advance(&mut self) -> Result<(), Error>;
}
pub struct Context<'a, 'r> {
    pub request: &'a Request<'r>,
    pub account: &'a Account,
    pub workspace: &'a Workspace,
    pub prefix: &'a str,
    pub stopped: &'a dyn Fn() -> bool,
}
/// Fixed archive handoff, in the preallocated per-archive startup storage.
/// No archive becomes present until a fresh expected-hash pass matches it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Listed {
    pub etag: Etag,
    pub size: u64,
}
impl Listed {
    #[must_use]
    pub fn matches(&self, expected: &[u8], size: u64) -> bool {
        self.size == size && self.etag.matches(expected)
    }
}
pub(super) fn list_error(error: crate::store::list::Error) -> Result<bool, Error> {
    use crate::store::{attempt::Fault, list::Error as L};
    match error {
        L::Stopped => Ok(true),
        L::Fault(Fault::Memory) => Err(Error::memory()),
        L::Fault(Fault::Internal) | L::PageHeld | L::Busy => Err(Error::internal()),
        L::Phase(e) => Err(e),
        L::Failed | L::Order | L::Reservation => Ok(false),
    }
}
