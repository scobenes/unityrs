// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/Texture2DConverter.py's texture export for the formats it decodes with Pillow's raw and bcn decoders (Alpha8, ARGB4444, RGB24, RGBA32, ARGB32, RGB565, BGR24, DXT1, DXT3, DXT5, RGBA4444, BGRA32, BC7, BC4, BC5 and R8) and the ETC and EAC formats, with Pillow 9.5.0's and texture2ddecoder 1.0.5's decoders as it calls them. See THIRD_PARTY_LICENSES.
//! `Texture2D` decoding for the batch engine (`docs/plans/extract.md` E15,
//! E18, E59, E66): the five formats P1b exports, BC7, DXT1, DXT5, RGBA32 and
//! RGB24; of P3, the rest of the formats UnityPy decodes with Pillow's raw
//! and `bcn` decoders (Alpha8, ARGB4444, ARGB32, RGB565, BGR24, DXT3,
//! RGBA4444, BGRA32, BC4, BC5 and R8) and the eight ETC and EAC formats
//! (`ETC_RGB4`, `ETC2_RGB`, `ETC2_RGBA1`, `ETC2_RGBA8`, `EAC_R`,
//! `EAC_R_SIGNED`, `EAC_RG` and `EAC_RG_SIGNED`), decoded to the pixels
//! UnityPy 1.25.2 with Pillow 9.5.0 and texture2ddecoder 1.0.5 gives
//! (`Texture2D.image`, `export/Texture2DConverter.py:237-320`), bit for bit,
//! but for `ETC2_RGBA1`'s transparent pixels and a signed EAC base of -128
//! (Tier C, below), in its mode (`L` for BC4, `RGB` for RGB24, RGB565,
//! BGR24, R8 and BC5, `RGBA` for the rest: [`Format::mode`]) and its
//! orientation (flipped top to bottom), and which textures are exported or
//! skipped.
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
//! **How each format matches UnityPy.** UnityPy decodes the five, and the
//! eleven of P3's Pillow formats, with Pillow (`CONV_TABLE`,
//! `export/Texture2DConverter.py:488-566`), so each of their decoders here is
//! Pillow 9.5.0's, ported: the raw formats are its raw decoder
//! (`Image.frombytes(mode, size, data, "raw", rawmode)`, row by row through
//! the raw mode's unpacker, `src/libImaging/Unpack.c`, then UnityPy's band
//! order where it gives one, in the `raw` module), DXT1, DXT3, DXT5, BC4,
//! BC5 and BC7 its `bcn` decoder with 1, 2, 3, 4, 5 and 7
//! (`src/libImaging/BcnDecode.c`, in the `bcn` module), on the image padded
//! to whole 4x4 blocks, then cropped and flipped as UnityPy does
//! (`decode`'s module docs). texture2ddecoder, UnityPy's other decoder
//! library, decodes none of them; it decodes the ETC and EAC formats,
//! and its decoders for them are ported in the `etc` module, which writes
//! an `ETC2_RGBA1` pixel its punch-through rule makes transparent as
//! (0, 0, 0, 0), as the Khronos Data Format Specification 1.3, section
//! 22.9, does, where texture2ddecoder keeps its colour (`PARITY.md`
//! section 19.1). `tests/etc_oracle.rs` replays what UnityPy recorded for
//! them (`tools/fixtures/texture.py`, with `tools/fixtures/texture_etc.py`'s
//! blocks): every block mode, every size
//! class, long data, zero and negative sides, and Pillow's limits on
//! texture2ddecoder's output. `tests/oracle.rs` replays what the
//! oracle recorded (`tools/fixtures/texture.py`): each of the five at sizes
//! from 1x1 to 257x3, every BC7 mode, short, long and empty data, zero and
//! negative sides, Pillow's raw width cap and its crop's decompression-bomb
//! check, and the skips. `tests/raw_oracle.rs` replays the same for the
//! eleven others (`tools/fixtures/texture.py`, with
//! `tools/fixtures/texture_raw.py`'s data): every size class, every value of
//! each packed field, every endpoint order of the block formats, long, short
//! and empty data, zero and negative sides, each raw mode's width cap and
//! the crop bound.
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
mod etc;
mod format;
mod raw;
mod select;
pub mod sprite;

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
