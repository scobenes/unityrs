// Ported from UnityPy 1.25.2 (MIT): UnityPy/classes/generated.py. See THIRD_PARTY_LICENSES.
//! The `sprites` kind's scenes of atlases, file identities and fields
//! (`docs/plans/extract.md` E17; B3's review round 1, R2, R4, R8 and T1):
//! Sprites whose render data is a `SpriteAtlas`'s, found by `m_SpriteAtlas`
//! or by a tag, in the Sprite's file or a dependency; two files of one CAB
//! name, one nested; and Sprites whose fields UnityPy's class or E17 does
//! not take.
//!
//! A `SpriteAtlas` ([`sprite_atlas`]) holds the fields
//! `tools/fixtures/sprite_build.py`'s atlas tree holds, in its order, from
//! the same sources (nothing from a TPK or a dump, `docs/plans/core.md`
//! section 9, Q2): of the fields UnityPy 1.25.2's MIT `SpriteAtlas`
//! (`classes/generated.py`) declares, those without a default: `m_Name`,
//! `m_PackedSprites`, `m_PackedSpriteNamesToIndex`, `m_RenderDataMap` (a
//! `map` of a `pair<GUID, SInt64>` to a `SpriteAtlasData`), `m_Tag` and
//! `m_IsVariant`. Its `SpriteAtlasData` is the tree's: `texture`,
//! `alphaTexture`, `textureRect`, `textureRectOffset`, `uvTransform`,
//! `downscaleMultiplier` and `settingsRaw`.

use super::scenes::{one, scene};
use super::sprites::{RECT, SpriteSpec, alone, floats_of, key, rectf, rgba, sprites};
use super::{AtlasScene, BuildError, Object, V, file, pptr, s, st, vector};
use crate::env::bundle;

/// `SpriteAtlas`'s class id.
const SPRITE_ATLAS: i32 = 687_078_895;

/// One entry of an atlas's `m_RenderDataMap`: its key (a `GUID`'s words
/// and an id) and its `SpriteAtlasData`'s texture, alpha texture (null:
/// none), rectangle and settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AtlasEntry {
    /// `first`: the `GUID`'s words and the id.
    pub key: ([u32; 4], i64),
    /// `texture`.
    pub texture: (i32, i64),
    /// `alphaTexture`.
    pub alpha: (i32, i64),
    /// `textureRect`.
    pub rect: [f32; 4],
    /// `settingsRaw`.
    pub settings: u32,
}

impl AtlasEntry {
    /// A rectangle entry of `key` on `texture`, no alpha texture.
    #[must_use]
    pub fn rect(key: ([u32; 4], i64), texture: (i32, i64), rect: [f32; 4]) -> Self {
        Self {
            key,
            texture,
            alpha: (0, 0),
            rect,
            settings: RECT,
        }
    }
}

/// The `SpriteAtlasData` of `e`.
pub(super) fn atlas_data(e: &AtlasEntry) -> V {
    st(
        "SpriteAtlasData",
        vec![
            ("texture", pptr("Texture2D", e.texture)),
            ("alphaTexture", pptr("Texture2D", e.alpha)),
            ("textureRect", rectf(e.rect)),
            (
                "textureRectOffset",
                floats_of("Vector2f", &["x", "y"], &[0.0; 2]),
            ),
            (
                "uvTransform",
                floats_of("Vector4f", &["x", "y", "z", "w"], &[0.0; 4]),
            ),
            ("downscaleMultiplier", V::Float(1.0)),
            ("settingsRaw", V::UInt(e.settings)),
        ],
    )
}

/// A `SpriteAtlas` named `name` (none: no `m_Name` field, so `peek_name`
/// finds none) of `entries`, in order.
#[must_use]
pub fn sprite_atlas(name: Option<&str>, entries: &[AtlasEntry]) -> V {
    let shape = super::pair(
        key([0; 4], 0),
        atlas_data(&AtlasEntry::rect(([0; 4], 0), (0, 0), [0.0; 4])),
    );
    let map = V::Vector(
        "map".to_owned(),
        Box::new(shape),
        entries
            .iter()
            .map(|e| super::pair(key(e.key.0, e.key.1), atlas_data(e)))
            .collect(),
    );
    let mut fields = Vec::new();
    if let Some(n) = name {
        fields.push(("m_Name", s(n)));
    }
    fields.extend([
        (
            "m_PackedSprites",
            vector(pptr("Sprite", (0, 0)), Vec::new()),
        ),
        ("m_PackedSpriteNamesToIndex", vector(s(""), Vec::new())),
        ("m_RenderDataMap", map),
        ("m_Tag", s(name.unwrap_or(""))),
        ("m_IsVariant", V::Bool(false)),
    ]);
    st("SpriteAtlas", fields)
}

fn atlas_object(path_id: i64, name: Option<&str>, entries: &[AtlasEntry]) -> Object {
    Object::new(path_id, SPRITE_ATLAS, sprite_atlas(name, entries))
}

const K1: ([u32; 4], i64) = ([9, 8, 7, 6], 21);
const K2: ([u32; 4], i64) = ([9, 8, 7, 6], 22);
const K3: ([u32; 4], i64) = ([5, 5, 5, 5], 21);

/// Atlases in the Sprites' own file (`{name}.png`). The file's atlases, in
/// object order: `other` (another tag), one with no name, `packs` (its
/// entries `K1` and `K2` on the sheet `packed`), and one whose name's read
/// fails (cut short). `by_tag` and `by_tag_2` find `packs` by their tag
/// past the first two and render its entries (the atlas read once), never
/// reading the last; `missing_key`'s key is no entry's (`render`);
/// `unmatched`'s tag is none's, so every name is read, the last's failure
/// its own (`read`); `direct` names `packs` by `m_SpriteAtlas`;
/// `unresolved`'s `m_SpriteAtlas` names no object (`bad_pointer`) and
/// `not_an_atlas`'s a texture, which has no `m_RenderDataMap` (`render`).
pub fn atlas_lookup() -> Result<AtlasScene, BuildError> {
    let packs = [
        AtlasEntry::rect(K1, (0, 2), [1.0, 1.0, 3.0, 2.0]),
        AtlasEntry::rect(K2, (0, 2), [4.0, 4.0, 2.0, 3.0]),
    ];
    let other = [AtlasEntry::rect(K1, (0, 1), [0.0, 0.0, 1.0, 1.0])];
    let own = |name: &str| SpriteSpec::rect(name, [0.0, 0.0, 2.0, 2.0], (0, 1));
    alone(
        "CAB-a1",
        &[
            rgba(1, "own", 0x01, 8, 8),
            rgba(2, "packed", 0x41, 8, 8),
            atlas_object(3, Some("other"), &other),
            atlas_object(4, None, &other),
            atlas_object(5, Some("packs"), &packs),
            atlas_object(6, Some("broken"), &other).cut_to(6),
            own("by_tag")
                .tagged(&["packs", "other"])
                .keyed(K1)
                .object(10),
            own("by_tag_2").tagged(&["packs"]).keyed(K2).object(11),
            own("missing_key").tagged(&["packs"]).keyed(K3).object(12),
            own("unmatched").tagged(&["nothing"]).keyed(K1).object(13),
            own("direct").in_atlas((0, 5)).keyed(K2).object(14),
            own("unresolved").in_atlas((0, 99)).keyed(K1).object(15),
            own("not_an_atlas").in_atlas((0, 1)).keyed(K1).object(16),
        ],
        "{name}.png",
    )
}

/// An atlas in a dependency: `far` and `far_2` name it by `m_SpriteAtlas`,
/// its entries' textures (one with an alpha texture) the dependency's; a
/// tag is looked for in the Sprite's own file only, so `tag_elsewhere`
/// (the dependency's atlas's name) and `fallback` (no atlas's) render
/// their own `m_RD`.
pub fn atlas_dependency() -> Result<AtlasScene, BuildError> {
    let entries = [
        AtlasEntry {
            alpha: (0, 2),
            ..AtlasEntry::rect(K1, (0, 1), [0.0, 0.0, 4.0, 4.0])
        },
        AtlasEntry::rect(K2, (0, 1), [2.0, 2.0, 3.0, 3.0]),
    ];
    let dep = file(
        &[],
        &[
            rgba(1, "dep_packed", 0x51, 8, 8),
            rgba(2, "dep_alpha", 0x91, 8, 8),
            atlas_object(3, Some("dep"), &entries),
        ],
        true,
    )?;
    let local = [AtlasEntry::rect(K1, (0, 1), [0.0, 0.0, 1.0, 1.0])];
    let own = |name: &str| SpriteSpec::rect(name, [1.0, 0.0, 2.0, 3.0], (0, 1));
    let cab = file(
        &["archive:/CAB-a3/CAB-a3"],
        &[
            rgba(1, "own", 0x21, 4, 4),
            atlas_object(2, Some("local"), &local),
            own("far").in_atlas((1, 3)).keyed(K1).object(3),
            own("far_2").in_atlas((1, 3)).keyed(K2).object(4),
            own("tag_elsewhere").tagged(&["dep"]).keyed(K1).object(5),
            own("fallback").tagged(&["zzz"]).keyed(K1).object(6),
        ],
        true,
    )?;
    Ok(scene(
        vec![
            one("d.bundle", &[("CAB-a3", dep)], &[]),
            one("x.bundle", &[("CAB-a2", cab)], &["d.bundle"]),
        ],
        &["x.bundle"],
        sprites("{name}.png"),
    ))
}

/// Two files of one CAB name in one input (B3's review round 1, R4): the
/// input's rows are `CAB-x` (a texture at path id 1), `CAB-n1` (the
/// Sprites) and a nested bundle whose rows are another `CAB-x` (another
/// texture at path id 1) and `CAB-n2` (an atlas whose entry is on its
/// sibling's texture). `top` renders from the top-level `CAB-x` (its
/// sibling); `nested`, by the atlas, from the nested one: two sheets, two
/// groups.
pub fn nested_cabs() -> Result<AtlasScene, BuildError> {
    let top_x = file(&[], &[rgba(1, "top", 0x23, 4, 4)], true)?;
    let inner_x = file(&[], &[rgba(1, "inner", 0xA3, 4, 4)], true)?;
    let entries = [AtlasEntry::rect(K1, (1, 1), [0.0, 0.0, 3.0, 3.0])];
    let inner_atlas = file(
        &["archive:/CAB-x/CAB-x"],
        &[atlas_object(1, Some("inner_atlas"), &entries)],
        true,
    )?;
    let sprites_file = file(
        &["archive:/CAB-x/CAB-x", "archive:/CAB-n2/CAB-n2"],
        &[
            SpriteSpec::rect("top", [0.0, 0.0, 3.0, 3.0], (1, 1)).object(1),
            SpriteSpec::rect("nested", [0.0, 0.0, 1.0, 1.0], (1, 1))
                .in_atlas((2, 1))
                .keyed(K1)
                .object(2),
        ],
        true,
    )?;
    let inner = bundle(&[("CAB-x", inner_x), ("CAB-n2", inner_atlas)]);
    Ok(scene(
        vec![one(
            "x.bundle",
            &[("CAB-x", top_x), ("CAB-n1", sprites_file), ("inner", inner)],
            &[],
        )],
        &["x.bundle"],
        sprites("{name}_{path_id}.png"),
    ))
}

/// `v`, a struct, with a field `name` of `value` after its others.
pub(super) fn add(v: V, name: &str, value: V) -> V {
    match v {
        V::Struct(ty, mut fields) => {
            fields.push((name.to_owned(), value));
            V::Struct(ty, fields)
        }
        other => other,
    }
}

/// `v`, a struct, with its field `name` replaced by `with` (none: removed).
pub(super) fn edit(v: V, name: &str, with: Option<V>) -> V {
    let V::Struct(ty, fields) = v else {
        return v;
    };
    let mut with = with;
    let fields = fields
        .into_iter()
        .filter_map(|(n, f)| {
            if n == name {
                with.take().map(|w| (n, w))
            } else {
                Some((n, f))
            }
        })
        .collect();
    V::Struct(ty, fields)
}

/// Sprites whose fields UnityPy's class or E17 does not take (B3's review
/// round 1, R8), each `read`, before any pointer is followed: no
/// `m_Extrude` (UnityPy's `Sprite` requires it), an `m_PixelsToUnits` that
/// is an `int` and `m_RD`'s `settingsRaw` a `float`; and `fine`. (A vector
/// of `int`s as `m_AtlasTags` is `sprite_objects.rs`'s `int_tags`, beside
/// an atlas: B3's review round 3, T2.)
pub fn sprite_fields() -> Result<AtlasScene, BuildError> {
    let spec = |name: &str| SpriteSpec::rect(name, [0.0, 0.0, 2.0, 2.0], (0, 1));
    let sprite = |path_id: i64, v: V| Object::new(path_id, 213, v);
    let rd = |v: &V| match v {
        V::Struct(_, fields) => fields
            .iter()
            .find(|(n, _)| n == "m_RD")
            .map(|(_, f)| f.clone()),
        _ => None,
    };
    let base = spec("settings_float").value();
    let settings = rd(&base).map(|r| edit(r, "settingsRaw", Some(V::Float(2.0))));
    alone(
        "CAB-f1",
        &[
            rgba(1, "sheet", 0x33, 4, 4),
            sprite(2, edit(spec("no_extrude").value(), "m_Extrude", None)),
            sprite(
                3,
                edit(spec("ppu_int").value(), "m_PixelsToUnits", Some(V::Int(1))),
            ),
            sprite(4, edit(base, "m_RD", settings)),
            spec("fine").object(6),
        ],
        "{name}.png",
    )
}

/// A tag that raises beside a later name's kept failure (B3's review round
/// 4, R3; `{name}.png`): the file's atlases are `a` (2) and one whose name's
/// read fails (3, cut short). `unmatched`'s tag is none's, so its walk reads
/// both names and fails at the second's, which is kept; `pointer_tags`'
/// `m_AtlasTags` is a true `PPtr`, not indexable, which raises after the
/// first name's read, before the kept failure is reached: `render`, as in
/// UnityPy's loop. `swapped` puts `pointer_tags` first, so the answers do
/// not hang on the Sprites' order.
fn raised_tags_in(cab: &str, swapped: bool) -> Result<AtlasScene, BuildError> {
    let entries = [AtlasEntry::rect(K1, (0, 1), [0.0, 0.0, 1.0, 1.0])];
    let spec = |name: &str| SpriteSpec::rect(name, [0.0, 0.0, 2.0, 2.0], (0, 1)).keyed(K1);
    let unmatched = spec("unmatched").tagged(&["zzz"]).value();
    let pointer = edit(
        spec("pointer_tags").value(),
        "m_AtlasTags",
        Some(pptr("SpriteAtlas", (0, 2))),
    );
    let (first, second) = if swapped {
        (pointer, unmatched)
    } else {
        (unmatched, pointer)
    };
    alone(
        cab,
        &[
            rgba(1, "own", 0x2E, 4, 4),
            atlas_object(2, Some("a"), &entries),
            atlas_object(3, Some("broken"), &entries).cut_to(6),
            Object::new(10, 213, first),
            Object::new(11, 213, second),
        ],
        "{name}.png",
    )
}

/// [`raised_tags_in`], the unmatched tag's Sprite first.
pub fn raised_tags() -> Result<AtlasScene, BuildError> {
    raised_tags_in("CAB-a4", false)
}

/// [`raised_tags_in`], the raising tag's Sprite first.
pub fn raised_tags_swapped() -> Result<AtlasScene, BuildError> {
    raised_tags_in("CAB-a5", true)
}
