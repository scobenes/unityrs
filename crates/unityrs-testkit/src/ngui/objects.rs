// Ported from UnityPy 1.25.2 (MIT): UnityPy/classes/generated.py. See THIRD_PARTY_LICENSES.
//! The objects of an NGUI scene, each a [`super::V`] of its class's fields
//! in the 2022.3 layout: the atlas MonoBehaviour and its sprites, the
//! `MonoScript`, the `Material` and the `Texture2D`, inline or streamed.

use super::{Object, V, pair, pptr, s, st, vector};

/// The MonoBehaviour head every MonoBehaviour starts with (the TPK's, so
/// UnityPy's `parse_monobehaviour_head` reads it from a file with or
/// without typetrees): `m_GameObject`, `m_Enabled`, `m_Script`, `m_Name`.
fn head(script: (i32, i64), name: &[u8]) -> Vec<(String, V)> {
    vec![
        ("m_GameObject".to_owned(), pptr("GameObject", (0, 0))),
        ("m_Enabled".to_owned(), V::Bool(true)),
        ("m_Script".to_owned(), pptr("MonoScript", script)),
        ("m_Name".to_owned(), V::Str(name.to_vec())),
    ]
}

/// A MonoBehaviour (class 114): the head, then `fields`.
#[must_use]
pub fn mono_behaviour(
    path_id: i64,
    name: &[u8],
    script: (i32, i64),
    fields: Vec<(&str, V)>,
) -> Object {
    let mut all = head(script, name);
    all.extend(fields.into_iter().map(|(n, v)| (n.to_owned(), v)));
    Object::new(path_id, 114, V::Struct("MonoBehaviour".to_owned(), all))
}

/// A MonoScript (class 115) of `class_name`, in no namespace, with the
/// 2022.3 fields the TPK reads a stripped one by, `m_PropertiesHash` (a
/// `Hash128`) among them.
#[must_use]
pub fn mono_script(path_id: i64, class_name: &str) -> Object {
    let hash = V::Struct(
        "Hash128".to_owned(),
        (0..16u8)
            .map(|i| (format!("bytes[{i}]"), V::UInt8(i)))
            .collect(),
    );
    Object::new(
        path_id,
        115,
        st(
            "MonoScript",
            vec![
                ("m_Name", s(class_name)),
                ("m_ExecutionOrder", V::Int(0)),
                ("m_PropertiesHash", hash),
                ("m_ClassName", s(class_name)),
                ("m_Namespace", s("")),
                ("m_AssemblyName", s("Assembly-CSharp.dll")),
            ],
        ),
    )
}

/// An NGUI `UISpriteData`: `name`, its rectangle, then the borders and
/// paddings, which the cutter does not apply (`docs/BATCH_SPEC.md` section 5).
#[must_use]
pub fn sprite(name: &[u8], rect: (i32, i32, i32, i32), trim: [i32; 8]) -> V {
    let (x, y, width, height) = rect;
    let names = [
        "borderLeft",
        "borderRight",
        "borderTop",
        "borderBottom",
        "paddingLeft",
        "paddingRight",
        "paddingTop",
        "paddingBottom",
    ];
    let mut fields = vec![
        ("name".to_owned(), V::Str(name.to_vec())),
        ("x".to_owned(), V::Int(x)),
        ("y".to_owned(), V::Int(y)),
        ("width".to_owned(), V::Int(width)),
        ("height".to_owned(), V::Int(height)),
    ];
    fields.extend(
        names
            .iter()
            .zip(trim)
            .map(|(n, v)| ((*n).to_owned(), V::Int(v))),
    );
    V::Struct("UISpriteData".to_owned(), fields)
}

/// An NGUI atlas: a MonoBehaviour whose `m_Script` is `script`, with
/// `material`, `mSprites` (`sprites`, each shaped as the first or as
/// [`sprite`]'s), `mPixelSize` 1, and `extra` fields after them.
#[must_use]
pub fn atlas(
    path_id: i64,
    name: &[u8],
    script: (i32, i64),
    material: (i32, i64),
    sprites: Vec<V>,
    extra: Vec<(&str, V)>,
) -> Object {
    let shape = sprites
        .first()
        .cloned()
        .unwrap_or_else(|| sprite(b"", (0, 0, 0, 0), [0; 8]));
    let mut fields = vec![
        ("material", pptr("Material", material)),
        ("mSprites", vector(shape, sprites)),
        ("mPixelSize", V::Float(1.0)),
    ];
    fields.extend(extra);
    mono_behaviour(path_id, name, script, fields)
}

/// A `_MainTex`-style property name: a plain string, or a
/// `FastPropertyName` struct with a `name`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropName {
    /// `first` a `string`.
    Plain,
    /// `first` a `FastPropertyName { name }`.
    Fast,
}

fn prop(kind: PropName, name: &str) -> V {
    match kind {
        PropName::Plain => s(name),
        PropName::Fast => st("FastPropertyName", vec![("name", s(name))]),
    }
}

/// A Material (class 21) whose `m_SavedProperties.m_TexEnvs` holds `envs`,
/// each a property name and its texture pointer, the names written as `kind`.
#[must_use]
pub fn material(path_id: i64, name: &str, kind: PropName, envs: &[(&str, (i32, i64))]) -> Object {
    let vec2 = || st("Vector2f", vec![("x", V::Float(1.0)), ("y", V::Float(1.0))]);
    let env = |texture: (i32, i64)| {
        st(
            "UnityTexEnv",
            vec![
                ("m_Texture", pptr("Texture", texture)),
                ("m_Scale", vec2()),
                ("m_Offset", vec2()),
            ],
        )
    };
    let map = |shape: V, items: Vec<V>| V::Vector("map".to_owned(), Box::new(shape), items);
    let tex_envs = map(
        pair(prop(kind, ""), env((0, 0))),
        envs.iter()
            .map(|(n, at)| pair(prop(kind, n), env(*at)))
            .collect(),
    );
    let floats = map(
        pair(prop(kind, ""), V::Float(0.0)),
        vec![pair(prop(kind, "_Cutoff"), V::Float(0.5))],
    );
    let color = st(
        "ColorRGBA",
        vec![
            ("r", V::Float(1.0)),
            ("g", V::Float(1.0)),
            ("b", V::Float(1.0)),
            ("a", V::Float(1.0)),
        ],
    );
    let colors = map(
        pair(prop(kind, ""), color.clone()),
        vec![pair(prop(kind, "_Color"), color)],
    );
    let sheet = st(
        "UnityPropertySheet",
        vec![
            ("m_TexEnvs", tex_envs),
            ("m_Floats", floats),
            ("m_Colors", colors),
        ],
    );
    Object::new(
        path_id,
        21,
        st(
            "Material",
            vec![
                ("m_Name", s(name)),
                ("m_Shader", pptr("Shader", (0, 0))),
                ("m_SavedProperties", sheet),
            ],
        ),
    )
}

/// A Material (class 21) of every field of the 2022.3 layout, in the bytes
/// the TPK reads a stripped one by, its `m_TexEnvs` holding `envs` (plain
/// names): [`material`]'s value with the keyword lists, flags, tag map,
/// shader passes, `m_Ints` and texture stacks between, all empty. The two
/// flags are a `UInt8` and an aligned `bool`, the TPK's alignment.
#[must_use]
pub fn material_tpk(path_id: i64, name: &str, envs: &[(&str, (i32, i64))]) -> Object {
    let mut value = material(path_id, name, PropName::Plain, envs).value;
    let strings = || vector(s(""), Vec::new());
    let map = |shape: V| V::Vector("map".to_owned(), Box::new(shape), Vec::new());
    if let V::Struct(_, fields) = &mut value {
        let sheet = fields.pop();
        fields.extend(
            [
                ("m_ValidKeywords", strings()),
                ("m_InvalidKeywords", strings()),
                ("m_LightmapFlags", V::UInt(4)),
                ("m_EnableInstancingVariants", V::UInt8(0)),
                ("m_DoubleSidedGI", V::Bool(false)),
                ("m_CustomRenderQueue", V::Int(-1)),
                ("stringTagMap", map(pair(s(""), s("")))),
                ("disabledShaderPasses", strings()),
            ]
            .map(|(n, v)| (n.to_owned(), v)),
        );
        if let Some((n, V::Struct(ty, mut sheet))) = sheet {
            sheet.insert(1, ("m_Ints".to_owned(), map(pair(s(""), V::Int(0)))));
            fields.push((n, V::Struct(ty, sheet)));
        }
        let stack = st(
            "BuildTextureStackReference",
            vec![("groupName", s("")), ("itemName", s(""))],
        );
        fields.push(("m_BuildTextureStacks".to_owned(), vector(stack, Vec::new())));
    }
    Object::new(path_id, 21, value)
}

/// `TextureFormat` values the scenes use.
pub mod format {
    /// `Alpha8`, outside E15's five.
    pub const ALPHA8: i32 = 1;
    /// `RGB24`.
    pub const RGB24: i32 = 3;
    /// `RGBA32`.
    pub const RGBA32: i32 = 4;
    /// `DXT1`.
    pub const DXT1: i32 = 10;
}

/// A `Texture2D`'s fields up to `image_data`, which UnityPy 1.25.2's class
/// requires, so `obj.read().image` decodes it.
fn texture_fields(name: &[u8], fmt: i32, size: (i32, i32), complete: u32) -> Vec<(String, V)> {
    let settings = st(
        "GLTextureSettings",
        vec![
            ("m_FilterMode", V::Int(1)),
            ("m_Aniso", V::Int(1)),
            ("m_MipBias", V::Float(0.0)),
            ("m_WrapU", V::Int(1)),
            ("m_WrapV", V::Int(1)),
            ("m_WrapW", V::Int(1)),
        ],
    );
    vec![
        ("m_Name".to_owned(), V::Str(name.to_vec())),
        ("m_ForcedFallbackFormat".to_owned(), V::Int(4)),
        ("m_DownscaleFallback".to_owned(), V::Bool(false)),
        ("m_Width".to_owned(), V::Int(size.0)),
        ("m_Height".to_owned(), V::Int(size.1)),
        ("m_CompleteImageSize".to_owned(), V::UInt(complete)),
        ("m_TextureFormat".to_owned(), V::Int(fmt)),
        ("m_MipCount".to_owned(), V::Int(1)),
        ("m_IsReadable".to_owned(), V::Bool(false)),
        ("m_ImageCount".to_owned(), V::Int(1)),
        ("m_TextureDimension".to_owned(), V::Int(2)),
        ("m_TextureSettings".to_owned(), settings),
        ("m_LightmapFormat".to_owned(), V::Int(0)),
        ("m_ColorSpace".to_owned(), V::Int(1)),
    ]
}

/// A `Texture2D` (class 28) with `data` inline in `image_data`, and no
/// `m_StreamData` (a tree from before 5.3).
#[must_use]
pub fn texture(path_id: i64, name: &[u8], fmt: i32, size: (i32, i32), data: Vec<u8>) -> Object {
    let complete = u32::try_from(data.len()).unwrap_or(u32::MAX);
    let mut fields = texture_fields(name, fmt, size, complete);
    fields.push(("image_data".to_owned(), V::Bytes(data)));
    Object::new(path_id, 28, V::Struct("Texture2D".to_owned(), fields))
}

/// A `Texture2D` (class 28) with an empty `image_data` and an `m_StreamData`
/// naming `len` bytes at `offset` of the resource `path` (empty: no stream).
#[must_use]
pub fn texture_streamed(
    path_id: i64,
    name: &[u8],
    fmt: i32,
    size: (i32, i32),
    path: &str,
    offset: u64,
    len: u32,
) -> Object {
    let mut fields = texture_fields(name, fmt, size, len);
    fields.push(("image_data".to_owned(), V::Bytes(Vec::new())));
    let stream = st(
        "StreamingInfo",
        vec![
            ("offset", V::UInt64(offset)),
            ("size", V::UInt(len)),
            ("path", s(path)),
        ],
    );
    fields.push(("m_StreamData".to_owned(), stream));
    Object::new(path_id, 28, V::Struct("Texture2D".to_owned(), fields))
}

/// `object` with its field `name` set to `value` (in place, keeping its
/// position) and renamed `rename` when that is set; a field it lacks is
/// appended. A scene's way to give a builder's object another shape.
#[must_use]
pub fn set_field(mut object: Object, name: &str, value: V, rename: Option<&str>) -> Object {
    if let V::Struct(_, fields) = &mut object.value {
        let to = rename.unwrap_or(name).to_owned();
        match fields.iter_mut().find(|(n, _)| n == name) {
            Some(f) => *f = (to, value),
            None => fields.push((to, value)),
        }
    }
    object
}
