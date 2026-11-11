//! PNG files byte-identical to Pillow 9.5.0's, and the visible-pixel hash
//! (`docs/plans/extract.md` E19, E20, E59; `docs/DESIGN.md` section 4).
//!
//! - [`encode`] writes an image of mode `L`, `LA`, `RGB` or `RGBA` as
//!   Pillow 9.5.0's `Image.save(fp, "PNG")` writes it at its defaults, byte
//!   for byte: IHDR, the IDAT chunks as Pillow splits them (every chunk but
//!   the last `max(65536, 4 x width)` bytes), IEND; each row filtered as
//!   `ZipEncode.c` chooses (none, up, sub or Paeth, by least sum of
//!   distances from zero, the earlier on a tie); deflated by zlib 1.2.13 at
//!   level 6, windowBits 15, memLevel 9, `Z_FILTERED`. The PNG writer is
//!   `src/png.rs`, the filter `src/filter.rs`.
//! - [`visible_hash`] is E20's hash: SHA-256 over `"{width}x{height}:"` and
//!   the RGBA bytes, alpha-0 pixels cleared, other modes converted as
//!   Pillow's `convert("RGBA")` converts them.
//! - zlib is the vendored zlib 1.2.13 (`vendor/zlib-1.2.13/`, pristine,
//!   each file's sha256 in `vendor/zlib-1.2.13.sha256`), compiled
//!   statically by `build.rs` under `Z_PREFIX` and allocating through Rust's
//!   global allocator ([`zlib_version`] says which zlib it is). The core's
//!   gzip stays on zlib-rs: `tests/flate2_backends.rs` fails when any crate
//!   enables a C zlib of flate2's.
//! - Every buffer sized by the image, and zlib's deflate state, is offered
//!   to the caller's [`Charger`] before it is allocated ([`charge`] module
//!   docs; [`plan`] gives the same figures ahead of time), and allocated
//!   fallibly: a refusal is an error, never an abort.
//!
//! The tests replay oracle fixtures that `tools/fixtures/imageio.py`
//! records with Pillow 9.5.0 over the oracle image's zlib 1.2.13
//! (`tests/pillow_png.rs`, gate rows X3 and X4).
//!
//! Untrusted sizes and bytes never panic: clippy denies `unwrap`, `expect`,
//! `panic` and indexing outside tests, as in the core (`docs/plans/core.md`
//! 3.15). `unsafe` is confined to the zlib bindings (`src/zlib.rs`) and the
//! two calls of [`encode`] that hand zlib its input and output.

#![deny(unsafe_code)]
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

pub mod charge;
mod filter;
mod hash;
mod image;
#[cfg(test)]
mod memory_tests;
mod png;
mod zlib;

pub use charge::{Buffer, Charger, DEFLATE_STATE_REQUESTS, Plan, Unmetered, plan};
pub use hash::visible_hash;
pub use image::{Image, ImageError, Mode};
pub use png::{EncodeError, Png, ZlibFailure, encode};
pub use zlib::zlib_version;

/// Asserts at compile time that `T` is `Send + Sync` (`docs/plans/core.md`
/// 3.16).
const fn send_sync<T: Send + Sync + ?Sized>() {}

const _: () = send_sync::<Image<'static>>();
const _: () = send_sync::<Mode>();
const _: () = send_sync::<ImageError>();
const _: () = send_sync::<Plan>();
const _: () = send_sync::<Buffer>();
const _: () = send_sync::<Unmetered>();
const _: () = send_sync::<Png<()>>();
const _: () = send_sync::<EncodeError<std::convert::Infallible>>();
const _: () = send_sync::<ZlibFailure>();
