//! The batch engine behind `unityrs batch` (`docs/plans/batch.md`,
//! `docs/BATCH_SPEC.md`): validation and sealed-attempt artifact primitives.
//!
//! - [`spec`]: the job spec, version 1 (`BATCH_SPEC` section 1), read from
//!   `JOB.json` and validated in section 10's `bad_spec` order to a typed
//!   [`spec::model::Spec`] with its defaults, or the first
//!   [`spec::BadSpec`] (`<rule> <pointer>`, exit 78), the `template` and
//!   `key(<rule>)` rules through `unityrs_export::keys`.
//! - [`view`]: the id table in bytewise order, the `deps` graph and the
//!   iterative walk of an input's view (4.1, 4.2; `batch.md` 2.3).
//! - [`alloc`]: the counting allocator the binary installs, which counts
//!   each refusal and each thread's live bytes (`batch.md` 2.7).
//! - [`bound`]: 5.1's memory bound, `--check`'s `memory_bound`, whose
//!   constants are provisional until X12 measures them.
//! - [`progress`]: the progress stream's lines (section 10), `ms` on the
//!   boot clock from the process's start ([`progress::clock`]), and the
//!   main thread's own `exit` line, bounded in time ([`progress::early`]).
//! - [`sys`]: the C library's calls std lacks: 2.11 step 1's `SIG_IGN`
//!   for SIGTTIN, SIGTTOU and SIGXFSZ, and `CLOCK_BOOTTIME`.
//! - [`budget`], [`work`]: reservation accounts, leak snapshots and export work.
//! - [`spool`], [`digest`], [`records`]: owned disk claims, checksums and
//!   bounded typed attempt artifacts, without orchestration or publication.
//! - [`reason`]: exit codes and their precedence, the closed reason sets,
//!   the limit-kind table and the map from outcomes to codes (9.1 to 9.3).
//!
//! Requirements: `docs/plans/extract.md` E1, E5, E9, E11's view size and
//! E48's ranges; E2, E3 and E49 for `unityrs batch --check` and the
//! allocator, which the CLI (`unityrs-cli`) drives.
// `unsafe` only in `alloc`, the counting allocator, whose `GlobalAlloc`
// methods need it, and `sys`, the C library's calls std lacks
// (`tests/unsafe_scan.rs` pins both).
#![deny(unsafe_code)]
// No panics on a spec's contents (docs/plans/core.md section 3.15), as in
// the core and the decoders. Tests may opt out.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable,
        clippy::indexing_slicing
    )
)]

#[allow(unsafe_code)]
pub mod alloc;
pub mod bound;
pub mod budget;
pub mod digest;
pub mod progress;
pub mod reason;
pub mod records;
pub mod spec;
pub mod spool;
#[allow(unsafe_code)]
pub mod sys;
pub mod view;
pub mod work;
