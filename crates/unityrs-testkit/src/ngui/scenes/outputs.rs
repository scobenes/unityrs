//! The scenes of keys, units and the JSON: collisions inside an input, a
//! sheet's unit, the export's order, and the JSON's values.

use super::super::{
    Object, PropName, V, atlas, file, format, material, mono_script, pair, s, sheet, sprite, st,
    texture, vector,
};
use super::{Built, CutOptions, NULL, SCRIPT, alone, by_name, one, plain, rgba, scene, with};

/// Collisions in one input: two sprites of one atlas with one key (the later
/// wins, E30), a second atlas of the same name (`duplicate_name`), and two
/// atlases whose sprites expand to one key (`p` / `q/r` and `p/q` / `r`), where
/// the first in plan order keeps it.
pub fn duplicates() -> Built {
    alone(
        "CAB-x6",
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "dsheet", 2, 4, 2),
            atlas(
                3,
                b"dup",
                SCRIPT,
                (0, 4),
                vec![
                    plain("s", (0, 0, 1, 1)),
                    plain("t", (1, 0, 1, 1)),
                    plain("s", (2, 0, 2, 2)),
                ],
                Vec::new(),
            ),
            material(4, "dmat", PropName::Plain, &[("_MainTex", (0, 2))]),
            atlas(
                5,
                b"dup",
                SCRIPT,
                (0, 4),
                vec![plain("u", (0, 0, 1, 1))],
                Vec::new(),
            ),
            atlas(
                6,
                b"p",
                SCRIPT,
                (0, 4),
                vec![plain("q/r", (0, 0, 1, 1))],
                Vec::new(),
            ),
            atlas(
                7,
                b"p/q",
                SCRIPT,
                (0, 4),
                vec![plain("r", (1, 1, 1, 1))],
                Vec::new(),
            ),
        ],
    )
}

fn two_atlases(textures: [Object; 2], shared: i64) -> Vec<Object> {
    let [first, second] = textures;
    vec![
        mono_script(1, "NGUIAtlas"),
        first,
        second,
        material(4, "smat", PropName::Plain, &[("_MainTex", (0, shared))]),
        atlas(
            5,
            b"left",
            SCRIPT,
            (0, 4),
            vec![plain("l", (0, 0, 1, 2))],
            Vec::new(),
        ),
        atlas(
            6,
            b"right",
            SCRIPT,
            (0, 4),
            vec![plain("r", (1, 0, 2, 2))],
            Vec::new(),
        ),
    ]
}

fn shared_first() -> Vec<Object> {
    two_atlases(
        [rgba(2, "shared", 0x11, 3, 2), rgba(3, "other", 0x21, 1, 1)],
        2,
    )
}

/// Two atlases cutting one sheet, with textures exported: one unit, keyed
/// by the sheet's texture key. The other texture, which no atlas cuts, is a
/// plain texture output, exported after the sheet's group.
pub fn two_atlases_one_sheet() -> Built {
    let cab = file(&[], &shared_first(), true)?;
    Ok(scene(
        vec![one("x.bundle", &[("CAB-x7", cab)], &[])],
        &["x.bundle"],
        CutOptions::textures("{name}.png"),
    ))
}

/// Two atlases cutting one sheet, with textures exported under a key every
/// texture expands to (`sheet.png`): a texture before the sheet in object
/// order takes it (a plain texture output), the sheet's texture is
/// `duplicate_key`, so the input makes no texture output for the sheet, and
/// each atlas is a unit of its own.
pub fn sheet_texture_key_taken() -> Built {
    let objects = two_atlases(
        [rgba(2, "decoy", 0x61, 1, 1), rgba(3, "shared", 0x11, 3, 2)],
        3,
    );
    let cab = file(&[], &objects, true)?;
    Ok(scene(
        vec![one("x.bundle", &[("CAB-x13", cab)], &[])],
        &["x.bundle"],
        CutOptions::textures("sheet.png"),
    ))
}

/// [`two_atlases_one_sheet`]'s bundle without textures: each atlas a unit.
pub fn two_atlases_without_textures() -> Built {
    let cab = file(&[], &shared_first(), true)?;
    Ok(scene(
        vec![one("x.bundle", &[("CAB-x7", cab)], &[])],
        &["x.bundle"],
        CutOptions::default(),
    ))
}

/// A texture key equal to its atlas's JSON key (`{name}.json` and
/// `{atlas}.json`): textures come first in plan order, so the sheet's
/// texture takes `tk.json`, the atlas's JSON is `duplicate_key`, and the
/// crop is in the unit `tk.json`, the texture's.
pub fn texture_key_is_json_key() -> Built {
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "tk", 0x1C, 2, 1),
            by_name(3, "tk", vec![plain("s", (0, 0, 2, 1))]),
        ],
        true,
    )?;
    Ok(scene(
        vec![one("x.bundle", &[("CAB-x18", cab)], &[])],
        &["x.bundle"],
        CutOptions::textures("{name}.json"),
    ))
}

/// Two sheet groups whose first atlases come in the other order than their
/// textures: `ga` and `gc` cut `s2`, `gb` cuts `s1`, and `ga` and `gb` each
/// have a JSON that is unwritable, an export skip. The groups export in the
/// order of their first atlas (`s2`'s, then `s1`'s), so `ga`'s skip is
/// recorded before `gb`'s.
pub fn two_sheet_groups() -> Built {
    let blob = |n: &str| with(plain(n, (0, 0, 1, 1)), vec![("blob", V::Bytes(vec![5]))]);
    alone(
        "CAB-x19",
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "s1", 0x1D, 1, 1),
            rgba(3, "s2", 0x1E, 1, 1),
            material(4, "m1", PropName::Plain, &[("_MainTex", (0, 2))]),
            material(5, "m2", PropName::Plain, &[("_MainTex", (0, 3))]),
            atlas(6, b"ga", SCRIPT, (0, 5), vec![blob("x")], Vec::new()),
            atlas(7, b"gb", SCRIPT, (0, 4), vec![blob("x")], Vec::new()),
            atlas(
                8,
                b"gc",
                SCRIPT,
                (0, 5),
                vec![plain("x", (0, 0, 1, 1))],
                Vec::new(),
            ),
        ],
    )
}

fn bytes_cab() -> Built {
    let with_blob =
        |name: &str, rect| with(plain(name, rect), vec![("blob", V::Bytes(vec![1, 2, 3]))]);
    alone(
        "CAB-x8",
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "bin", 9, 2, 2),
            by_name(
                3,
                "bin",
                vec![with_blob("c", (0, 0, 1, 1)), with_blob("d", (1, 1, 1, 1))],
            ),
        ],
    )
}

/// An atlas whose sprites hold `TypelessData` (bytes): its JSON is
/// `json_unwritable` and its crops stay, with no unit (textures off).
pub fn json_bytes() -> Built {
    bytes_cab()
}

/// [`json_bytes`] with textures: the crops are in the sheet's unit.
pub fn json_bytes_with_textures() -> Built {
    let mut built = bytes_cab()?;
    built.options = CutOptions::textures("{name}.png");
    Ok(built)
}

/// Floats Python's `repr` writes in its own forms: doubles 1e-05, 1e+16,
/// -0.0, NaN and both infinities, floats widened exactly (0.1, 1e-05,
/// 16777217 as 16777216, -0.0, NaN, the largest finite), an `mSettings`
/// struct holding them and a pair (an array of two), and `mPixelSize` 0.1.
pub fn floats() -> Built {
    let settings = st(
        "AtlasSettings",
        vec![
            ("d1", V::Double(1e-05)),
            ("d2", V::Double(1e16)),
            ("d3", V::Double(-0.0)),
            ("d4", V::Double(f64::NAN)),
            ("d5", V::Double(f64::INFINITY)),
            ("d6", V::Double(f64::NEG_INFINITY)),
            ("f1", V::Float(1e-05)),
            ("f2", V::Float(16_777_217.0)),
            ("f3", V::Float(-0.0)),
            ("f4", V::Float(f32::NAN)),
            ("f5", V::Float(f32::MAX)),
            ("i", V::Int(-7)),
            ("b", V::Bool(true)),
            (
                "list",
                vector(V::Float(0.0), vec![V::Float(0.1), V::Float(2.5)]),
            ),
            ("pair", pair(V::Int(3), s("three"))),
        ],
    );
    let with_scale = with(
        plain("x", (0, 0, 1, 1)),
        vec![("scale", V::Float(0.1)), ("weight", V::Double(0.1))],
    );
    let mut f_atlas = atlas(
        3,
        b"f",
        SCRIPT,
        NULL,
        vec![with_scale],
        vec![("mSettings", settings)],
    );
    if let V::Struct(_, fields) = &mut f_atlas.value {
        for (n, v) in fields.iter_mut() {
            if n == "mPixelSize" {
                *v = V::Float(0.1);
            }
        }
    }
    alone(
        "CAB-x9",
        &[mono_script(1, "NGUIAtlas"), rgba(2, "f", 4, 1, 1), f_atlas],
    )
}

/// Names outside ASCII: an atlas `ätlas`, sprites `精灵`, one with an
/// invalid UTF-8 byte (UnityPy's `surrogateescape` gives `\udcff`), and one
/// with a tab. The sprite key writes the names verbatim, so the last two are
/// `unwritable_key` (`not_utf8`, `control_char`); the JSON key is `safe()`'s.
pub fn non_ascii() -> Built {
    let name = "ätlas".as_bytes();
    let mut built = alone(
        "CAB-x10",
        &[
            mono_script(1, "NGUIAtlas"),
            texture(2, name, format::RGBA32, (2, 2), sheet(6, 2, 2, false)),
            atlas(
                3,
                name,
                SCRIPT,
                NULL,
                vec![
                    sprite("精灵".as_bytes(), (0, 0, 1, 1), [0; 8]),
                    sprite(b"bad\xff", (1, 0, 1, 1), [0; 8]),
                    sprite(b"tab\tname", (0, 1, 1, 1), [0; 8]),
                ],
                Vec::new(),
            ),
        ],
    )?;
    "{safe_atlas}.json".clone_into(&mut built.options.json_key);
    Ok(built)
}
