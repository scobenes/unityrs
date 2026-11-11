//! The scenes of keys, units and the JSON: collisions inside an input, a
//! sheet's unit, the JSON's values, and several inputs for the winners.

use super::super::{
    PropName, V, atlas, file, format, material, mono_script, sheet, sprite, st, texture, vector,
};
use super::{AtlasScene, CutOptions, NULL, SCRIPT, SceneBundle, one, plain, rgba, scene};

/// Collisions in one input: two sprites of one atlas with one key (the later
/// wins, E30), a second atlas of the same name (`duplicate_name`), and two
/// atlases whose sprites expand to one key (`p` / `q/r` and `p/q` / `r`), where
/// the first in plan order keeps it.
#[must_use]
pub fn duplicates() -> AtlasScene {
    let cab = file(
        &[],
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
        true,
    );
    scene(
        vec![one("x.bundle", &[("CAB-x6", cab)], &[])],
        &["x.bundle"],
        CutOptions::default(),
    )
}

fn two_atlases_cab() -> Vec<u8> {
    file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "shared", 0x11, 3, 2),
            rgba(3, "other", 0x21, 1, 1),
            material(4, "smat", PropName::Plain, &[("_MainTex", (0, 2))]),
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
        ],
        true,
    )
}

/// Two atlases cutting one sheet, with textures exported: one unit, keyed
/// by the sheet's texture key. The other texture, which no atlas cuts, is
/// planned, and left to the texture exporter.
#[must_use]
pub fn two_atlases_one_sheet() -> AtlasScene {
    scene(
        vec![one("x.bundle", &[("CAB-x7", two_atlases_cab())], &[])],
        &["x.bundle"],
        CutOptions {
            texture_key: Some("{name}.png".to_owned()),
            ..CutOptions::default()
        },
    )
}

/// Two atlases cutting one sheet, with textures exported under a key every
/// texture expands to (`sheet.png`): a texture before the sheet in object
/// order takes it, the sheet's texture is `duplicate_key`, so the input makes
/// no texture output for the sheet, and each atlas is a unit of its own.
#[must_use]
pub fn sheet_texture_key_taken() -> AtlasScene {
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "decoy", 0x61, 1, 1),
            rgba(3, "shared", 0x11, 3, 2),
            material(4, "smat", PropName::Plain, &[("_MainTex", (0, 3))]),
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
        ],
        true,
    );
    scene(
        vec![one("x.bundle", &[("CAB-x13", cab)], &[])],
        &["x.bundle"],
        CutOptions {
            texture_key: Some("sheet.png".to_owned()),
            ..CutOptions::default()
        },
    )
}

/// [`two_atlases_one_sheet`]'s bundle without textures: each atlas a unit.
#[must_use]
pub fn two_atlases_without_textures() -> AtlasScene {
    scene(
        vec![one("x.bundle", &[("CAB-x7", two_atlases_cab())], &[])],
        &["x.bundle"],
        CutOptions::default(),
    )
}

fn bytes_cab() -> Vec<u8> {
    let with_blob = |name: &str, rect| match plain(name, rect) {
        V::Struct(ty, mut fields) => {
            fields.push(("blob".to_owned(), V::Bytes(vec![1, 2, 3])));
            V::Struct(ty, fields)
        }
        other => other,
    };
    file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "bin", 9, 2, 2),
            atlas(
                3,
                b"bin",
                SCRIPT,
                NULL,
                vec![with_blob("c", (0, 0, 1, 1)), with_blob("d", (1, 1, 1, 1))],
                Vec::new(),
            ),
        ],
        true,
    )
}

/// An atlas whose sprites hold `TypelessData` (bytes): its JSON is
/// `json_unwritable` and its crops stay, with no unit (textures off).
#[must_use]
pub fn json_bytes() -> AtlasScene {
    scene(
        vec![one("x.bundle", &[("CAB-x8", bytes_cab())], &[])],
        &["x.bundle"],
        CutOptions::default(),
    )
}

/// [`json_bytes`] with textures: the crops are in the sheet's unit.
#[must_use]
pub fn json_bytes_with_textures() -> AtlasScene {
    scene(
        vec![one("x.bundle", &[("CAB-x8", bytes_cab())], &[])],
        &["x.bundle"],
        CutOptions {
            texture_key: Some("{name}.png".to_owned()),
            ..CutOptions::default()
        },
    )
}

/// Floats Python's `repr` writes in its own forms: doubles 1e-05, 1e+16,
/// -0.0, NaN and both infinities, floats widened exactly (0.1, 1e-05,
/// 16777217 as 16777216, -0.0, NaN, the largest finite), an `mSettings`
/// struct holding them, and `mPixelSize` 0.1.
#[must_use]
pub fn floats() -> AtlasScene {
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
        ],
    );
    let with_scale = match plain("x", (0, 0, 1, 1)) {
        V::Struct(ty, mut fields) => {
            fields.push(("scale".to_owned(), V::Float(0.1)));
            fields.push(("weight".to_owned(), V::Double(0.1)));
            V::Struct(ty, fields)
        }
        other => other,
    };
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
    let cab = file(
        &[],
        &[mono_script(1, "NGUIAtlas"), rgba(2, "f", 4, 1, 1), f_atlas],
        true,
    );
    scene(
        vec![one("x.bundle", &[("CAB-x9", cab)], &[])],
        &["x.bundle"],
        CutOptions::default(),
    )
}

/// Names outside ASCII: an atlas `ätlas`, sprites `精灵`, one with an
/// invalid UTF-8 byte (UnityPy's `surrogateescape` gives `\udcff`), and one
/// with a tab. The sprite key writes the names verbatim, so the last two are
/// `unwritable_key` (`not_utf8`, `control_char`); the JSON key is `safe()`'s.
#[must_use]
pub fn non_ascii() -> AtlasScene {
    let name = "ätlas".as_bytes();
    let cab = file(
        &[],
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
        true,
    );
    scene(
        vec![one("x.bundle", &[("CAB-x10", cab)], &[])],
        &["x.bundle"],
        CutOptions {
            json_key: "{safe_atlas}.json".to_owned(),
            ..CutOptions::default()
        },
    )
}

/// One input's bundle for [`winners`]: an atlas `atlas` cutting its own
/// sheet `sheet` of base `base`, with `sprites`, and `blob` when its JSON is
/// to be unwritable.
fn winner_input(id: &str, atlas_name: &str, base: u8, sprites: &[&str], blob: bool) -> SceneBundle {
    let list = sprites
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let x = i32::try_from(i % 2).unwrap_or(0);
            match plain(n, (x, 0, 1, 1)) {
                V::Struct(ty, mut fields) if blob => {
                    fields.push(("blob".to_owned(), V::Bytes(vec![0])));
                    V::Struct(ty, fields)
                }
                other => other,
            }
        })
        .collect();
    let cab_name = format!("CAB-{}", id.trim_end_matches(".bundle"));
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "sheet", base, 2, 1),
            material(3, "wmat", PropName::Plain, &[("_MainTex", (0, 2))]),
            atlas(4, atlas_name.as_bytes(), SCRIPT, (0, 3), list, Vec::new()),
        ],
        true,
    );
    one(id, &[(cab_name.as_str(), cab)], &[])
}

/// Several inputs cutting atlases into the same keys, for the winners
/// (`docs/BATCH_SPEC.md` 4.5), the sprite key `{sprite}.png`: `a` and `b`
/// alike, `c` with other pixels and one more sprite (so its JSON differs),
/// `d` whose JSON is unwritable (its crops plain candidates), and `e`, an
/// atlas `other` whose unit shares the key `k.png` with the others'.
#[must_use]
pub fn winners() -> AtlasScene {
    let bundles = vec![
        winner_input("a.bundle", "shared", 1, &["k", "j"], false),
        winner_input("b.bundle", "shared", 1, &["k", "j"], false),
        winner_input("c.bundle", "shared", 0x31, &["k", "j", "i"], false),
        winner_input("d.bundle", "shared", 0x41, &["k", "h"], true),
        winner_input("e.bundle", "other", 0x51, &["k"], false),
    ];
    let inputs = bundles.iter().map(|b| b.id.clone()).collect();
    AtlasScene {
        bundles,
        inputs,
        options: CutOptions {
            sprite_key: "{sprite}.png".to_owned(),
            ..CutOptions::default()
        },
    }
}
