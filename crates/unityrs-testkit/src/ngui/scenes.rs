//! The NGUI scenes, one function each, and [`scenes`], which lists them by
//! name for `tools/parity/ngui_scenes.py`.
//!
//! Each scene is a set of bundles with the `deps` a job spec would give them
//! (`docs/BATCH_SPEC.md` 1.2), the inputs to cut, each over its own view
//! (4.2), and the cutter's options (1.5's `atlases`, and `textures`'s key
//! when the scene exports textures). Sheets are [`super::sheet`]s, so a
//! test computes every crop's pixels from [`super::pixel`] by hand.

use super::{
    Object, PropName, V, atlas, file, format, material, mono_behaviour, mono_script, s, sheet,
    sprite, st, texture, vector,
};
use crate::env::bundle;

mod outputs;
pub use outputs::{
    duplicates, floats, json_bytes, json_bytes_with_textures, non_ascii, sheet_texture_key_taken,
    two_atlases_one_sheet, two_atlases_without_textures, winners,
};

/// One bundle of a scene: its spec `id`, its bytes and its `deps`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneBundle {
    /// The spec `id`.
    pub id: String,
    /// The bundle's bytes.
    pub data: Vec<u8>,
    /// The spec `deps`.
    pub deps: Vec<String>,
}

/// The cutter's options (`docs/BATCH_SPEC.md` 1.5), the sheet scope
/// `own_files_by_name` being version 1's only one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CutOptions {
    /// `atlases.sprite_key`.
    pub sprite_key: String,
    /// `atlases.json_key`.
    pub json_key: String,
    /// `textures.key`, when the scene exports textures.
    pub texture_key: Option<String>,
    /// `atlases.script_classes`.
    pub script_classes: Vec<String>,
    /// `atlases.skip_name_suffixes`.
    pub skip_name_suffixes: Vec<String>,
}

impl Default for CutOptions {
    fn default() -> Self {
        Self {
            sprite_key: "{atlas}/{sprite}.png".to_owned(),
            json_key: "{atlas}.json".to_owned(),
            texture_key: None,
            script_classes: vec!["NGUIAtlas".to_owned()],
            skip_name_suffixes: Vec::new(),
        }
    }
}

/// A scene: bundles, the inputs cut (each over its view) and the options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AtlasScene {
    /// The bundles, inputs and dependencies alike.
    pub bundles: Vec<SceneBundle>,
    /// The ids of the inputs, in the order a test cuts them.
    pub inputs: Vec<String>,
    /// The cutter's options.
    pub options: CutOptions,
}

pub(super) fn one(id: &str, cabs: &[(&str, Vec<u8>)], deps: &[&str]) -> SceneBundle {
    SceneBundle {
        id: id.to_owned(),
        data: bundle(cabs),
        deps: deps.iter().map(|d| (*d).to_owned()).collect(),
    }
}

pub(super) fn scene(bundles: Vec<SceneBundle>, inputs: &[&str], options: CutOptions) -> AtlasScene {
    AtlasScene {
        bundles,
        inputs: inputs.iter().map(|i| (*i).to_owned()).collect(),
        options,
    }
}

/// The atlas script every scene's atlases name, at path id 1.
pub(super) const SCRIPT: (i32, i64) = (0, 1);

pub(super) fn rgba(path_id: i64, name: &str, base: u8, w: u8, h: u8) -> Object {
    let size = (i32::from(w), i32::from(h));
    texture(
        path_id,
        name.as_bytes(),
        format::RGBA32,
        size,
        sheet(base, w, h, false),
    )
}

pub(super) fn plain(name: &str, rect: (i32, i32, i32, i32)) -> V {
    sprite(name.as_bytes(), rect, [0; 8])
}

pub(super) const NULL: (i32, i64) = (0, 0);

/// Step 1 by a plain-string `_MainTex`, in the atlas's own file. A texture
/// named as the atlas comes first in object order, and loses to the
/// material's. The sprites cover a crop inside the sheet, one past its right
/// and bottom edges, one past its top-left, and two empty rectangles; the
/// first has borders and paddings, which are not applied, and a `rotated`
/// field is written as read.
#[must_use]
pub fn own_sheet_by_material_name() -> AtlasScene {
    let trimmed = match sprite(b"a", (0, 0, 2, 2), [1, 2, 3, 4, 5, 6, 7, 8]) {
        V::Struct(ty, mut fields) => {
            fields.push(("rotated".to_owned(), V::Bool(true)));
            V::Struct(ty, fields)
        }
        other => other,
    };
    let mut sprites = vec![trimmed];
    for (name, rect) in [
        ("b", (1, 2, 3, 2)),
        ("edge", (3, 3, 2, 2)),
        ("neg", (-1, -1, 2, 2)),
        ("zero", (0, 0, 0, 2)),
        ("negw", (2, 2, -1, 1)),
    ] {
        let s = match plain(name, rect) {
            V::Struct(ty, mut fields) => {
                fields.push(("rotated".to_owned(), V::Bool(false)));
                V::Struct(ty, fields)
            }
            other => other,
        };
        sprites.push(s);
    }
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "ui", 0x90, 2, 2),
            rgba(3, "sheet_a", 1, 4, 4),
            material(
                4,
                "ui_mat",
                PropName::Plain,
                &[("_BumpMap", NULL), ("_MainTex", (0, 3))],
            ),
            atlas(5, b"ui", SCRIPT, (0, 4), sprites, Vec::new()),
        ],
        true,
    );
    scene(
        vec![one("x.bundle", &[("CAB-x1", cab)], &[])],
        &["x.bundle"],
        CutOptions::default(),
    )
}

/// Step 1 by a `FastPropertyName` `_MainTex` whose texture is in a
/// dependency: its name is read there, and matched among the input's own
/// textures, where a texture of that name is the sheet (and a texture named
/// as the atlas loses to it).
#[must_use]
pub fn struct_maintex_in_dependency() -> AtlasScene {
    let dep = file(&[], &[rgba(10, "shared_sheet", 0x50, 1, 1)], true);
    let cab = file(
        &["archive:/CAB-d1/CAB-d1"],
        &[
            mono_script(1, "NGUIAtlas"),
            material(2, "m", PropName::Fast, &[("_MainTex", (1, 10))]),
            rgba(3, "shared_sheet", 0x30, 3, 3),
            atlas(
                4,
                b"far",
                SCRIPT,
                (0, 2),
                vec![plain("s", (0, 0, 3, 3))],
                Vec::new(),
            ),
            rgba(5, "far", 0x70, 1, 1),
        ],
        true,
    );
    scene(
        vec![
            one("d.bundle", &[("CAB-d1", dep)], &[]),
            one("x.bundle", &[("CAB-x2", cab)], &["d.bundle"]),
        ],
        &["x.bundle"],
        CutOptions::default(),
    )
}

/// The material and its `_MainTex` texture both in a dependency: the name
/// read there is none of the input's textures, so step 2 takes the texture
/// named as the atlas, an RGB24 sheet, cropped past its edge.
#[must_use]
pub fn material_in_dependency() -> AtlasScene {
    let dep = file(
        &[],
        &[
            material(20, "m2", PropName::Plain, &[("_MainTex", (0, 21))]),
            rgba(21, "only_in_dep", 0x60, 1, 1),
        ],
        true,
    );
    let sheet_b = texture(22, b"atlas_b", format::RGB24, (2, 2), sheet(5, 2, 2, true));
    let cab = file(
        &["archive:/CAB-d2/CAB-d2"],
        &[
            mono_script(1, "NGUIAtlas"),
            atlas(
                2,
                b"atlas_b",
                SCRIPT,
                (1, 20),
                vec![plain("wide", (1, 0, 3, 3)), plain("in", (0, 1, 2, 1))],
                Vec::new(),
            ),
            sheet_b,
        ],
        true,
    );
    scene(
        vec![
            one("d.bundle", &[("CAB-d2", dep)], &[]),
            one("x.bundle", &[("CAB-x3", cab)], &["d.bundle"]),
        ],
        &["x.bundle"],
        CutOptions::default(),
    )
}

/// A null material gives no name, so step 2 takes the texture named as the
/// atlas; `skip_name_suffixes` `["_ref"]` skips the reference atlas; a null
/// material with no texture of the atlas's name is `sheet_not_found`.
#[must_use]
pub fn null_material_and_reference() -> AtlasScene {
    let one_sprite = || vec![plain("p", (0, 0, 2, 1))];
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "plain", 7, 2, 1),
            atlas(3, b"plain", SCRIPT, NULL, one_sprite(), Vec::new()),
            atlas(4, b"plain_ref", SCRIPT, NULL, one_sprite(), Vec::new()),
            atlas(5, b"lost", SCRIPT, NULL, one_sprite(), Vec::new()),
        ],
        true,
    );
    scene(
        vec![one("x.bundle", &[("CAB-x4", cab)], &[])],
        &["x.bundle"],
        CutOptions {
            skip_name_suffixes: vec!["_ref".to_owned()],
            ..CutOptions::default()
        },
    )
}

/// Every way step 1 gives no name, each then `sheet_not_found` (no texture
/// has the atlas's name): no `_MainTex` pair (n1, whose `_BumpMap` names an
/// own texture, which a cutter taking the first pair would find), a
/// material in a CAB outside the view (n2), a missing object (n3), a null
/// `m_Texture` (n4) and an `m_FileID` past the externals (n5); and a
/// `_MainTex` texture, read in a dependency, whose name no own texture has
/// (n6).
#[must_use]
pub fn missing_sheets() -> AtlasScene {
    let one_sprite = || vec![plain("p", (0, 0, 1, 1))];
    let dep = file(&[], &[rgba(1, "elsewhere", 0x21, 1, 1)], true);
    let cab = file(
        &["archive:/CAB-gone/CAB-gone", "archive:/CAB-d5/CAB-d5"],
        &[
            mono_script(1, "NGUIAtlas"),
            material(2, "no_main", PropName::Plain, &[("_BumpMap", (0, 4))]),
            material(3, "null_tex", PropName::Plain, &[("_MainTex", NULL)]),
            rgba(4, "bump", 3, 1, 1),
            material(5, "named", PropName::Plain, &[("_MainTex", (2, 1))]),
            atlas(10, b"n1", SCRIPT, (0, 2), one_sprite(), Vec::new()),
            atlas(11, b"n2", SCRIPT, (1, 99), one_sprite(), Vec::new()),
            atlas(12, b"n3", SCRIPT, (0, 98), one_sprite(), Vec::new()),
            atlas(13, b"n4", SCRIPT, (0, 3), one_sprite(), Vec::new()),
            atlas(14, b"n5", SCRIPT, (5, 2), one_sprite(), Vec::new()),
            atlas(15, b"n6", SCRIPT, (0, 5), one_sprite(), Vec::new()),
        ],
        true,
    );
    scene(
        vec![
            one("d.bundle", &[("CAB-d5", dep)], &[]),
            one("x.bundle", &[("CAB-x5", cab)], &["d.bundle"]),
        ],
        &["x.bundle"],
        CutOptions::default(),
    )
}

/// Recognition: a MonoBehaviour of another class, a null `m_Script` and one
/// to a missing object (both `unresolved_scripts`), one whose `m_Script` is
/// no MonoScript, NGUI 2's `sprites` layout and an `mSprites` that is no
/// array (`ngui_layout_unsupported`), an empty name (`unnamed_atlas`), and,
/// in a second file with no typetrees, an atlas script's MonoBehaviour
/// (`no_typetree`).
#[must_use]
pub fn layouts() -> AtlasScene {
    let legacy = st(
        "Sprite",
        vec![
            ("name", s("old")),
            (
                "outer",
                st(
                    "Rect",
                    vec![
                        ("x", V::Float(0.0)),
                        ("y", V::Float(0.0)),
                        ("width", V::Float(1.0)),
                        ("height", V::Float(1.0)),
                    ],
                ),
            ),
            ("rotated", V::Bool(false)),
        ],
    );
    let first = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            mono_script(2, "UIPanel"),
            rgba(3, "t", 1, 1, 1),
            mono_behaviour(10, b"panel", (0, 2), vec![("depth", V::Int(3))]),
            mono_behaviour(
                11,
                b"t",
                NULL,
                vec![("mSprites", vector(V::Int(0), Vec::new()))],
            ),
            mono_behaviour(
                12,
                b"t",
                (0, 77),
                vec![("mSprites", vector(V::Int(0), Vec::new()))],
            ),
            mono_behaviour(
                13,
                b"old",
                SCRIPT,
                vec![("sprites", vector(legacy.clone(), vec![legacy]))],
            ),
            mono_behaviour(14, b"flat", SCRIPT, vec![("mSprites", V::Int(0))]),
            atlas(
                15,
                b"",
                SCRIPT,
                NULL,
                vec![plain("e", (0, 0, 1, 1))],
                Vec::new(),
            ),
            mono_behaviour(
                16,
                b"t",
                (0, 3),
                vec![("mSprites", vector(V::Int(0), Vec::new()))],
            ),
        ],
        true,
    );
    let bare = file(
        &["archive:/CAB-x11a/CAB-x11a"],
        &[atlas(
            1,
            b"bare",
            (1, 1),
            NULL,
            vec![plain("e", (0, 0, 1, 1))],
            Vec::new(),
        )],
        false,
    );
    scene(
        vec![one(
            "x.bundle",
            &[("CAB-x11a", first), ("CAB-x11b", bare)],
            &[],
        )],
        &["x.bundle"],
        CutOptions::default(),
    )
}

/// Sheets whose answers are skips, with textures exported: an `Alpha8`
/// sheet (`unsupported_format(Alpha8)`) and a zero-height one
/// (`empty_image`), selection's skips, which skip their atlases at the plan;
/// and an RGBA32 sheet with too few bytes, whose decode fails
/// (`decode_error`, `not_enough_data`) and skips its atlas at the export.
#[must_use]
pub fn sheet_skips() -> AtlasScene {
    let one_sprite = || vec![plain("a", (0, 0, 1, 1))];
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            texture(2, b"alpha", format::ALPHA8, (2, 2), vec![1, 2, 3, 4]),
            texture(3, b"short", format::RGBA32, (4, 4), vec![7; 10]),
            texture(4, b"flat", format::RGBA32, (2, 0), Vec::new()),
            atlas(5, b"alpha", SCRIPT, NULL, one_sprite(), Vec::new()),
            atlas(6, b"short", SCRIPT, NULL, one_sprite(), Vec::new()),
            atlas(7, b"flat", SCRIPT, NULL, one_sprite(), Vec::new()),
        ],
        true,
    );
    scene(
        vec![one("x.bundle", &[("CAB-x12", cab)], &[])],
        &["x.bundle"],
        CutOptions {
            texture_key: Some("{name}.png".to_owned()),
            ..CutOptions::default()
        },
    )
}

/// Every scene, by name.
#[must_use]
pub fn scenes() -> Vec<(&'static str, AtlasScene)> {
    vec![
        ("own_sheet_by_material_name", own_sheet_by_material_name()),
        (
            "struct_maintex_in_dependency",
            struct_maintex_in_dependency(),
        ),
        ("material_in_dependency", material_in_dependency()),
        ("null_material_and_reference", null_material_and_reference()),
        ("missing_sheets", missing_sheets()),
        ("duplicates", duplicates()),
        ("two_atlases_one_sheet", two_atlases_one_sheet()),
        (
            "two_atlases_without_textures",
            two_atlases_without_textures(),
        ),
        ("sheet_texture_key_taken", sheet_texture_key_taken()),
        ("json_bytes", json_bytes()),
        ("json_bytes_with_textures", json_bytes_with_textures()),
        ("floats", floats()),
        ("non_ascii", non_ascii()),
        ("layouts", layouts()),
        ("sheet_skips", sheet_skips()),
        ("winners", winners()),
    ]
}
