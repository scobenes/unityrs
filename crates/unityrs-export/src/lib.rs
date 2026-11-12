//! Exports beside the textures, for the batch engine (`docs/DESIGN.md`
//! section 4's `unityrs-export` row): the NGUI atlas cutter
//! (`docs/plans/extract.md` E21 to E26, gate row X5) and the JSON writer it
//! writes with.
//!
//! - [`ngui::cut`]: one input's view to its atlases' crops (PNG), each
//!   atlas's JSON, the skips with their closed reasons and the sheet-to-atlas
//!   units, as `docs/BATCH_SPEC.md` sections 3, 4.3, 4.4 and 5 say, the
//!   textures `outputs.textures` selects among them.
//! - [`ngui::winners`]: 4.5's steps 1 to 3 over the cuts of several inputs.
//! - [`keys`]: key templates, `safe()` and the key rules, which the
//!   engine's validation checks a spec's templates and keys with.
//! - [`pyjson`]: `CPython` 3.11's `json.dumps(value, indent=2)` of a typetree
//!   value, byte for byte (E24).
//!
//! It is written from the specification, not ported from the Python
//! reference (`tools/parity/ngui_cutter.py`), which records the fixture it
//! is tested against (`unityrs-testkit`'s `tests/fixtures/ngui.json`).
//! The cutter checks `max_attempt_evictions` itself, after each Environment
//! call (registration's included), and `max_export_work` through the
//! engine's [`ngui::Hooks::export_work`], as it does the stop, the decode
//! work and the memory budgets; their values, the spool, the `sprites`
//! kind and 4.6's prefix conflicts are the engine's.
#![forbid(unsafe_code)]
// No panics on an input's contents (docs/plans/core.md section 3.15), as in
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

pub mod ngui;
pub mod pyjson;

/// Key templates, `safe()` and the key rules (`docs/BATCH_SPEC.md` 2.1 to
/// 2.3), the engine's and the cutter's one copy: validation checks a spec's
/// templates and literal keys through it ([`keys::check_template`],
/// [`keys::check_prefixed`]), and an attempt expands and checks its keys.
pub use ngui::keys;
