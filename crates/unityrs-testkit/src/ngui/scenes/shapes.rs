//! The scenes of the shapes a read takes (`docs/BATCH_SPEC.md` 4.4 step 2,
//! section 3's answers, section 5 step 1): a texture's fields found by
//! their cleaned names and an atlas's by their raw names, a peeked name by
//! the raw key the peek selects, a texture's integers wide or signed as any
//! integer node holds them, a platform blob of any list, `m_TexEnvs` pairs
//! written as two-element lists, and keys past 1,024 bytes.

use super::super::{
    V, file, file_on, format, mono_script, pptr, s, set_field, sheet, st, texture,
    texture_streamed, vector,
};
use super::{Built, CutOptions, NULL, by_name, one, plain, rgba, scene};
use crate::ngui::{Object, atlas};

/// `BuildTarget.Switch`.
const SWITCH: i32 = 38;
/// `BuildTarget.XBOX360`.
const XBOX360: i32 = 11;

fn one_sprite() -> Vec<V> {
    vec![plain("s", (0, 0, 1, 1))]
}

fn stream(offset: V, size: V, path: &str) -> V {
    st(
        "StreamingInfo",
        vec![("offset", offset), ("size", size), ("path", s(path))],
    )
}

/// Fields found by UnityPy's cleaned names, with textures exported: a sheet
/// whose data node is named `image data` (`image_data` to UnityPy's
/// object), and one with an empty `image_data` and, after it, an
/// `image data` holding its pixels, the later of the two names that clean
/// to one key winning. Each has an atlas of its name.
pub fn cleaned_names() -> Built {
    let spaced = set_field(
        rgba(2, "spaced", 0xB0, 1, 1),
        "image_data",
        V::Bytes(sheet(0xB0, 1, 1, false)),
        Some("image data"),
    );
    let twice = set_field(
        rgba(3, "twice", 0xB1, 1, 1),
        "image_data",
        V::Bytes(Vec::new()),
        None,
    );
    let twice = set_field(
        twice,
        "image data",
        V::Bytes(sheet(0xB1, 1, 1, false)),
        None,
    );
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            spaced,
            twice,
            by_name(4, "spaced", one_sprite()),
            by_name(5, "twice", one_sprite()),
        ],
        true,
    )?;
    Ok(scene(
        vec![one("x.bundle", &[("CAB-xc", cab)], &[])],
        &["x.bundle"],
        CutOptions::textures("{name}.png"),
    ))
}

/// A texture's integers as any integer node holds them, with textures
/// exported (section 3's answers on the values as read):
/// - `neg_inline`: inline data and an `m_StreamData` whose `offset` is an
///   `SInt64` -1, never read, so the texture and its atlas export;
/// - `zero_huge`: `m_Width` 0 and a stream at offset 2^64 - 1:
///   `empty_image`, the stream unread;
/// - `wide_side`: `m_Width` an `unsigned int` 2^32 - 1, past `i32`:
///   more pixels than the decoders' bound;
/// - `blob_ints`: an `m_PlatformBlob` of `SInt32`s out of a byte's range,
///   on a platform that never reads it: exported;
/// - `wide_format`: `m_TextureFormat` an `SInt64` 2^40:
///   `unsupported_format(1099511627776)`;
/// - `neg_stream`, `neg_size`, `neg_empty`: streams of the input's own
///   8-byte `.resS` (two 1 x 1 sheets) at offset -4 for 12 bytes (the
///   second sheet), at 0 for -4 bytes (the first) and at -1 for 1 byte
///   (none: `no_image_data`), read as the reference slices them;
/// - `to_end`, `neg_to_end`: streams at offset 4 and at -4 of size -1,
///   which the memoryview reader's `read(-1)` reads to the end (the second
///   sheet; review round 2's C1).
pub fn wide_texture_fields() -> Built {
    let res_path = "archive:/CAB-xw/CAB-xw.resS";
    let mut res = sheet(0xC6, 1, 1, false);
    res.extend(sheet(0xC5, 1, 1, false));
    let neg_inline = set_field(
        rgba(2, "neg_inline", 0xC0, 1, 1),
        "m_StreamData",
        stream(V::Int64(-1), V::UInt(4), res_path),
        None,
    );
    let zero_huge = texture_streamed(
        3,
        b"zero_huge",
        format::RGBA32,
        (0, 1),
        res_path,
        u64::MAX,
        4,
    );
    let wide_side = set_field(
        rgba(4, "wide_side", 0xC2, 1, 1),
        "m_Width",
        V::UInt(u32::MAX),
        None,
    );
    let ints = vector(V::Int(0), vec![V::Int(300), V::Int(-1), V::Int(7)]);
    let blob_ints = set_field(
        rgba(5, "blob_ints", 0xC3, 1, 1),
        "m_PlatformBlob",
        ints,
        None,
    );
    let wide_format = set_field(
        rgba(6, "wide_format", 0xC4, 1, 1),
        "m_TextureFormat",
        V::Int64(1 << 40),
        None,
    );
    let streamed = |id: i64, name: &[u8], offset: V, size: V| {
        set_field(
            texture_streamed(id, name, format::RGBA32, (1, 1), res_path, 0, 4),
            "m_StreamData",
            stream(offset, size, res_path),
            None,
        )
    };
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            neg_inline,
            zero_huge,
            wide_side,
            blob_ints,
            wide_format,
            streamed(7, b"neg_stream", V::Int64(-4), V::UInt(12)),
            streamed(8, b"neg_size", V::UInt64(0), V::Int64(-4)),
            streamed(9, b"neg_empty", V::Int64(-1), V::UInt(1)),
            by_name(10, "neg_inline", one_sprite()),
            by_name(11, "neg_stream", one_sprite()),
            streamed(12, b"to_end", V::UInt64(4), V::Int64(-1)),
            streamed(13, b"neg_to_end", V::Int64(-4), V::Int64(-1)),
        ],
        true,
    )?;
    Ok(scene(
        vec![one(
            "x.bundle",
            &[("CAB-xw", cab), ("CAB-xw.resS", res)],
            &[],
        )],
        &["x.bundle"],
        CutOptions::textures("{name}.png"),
    ))
}

/// Fields by the raw names a typetree's dict keys them by, with textures
/// exported (review round 2's C2 and C3):
/// - the atlas `rn`, whose `mSprites` (one sprite) and `mPixelSize` (1.0)
///   are followed by `mSprites?` (two sprites) and `mPixelSize?` (2.0),
///   which UnityPy's object would take for them (each cleans to the other's
///   name) but its dict does not: one sprite, `pixel_size` 1.0;
/// - a texture whose `m_Name` is renamed `m Name` (`wrong`), followed by
///   `name` (`right`) and `m_Name` (`full`): the peek selects `name`, the
///   first child named `m_Name` or `name`, and reads it by that raw key,
///   so the texture is `right`, the sheet of the atlas `right`.
pub fn raw_names() -> Built {
    let two = vec![plain("q1", (0, 0, 1, 1)), plain("q2", (0, 0, 1, 1))];
    let rn = by_name(3, "rn", vec![plain("one", (0, 0, 1, 1))]);
    let rn = set_field(rn, "mSprites?", vector(plain("", (0, 0, 0, 0)), two), None);
    let rn = set_field(rn, "mPixelSize?", V::Float(2.0), None);
    let named = set_field(
        rgba(4, "x", 0xF1, 1, 1),
        "m_Name",
        s("wrong"),
        Some("m Name"),
    );
    let named = set_field(named, "name", s("right"), None);
    let named = set_field(named, "m_Name", s("full"), None);
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "rn", 0xF0, 1, 1),
            rn,
            named,
            by_name(5, "right", one_sprite()),
        ],
        true,
    )?;
    Ok(scene(
        vec![one("x.bundle", &[("CAB-xr", cab)], &[])],
        &["x.bundle"],
        CutOptions::textures("{name}.png"),
    ))
}

/// Keys past 1,024 bytes (2.3, review round 2's C7): textures exported
/// under `{name}`, each name 1,035 bytes of segments within 255, then a
/// control character, a trailing `/` or a last segment `..`, each past
/// byte 1,025, so each key is `too_long`; and one short name, written.
pub fn long_keys() -> Built {
    let long = |c: char, tail: &str| {
        let seg = c.to_string().repeat(200);
        format!("{}/{}{tail}", [seg.as_str(); 5].join("/"), "t".repeat(30))
    };
    let cab = file(
        &[],
        &[
            rgba(2, &long('c', "\u{1}"), 0xF4, 1, 1),
            rgba(3, &long('d', "/"), 0xF5, 1, 1),
            rgba(4, &long('e', "/.."), 0xF6, 1, 1),
            rgba(5, "ok", 0xF7, 1, 1),
        ],
        true,
    )?;
    Ok(scene(
        vec![one("x.bundle", &[("CAB-xk", cab)], &[])],
        &["x.bundle"],
        CutOptions::textures("{name}"),
    ))
}

/// A 12-element blob of `item(i)`s.
fn blob(shape: V, item: impl Fn(usize) -> V) -> V {
    vector(shape, (0..12).map(item).collect())
}

/// Platform blobs and byte-swapped formats, with textures exported: on a
/// Switch file, a blob of bytes whose GOB count (bytes 8 to 12) is 1
/// (`unsupported_platform(switch)`, the texture and its atlas), one whose
/// count is 0 (exported), one of 11 `SInt32`s, shorter than the 12 bytes
/// the check reads (exported); on an Xbox 360 file, a DXT1 texture
/// (`unsupported_platform(xbox360)`) and an RGBA32 one (exported). A Switch
/// blob whose bytes 8 to 12 are not all bytes is not here: the reference
/// raises out of its cut there (UnityPy's `int.from_bytes`), so the rule
/// the spec names for it is the cutter's unit test's.
pub fn platform_blobs() -> Built {
    let with_blob = |o: Object, b: V| set_field(o, "m_PlatformBlob", b, None);
    let bytes = |eight: u8| {
        blob(V::UInt8(0), move |i| {
            V::UInt8(if i == 8 { eight } else { 0 })
        })
    };
    let short = vector(V::Int(0), (0..11).map(|_| V::Int(-1)).collect());
    let switch = file_on(
        Some(SWITCH),
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            with_blob(rgba(2, "gobs", 0xD0, 1, 1), bytes(1)),
            with_blob(rgba(3, "flat", 0xD1, 1, 1), bytes(0)),
            with_blob(rgba(5, "short", 0xD3, 1, 1), short),
            by_name(7, "gobs", one_sprite()),
        ],
        true,
    )?;
    let xbox = file_on(
        Some(XBOX360),
        &[],
        &[
            texture(2, b"xb_dxt1", format::DXT1, (4, 4), vec![0; 8]),
            rgba(3, "xb_rgba", 0xD5, 1, 1),
        ],
        true,
    )?;
    Ok(scene(
        vec![one(
            "x.bundle",
            &[("CAB-xs", switch), ("CAB-xx", xbox)],
            &[],
        )],
        &["x.bundle"],
        CutOptions::textures("{name}.png"),
    ))
}

/// Section 5 step 1 over `m_TexEnvs` pairs written as two-element lists (a
/// `vector` of two structs, each a `name` and an `m_Texture`): the first's
/// `name` is `_MainTex`, the second's `m_Texture` the sheet, which the atlas
/// cuts rather than the texture of its own name.
pub fn list_pairs() -> Built {
    let entry = |name: &str, tex: (i32, i64)| {
        st(
            "TexEnvEntry",
            vec![("name", s(name)), ("m_Texture", pptr("Texture", tex))],
        )
    };
    let pair = |tex: (i32, i64)| {
        vector(
            entry("", NULL),
            vec![entry("_MainTex", NULL), entry("", tex)],
        )
    };
    let envs = V::Vector("map".to_owned(), Box::new(pair(NULL)), vec![pair((0, 3))]);
    let sheet_props = st("UnityPropertySheet", vec![("m_TexEnvs", envs)]);
    let material = Object::new(
        10,
        21,
        st(
            "Material",
            vec![
                ("m_Name", s("lpm")),
                ("m_Shader", pptr("Shader", (0, 0))),
                ("m_SavedProperties", sheet_props),
            ],
        ),
    );
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "lp", 0xE0, 1, 1),
            rgba(3, "by_material", 0xE1, 1, 1),
            material,
            atlas(4, b"lp", super::SCRIPT, (0, 10), one_sprite(), Vec::new()),
        ],
        true,
    )?;
    Ok(scene(
        vec![one("x.bundle", &[("CAB-xl", cab)], &[])],
        &["x.bundle"],
        CutOptions::default(),
    ))
}

/// An atlas whose `m_Script` names a file past the externals
/// (`bad_pointer`): not an atlas, counted in `unresolved_scripts`, beside
/// one that cuts.
pub fn bad_script_pointer() -> Built {
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "ok", 0xE8, 1, 1),
            rgba(3, "lost", 0xE9, 1, 1),
            by_name(4, "ok", one_sprite()),
            atlas(5, b"lost", (5, 1), NULL, one_sprite(), Vec::new()),
        ],
        true,
    )?;
    Ok(scene(
        vec![one("x.bundle", &[("CAB-xp", cab)], &[])],
        &["x.bundle"],
        CutOptions::default(),
    ))
}
