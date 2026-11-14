//! The object-store sink's client side (`docs/plans/batch.md` 2.9,
//! `docs/BATCH_SPEC.md` 1.6, `docs/plans/extract.md` E37), on `object_store`
//! 0.14.2 (`aws-base` and `ring`: no `reqwest`, no aws-lc-rs) wired to the
//! engine's own HTTP service. Nothing here executes a
//! run: the CLI still refuses `object_store` sinks.
//!
//! - [`client`]: the one place an `AmazonS3Builder` is made.
//! - [`connector`]: `object_store`'s `HttpConnector` over [`crate::http`],
//!   one request per call, recording each try in its [`attempt::TrySlot`].
//! - [`attempt`]: a request's tries, its timer, stop and cutoff checks,
//!   backoff and the unsettled count.
//! - [`classify`]: 1.6's table as a pure function.
//! - [`credentials`]: the environment's credentials, checked at preflight.
//! - [`etag`]: `ETag`s as the engine keeps and compares them.
pub mod attempt;
mod body;
pub mod classify;
pub mod client;
pub mod connector;
pub mod credentials;
pub mod etag;
pub mod scan;

const KIB: u64 = 1_024;
const MIB: u64 = 1_024 * KIB;
/// A LIST page's 2xx body (`batch.md` 2.9): 1,000 keys of at most 1,024
/// bytes, each byte escaped to at most 6, with their other fields.
pub const LIST_CAP: u64 = 7 * MIB;
/// A head GET's 2xx body, read into the engine's own buffer of this size: a
/// head that fills it is unreadable (`BATCH_SPEC.md` 7.2).
pub const HEAD_CAP: u64 = MIB + 1;
/// Any other 2xx body (a create's, a complete's).
pub const SMALL_CAP: u64 = 64 * KIB;
/// A non-2xx body, read by the connector into one buffer and cut there.
pub const ERROR_CAP: u64 = 64 * KIB;

/// What a store request does, which picks its body cap and its rows of
/// `BATCH_SPEC.md` 1.6's table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    /// A single PUT of an output, an archive or a row file.
    Put,
    /// `UploadPart`.
    Part,
    /// `CreateMultipartUpload`.
    Create,
    /// `CompleteMultipartUpload`; its tries after the first are retries.
    Complete,
    /// `AbortMultipartUpload`, in the run, the finisher or the spool's start.
    Abort,
    /// Preflight's bucket proof: a LIST of one key whose body is dropped.
    ListProof,
    /// A phase-7 LIST page (resume, verification).
    ListPage,
    /// A head GET: preflight's read, or a head write's read-back.
    HeadRead,
    /// A fenced head write; `if_match` when it names an `ETag`.
    HeadWrite { if_match: bool },
    /// The HEAD of a key after `NoSuchUpload` on a retried complete.
    Head,
}

impl Kind {
    /// Whether a try that is not settled counts in `unsettled_tries`: an
    /// output's, an archive's or a row file's request (`BATCH_SPEC.md` 1.6,
    /// 7.3), never an abort, a read or a head write.
    #[must_use]
    pub const fn counted(self) -> bool {
        matches!(self, Self::Put | Self::Part | Self::Create | Self::Complete)
    }
    /// The most 2xx body bytes the connector passes on.
    #[must_use]
    pub const fn cap(self) -> u64 {
        match self {
            Self::ListProof | Self::ListPage => LIST_CAP,
            Self::HeadRead => HEAD_CAP,
            _ => SMALL_CAP,
        }
    }
}
