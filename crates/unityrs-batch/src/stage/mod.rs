//! Directly driven local staging seams. B5b owns launch order/concurrency,
//! barrier failure selection and commits; this layer starts no threads.
pub mod local;
use crate::{job::RunFailure, spec::model::Spec};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataFailure {
    SourceMissing,
    SourceNotRegular,
    SourceChanged,
    IntegrityMismatch,
    InputBytes(u64),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Data(DataFailure),
    Stopped,
    Run(RunFailure),
}
impl From<crate::spool::Error> for Failure {
    fn from(error: crate::spool::Error) -> Self {
        if matches!(error, crate::spool::Error::Stopped) {
            Self::Stopped
        } else {
            Self::Run(error.into())
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Facts {
    pub size: u64,
    pub sha256: [u8; 32],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StageEnd {
    pub bundle: u32,
    pub failure: Option<Failure>,
    /// Only a staged body or a complete unchanged integrity mismatch has facts.
    pub facts: Option<Facts>,
    pub claimed_bytes: u64,
}
/// Local execution is a capability of the whole run, including unused
/// dependencies. Check this before creating any staging destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UrlUnsupported;
pub fn require_local(spec: &Spec) -> Result<(), UrlUnsupported> {
    if spec.bundles().any(|b| b.url().is_some()) {
        Err(UrlUnsupported)
    } else {
        Ok(())
    }
}

/// Generated names only. `directory` is the trusted prepared `in/` directory.
/// No per-bundle path strings need to be retained in phase 1.
#[must_use]
pub fn path(directory: &Path, bundle: u32, inputs: usize, peek: bool) -> PathBuf {
    let index = bundle as usize;
    let (kind, n) = match index.checked_sub(inputs) {
        None => ('i', index),
        Some(n) => ('d', n),
    };
    directory.join(format!("{}{kind}{n}", if peek { "p" } else { "" }))
}
