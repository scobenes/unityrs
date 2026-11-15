//! Image files byte-identical to Pillow 9.5.0's, and the visible-pixel hash
//! (`docs/plans/extract.md` E19, E20, E59; `docs/DESIGN.md` section 4).
//! [`writers`] also provides BMP/DIB, TGA, PPM, PCX, SGI, IM, raw TIFF,
//! DDS, ICO, ICNS and EPS, their typed refusals, and the extractor's PNG
//! fallback policy. `tools/fixtures/image_writers.py` records their oracle.
//!
//! - [`encode`] writes an image of mode `L`, `LA`, `RGB` or `RGBA` as
//!   Pillow 9.5.0's `Image.save(fp, "PNG")` writes it at its defaults, byte
//!   for byte: IHDR, the IDAT chunks as Pillow splits them (every chunk but
//!   the last `max(65536, 4 x width)` bytes), IEND; each row filtered as
//!   `ZipEncode.c` chooses (none, up, sub or Paeth, by least sum of
//!   distances from zero, the earlier on a tie); deflated by zlib 1.2.13 at
//!   level 6, windowBits 15, memLevel 9, `Z_FILTERED`. The PNG writer is
//!   `src/png.rs`, the filter `src/filter.rs`.
//!   Palette mode `P` adds PLTE and uses unfiltered rows with zlib's default
//!   strategy: 8-bit indices with an explicit palette, or packed 1-bit
//!   indices with Pillow's implicit one-entry palette.
//! - [`encode_webp`] writes Pillow 9.5.0's still-image WebP bytes through
//!   libwebp 1.3.0 with upstream failure-path fixes, generic C on every target, with bounded fallible
//!   allocation. [`webp_plan`] gives its measured and enforced allowance.
//! - [`encode_fast`] opts into zlib level 1 and a fixed Sub filter, with
//!   the same windowBits 15, memLevel 9 and `Z_FILTERED`, charge plan and
//!   fallible allocations. It preserves hidden RGB and uses the same
//!   palette depth and packing as [`encode`], trading compression ratio
//!   for speed; default byte parity is unchanged.
//! - [`visible_hash`] is E20's hash: SHA-256 over `"{width}x{height}:"` and
//!   the RGBA bytes, alpha-0 pixels cleared, other modes converted as
//!   Pillow's `convert("RGBA")` converts them.
//! - zlib is the vendored zlib 1.2.13 (`vendor/zlib-1.2.13/`, pristine,
//!   each file's sha256 in `vendor/zlib-1.2.13.sha256`). `build.rs` derives
//!   an altered deflate.c in `OUT_DIR`: a hash-guarded replacement of
//!   `longest_match` (`zlib-match/longest_match.c`) with equivalent memcpy
//!   word comparisons. It compiles statically under `Z_PREFIX`, at opt 3,
//!   allocating through Rust's
//!   global allocator ([`zlib_version`] says which zlib it is). The core's
//!   gzip stays on zlib-rs: `tests/flate2_backends.rs` fails when any crate
//!   enables a C zlib of flate2's.
//! - Every buffer sized by the image, and zlib's deflate state, is offered
//!   to the caller's [`Charger`] before it is allocated ([`charge`] module
//!   docs; [`plan_image`] gives the same figures ahead of time), and allocated
//!   fallibly: a refusal is an error, never an abort.
//!
//! The tests replay oracle fixtures that `tools/fixtures/imageio.py`
//! records with Pillow 9.5.0 over the oracle image's zlib 1.2.13
//! (`tests/pillow_png.rs`, gate rows X3 and X4).
//!
//! Untrusted sizes and bytes never panic: clippy denies `unwrap`, `expect`,
//! `panic` and indexing outside tests, as in the core (`docs/plans/core.md`
//! 3.15). WebP unsafe is confined to `src/webp_ffi.rs`; PNG unsafe to the
//! zlib bindings (`src/zlib.rs`) and the
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
mod webp;
mod webp_ffi;
pub mod writers;
mod zlib;
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub use webp::test_encode_webp_lossless;
pub use webp::{
    Webp, WebpError, WebpOptions, WebpPlan, encode_webp, webp_plan, webp_plan_with_options,
};

pub use charge::{Buffer, Charger, DEFLATE_STATE_REQUESTS, Plan, Unmetered, plan, plan_image};
pub use hash::visible_hash;
pub use image::{Image, ImageError, Mode};
pub use png::{EncodeError, Png, ZlibFailure, encode, encode_fast};
pub use zlib::zlib_version;

/// Test/fuzz support comparing the derived deflate with pristine zlib.
#[cfg(feature = "test-reference")]
#[doc(hidden)]
pub use zlib::reference as test_reference;

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

const _: () = send_sync::<WebpOptions>();
const _: () = send_sync::<WebpPlan>();
const _: () = send_sync::<Webp<()>>();
const _: () = send_sync::<WebpError<std::convert::Infallible>>();
