// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/Texture2DConverter.py's texture export for the formats it decodes with Pillow's raw and bcn decoders (Alpha8, ARGB4444, RGB24, RGBA32, ARGB32, RGB565, BGR24, DXT1, DXT3, DXT5, RGBA4444, BGRA32, BC7, BC4, BC5 and R8), the ETC and EAC formats, the ASTC LDR formats, PVRTC and ATC, with Pillow 9.5.0's, texture2ddecoder 1.0.5's and astc-encoder-py 0.1.12's decoders as it calls them. See THIRD_PARTY_LICENSES.
//! `Texture2D` decoding for the batch engine (`docs/plans/extract.md` E15,
//! E18, E59, E66): the five formats P1b exports, BC7, DXT1, DXT5, RGBA32 and
//! RGB24; of P3, the rest of the formats UnityPy decodes with Pillow's raw
//! and `bcn` decoders (Alpha8, ARGB4444, ARGB32, RGB565, BGR24, DXT3,
//! RGBA4444, BGRA32, BC4, BC5 and R8) and the eight ETC and EAC formats
//! (`ETC_RGB4`, `ETC2_RGB`, `ETC2_RGBA1`, `ETC2_RGBA8`, `EAC_R`,
//! `EAC_R_SIGNED`, `EAC_RG` and `EAC_RG_SIGNED`), and the twelve ASTC LDR
//! formats (`ASTC_RGB_NxN` and `ASTC_RGBA_NxN`, N of 4, 5, 6, 8, 10 and
//! 12), the four PVRTC formats and the two ATC formats, decoded to the
//! pixels UnityPy 1.25.2 with Pillow 9.5.0,
//! texture2ddecoder 1.0.5 and astc-encoder-py 0.1.12 gives
//! (`Texture2D.image`, `export/Texture2DConverter.py:237-320`), bit for bit,
//! but for `ETC2_RGBA1`'s transparent pixels and a signed EAC base of -128
//! (Tier C, below), in its mode (`L` for BC4, `RGB` for RGB24, RGB565,
//! BGR24, R8 and BC5, `RGBA` for the rest: [`Format::mode`]) and its
//! orientation (flipped top to bottom), and which textures are exported or
//! skipped.
//!
//! Legacy `DXT1Crunched` and `DXT5Crunched` are also supported through the
//! `crunch` module and existing BC1/BC3 decoders. [`select_in`] and
//! [`select_crunch`] take the Unity version and choose legacy or Unity fork
//! DXT; crunched ETC always uses the Unity fork and existing ETC decoders.
//! Version 2 may allow-list Unity's four crunched names; version 1 keeps
//! its five formats. Native builder fixtures and the pinned UnityPy pixel
//! fixture cover both forks (`tests/crunch_oracle.rs` and
//! `tests/unitycrunch_oracle.rs`).
//!
//! - [`select()`]: a texture's fields to [`Selection::Export`] with its
//!   [`Format`], or [`Selection::Skip`] with the [`Skip`] reason the manifest
//!   records (`docs/BATCH_SPEC.md` section 3). Pure; it reads no data.
//! - [`plan()`]: what decoding a format at a size from data of a given
//!   length needs (mip 0's bytes, or crunch's minimum header) and gives (the image's mode, size and
//!   bytes, and what it charges), or the error the decode would return,
//!   before any data is read. Crunch can also return a content error after
//!   planning succeeds. This lets the engine count the work and the
//!   memory first.
//! - [`decode()`]: a texture's data (mip 0 first; the rest is ignored, as
//!   Pillow ignores it) to an [`Image`], its pixels charged to the caller's
//!   meter before they are allocated (and ASTC tables or crunch scratch, while
//!   it runs), its pixel loop run under [`contain()`] so a panic is an
//!   error ([`DecodeError::Panicked`]).
//!
//! [`plan_in`] and [`decode_in`] add platform fields: Switch GOB deswizzling
//! (including RGB24/BGR24 promotion), Xbox byte pairs before crunch, and the
//! crop after decoding. [`array`] maps `GraphicsFormat` values, resolves streamed
//! data and decodes slices, applying PARITY section 19.2's B/C corrections
//! and platform refusals. [`pad_image`] repeats edge pixels for encoder callers.
//!
//! [`write`] converts Pillow image storage to raw texture bytes, generates
//! exact BICUBIC mips, pads and swizzles Switch writes, and installs object
//! edits through `SerializedFile` save. Nonempty encoder-backed targets return
//! [`write::WriteError::EncoderUnavailable`].
//!
//! **Tier B/C raw formats.** The twenty-two raw formats of `PARITY.md`
//! sections 17.1, 19.1 and 19.2 use independent closed forms in `tierb`,
//! with no Pillow conversion. `tests/tierb_oracle.rs` pins UnityPy's actual
//! images/exceptions beside those references. Nonempty `ARGBFloat` and
//! odd-width YUY2 layouts are refused by selection and planning; zero
//! sides retain the empty image behavior. These decoders charge only
//! their output and need no scratch allocation.
//!
//! **BC6H and ASTC HDR.** BC6H implements Khronos Data Format Specification
//! 1.3 section 20.2; ASTC HDR ports the full HDR profile of the same
//! astcenc revision as LDR. Both produce binary16 values before applying
//! PARITY section 17.1's f64 clamp and rounding rule. `tests/bc6h_oracle.rs`
//! and `tests/astc_hdr_oracle.rs` replay the references beside UnityPy's
//! failures or incorrect LDR-profile results (Tier B and Tier C).
//! [`decode_bc6h_signed`] exposes the signed form; Unity's format selects
//! unsigned. Reserved or illegal HDR encodings return [`HdrError`].
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
//! the crop bound. UnityPy decodes the ASTC formats with astcenc
//! (astc-encoder-py 0.1.12's, Arm astcenc at commit 631fb04), with the LDR
//! profile into 8-bit output; the `astc` module ports its decoder, error
//! colour and edge handling included. `tests/astc_oracle.rs` replays what
//! UnityPy recorded for the twelve (`tools/fixtures/texture_astc.py`):
//! blocks astcenc's encoder made, random blocks, cut edges, long, short and
//! empty data, zero and negative sides, and Pillow's limits on astcenc's
//! output; the crate's unit tests replay `Texture2DConverter.astc` alone at
//! every footprint astcenc takes, on blocks crafted to reach every block
//! mode class, partition count, dual-plane channel, endpoint mode, void
//! extent and error block.
//!
//! PVRTC and ATC port texture2ddecoder 1.0.5 (`pvrtc` and `atc` modules).
//! The four PVRTC names retain encoded alpha, use Morton order with wrapped
//! endpoint/modulation neighbors, and pad only to the block footprint;
//! ATC retains the original dimensions and uses interpolated alpha for
//! `ATC_RGBA8`. See `PARITY.md` section 17 for the reference quirks and the
//! coordinator-generated oracle replay (`tests/pvrtc_atc_oracle.rs`).
//!
//! **No panics, bounded work** (`docs/plans/core.md` section 3.15): indexing,
//! `unwrap` and the like are denied outside tests; sizes are computed in
//! `u64` from the `i32` sides; a texture past [`MAX_PIXELS`] (16384 x
//! 16384, padded) is refused whatever its data's length once it is not
//! empty, before any allocation, the
//! one bound UnityPy does not have (`PARITY.md` section 19.3), while
//! Pillow's own limits below it, its raw decoder's width cap and its crop's
//! decompression-bomb check, are matched (`decode`'s module docs); a
//! decode's pixel work is linear in its output. Crunch's header-derived
//! palette and model setup also counts against the meter's work allowance.

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

pub mod array;
mod astc;
mod atc;
mod bc6h;
mod bcn;
mod contain;
mod crunch;
mod decode;
mod etc;
pub mod fetch;
mod format;
mod formats;
mod hdr;
mod image;
pub mod platform;
mod pvrtc;
mod raw;
mod select;
pub mod sprite;
mod tierb;
pub mod write;

pub use contain::{MESSAGE_BYTES, NON_STRING_PAYLOAD, Payload, contain};
pub use crunch::CrunchError;
pub use decode::{
    DecodeError, Image, MAX_CROP_PIXELS, MAX_PIXELS, Plan, bcn_length, decode, decode_bc6h_signed,
    plan,
};
pub use format::{Format, Mode, TEXTURE_FORMATS, texture_format_name};
pub use formats::Formats;
pub use hdr::HdrError;
pub use image::{image_from_pixels, pad_image};
pub use platform::{PlatformError, decode_in, decode_in_as, plan_in, plan_in_as};

/// Decode one BC6H block to row-major binary16 RGB bit patterns, using
/// Khronos Data Format Specification 1.3 section 20.2. Reserved modes are
/// refused. `signed` selects the signed encoding, including negative zero.
pub fn decode_bc6h_block(data: &[u8; 16], signed: bool) -> Result<[[u16; 3]; 16], HdrError> {
    bc6h::block(data, signed).ok_or(HdrError::ReservedMode)
}
pub use select::{
    SWITCH, Selection, Skip, SkippedPlatform, TextureFields, XBOX_SWAP_FORMATS, XBOX360,
    is_switch_swizzled, select, select_crunch, select_in,
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
