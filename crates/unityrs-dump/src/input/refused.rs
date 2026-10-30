//! Which errors are a refusal of memory by the caller's budget or the
//! allocator: the oracle's `MemoryError`, which is never an L2 or L3 outcome
//! but the input's `crash` record (`docs/CANONICAL_DUMP.md` section 4.2). It
//! depends on the machine and on what the other inputs hold, not on the
//! input. The input's own bound, `InputMemory`, is a limit refusal instead.

use unityrs_core::Error;
use unityrs_core::budget::BudgetExceeded;
use unityrs_core::env::DerefError;
use unityrs_core::limits::ChargeError;
use unityrs_core::typetree::TreeError;
use unityrs_core::walk::{ParseError, ReadDetail, WriteError};

/// `Err(Error::Budget)` when `e` is the caller's budget or the allocator
/// refusing memory to an object's read: its value, its tree (an embedded
/// tree's compile, or a provider's walk: [`unityrs_core::provider::ProviderError::memory`]),
/// or the load `m_Script` names.
pub(super) fn out_of_memory(e: &ParseError) -> Result<(), Error> {
    read_refused(e).map_or(Ok(()), |b| Err(Error::Budget(b.clone())))
}

/// `Err(Error::Budget)` when `e` is the caller's budget or the allocator
/// refusing memory to an object's resave: its output, or the tree it looks
/// up again (a provider asked anew: a refusal is never kept).
pub(super) fn write_out_of_memory(e: &WriteError) -> Result<(), Error> {
    write_refused(e).map_or(Ok(()), |b| Err(Error::Budget(b.clone())))
}

/// The budget's or the allocator's refusal under `e`, through every error
/// that wraps one (an entry's, the load-time AssetBundle read's, a
/// dereference's load), or `None`: what [`super::dump_on`] makes a `crash`
/// record.
pub(super) fn memory_refused(e: &Error) -> Option<&BudgetExceeded> {
    match e {
        Error::Budget(b) => Some(b),
        Error::Tree(t) => tree_refused(t),
        Error::Entry { source, .. } => memory_refused(source),
        Error::AssetBundle(p) => read_refused(p),
        Error::Deref(d) => deref_refused(d),
        _ => None,
    }
}

fn read_refused(e: &ParseError) -> Option<&BudgetExceeded> {
    match e {
        ParseError::Read(r) => match &r.detail {
            ReadDetail::Charge(c) => charge_refused(c),
            ReadDetail::Tree(t) => tree_refused(t),
            _ => None,
        },
        ParseError::Tree(t) => tree_refused(t),
        ParseError::Provider(p) => p.memory.as_ref(),
        ParseError::Deref(d) => deref_refused(d),
        _ => None,
    }
}

fn write_refused(e: &WriteError) -> Option<&BudgetExceeded> {
    match e {
        WriteError::Charge(c) => charge_refused(c),
        WriteError::Tree(p) => read_refused(p),
        _ => None,
    }
}

fn deref_refused(e: &DerefError) -> Option<&BudgetExceeded> {
    match e {
        DerefError::Load(e) => memory_refused(e),
        _ => None,
    }
}

fn tree_refused(e: &TreeError) -> Option<&BudgetExceeded> {
    match e {
        TreeError::Charge(c) => charge_refused(c),
        _ => None,
    }
}

fn charge_refused(e: &ChargeError) -> Option<&BudgetExceeded> {
    match e {
        ChargeError::Budget(b) => Some(b),
        ChargeError::Limit(_) => None,
    }
}
