// Ported from UnityPy 1.25.2 (MIT): UnityPy/classes/generated.py, classes/PPtr.py, classes/Object.py, classes/legacy_patch/Sprite.py and classes/legacy_patch/Texture2D.py. See THIRD_PARTY_LICENSES.
//! The `sprites` kind's scenes of UnityPy's object construction and of
//! file identities (B3's review rounds 2 and 3, S2, S3, S3a, S3b and S6):
//! Sprites, atlases, atlas entries, keys and pointers whose typetrees
//! UnityPy constructs as their class or as an `UnknownObject`
//! (`classes/generated.py`'s fields of no default, the slotted classes
//! `PPtr`, `SpriteRenderData`, `SpriteAtlasData` and `GUID`, and the
//! attributes a `Sprite` cannot set), objects whose read cannot attach its
//! reader, two nested bundles at two rows of one path, and a tag that is
//! no string beside an atlas whose name is no string.

use super::scenes::{one, scene};
use super::sprite_atlases::{AtlasEntry, add, atlas_data, edit, sprite_atlas};
use super::sprites::{SpriteSpec, alone, key, rgba, sprites};
use super::{AtlasScene, BuildError, Object, V, file, pair, pptr, s, st, vector};
use crate::env::bundle;

/// `SpriteAtlas`'s class id.
const SPRITE_ATLAS: i32 = 687_078_895;

const K1: ([u32; 4], i64) = ([3, 1, 4, 1], 5);

/// `v`, a struct, as a struct of type `ty`.
fn retyped(v: V, ty: &str) -> V {
    match v {
        V::Struct(_, fields) => V::Struct(ty.to_owned(), fields),
        other => other,
    }
}

/// `v`, a struct, with its field `from` named `to`.
fn renamed(v: V, from: &str, to: &str) -> V {
    match v {
        V::Struct(ty, fields) => V::Struct(
            ty,
            fields
                .into_iter()
                .map(|(n, f)| {
                    if n == from {
                        (to.to_owned(), f)
                    } else {
                        (n, f)
                    }
                })
                .collect(),
        ),
        other => other,
    }
}

/// A `GUID` of `words`, with a field `extra` after them where `extra`:
/// the `GUID` of `sprites.rs`'s `key`, whose layout is
/// `tools/fixtures/sprite_build.py`'s (four `unsigned int`s `data[0]` to
/// `data[3]`, the 16 bytes AssetStudio reads, which UnityPy's `GUID`,
/// `classes/generated.py:8023-8027`, names `data_0_` to `data_3_`).
fn guid(words: [u32; 4], extra: bool) -> V {
    let g = match key(words, 0) {
        V::Pair(g, _) => *g,
        other => other,
    };
    if extra {
        add(g, "extra", V::UInt(0))
    } else {
        g
    }
}

/// A `SpriteAtlas` named `name` whose `m_RenderDataMap` holds `entries`,
/// each a key and a value, every one of `shape`'s layout.
fn atlas_of(name: &str, shape: (V, V), entries: Vec<(V, V)>) -> V {
    let map = V::Vector(
        "map".to_owned(),
        Box::new(pair(shape.0, shape.1)),
        entries.into_iter().map(|(k, v)| pair(k, v)).collect(),
    );
    edit(sprite_atlas(Some(name), &[]), "m_RenderDataMap", Some(map))
}

/// Sprites whose objects UnityPy constructs as its `Sprite`, or not
/// (`{name}.png`): `cleaned`, whose `m_PixelsToUnits` is named
/// `m_PixelsToUnits?` (UnityPy's attribute is the cleaned name: it
/// renders); `other_type`, a root of type `NotASprite` with every field (an
/// `UnknownObject`: `read`); and `fine`.
pub fn sprite_classes() -> Result<AtlasScene, BuildError> {
    let spec = |name: &str| SpriteSpec::rect(name, [1.0, 1.0, 2.0, 2.0], (0, 1));
    let sprite = |path_id: i64, v: V| Object::new(path_id, 213, v);
    alone(
        "CAB-o1",
        &[
            rgba(1, "sheet", 0x35, 4, 4),
            sprite(
                2,
                renamed(
                    spec("cleaned").value(),
                    "m_PixelsToUnits",
                    "m_PixelsToUnits?",
                ),
            ),
            sprite(3, retyped(spec("other_type").value(), "NotASprite")),
            spec("fine").object(4),
        ],
        "{name}.png",
    )
}

/// Atlases, entries and keys as UnityPy constructs them (`{name}.png`),
/// each found by `m_SpriteAtlas` but `no_variant`'s and `bytes_tag`'s:
///
/// - `no_variant`, by its tag, on an atlas with no `m_IsVariant` (an
///   `UnknownObject`, used as UnityPy uses it: it renders);
/// - `other_type`, on an atlas whose root is of type `NotAnAtlas` (the
///   same: it renders);
/// - `no_uv`, on an entry with no `uvTransform`, and `extra_field`, on one
///   with a field more (neither is a `SpriteAtlasData`: `render`);
/// - `guid_extra`, on an atlas whose key's `GUID` has a field more (an
///   `UnknownObject`, equal to no key: `render`);
/// - `big_id`, whose key's id and its entry's are a `UInt64` past `2^63`
///   (equal: it renders);
/// - `bytes_tag`, whose `m_AtlasTags` is a vector of `UInt8` (its first
///   item an `int`, equal to no name, though the bytes are an atlas's
///   name): its own `m_RD`.
pub fn atlas_classes() -> Result<AtlasScene, BuildError> {
    let entry = AtlasEntry::rect(K1, (0, 2), [2.0, 1.0, 3.0, 2.0]);
    let data = || atlas_data(&entry);
    let key = |extra: bool| pair(guid(K1.0, extra), V::Int64(K1.1));
    let big = 1_u64 << 63 | 5;
    let big_key = || pair(guid(K1.0, false), V::UInt64(big));
    let plain = |name: &str| atlas_of(name, (key(false), data()), vec![(key(false), data())]);
    let no_uv = edit(data(), "uvTransform", None);
    let extra = add(data(), "extra", V::UInt(1));
    let atlas = |path_id: i64, v: V| Object::new(path_id, SPRITE_ATLAS, v);
    let own = |name: &str| SpriteSpec::rect(name, [0.0, 0.0, 2.0, 2.0], (0, 1)).keyed(K1);
    let big_sprite = edit(
        own("big_id").in_atlas((0, 8)).value(),
        "m_RenderDataKey",
        Some(big_key()),
    );
    let bytes_tag = edit(
        own("bytes_tag").value(),
        "m_AtlasTags",
        Some(vector(
            V::UInt8(0),
            b"no_variant".iter().map(|b| V::UInt8(*b)).collect(),
        )),
    );
    alone(
        "CAB-o2",
        &[
            rgba(1, "own", 0x45, 4, 4),
            rgba(2, "packed", 0x65, 8, 8),
            atlas(3, edit(plain("no_variant"), "m_IsVariant", None)),
            atlas(4, retyped(plain("other_type"), "NotAnAtlas")),
            atlas(
                5,
                atlas_of(
                    "no_uv",
                    (key(false), no_uv.clone()),
                    vec![(key(false), no_uv)],
                ),
            ),
            atlas(
                6,
                atlas_of(
                    "extra",
                    (key(false), extra.clone()),
                    vec![(key(false), extra)],
                ),
            ),
            atlas(
                7,
                atlas_of("guid_extra", (key(true), data()), vec![(key(true), data())]),
            ),
            atlas(
                8,
                atlas_of("big", (big_key(), data()), vec![(big_key(), data())]),
            ),
            own("no_variant").tagged(&["no_variant"]).object(10),
            own("other_type").in_atlas((0, 4)).object(11),
            own("no_uv").in_atlas((0, 5)).object(12),
            own("extra_field").in_atlas((0, 6)).object(13),
            own("guid_extra").in_atlas((0, 7)).object(14),
            Object::new(15, 213, big_sprite),
            Object::new(16, 213, bytes_tag),
        ],
        "{name}.png",
    )
}

/// Two nested bundles at two rows of one path (B3's review round 2, S2):
/// the input's rows are `inner` (a bundle of `CAB-ta`, a texture at path
/// id 1), `inner` again (a bundle of `CAB-tb`, another texture at path id
/// 1, of other pixels) and `CAB-s`, whose `first` and `second` render from
/// each: two sheets, two groups.
pub fn shadowed_rows() -> Result<AtlasScene, BuildError> {
    let a = file(&[], &[rgba(1, "a", 0x17, 4, 4)], true)?;
    let b = file(&[], &[rgba(1, "b", 0x97, 4, 4)], true)?;
    let cab = file(
        &["archive:/CAB-ta/CAB-ta", "archive:/CAB-tb/CAB-tb"],
        &[
            SpriteSpec::rect("first", [0.0, 0.0, 3.0, 3.0], (1, 1)).object(1),
            SpriteSpec::rect("second", [0.0, 0.0, 3.0, 3.0], (2, 1)).object(2),
        ],
        true,
    )?;
    Ok(scene(
        vec![one(
            "x.bundle",
            &[
                ("inner", bundle(&[("CAB-ta", a)])),
                ("inner", bundle(&[("CAB-tb", b)])),
                ("CAB-s", cab),
            ],
            &[],
        )],
        &["x.bundle"],
        sprites("{name}.png"),
    ))
}

/// `v`, a Sprite, with its `m_RD` changed by `f`.
fn with_rd(v: V, f: impl FnOnce(V) -> V) -> V {
    let V::Struct(ty, mut fields) = v else {
        return v;
    };
    if let Some(at) = fields.iter().position(|(n, _)| n == "m_RD") {
        let (n, x) = fields.remove(at);
        fields.insert(at, (n, f(x)));
    }
    V::Struct(ty, fields)
}

/// A struct of type `ty` holding a pointer's two fields, `m_FileID` an
/// `int` and `m_PathID` the value `path_id`, then `extra`.
fn pointer_struct(ty: &str, path_id: V, extra: Vec<(&str, V)>) -> V {
    let mut fields = vec![("m_FileID", V::Int(0)), ("m_PathID", path_id)];
    fields.extend(extra);
    st(ty, fields)
}

/// Pointers and render data as UnityPy constructs them (B3's review round
/// 3, S3a; `{name}.png`), on the sheets `own` (1) and `packed` (2), beside
/// an atlas `packs` (3, its entry `K1` on `packed`), an object whose root
/// is a `GUID` (4) and one whose root is a `PPtr` (5):
///
/// - `texture_struct`, whose texture is a struct of type `NotAPointer` with
///   both ids, and `pptr_extra`, whose texture is a `PPtr<Texture2D>` with
///   a field more: an `UnknownObject`, whose `.path_id` raises, `render`;
/// - `pptr_assetsfile`, whose `PPtr` has a field `assetsfile`, which
///   `parse_class` sets itself: it renders;
/// - `wide_path`, whose texture's `m_PathID` is a `UInt64` past `2^63`,
///   which no object has: `bad_pointer`;
/// - `falsy_atlas`, whose `m_SpriteAtlas` has path id 0 (and a file id
///   past the externals): false, so its tag finds `packs` (it renders);
/// - `null_tags` and `pointer_tags`, whose `m_AtlasTags` is a `PPtr`, of
///   path id 0 (false: its own `m_RD`) and not (true, not indexable:
///   `render`);
/// - `unknown_rd`, whose `m_RD` is of type `NotRenderData` with no
///   `alphaTexture` (an `UnknownObject`, its attribute's read `render`),
///   `unknown_rd_atlas`, the same with an atlas found by its tag (`m_RD`
///   unread: it renders), and `rd_no_alpha`, a `SpriteRenderData` with no
///   `alphaTexture` (its default, none: it renders);
/// - `guid_atlas` and `pptr_atlas`, whose `m_SpriteAtlas` names objects 4
///   and 5, classes with no `set_object_reader`: `read`;
/// - `pptr_twice`, whose texture's `PPtr` has two children `m_FileID`, 7
///   then 0: `parse_class`'s keyword dict keeps the name's first place and
///   its last value, so the pointer is a `PPtr` of file id 0 (it renders;
///   B3's review round 4, R8).
pub fn sprite_pointers() -> Result<AtlasScene, BuildError> {
    let entry = AtlasEntry::rect(K1, (0, 2), [2.0, 1.0, 3.0, 2.0]);
    let spec = |name: &str| SpriteSpec::rect(name, [0.0, 0.0, 2.0, 2.0], (0, 1)).keyed(K1);
    let sprite = |path_id: i64, v: V| Object::new(path_id, 213, v);
    let texture = |name: &str, t: V| with_rd(spec(name).value(), |rd| edit(rd, "texture", Some(t)));
    let atlas = |path_id: i64, v: V| Object::new(path_id, SPRITE_ATLAS, v);
    let big = 1_u64 << 63 | 1;
    let unknown = |name: &str, tags: &[&str]| {
        with_rd(spec(name).tagged(tags).value(), |rd| {
            let V::Struct(_, fields) = edit(rd, "alphaTexture", None) else {
                return V::Int(0);
            };
            V::Struct("NotRenderData".to_owned(), fields)
        })
    };
    let tags = |name: &str, at: (i32, i64)| {
        edit(
            spec(name).value(),
            "m_AtlasTags",
            Some(pptr("SpriteAtlas", at)),
        )
    };
    alone(
        "CAB-o3",
        &[
            rgba(1, "own", 0x55, 4, 4),
            rgba(2, "packed", 0x75, 8, 8),
            atlas(3, sprite_atlas(Some("packs"), &[entry])),
            atlas(4, guid(K1.0, false)),
            atlas(
                5,
                pointer_struct("PPtr<SpriteAtlas>", V::Int64(3), Vec::new()),
            ),
            sprite(
                10,
                texture(
                    "texture_struct",
                    pointer_struct("NotAPointer", V::Int64(1), Vec::new()),
                ),
            ),
            sprite(
                11,
                texture(
                    "pptr_extra",
                    pointer_struct("PPtr<Texture2D>", V::Int64(1), vec![("extra", V::Int(0))]),
                ),
            ),
            sprite(
                12,
                texture(
                    "pptr_assetsfile",
                    pointer_struct(
                        "PPtr<Texture2D>",
                        V::Int64(1),
                        vec![("assetsfile", V::Int(0))],
                    ),
                ),
            ),
            sprite(
                13,
                texture(
                    "wide_path",
                    pointer_struct("PPtr<Texture2D>", V::UInt64(big), Vec::new()),
                ),
            ),
            spec("falsy_atlas")
                .in_atlas((5, 0))
                .tagged(&["packs"])
                .object(14),
            sprite(15, tags("null_tags", (0, 0))),
            sprite(16, tags("pointer_tags", (0, 3))),
            sprite(17, unknown("unknown_rd", &[])),
            sprite(18, unknown("unknown_rd_atlas", &["packs"])),
            sprite(
                19,
                with_rd(spec("rd_no_alpha").value(), |rd| {
                    edit(rd, "alphaTexture", None)
                }),
            ),
            spec("guid_atlas").in_atlas((0, 4)).object(20),
            spec("pptr_atlas").in_atlas((0, 5)).object(21),
            sprite(
                22,
                texture(
                    "pptr_twice",
                    st(
                        "PPtr<Texture2D>",
                        vec![
                            ("m_FileID", V::Int(7)),
                            ("m_FileID", V::Int(0)),
                            ("m_PathID", V::Int64(1)),
                        ],
                    ),
                ),
            ),
        ],
        "{name}.png",
    )
}

/// Attribute names a class's instances cannot set, and the method an
/// object read calls (B3's review round 3, S3b; `{name}.png`), on the sheet
/// `sheet` (1):
///
/// - `image_child`, `assets_file_child` and `class_child`, Sprites with an
///   extra field `image` (`classes/legacy_patch/Sprite.py:10`),
///   `assets_file` (`classes/Object.py:23-27`) or `__class__`: an
///   `UnknownObject`, no `Sprite`, `read`;
/// - `atlas_assets_file`, on an atlas (3) with an extra `assets_file`: an
///   `UnknownObject`, used as UnityPy uses it (it renders);
/// - `shadow_sprite`, a Sprite with a field `set_object_reader`, and
///   `shadow_atlas` and `shadow_texture`, on an atlas (4) and a sheet (2)
///   with one: the object read calls it, `read`;
/// - `texture_assets_file` and `texture_image`, on sheets with an extra
///   `assets_file` (5) or `image` (6, `classes/legacy_patch/Texture2D.py:78`,
///   whose setter raises for a number): an `UnknownObject`, no `Texture2D`,
///   so the sheet's read fails, `read` (B3's review round 4, R4);
/// - `texture_image_path`, on a sheet (7) whose `image` is a string,
///   [`IMAGE_PATH`]: the setter opens it as a path (`Image.open`), so
///   UnityPy builds the `Texture2D` where the file is an image, and the
///   engine and the references refuse it whatever it names, `read` (a
///   stated divergence, `PARITY.md`; round 5, Q2);
/// - `nested_sprite`, `nested_atlas` and `nested_texture`, with an unused
///   field holding a complete `Texture2D` whose `image` names [`IMAGE_PATH`],
///   in the Sprite itself, its atlas (9) or its sheet (8). The references
///   refuse the nested setter without opening a path, leaving an unused
///   `UnknownObject`; the engine keeps the unused field as data. All render
///   (round 6, S1);
/// - `fine`.
pub fn sprite_attributes() -> Result<AtlasScene, BuildError> {
    let entry = AtlasEntry::rect(K1, (0, 1), [1.0, 1.0, 2.0, 2.0]);
    let spec = |name: &str| SpriteSpec::rect(name, [0.0, 0.0, 2.0, 2.0], (0, 1)).keyed(K1);
    let sprite = |path_id: i64, v: V| Object::new(path_id, 213, v);
    let child = |name: &str, field: &str| sprite_with(spec(name).value(), field);
    let atlas = |path_id: i64, name: &str, field: &str| {
        Object::new(
            path_id,
            SPRITE_ATLAS,
            sprite_with(sprite_atlas(Some(name), &[entry]), field),
        )
    };
    let mut shadowed = rgba(2, "shadowed", 0x8A, 4, 4);
    shadowed.value = sprite_with(shadowed.value, "set_object_reader");
    let mut file_sheet = rgba(5, "file_sheet", 0x8B, 4, 4);
    file_sheet.value = sprite_with(file_sheet.value, "assets_file");
    let mut image_sheet = rgba(6, "image_sheet", 0x8C, 4, 4);
    image_sheet.value = sprite_with(image_sheet.value, "image");
    let mut path_sheet = rgba(7, "path_sheet", 0x8D, 4, 4);
    path_sheet.value = add(path_sheet.value, "image", s(IMAGE_PATH));
    let nested = || path_sheet.value.clone();
    let mut nested_sheet = rgba(8, "nested_sheet", 0x8E, 4, 4);
    nested_sheet.value = add(nested_sheet.value, "unused", nested());
    let nested_atlas = add(sprite_atlas(Some("nested"), &[entry]), "unused", nested());
    let nested_sprite = add(spec("nested_sprite").value(), "unused", nested());
    let on = |name: &str, sheet: i64| SpriteSpec::rect(name, [0.0, 0.0, 2.0, 2.0], (0, sheet));
    alone(
        "CAB-o4",
        &[
            rgba(1, "sheet", 0x5A, 4, 4),
            shadowed,
            atlas(3, "a_file", "assets_file"),
            atlas(4, "a_shadow", "set_object_reader"),
            file_sheet,
            image_sheet,
            path_sheet,
            nested_sheet,
            Object::new(9, SPRITE_ATLAS, nested_atlas),
            sprite(10, child("image_child", "image")),
            sprite(11, child("assets_file_child", "assets_file")),
            sprite(12, child("class_child", "__class__")),
            sprite(13, child("shadow_sprite", "set_object_reader")),
            spec("atlas_assets_file").in_atlas((0, 3)).object(14),
            spec("shadow_atlas").in_atlas((0, 4)).object(15),
            SpriteSpec::rect("shadow_texture", [0.0, 0.0, 2.0, 2.0], (0, 2)).object(16),
            spec("fine").object(17),
            on("texture_assets_file", 5).object(18),
            on("texture_image", 6).object(19),
            on("texture_image_path", 7).object(20),
            sprite(21, nested_sprite),
            spec("nested_atlas").in_atlas((0, 9)).object(22),
            on("nested_texture", 8).object(23),
        ],
        "{name}.png",
    )
}

/// The relative path `sprite_attributes`' sheet 7 names as its `image`:
/// `test_sprites_kind.py` writes an image there in a scratch folder, to show
/// UnityPy's setter takes it.
pub const IMAGE_PATH: &str = "unityrs-image-path.png";

/// `v`, a struct, with an `int` field `field` after its others.
fn sprite_with(v: V, field: &str) -> V {
    add(v, field, V::Int(0))
}

/// A tag that is no string beside an atlas whose name is that number
/// (B3's review round 3, S6 and T2; `{name}.png`): `tags_int`'s
/// `m_AtlasTags` is a vector of `int`s `[1]`, and the file's atlas (3), its
/// entry the Sprite's key on the sheet `packed` (2), is named the `int` 1.
/// UnityPy's `==` finds the atlas; the engine's stated rule, that a tag
/// equals only a name that is a string, renders the Sprite's own `m_RD`,
/// on `own` (1), and so does the reference, at its plan and its render.
pub fn int_tags() -> Result<AtlasScene, BuildError> {
    let entry = AtlasEntry::rect(K1, (0, 2), [1.0, 1.0, 3.0, 3.0]);
    let named = edit(sprite_atlas(Some("x"), &[entry]), "m_Name", Some(V::Int(1)));
    let tags = edit(
        SpriteSpec::rect("tags_int", [0.0, 0.0, 2.0, 2.0], (0, 1))
            .keyed(K1)
            .value(),
        "m_AtlasTags",
        Some(vector(V::Int(0), vec![V::Int(1)])),
    );
    alone(
        "CAB-o5",
        &[
            rgba(1, "own", 0x5C, 4, 4),
            rgba(2, "packed", 0x7C, 8, 8),
            Object::new(3, SPRITE_ATLAS, named),
            Object::new(4, 213, tags),
        ],
        "{name}.png",
    )
}
