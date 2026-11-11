//! The scenes of a sheet's answers (`docs/BATCH_SPEC.md` section 3's order),
//! of sheets streamed from a `.resS` (4.3's own-chain procedure) and of the
//! crops Pillow refuses.

use super::super::{
    BuildError, PropName, atlas, file, format, material, mono_script, sheet, texture,
    texture_streamed,
};
use super::{Built, CutOptions, NULL, SCRIPT, SceneBundle, by_name, one, plain, rgba, scene};
use crate::env::{bundle, res_bytes, web_file};
use crate::ngui::V;

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
///
/// And 4.3's other rules. UnityPy's later candidates: `res.dat` is found as
/// `res.resource` (the second candidate), before the later row `res.resS`
/// (the fourth); `as.dat` as `as.assets.resS` (the third); `st.dat` as
/// `st.resS`. A container row of the key is passed over: `CAB-b.resS` is a
/// nested bundle (holding a row of that name, with other pixels, which the
/// registry would answer with) after the raw row `sub/CAB-b.resS`, of the
/// same key, which is read. Of two raw rows of one key, `CAB-c.resS` and
/// then `dir/CAB-c.resS`, the last is read. A stream path naming the reading
/// file itself, `CAB-x16`, finds a SerializedFile (`decode_error`, code
/// `malformed`, the texture and its atlas). A second SerializedFile,
/// `CAB-x16p`, has one external, naming the nested bundle `inner`, and an
/// atlas `pp` whose material pointer `(1, 1)` names that container row
/// (`bad_pointer`: no name, so `pp` cuts the texture of its own name), with
/// its MonoScript and that texture (on `CAB-x16` the external would be among
/// the names UnityPy's own lookup loads first).
pub fn streamed_sheets() -> Built {
    let streamed = |id, name: &str, size, path: &str, offset, len| {
        texture_streamed(id, name.as_bytes(), format::RGBA32, size, path, offset, len)
    };
    let mut own_res = res_bytes(5, 0x70);
    own_res.extend(sheet(0x17, 2, 2, false));
    let rows: [(i64, &str, &str); 6] = [
        (14, "res", "archive:/CAB-x16/res.dat"),
        (15, "as", "archive:/CAB-x16/as.dat"),
        (16, "st", "archive:/CAB-x16/st.dat"),
        (17, "bb", "archive:/CAB-x16/CAB-b.resS"),
        (18, "cc", "archive:/CAB-x16/CAB-c.resS"),
        (19, "self", "archive:/CAB-x16/CAB-x16"),
    ];
    let mut objects = vec![
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
    ];
    for (id, name, path) in rows {
        objects.push(streamed(id, name, (1, 1), path, 0, 4));
        objects.push(by_name(id + 10, name, vec![plain("s", (0, 0, 1, 1))]));
    }
    let cab = file(&[], &objects, true)?;
    let pointing = file(
        &["archive:/inner/inner"],
        &[
            mono_script(1, "NGUIAtlas"),
            atlas(
                2,
                b"pp",
                SCRIPT,
                (1, 1),
                vec![plain("s", (0, 0, 1, 1))],
                Vec::new(),
            ),
            rgba(3, "pp", 0x49, 1, 1),
        ],
        true,
    )?;
    let inner = bundle(&[("CAB-twin.resS", sheet(0x19, 2, 1, false))]);
    let nested_b = bundle(&[("CAB-b.resS", sheet(0x46, 1, 1, false))]);
    let dep = file(&[], &[], true)?;
    let x16 = |base| sheet(base, 1, 1, false);
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
                    ("CAB-x16p", pointing),
                    ("CAB-x16.resS", own_res),
                    ("CAB-twin.resS", sheet(0x18, 2, 1, false)),
                    ("inner", inner),
                    ("res.resource", x16(0x41)),
                    ("res.resS", x16(0x44)),
                    ("as.assets.resS", x16(0x42)),
                    ("st.resS", x16(0x43)),
                    ("sub/CAB-b.resS", x16(0x45)),
                    ("CAB-b.resS", nested_b),
                    ("CAB-c.resS", x16(0x47)),
                    ("dir/CAB-c.resS", x16(0x48)),
                ],
                &["d.bundle"],
            ),
        ],
        &["x.bundle"],
        CutOptions::textures("{name}.png"),
    ))
}

/// A bundle whose directory repeats a path, for 4.3's own-chain procedure on
/// the rows themselves, where UnityPy's `files` dict keeps one object per
/// path, the last row's, in the first row's place. Three raw rows of one key,
/// `sheet.resS`, `SHEET.resS` and `sheet.resS` again, each with other pixels:
/// the last is read (the dict, reversed, would give `SHEET.resS`'s). A raw
/// row `CAB-n.resS` and then a nested bundle at the same path, holding a row
/// of that name with other pixels: the bundle is passed over and the raw row
/// read (the dict holds only the bundle, and the registry the nested row).
pub fn repeated_rows() -> Built {
    let x16 = |base| sheet(base, 1, 1, false);
    let streamed = |id, name: &[u8], path: &str| {
        texture_streamed(id, name, format::RGBA32, (1, 1), path, 0, 4)
    };
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            streamed(2, b"rep", "archive:/CAB-x20/sheet.resS"),
            streamed(3, b"nest", "archive:/CAB-x20/CAB-n.resS"),
            by_name(4, "rep", vec![plain("s", (0, 0, 1, 1))]),
            by_name(5, "nest", vec![plain("s", (0, 0, 1, 1))]),
        ],
        true,
    )?;
    let nested = bundle(&[("CAB-n.resS", x16(0x55))]);
    Ok(scene(
        vec![one(
            "x.bundle",
            &[
                ("CAB-x20", cab),
                ("sheet.resS", x16(0x51)),
                ("SHEET.resS", x16(0x52)),
                ("sheet.resS", x16(0x53)),
                ("CAB-n.resS", x16(0x54)),
                ("CAB-n.resS", nested),
            ],
            &[],
        )],
        &["x.bundle"],
        CutOptions::textures("{name}.png"),
    ))
}

/// 4.3's lookup through the view, after the own chain finds no row, where a
/// container row's own name is a key (the core's `resource_try`). The
/// dependency `d.bundle` holds a raw `r.resS` and then a nested bundle at
/// the same path (holding only `other.resS`): `dr`'s stream names `r.resS`,
/// which no row of the input holds, and the key's last registration is the
/// container, so nothing is read (`outside_view(<path>)`), where UnityPy's
/// registry, which gives a container no key, answers with the raw row. The
/// input holds a nested bundle `n.resS` holding a raw `n.resS`: `nn`'s
/// stream names it, the own chain passes the container over, and the view's
/// lookup answers as the core does.
///
/// Pointers by name take the same registry. The dependency holds the
/// SerializedFile `CAB-dm` (a material, a MonoScript and the texture `mx`
/// the material names), then a nested bundle at the same path; the
/// SerializedFile `CAB-dn` is the same, but the container that takes its
/// key is the later dependency `e.bundle`'s. Through either, a pointer finds
/// nothing (`outside_view`): the atlases `am` and `an`, whose materials are
/// there, get no name and cut the textures of their own names, not `mx` or
/// `nx`; `ms` and `ns`, whose scripts are there, are unresolved.
///
/// The dependency's raw `w.resS` is followed by a top-level bundle holding
/// a container `w.resS`, two levels down and so no key: `wq` reads the raw
/// row. The input holds a container `q.resS` holding a raw `q.resS`, and the
/// dependency a raw `q.resource`: 4.3 reaches the container at its
/// registration, so the nested row, registered after the container's key,
/// answers `qq`'s first candidate. `sd`'s stream names the dependency's
/// SerializedFile `CAB-d22`: `decode_error`, `malformed`. The input's fifth
/// external names the dependency's raw `w.resS`: a pointer through it is
/// `bad_pointer`.
///
/// WebFile rows, each a key, top-level or inside a reached container, its
/// entries never registered; version 1's core refuses to open one
/// (`decode_error`, `unsupported`). The dependency's raw `w2.resS` is
/// followed by a top-level bundle holding a WebFile at `w2.resS`, which the
/// reach keys above it, so `wv`'s read fails; so does `wt`'s, whose stream
/// names a top-level WebFile row, and `wo`'s, whose names the input's own
/// WebFile row. The SerializedFile `CAB-dw` is followed by a top-level
/// WebFile row of its key (`sub/CAB-dw`), and `CAB-dz` by a bundle holding
/// a WebFile at `CAB-dz`: the atlases `aw` and `az`, whose materials are
/// there, get no name and cut the textures of their own names, not `wx` or
/// `zx`.
pub fn fallback_rows() -> Built {
    let x22 = |base| sheet(base, 1, 1, false);
    let streamed = |id, name: &[u8], path: &str| {
        texture_streamed(id, name, format::RGBA32, (1, 1), path, 0, 4)
    };
    let one_sprite = || vec![plain("s", (0, 0, 1, 1))];
    let cab = file(
        &[
            "archive:/CAB-dm/CAB-dm",
            "archive:/CAB-dn/CAB-dn",
            "archive:/CAB-dw/CAB-dw",
            "archive:/CAB-dz/CAB-dz",
            "archive:/CAB-d22/w.resS",
        ],
        &[
            mono_script(1, "NGUIAtlas"),
            streamed(2, b"dr", "archive:/CAB-x22/r.resS"),
            streamed(3, b"nn", "archive:/CAB-x22/n.resS"),
            by_name(4, "dr", one_sprite()),
            by_name(5, "nn", one_sprite()),
            atlas(6, b"am", SCRIPT, (1, 1), one_sprite(), Vec::new()),
            atlas(7, b"ms", (1, 2), NULL, one_sprite(), Vec::new()),
            atlas(8, b"an", SCRIPT, (2, 1), one_sprite(), Vec::new()),
            atlas(9, b"ns", (2, 2), NULL, one_sprite(), Vec::new()),
            rgba(10, "mx", 0x84, 1, 1),
            rgba(11, "am", 0x85, 1, 1),
            rgba(12, "nx", 0x86, 1, 1),
            rgba(13, "an", 0x87, 1, 1),
            streamed(14, b"wq", "archive:/CAB-x22/w.resS"),
            by_name(15, "wq", one_sprite()),
            streamed(16, b"qq", "archive:/CAB-x22/q.resS"),
            by_name(17, "qq", one_sprite()),
            streamed(18, b"sd", "archive:/CAB-d22/CAB-d22"),
            by_name(19, "sd", one_sprite()),
            streamed(20, b"wv", "archive:/CAB-x22/w2.resS"),
            by_name(21, "wv", one_sprite()),
            streamed(22, b"wt", "archive:/CAB-x22/w4.resS"),
            by_name(23, "wt", one_sprite()),
            atlas(24, b"aw", SCRIPT, (3, 1), one_sprite(), Vec::new()),
            atlas(25, b"az", SCRIPT, (4, 1), one_sprite(), Vec::new()),
            rgba(26, "wx", 0x94, 1, 1),
            rgba(27, "aw", 0x95, 1, 1),
            rgba(28, "zx", 0x96, 1, 1),
            rgba(29, "az", 0x97, 1, 1),
            streamed(30, b"wo", "archive:/CAB-x22/w5.resS"),
            by_name(31, "wo", one_sprite()),
        ],
        true,
    )?;
    let holder = bundle(&[("n.resS", x22(0x83))]);
    Ok(scene(
        vec![
            fallback_dependency()?,
            one(
                "e.bundle",
                &[
                    ("CAB-e22", file(&[], &[mono_script(1, "Dependency")], true)?),
                    ("CAB-dn", bundle(&[("pad.resS", x22(0x8D))])),
                ],
                &[],
            ),
            one(
                "x.bundle",
                &[
                    ("CAB-x22", cab),
                    ("n.resS", holder),
                    ("q.resS", bundle(&[("q.resS", x22(0x89))])),
                    ("w5.resS", web_file(&[("in5.resS", x22(0x99))])),
                ],
                &["d.bundle", "e.bundle"],
            ),
        ],
        &["x.bundle"],
        CutOptions::textures("{name}.png"),
    ))
}

/// `fallback_rows`' dependency `d.bundle`: its rows, as that scene's
/// documentation lists them.
fn fallback_dependency() -> Result<SceneBundle, BuildError> {
    let x22 = |base| sheet(base, 1, 1, false);
    let wrap = bundle(&[("w.resS", bundle(&[("inner.resS", x22(0x8A))]))]);
    let dep = file(&[], &[mono_script(1, "Dependency")], true)?;
    // A material naming its file's `<x>` (3), a MonoScript (2) and `<x>`.
    let held = |x: &str| {
        file(
            &[],
            &[
                material(1, "held", PropName::Plain, &[("_MainTex", (0, 3))]),
                mono_script(2, "NGUIAtlas"),
                rgba(3, x, 0x8F, 1, 1),
            ],
            true,
        )
    };
    let shadow = bundle(&[("other.resS", x22(0x82))]);
    Ok(one(
        "d.bundle",
        &[
            ("CAB-d22", dep),
            ("r.resS", x22(0x81)),
            ("r.resS", shadow),
            ("CAB-dm", held("mx")?),
            ("sub/CAB-dm", bundle(&[("pad.resS", x22(0x8B))])),
            ("CAB-dn", held("nx")?),
            ("w.resS", x22(0x88)),
            ("wrap", wrap),
            ("q.resource", x22(0x8C)),
            ("w2.resS", x22(0x90)),
            (
                "wrap2",
                bundle(&[("w2.resS", web_file(&[("in2.resS", x22(0x91))]))]),
            ),
            ("w4.resS", web_file(&[("in4.resS", x22(0x92))])),
            ("CAB-dw", held("wx")?),
            ("sub/CAB-dw", web_file(&[("pad3.resS", x22(0x93))])),
            ("CAB-dz", held("zx")?),
            (
                "wrap3",
                bundle(&[("CAB-dz", web_file(&[("pad4.resS", x22(0x98))]))]),
            ),
        ],
        &[],
    ))
}

/// Crops Pillow 9.5.0 refuses, on a 1 x 1 sheet: one of 20000 x 10000
/// pixels, past twice `Image.MAX_IMAGE_PIXELS` (`decompression_bomb`), one
/// at `x` 2^31 - 1 with a width of 1, whose right edge is past a C `int`
/// (`crop_overflow`), and one at `y` 2^31 - 1 with a height of 1, whose
/// bottom edge is (`crop_overflow`); each the sprite's skip at the plan,
/// beside a crop that is cut.
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
                    plain("low", (0, i32::MAX, 1, 1)),
                ],
            ),
            rgba(4, "big", 0x1F, 1, 1),
            by_name(5, "big", vec![wide_sprite()]),
        ],
    )
}

/// A sprite of `SInt64` fields at the origin, 2^33 wide and 2^31 high: its
/// area is 2^64, which a 64-bit product wraps to 0.
fn wide_sprite() -> V {
    let int64 = |name: &str, v: i64| (name.to_owned(), V::Int64(v));
    V::Struct(
        "UISpriteData".to_owned(),
        vec![
            ("name".to_owned(), V::Str(b"wide".to_vec())),
            int64("x", 0),
            int64("y", 0),
            int64("width", 1 << 33),
            int64("height", 1 << 31),
        ],
    )
}

/// A sheet whose decode fails, with textures exported under one key
/// (`sheet.png`), the JSON key one key too (`atlas.json`) and the sprite key
/// `{sprite}.png`. The texture `decoy`, first in plan order, takes
/// `sheet.png`, so the sheet `bad`'s texture output is `duplicate_key` at
/// the plan; of the two atlases cutting `bad`, the second, `a2`, has every
/// output `duplicate_key`. At the export the decode fails
/// (`not_enough_data`): one `skips` row, for `a1`, and none for the texture
/// (its output did not keep its key) nor for `a2` (no output still planned).
pub fn failed_sheet_rows() -> Built {
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "decoy", 0x61, 1, 1),
            texture(3, b"bad", format::RGBA32, (4, 4), vec![7; 10]),
            material(4, "bmat", PropName::Plain, &[("_MainTex", (0, 3))]),
            atlas(
                5,
                b"a1",
                SCRIPT,
                (0, 4),
                vec![plain("s", (0, 0, 1, 1))],
                Vec::new(),
            ),
            atlas(
                6,
                b"a2",
                SCRIPT,
                (0, 4),
                vec![plain("s", (0, 0, 1, 1))],
                Vec::new(),
            ),
        ],
        true,
    )?;
    let options = CutOptions {
        sprite_key: "{sprite}.png".to_owned(),
        json_key: "atlas.json".to_owned(),
        ..CutOptions::textures("sheet.png")
    };
    Ok(scene(
        vec![one("x.bundle", &[("CAB-x21", cab)], &[])],
        &["x.bundle"],
        options,
    ))
}
