//! The scenes of recognition (`docs/BATCH_SPEC.md` section 5's first
//! paragraph, decided from the typetree alone) and of reads that fail.

use super::super::{
    Object, PropName, V, atlas, file, material, mono_behaviour, mono_script, pair, s, st, vector,
};
use super::{Built, CutOptions, NULL, SCRIPT, by_name, one, plain, rgba, scene};

/// An `mSprites` element of these fields, in this order.
fn element(fields: Vec<(&str, V)>) -> V {
    st("UISpriteData", fields)
}

/// A MonoBehaviour of the atlas script whose `mSprites` is `sprites`.
fn shaped(path_id: i64, name: &[u8], sprites: V) -> Object {
    mono_behaviour(path_id, name, SCRIPT, vec![("mSprites", sprites)])
}

/// `o`, a struct, with its fields edited by `edit`.
fn edited(o: Object, edit: impl FnOnce(&mut Vec<(String, V)>)) -> Object {
    match o.value {
        V::Struct(ty, mut fields) => {
            edit(&mut fields);
            Object::new(o.path_id, o.class_id, V::Struct(ty, fields))
        }
        other => Object::new(o.path_id, o.class_id, other),
    }
}

/// A vector of one `item`.
fn one_of(item: V) -> V {
    vector(item.clone(), vec![item])
}

/// The `mSprites` that are not a list of sprites, at path ids 17 to 23, and
/// one of mixed integer types that is, at 24.
fn odd_shapes() -> Vec<Object> {
    let rect = |name: V, x: V, height: V| {
        element(vec![
            ("name", name),
            ("x", x),
            ("y", V::Int(0)),
            ("width", V::Int(1)),
            ("height", height),
        ])
    };
    vec![
        shaped(17, b"str_empty", s("")),
        shaped(18, b"str_full", s("abc")),
        shaped(
            19,
            b"no_width",
            one_of(element(vec![
                ("name", s("w")),
                ("x", V::Int(0)),
                ("y", V::Int(0)),
                ("height", V::Int(1)),
            ])),
        ),
        shaped(
            20,
            b"float_rect",
            one_of(rect(s("f"), V::Float(0.0), V::Int(1))),
        ),
        shaped(
            21,
            b"bool_rect",
            one_of(rect(s("b"), V::Int(0), V::Bool(true))),
        ),
        shaped(
            22,
            b"map_sprites",
            V::Vector(
                "map".to_owned(),
                Box::new(pair(s(""), V::Int(0))),
                vec![pair(s("a"), V::Int(1))],
            ),
        ),
        shaped(
            23,
            b"int_name",
            one_of(rect(V::Int(0), V::Int(0), V::Int(1))),
        ),
        shaped(
            24,
            b"mixed",
            one_of(element(vec![
                ("name", s("m")),
                ("x", V::UInt8(0)),
                ("y", V::UInt(0)),
                ("width", V::UInt64(1)),
                ("height", V::Int(1)),
            ])),
        ),
    ]
}

/// Recognition: a MonoBehaviour of another class, a null `m_Script` and one
/// to a missing object (both `unresolved_scripts`), one whose `m_Script` is
/// no MonoScript (not counted), and every `mSprites` that is not a list of
/// sprites (`ngui_layout_unsupported`): NGUI 2's `sprites` layout, an `int`,
/// an empty and a non-empty `string` (whose node's first child is `Array`
/// too), elements without `width`, with a `float` or a `bool` rectangle
/// field or an `int` name, and a `map`. An empty name is `unnamed_atlas`;
/// an atlas whose rectangle fields are `UInt8`, `unsigned int`, `UInt64` and
/// `int` is recognised and cut. In a second file with no typetrees, an
/// atlas script's MonoBehaviour is `no_typetree`; in a third, with no
/// typetrees either, so is one whose MonoScript the TPK reads.
pub fn layouts() -> Built {
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
    let empty = || vector(V::Int(0), Vec::new());
    let mut objects = vec![
        mono_script(1, "NGUIAtlas"),
        mono_script(2, "UIPanel"),
        rgba(3, "t", 1, 1, 1),
        mono_behaviour(10, b"panel", (0, 2), vec![("depth", V::Int(3))]),
        mono_behaviour(11, b"t", NULL, vec![("mSprites", empty())]),
        mono_behaviour(12, b"t", (0, 77), vec![("mSprites", empty())]),
        mono_behaviour(13, b"old", SCRIPT, vec![("sprites", one_of(legacy))]),
        shaped(14, b"flat", V::Int(0)),
        by_name(15, "", vec![plain("e", (0, 0, 1, 1))]),
        mono_behaviour(16, b"t", (0, 3), vec![("mSprites", empty())]),
    ];
    objects.extend(odd_shapes());
    objects.push(rgba(25, "mixed", 0x0C, 1, 1));
    let first = file(&[], &objects, true)?;
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
    )?;
    let stripped = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            by_name(2, "stripped", vec![plain("e", (0, 0, 1, 1))]),
        ],
        false,
    )?;
    Ok(scene(
        vec![one(
            "x.bundle",
            &[
                ("CAB-x11a", first),
                ("CAB-x11b", bare),
                ("CAB-x11c", stripped),
            ],
            &[],
        )],
        &["x.bundle"],
        CutOptions::default(),
    ))
}

/// Reads that fail, with textures exported. The first texture named `ui`
/// is cut short after its name: `peek_name` reads it, so it takes the name
/// (the second `ui` is `duplicate_name`) and is the sheet of the atlas `ui`,
/// whose skip is that texture's read's failure. The atlas `short` is cut
/// short after its MonoBehaviour head, so it is recognised (from the
/// typetree) but its own read fails, before its sheet is looked for; that
/// sheet, which no atlas then cuts, is a plain texture output.
///
/// The atlas `via`'s material names the cut-short `ui`: step 1 reads its
/// name as `peek_name` does, so step 2 finds it and its failed read skips
/// `via` (a full read in step 1 would give no name, and `via` would cut the
/// texture of its own name). The atlas `tail` and the texture `ttail` hold
/// four bytes past their values: their reads end early, which UnityPy's
/// `check_read` refuses (`decode_error`, code `asset_bundle`), `ttail` the
/// sheet of the atlas `ttail` too. The texture `stub` has a typetree of
/// `m_Name` alone, which UnityPy reads as an `UnknownObject`. Two more files
/// each end with an object of no bytes, which UnityPy reads with its own
/// reader, past the file's end: a texture (`CAB-x15b`) and a MonoBehaviour
/// (`CAB-x15c`), whose head's read fails, so its script is unresolved.
///
/// The texture `cb` is cut just after `m_ForcedFallbackFormat`, so the next
/// read, `m_DownscaleFallback`'s `bool`, raises a `ValueError` that is not
/// the check's, with the check or without it. The texture `nolm` lacks only
/// `m_LightmapFormat`, which UnityPy's `Texture2D` requires; `extra` has
/// one field past UnityPy's. The material `trailmat` holds four bytes past
/// its value, so the atlas `mt`'s step 1 gives no name and `mt` cuts the
/// texture of its own name, not `mtx`, which the material names. The atlas
/// `ms`'s MonoScript holds four bytes past its value: its script is
/// unresolved.
pub fn failed_reads() -> Built {
    // `ui`'s `m_Name`: an `int` length, two bytes, two of alignment.
    let ui_name = 8;
    // The head: two pointers of 12 bytes, an aligned `bool` and `m_Name`
    // `short` (an `int`, five bytes, three of alignment).
    let head = 12 + 4 + 12 + 12;
    let one_sprite = || vec![plain("s", (0, 0, 1, 1))];
    let stub = Object::new(13, 28, st("Texture2D", vec![("m_Name", s("stub"))]));
    // `cb`'s `m_Name` (8 bytes, as `ui`'s) and `m_ForcedFallbackFormat`.
    let cb = rgba(14, "cb", 0x1C, 1, 1).cut_to(ui_name + 4);
    let nolm = edited(rgba(15, "nolm", 0x1D, 1, 1), |f| {
        f.retain(|(n, _)| n != "m_LightmapFormat");
    });
    let extra = edited(rgba(16, "extra", 0x1E, 1, 1), |f| {
        f.push(("m_Extra".to_owned(), V::Int(7)));
    });
    let cab = file(
        &[],
        &[
            mono_script(1, "NGUIAtlas"),
            rgba(2, "ui", 0x13, 2, 2).cut_to(ui_name),
            rgba(3, "ui", 0x14, 2, 2),
            by_name(4, "ui", one_sprite()),
            by_name(5, "short", one_sprite()).cut_to(head),
            rgba(6, "short", 0x15, 1, 1),
            material(7, "viamat", PropName::Plain, &[("_MainTex", (0, 2))]),
            atlas(8, b"via", SCRIPT, (0, 7), one_sprite(), Vec::new()),
            rgba(9, "via", 0x16, 1, 1),
            by_name(10, "tail", one_sprite()).with_trailing(4),
            rgba(11, "ttail", 0x17, 1, 1).with_trailing(4),
            by_name(12, "ttail", one_sprite()),
            stub,
            cb,
            nolm,
            extra,
            material(17, "trailmat", PropName::Plain, &[("_MainTex", (0, 19))]).with_trailing(4),
            atlas(18, b"mt", SCRIPT, (0, 17), one_sprite(), Vec::new()),
            rgba(19, "mtx", 0x19, 1, 1),
            rgba(20, "mt", 0x1A, 1, 1),
            mono_script(21, "NGUIAtlas").with_trailing(4),
            atlas(22, b"ms", (0, 21), NULL, one_sprite(), Vec::new()),
        ],
        true,
    )?;
    let zero_texture = file(&[], &[rgba(1, "zero", 0x18, 1, 1).cut_to(0)], true)?;
    let zero_head = file(&[], &[by_name(1, "zmb", one_sprite()).cut_to(0)], true)?;
    Ok(scene(
        vec![one(
            "x.bundle",
            &[
                ("CAB-x15", cab),
                ("CAB-x15b", zero_texture),
                ("CAB-x15c", zero_head),
            ],
            &[],
        )],
        &["x.bundle"],
        CutOptions::textures("{name}.png"),
    ))
}
