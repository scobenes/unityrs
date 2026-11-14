//! Which errors are a refusal, found through every error that wraps one
//! (an entry's, the load-time AssetBundle read's, a dereference's load, a
//! resave's tree), as [`Refusal`]:
//!
//! - of memory by the caller's budget or the allocator: the oracle's
//!   `MemoryError`, which is never an L2 or L3 outcome but the input's
//!   `crash` record (`docs/CANONICAL_DUMP.md` section 4.2). It depends on
//!   the machine and on what the other inputs hold, not on the input.
//! - by a bound of the open's limits (the input's own memory bound,
//!   `InputMemory`, among them): its text, `limit <Kind>: <value> > <max>`,
//!   is what `tools/parity/g3b.py` reads from the note, so the note keeps it
//!   whole ([`crate::record::note_keeping`]) however long the names before
//!   it.

use unityrs_core::budget::{AllocError, BudgetExceeded};
use unityrs_core::detect::DetectError;
use unityrs_core::env::DerefError;
use unityrs_core::limits::ChargeError;
use unityrs_core::typetree::TreeError;
use unityrs_core::walk::{ParseError, ReadDetail, WriteError};
use unityrs_core::{Error, LimitError};

use crate::record::note_keeping;

/// What refused the work an error stopped.
#[derive(Clone, Copy, Debug)]
enum Refusal<'a> {
    /// The caller's budget or the allocator.
    Memory(&'a BudgetExceeded),
    /// Host allocator provenance.
    Allocator(&'a AllocError),
    /// A bound of the open's limits.
    Limit(&'a LimitError),
}

impl<'a> Refusal<'a> {
    fn memory(self) -> Option<Error> {
        match self {
            Self::Memory(b) => Some(Error::Budget(b.clone())),
            Self::Allocator(a) => Some(Error::Allocator(*a)),
            Self::Limit(_) => None,
        }
    }

    fn limit(self) -> Option<&'a LimitError> {
        match self {
            Self::Limit(l) => Some(l),
            Self::Memory(_) | Self::Allocator(_) => None,
        }
    }
}

/// `Err(Error::Budget)` when `e` is the caller's budget or the allocator
/// refusing memory to an object's read: its value, its tree (an embedded
/// tree's compile, or a provider's walk: [`unityrs_core::provider::ProviderError::memory`]),
/// or the load `m_Script` names.
pub(crate) fn out_of_memory(e: &ParseError) -> Result<(), Error> {
    read_refusal(e)
        .and_then(Refusal::memory)
        .map_or(Ok(()), Err)
}

/// `Err(Error::Budget)` when `e` is the caller's budget or the allocator
/// refusing memory to an object's resave: its output, or the tree it looks
/// up again (a provider asked anew: a refusal is never kept).
pub(crate) fn write_out_of_memory(e: &WriteError) -> Result<(), Error> {
    write_refusal(e)
        .and_then(Refusal::memory)
        .map_or(Ok(()), Err)
}

/// The budget's or the allocator's refusal under `e`, or `None`: what
/// [`super::dump_on`] makes a `crash` record.
pub(super) fn memory_refused(e: &Error) -> Option<Error> {
    refusal(e).and_then(Refusal::memory)
}

/// A file record's note for `e`, its limit refusal kept whole.
pub(super) fn note(e: &Error) -> String {
    note_keeping(e, refusal(e).and_then(Refusal::limit))
}

/// An object's L2 note for `e`, its limit refusal kept whole.
pub(super) fn read_note(e: &ParseError) -> String {
    note_keeping(e, read_refusal(e).and_then(Refusal::limit))
}

/// An object's resave note for `e`, its limit refusal kept whole.
pub(super) fn write_note(e: &WriteError) -> String {
    note_keeping(e, write_refusal(e).and_then(Refusal::limit))
}

fn refusal(e: &Error) -> Option<Refusal<'_>> {
    match e {
        Error::Budget(b) => Some(Refusal::Memory(b)),
        Error::Allocator(a) | Error::Detect(DetectError::Allocator(a)) => {
            Some(Refusal::Allocator(a))
        }
        Error::Limit(l) | Error::Detect(DetectError::InflateLimit(l) | DetectError::Work(l)) => {
            Some(Refusal::Limit(l))
        }
        Error::Tree(t) => tree_refusal(t),
        Error::Entry { source, .. } => refusal(source),
        Error::AssetBundle(p) => read_refusal(p),
        Error::Deref(d) => deref_refusal(d),
        _ => None,
    }
}

fn read_refusal(e: &ParseError) -> Option<Refusal<'_>> {
    match e {
        ParseError::Read(r) => match &r.detail {
            ReadDetail::Limit(l) => Some(Refusal::Limit(l)),
            ReadDetail::Charge(c) => charge_refusal(c),
            ReadDetail::Tree(t) => tree_refusal(t),
            _ => None,
        },
        ParseError::Tree(t) => tree_refusal(t),
        ParseError::Provider(p) => p
            .allocator
            .as_ref()
            .map(Refusal::Allocator)
            .or_else(|| p.memory.as_ref().map(Refusal::Memory))
            .or_else(|| p.limit.as_ref().map(Refusal::Limit)),
        ParseError::Deref(d) => deref_refusal(d),
        _ => None,
    }
}

fn write_refusal(e: &WriteError) -> Option<Refusal<'_>> {
    match e {
        WriteError::Limit(l) => Some(Refusal::Limit(l)),
        WriteError::Charge(c) => charge_refusal(c),
        WriteError::Tree(p) => read_refusal(p),
        _ => None,
    }
}

fn deref_refusal(e: &DerefError) -> Option<Refusal<'_>> {
    match e {
        DerefError::Load(e) => refusal(e),
        _ => None,
    }
}

fn tree_refusal(e: &TreeError) -> Option<Refusal<'_>> {
    match e {
        TreeError::Limit(l) => Some(Refusal::Limit(l)),
        TreeError::Charge(c) => charge_refusal(c),
        _ => None,
    }
}

fn charge_refusal(e: &ChargeError) -> Option<Refusal<'_>> {
    match e {
        ChargeError::Budget(b) => Some(Refusal::Memory(b)),
        ChargeError::Allocator(a) => Some(Refusal::Allocator(a)),
        ChargeError::Limit(l) => Some(Refusal::Limit(l)),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
