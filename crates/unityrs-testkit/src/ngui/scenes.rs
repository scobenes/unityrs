//! The NGUI scenes, one function each, and [`scenes`], which lists them by
//! name for `tools/parity/ngui_scenes.py`.
//!
//! Each scene is a set of bundles with the `deps` a job spec would give them
//! (`docs/BATCH_SPEC.md` 1.2), the inputs to cut, each over its own view
//! (4.2), and the cutter's options (1.5's `atlases`, and `textures`'s key
//! when the scene exports textures). Sheets are [`super::sheet`]s, so a
//! test computes every crop's pixels from [`super::pixel`] by hand.

use super::{
    BuildError, Object, PropName, V, atlas, file, format, material, mono_behaviour, mono_script,
    sheet, sprite, texture, vector,
};
use crate::env::{bundle, res_bytes};

mod outputs;
mod recognition;
mod sheets;
mod winners;

pub use outputs::{
    duplicates, floats, json_bytes, json_bytes_with_textures, non_ascii, sheet_texture_key_taken,
    texture_key_is_json_key, two_atlases_one_sheet, two_atlases_without_textures, two_sheet_groups,
};
pub use recognition::{failed_reads, layouts};
pub use sheets::{crop_bounds, sheet_skips, streamed_sheets};
pub use winners::{textured_winners, winners};

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

impl CutOptions {
    /// The defaults, with textures exported under `key`.
    #[must_use]
    pub fn textures(key: &str) -> Self {
        Self {
            texture_key: Some(key.to_owned()),
            ..Self::default()
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

/// A scene's result.
pub type Built = Result<AtlasScene, BuildError>;

/// A scene's builder.
type Make = fn() -> Built;

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

/// One input, `x.bundle`, of one SerializedFile `cab`, with default options.
pub(super) fn alone(cab: &str, objects: &[Object]) -> Built {
    let data = file(&[], objects, true)?;
    Ok(scene(
        vec![one("x.bundle", &[(cab, data)], &[])],
        &["x.bundle"],
        CutOptions::default(),
    ))
}

/// The atlas script every scene's atlases name, at path id 1.
pub(super) const SCRIPT: (i32, i64) = (0, 1);

pub(super) const NULL: (i32, i64) = (0, 0);

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

/// `v`, a struct, with `extra` fields after its own.
pub(super) fn with(v: V, extra: Vec<(&str, V)>) -> V {
    match v {
        V::Struct(ty, mut fields) => {
            fields.extend(extra.into_iter().map(|(n, f)| (n.to_owned(), f)));
            V::Struct(ty, fields)
        }
        other => other,
    }
}

/// An atlas with a null material, cutting the own texture of its name.
pub(super) fn by_name(path_id: i64, name: &str, sprites: Vec<V>) -> Object {
    atlas(path_id, name.as_bytes(), SCRIPT, NULL, sprites, Vec::new())
}

/// Step 1 by a plain-string `_MainTex`, in the atlas's own file. A texture
/// named as the atlas comes first in object order, and loses to the
/// material's. The sprites cover a crop inside the sheet, one past its right
/// and bottom edges, one past its top-left, and two empty rectangles; the
/// first has borders and paddings, which are not applied, and a `rotated`
/// field is written as read.
pub fn own_sheet_by_material_name() -> Built {
    let trimmed = with(
        sprite(b"a", (0, 0, 2, 2), [1, 2, 3, 4, 5, 6, 7, 8]),
        vec![("rotated", V::Bool(true))],
    );
    let mut sprites = vec![trimmed];
    for (name, rect) in [
        ("b", (1, 2, 3, 2)),
        ("edge", (3, 3, 2, 2)),
        ("neg", (-1, -1, 2, 2)),
        ("zero", (0, 0, 0, 2)),
        ("negw", (2, 2, -1, 1)),
    ] {
        sprites.push(with(plain(name, rect), vec![("rotated", V::Bool(false))]));
    }
    alone(
        "CAB-x1",
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
    )
}

/// Step 1 by a `FastPropertyName` `_MainTex` whose texture is in a
/// dependency: its name is read there, and matched among the input's own
/// textures, where a texture of that name is the sheet (and a texture named
/// as the atlas loses to it).
pub fn struct_maintex_in_dependency() -> Built {
    let dep = file(&[], &[rgba(10, "shared_sheet", 0x50, 1, 1)], true)?;
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
    )?;
    Ok(scene(
        vec![
            one("d.bundle", &[("CAB-d1", dep)], &[]),
            one("x.bundle", &[("CAB-x2", cab)], &["d.bundle"]),
        ],
        &["x.bundle"],
        CutOptions::default(),
    ))
}

/// The material and its `_MainTex` texture both in a dependency: the name
/// read there is none of the input's textures, so step 2 takes the texture
/// named as the atlas, an RGB24 sheet, cropped past its edge.
pub fn material_in_dependency() -> Built {
    let dep = file(
        &[],
        &[
            material(20, "m2", PropName::Plain, &[("_MainTex", (0, 21))]),
            rgba(21, "only_in_dep", 0x60, 1, 1),
        ],
        true,
    )?;
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
    )?;
    Ok(scene(
        vec![
            one("d.bundle", &[("CAB-d2", dep)], &[]),
            one("x.bundle", &[("CAB-x3", cab)], &["d.bundle"]),
        ],
        &["x.bundle"],
        CutOptions::default(),
    ))
}

/// A null material gives no name, so step 2 takes the texture named as the
/// atlas; `skip_name_suffixes` `["_ref"]` skips the reference atlas; a null
/// material with no texture of the atlas's name is `sheet_not_found`; and an
/// atlas with no `material` field at all gives no name either, so step 2
/// takes the texture of its name.
pub fn null_material_and_reference() -> Built {
    let one_sprite = || vec![plain("p", (0, 0, 2, 1))];
    let no_field = mono_behaviour(
        6,
        b"nomat",
        SCRIPT,
        vec![
            (
                "mSprites",
                vector(plain("q", (0, 0, 1, 1)), vec![plain("q", (0, 0, 1, 1))]),
            ),
            ("mPixelSize", V::Float(1.0)),
        ],
    );
    let mut built = alone(
        "CAB-x4",
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "plain", 7, 2, 1),
            by_name(3, "plain", one_sprite()),
            by_name(4, "plain_ref", one_sprite()),
            by_name(5, "lost", one_sprite()),
            no_field,
            rgba(7, "nomat", 0x0B, 1, 1),
        ],
    )?;
    built.options.skip_name_suffixes = vec!["_ref".to_owned()];
    Ok(built)
}

/// Every way step 1 gives no name, each atlas then cutting the own texture
/// of its name (step 2): no `_MainTex` pair (n1, whose `_BumpMap` names an
/// own texture, which a cutter taking the first pair would find), a
/// material in a CAB outside the view (n2), a missing object (n3), a null
/// `m_Texture` (n4), an `m_FileID` past the externals (n5), a `_MainTex`
/// texture, read in a dependency, whose name no own texture has (n6), a
/// pointer into the input's raw `.resS` row (n7), and a CAB outside the view
/// whose name is a directory at the root of a Linux host (n8), which
/// UnityPy's own lookup would try to open.
pub fn missing_sheets() -> Built {
    let one_sprite = || vec![plain("p", (0, 0, 1, 1))];
    let dep = file(&[], &[rgba(1, "elsewhere", 0x21, 1, 1)], true)?;
    let mut objects = vec![
        mono_script(1, "NGUIAtlas"),
        material(2, "no_main", PropName::Plain, &[("_BumpMap", (0, 4))]),
        material(3, "null_tex", PropName::Plain, &[("_MainTex", NULL)]),
        rgba(4, "bump", 3, 1, 1),
        material(5, "named", PropName::Plain, &[("_MainTex", (2, 1))]),
    ];
    let materials = [
        (0, 2),
        (1, 99),
        (0, 98),
        (0, 3),
        (5, 2),
        (0, 5),
        (3, 1),
        (4, 1),
    ];
    for (n, at) in (1u8..).zip(materials) {
        let name = format!("n{n}");
        let path_id = 9 + i64::from(n);
        objects.push(atlas(
            path_id,
            name.as_bytes(),
            SCRIPT,
            at,
            one_sprite(),
            Vec::new(),
        ));
        objects.push(rgba(path_id + 10, &name, 0x30 + n, 1, 1));
    }
    let externals = [
        "archive:/CAB-gone/CAB-gone",
        "archive:/CAB-d5/CAB-d5",
        "archive:/CAB-x5/CAB-x5.resS",
        "archive:/etc/etc",
    ];
    let cab = file(&externals, &objects, true)?;
    Ok(scene(
        vec![
            one("d.bundle", &[("CAB-d5", dep)], &[]),
            one(
                "x.bundle",
                &[("CAB-x5", cab), ("CAB-x5.resS", res_bytes(16, 0))],
                &["d.bundle"],
            ),
        ],
        &["x.bundle"],
        CutOptions::default(),
    ))
}

/// Every scene, by name.
pub fn scenes() -> Result<Vec<(&'static str, AtlasScene)>, BuildError> {
    let all: [(&'static str, Make); 22] = [
        ("own_sheet_by_material_name", own_sheet_by_material_name),
        ("struct_maintex_in_dependency", struct_maintex_in_dependency),
        ("material_in_dependency", material_in_dependency),
        ("null_material_and_reference", null_material_and_reference),
        ("missing_sheets", missing_sheets),
        ("duplicates", duplicates),
        ("two_atlases_one_sheet", two_atlases_one_sheet),
        ("two_atlases_without_textures", two_atlases_without_textures),
        ("sheet_texture_key_taken", sheet_texture_key_taken),
        ("texture_key_is_json_key", texture_key_is_json_key),
        ("two_sheet_groups", two_sheet_groups),
        ("json_bytes", json_bytes),
        ("json_bytes_with_textures", json_bytes_with_textures),
        ("floats", floats),
        ("non_ascii", non_ascii),
        ("layouts", layouts),
        ("failed_reads", failed_reads),
        ("sheet_skips", sheet_skips),
        ("streamed_sheets", streamed_sheets),
        ("crop_bounds", crop_bounds),
        ("winners", winners),
        ("textured_winners", textured_winners),
    ];
    all.into_iter().map(|(name, f)| Ok((name, f()?))).collect()
}
