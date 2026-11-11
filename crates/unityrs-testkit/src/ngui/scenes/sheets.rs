//! The scenes of a sheet's answers (`docs/BATCH_SPEC.md` section 3's order),
//! of sheets streamed from a `.resS` (4.3's own-chain procedure) and of the
//! crops Pillow refuses.

use super::super::{file, format, mono_script, sheet, texture, texture_streamed};
use super::{Built, CutOptions, by_name, one, plain, rgba, scene};
use crate::env::{bundle, res_bytes};

/// Sheets whose answers are skips, with textures exported: an `Alpha8`
/// sheet (`unsupported_format(Alpha8)`) and a zero-height one
/// (`empty_image`), selection's skips, which skip their atlases at the plan;
/// then, at the export, an RGBA32 sheet with too few bytes
/// (`not_enough_data`), a DXT1 sheet of 16385 x 16381 pixels with 8 bytes
/// of data, under the decoders' bound of 2^28 pixels but over it padded to
/// whole blocks (16388 x 16384), so `limit(texture_pixels, 268435456)`
/// before any decode, an RGBA32 sheet one pixel wider than Pillow's raw
/// decoder's cap (`pillow_memory_error`), one with a negative side
/// (`negative_size`) and one with an empty `image_data` and no
/// `m_StreamData` (`no_image_data`).
pub fn sheet_skips() -> Built {
    let one_sprite = || vec![plain("a", (0, 0, 1, 1))];
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            texture(2, b"alpha", format::ALPHA8, (2, 2), vec![1, 2, 3, 4]),
            texture(3, b"short", format::RGBA32, (4, 4), vec![7; 10]),
            texture(4, b"flat", format::RGBA32, (2, 0), Vec::new()),
            by_name(5, "alpha", one_sprite()),
            by_name(6, "short", one_sprite()),
            by_name(7, "flat", one_sprite()),
            texture(8, b"huge", format::DXT1, (16_385, 16_381), vec![0; 8]),
            texture(9, b"wide", format::RGBA32, (67_108_857, 1), vec![0; 4]),
            texture(10, b"neg", format::RGBA32, (-2, 2), vec![0; 16]),
            texture(11, b"nodata", format::RGBA32, (2, 2), Vec::new()),
            by_name(12, "huge", one_sprite()),
            by_name(13, "wide", one_sprite()),
            by_name(14, "neg", one_sprite()),
            by_name(15, "nodata", one_sprite()),
        ],
        true,
    )?;
    Ok(scene(
        vec![one("x.bundle", &[("CAB-x12", cab)], &[])],
        &["x.bundle"],
        CutOptions::textures("{name}.png"),
    ))
}

/// Sheets streamed from a `.resS`, with textures exported: from the input's
/// own `CAB-x16.resS` (at offset 5, after five bytes of `res_bytes`), from a
/// dependency's (`CAB-d16.resS`), from one no bundle of the view holds
/// (`outside_view`, the texture and its atlas), and from `CAB-twin.resS`,
/// which the input holds as a top-level row and again inside a nested
/// bundle registered after it, with other pixels: the own-chain procedure
/// reads the top-level row, where UnityPy's registry would answer with the
/// nested one. A texture whose stream path is empty has no data
/// (`no_image_data`); one with a negative side and streamed data is
/// `negative_size`.
pub fn streamed_sheets() -> Built {
    let streamed = |id, name: &str, size, path: &str, offset, len| {
        texture_streamed(id, name.as_bytes(), format::RGBA32, size, path, offset, len)
    };
    let mut own_res = res_bytes(5, 0x70);
    own_res.extend(sheet(0x17, 2, 2, false));
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            streamed(2, "own", (2, 2), "archive:/CAB-x16/CAB-x16.resS", 5, 16),
            streamed(3, "dep", (1, 2), "archive:/CAB-d16/CAB-d16.resS", 0, 8),
            streamed(4, "gone", (1, 1), "archive:/CAB-x16/CAB-missing.resS", 0, 4),
            streamed(5, "twin", (2, 1), "archive:/CAB-x16/CAB-twin.resS", 0, 8),
            streamed(6, "empty", (2, 2), "", 0, 0),
            streamed(7, "negs", (-1, 1), "archive:/CAB-x16/CAB-x16.resS", 5, 4),
            by_name(8, "own", vec![plain("s", (0, 0, 2, 2))]),
            by_name(9, "dep", vec![plain("s", (0, 0, 1, 2))]),
            by_name(10, "gone", vec![plain("s", (0, 0, 1, 1))]),
            by_name(11, "twin", vec![plain("s", (0, 0, 2, 1))]),
            by_name(12, "empty", vec![plain("s", (0, 0, 1, 1))]),
            by_name(13, "negs", vec![plain("s", (0, 0, 1, 1))]),
        ],
        true,
    )?;
    let inner = bundle(&[("CAB-twin.resS", sheet(0x19, 2, 1, false))]);
    let dep = file(&[], &[], true)?;
    Ok(scene(
        vec![
            one(
                "d.bundle",
                &[("CAB-d16", dep), ("CAB-d16.resS", sheet(0x1A, 1, 2, false))],
                &[],
            ),
            one(
                "x.bundle",
                &[
                    ("CAB-x16", cab),
                    ("CAB-x16.resS", own_res),
                    ("CAB-twin.resS", sheet(0x18, 2, 1, false)),
                    ("inner", inner),
                ],
                &["d.bundle"],
            ),
        ],
        &["x.bundle"],
        CutOptions::textures("{name}.png"),
    ))
}

/// Crops Pillow 9.5.0 refuses, on a 1 x 1 sheet: one of 20000 x 10000
/// pixels, past twice `Image.MAX_IMAGE_PIXELS` (`decompression_bomb`), and
/// one at `x` 2^31 - 1 with a width of 1, whose right edge is past a C `int`
/// (`crop_overflow`); each the sprite's skip at the plan, beside a crop that
/// is cut.
pub fn crop_bounds() -> Built {
    super::alone(
        "CAB-x17",
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "cb", 0x1B, 1, 1),
            by_name(
                3,
                "cb",
                vec![
                    plain("ok", (0, 0, 1, 1)),
                    plain("bomb", (0, 0, 20_000, 10_000)),
                    plain("far", (i32::MAX, 0, 1, 1)),
                ],
            ),
        ],
    )
}
