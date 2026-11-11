// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/Texture2DConverter.py's texture export for BC7, DXT1, DXT5, RGBA32 and RGB24, with Pillow 9.5.0's decoders as it calls them. See THIRD_PARTY_LICENSES.
//! `Texture2D` decoding for the batch engine (`docs/plans/extract.md` E15,
//! E18, E59, E66): the five formats P1b exports, BC7, DXT1, DXT5, RGBA32 and
//! RGB24, decoded to the pixels UnityPy 1.25.2 with Pillow 9.5.0 gives
//! (`Texture2D.image`, `export/Texture2DConverter.py:237-320`), bit for bit,
//! in its mode (`RGB` for RGB24, `RGBA` for the rest) and its orientation
//! (flipped top to bottom), and which textures are exported or skipped.
//!
//! - [`select()`]: a texture's fields to [`Selection::Export`] with its
//!   [`Format`], or [`Selection::Skip`] with the [`Skip`] reason the manifest
//!   records (`docs/BATCH_SPEC.md` section 3). Pure; it reads no data.
//! - [`plan()`]: what decoding a format at a size from data of a given
//!   length needs (mip 0's bytes) and gives (the image's mode, size and
//!   bytes, and what it charges), or the error the decode would return,
//!   before any data is read, so the engine can count the work and the
//!   memory first.
//! - [`decode()`]: a texture's data (mip 0 first; the rest is ignored, as
//!   Pillow ignores it) to an [`Image`], its pixels charged to the caller's
//!   meter before they are allocated, its pixel loop run under [`contain()`]
//!   so a panic is an error ([`DecodeError::Panicked`]).
//!
//! **How each format matches UnityPy.** UnityPy decodes these five with
//! Pillow (`CONV_TABLE`, `export/Texture2DConverter.py:491-516`), so each
//! decoder here is Pillow 9.5.0's, ported: RGB24 and RGBA32 are its raw
//! decoder (`Image.frombytes(mode, size, data, "raw", mode)`, a row copy),
//! DXT1, DXT5 and BC7 its `bcn` decoder with 1, 3 and 7
//! (`src/libImaging/BcnDecode.c`, in the `bcn` module), on the image padded
//! to whole 4x4 blocks, then cropped and flipped as UnityPy does
//! (`decode`'s module docs). texture2ddecoder, UnityPy's other decoder
//! library, decodes none of the five. `tests/oracle.rs` replays what the
//! oracle recorded (`tools/fixtures/texture.py`): every format at sizes
//! from 1x1 to 257x3, every BC7 mode, short, long and empty data, zero and
//! negative sides, Pillow's raw width cap and its crop's decompression-bomb
//! check, and the skips.
//!
//! **No panics, bounded work** (`docs/plans/core.md` section 3.15): indexing,
//! `unwrap` and the like are denied outside tests; sizes are computed in
//! `u64` from the `i32` sides; a texture past [`MAX_PIXELS`] (16384 x
//! 16384, padded) is refused whatever its data's length once it is not
//! empty, before any allocation, the
//! one bound UnityPy does not have (`PARITY.md` section 19.3), while
//! Pillow's own limits below it, its raw decoder's width cap and its crop's
//! decompression-bomb check, are matched (`decode`'s module docs); a
//! decode's work is linear in its output.

#![forbid(unsafe_code)]
// No panics on a texture's contents (plan section 3.15). Tests may opt out.
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

mod bcn;
mod contain;
mod decode;
mod format;
mod select;

pub use contain::{MESSAGE_BYTES, NON_STRING_PAYLOAD, Payload, contain};
pub use decode::{DecodeError, Image, MAX_CROP_PIXELS, MAX_PIXELS, Plan, bcn_length, decode, plan};
pub use format::{Format, Mode, TEXTURE_FORMATS, texture_format_name};
pub use select::{
    SWITCH, Selection, Skip, SkippedPlatform, TextureFields, XBOX_SWAP_FORMATS, XBOX360,
    is_switch_swizzled, select,
};

/// Asserts at compile time that `T` is `Send + Sync` (plan section 3.16).
pub(crate) const fn send_sync<T: Send + Sync + ?Sized>() {}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Plan 3.16: every public type is `Send + Sync`, so the engine's pool
    /// threads may move and share them.
    #[test]
    fn every_public_type_is_asserted() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let missing = unityrs_testkit::public_types::unasserted(&src);
        assert!(missing.is_empty(), "not asserted Send + Sync: {missing:?}");
    }

    /// Plan 4.1: no thread, no wait; the decoders run on the engine's
    /// threads.
    #[test]
    fn the_decoders_start_no_thread_and_never_wait() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let found = unityrs_testkit::waits::waits(&src);
        assert!(found.is_empty(), "waits or starts a thread: {found:?}");
    }
}
