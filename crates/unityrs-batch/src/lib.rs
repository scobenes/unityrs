//! The batch engine behind `unityrs batch` (`docs/plans/batch.md`,
//! `docs/BATCH_SPEC.md`): this crate starts with phase 1.
//!
//! - [`spec`]: the job spec, version 1 (`BATCH_SPEC` section 1), read from
//!   `JOB.json` and validated in section 10's `bad_spec` order to a typed
//!   [`spec::model::Spec`] with its defaults, or the first
//!   [`spec::BadSpec`] (`<rule> <pointer>`, exit 78). The `template` and
//!   `key(<rule>)` rules join the order later, through
//!   `unityrs_export::keys`.
//! - [`view`]: the id table in bytewise order, the `deps` graph and the
//!   iterative walk of an input's view (4.1, 4.2; `batch.md` 2.3).
//! - [`reason`]: exit codes and their precedence, the closed reason sets,
//!   the limit-kind table and the map from outcomes to codes (9.1 to 9.3).
//!
//! Requirements: `docs/plans/extract.md` E1, E5, E9, E11's view size and
//! E48's ranges. Nothing is on the command line yet.
#![forbid(unsafe_code)]
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

pub mod reason;
pub mod spec;
pub mod view;
