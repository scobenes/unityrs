// Ported from UnityPy 1.25.2 (MIT): generated Material and Shader field lists. See THIRD_PARTY_LICENSES.
//! Small public-format material and shader facts on hand-built embedded trees.
//! These are synthetic field subsets, not complete Editor layouts.
use crate::ngui::{Object, V, pair, pptr, st};

/// A texture environment in Unity's bottom-left UV space.
#[must_use]
pub fn tex_env(texture: (i32, i64), scale: [f32; 2], offset: [f32; 2]) -> V {
    st(
        "UnityTexEnv",
        vec![
            ("m_Texture", pptr("Texture", texture)),
            ("m_Scale", super::xyz("Vector2f", &scale)),
            ("m_Offset", super::xyz("Vector2f", &offset)),
        ],
    )
}

fn color(value: [f32; 4]) -> V {
    st(
        "ColorRGBA",
        ["r", "g", "b", "a"]
            .iter()
            .zip(value)
            .map(|(name, x)| (*name, V::Float(x)))
            .collect(),
    )
}
fn map(shape: V, entries: Vec<V>) -> V {
    V::Vector("map".to_owned(), Box::new(shape), entries)
}
fn name(value: &str) -> V {
    V::Str(value.as_bytes().to_vec())
}

/// A Material with explicitly supplied properties and default render queue.
/// Entries are kept in caller order; duplicate names can exercise hostile input.
#[must_use]
pub fn material(
    path_id: i64,
    material_name: &[u8],
    shader: (i32, i64),
    floats: &[(&str, f32)],
    colors: &[(&str, [f32; 4])],
    textures: &[(&str, V)],
) -> Object {
    Object::new(
        path_id,
        21,
        st(
            "Material",
            vec![
                ("m_Name", V::Str(material_name.to_vec())),
                ("m_Shader", pptr("Shader", shader)),
                ("m_CustomRenderQueue", V::Int(-1)),
                (
                    "m_SavedProperties",
                    st(
                        "UnityPropertySheet",
                        vec![
                            (
                                "m_Floats",
                                map(
                                    pair(name(""), V::Float(0.)),
                                    floats
                                        .iter()
                                        .map(|(n, v)| pair(name(n), V::Float(*v)))
                                        .collect(),
                                ),
                            ),
                            (
                                "m_Colors",
                                map(
                                    pair(name(""), color([0.; 4])),
                                    colors
                                        .iter()
                                        .map(|(n, v)| pair(name(n), color(*v)))
                                        .collect(),
                                ),
                            ),
                            (
                                "m_TexEnvs",
                                map(
                                    pair(name(""), tex_env((0, 0), [1.; 2], [0.; 2])),
                                    textures
                                        .iter()
                                        .map(|(n, v)| pair(name(n), v.clone()))
                                        .collect(),
                                ),
                            ),
                        ],
                    ),
                ),
            ],
        ),
    )
}

/// A Shader's legacy name and modern parsed-form name, read by full parse.
#[must_use]
pub fn shader(path_id: i64, shader_name: &[u8]) -> Object {
    Object::new(
        path_id,
        48,
        st(
            "Shader",
            vec![
                ("m_Name", V::Str(shader_name.to_vec())),
                (
                    "m_ParsedForm",
                    st(
                        "SerializedShader",
                        vec![("m_Name", V::Str(shader_name.to_vec()))],
                    ),
                ),
            ],
        ),
    )
}
