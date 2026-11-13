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
/// The failed integrity check, with size taking precedence over SHA-256.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Integrity {
    Size,
    Sha256,
}
impl Integrity {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Sha256 => "sha256",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StageEnd {
    pub bundle: u32,
    pub failure: Option<Failure>,
    pub integrity: Option<Integrity>,
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

/// Fallible controller/worker counterpart to [`path`]. No derived path is
/// retained in a per-bundle or per-input table.
pub(crate) fn try_path(
    directory: &Path,
    bundle: u32,
    inputs: usize,
    peek: bool,
) -> Result<PathBuf, crate::spool::Error> {
    let (dependency, n) = match (bundle as usize).checked_sub(inputs) {
        None => (false, bundle as usize),
        Some(n) => (true, n),
    };
    let prefix = match (peek, dependency) {
        (false, false) => "i",
        (false, true) => "d",
        (true, false) => "pi",
        (true, true) => "pd",
    };
    crate::spool::path::indexed(
        directory,
        prefix,
        u64::try_from(n).map_err(|_| crate::spool::Error::Internal)?,
    )
}
