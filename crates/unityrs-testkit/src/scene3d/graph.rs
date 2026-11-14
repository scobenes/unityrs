// Ported from UnityPy 1.25.2 (MIT): UnityPy/classes/generated.py's GameObject, Transform, RectTransform, MeshFilter, MeshRenderer, SkinnedMeshRenderer and LODGroup field lists. See THIRD_PARTY_LICENSES.
//! Synthetic 3D graph facts on hand-built trees. These deliberately small
//! embedded trees describe only fields used by the scene graph, with no
//! TPK input. They are not complete `Editor` or stripped-file layouts.
//! Geometry and Material writers belong to the mesh/material work.
use crate::ngui::{BuildError, Object, V, nodes, pair, pptr, st, vector, write};
use crate::serialized::{ExternalSpec, ObjectSpec, SerializedSpec, TypeSpec};
use crate::writer::ByteWriter;

/// Engine versions exercised by scene tests. Graph fields have the same
/// shape in these versions; the file's version identifies each case.
#[derive(Clone, Copy, Debug)]
pub enum Layout {
    Unity2019,
    Unity2022,
    Unity6000,
}
impl Layout {
    /// The synthetic file's engine version.
    #[must_use]
    pub const fn version(self) -> &'static [u8] {
        match self {
            Self::Unity2019 => b"2019.4.0f1",
            Self::Unity2022 => b"2022.3.0f1",
            Self::Unity6000 => b"6000.0.0f1",
        }
    }
}
/// A `GameObject`'s dict facts, with modern `ComponentPairs`.
#[must_use]
pub fn game_object(name: &[u8], components: &[(i32, i64)]) -> V {
    st(
        "GameObject",
        vec![
            ("m_Name", V::Str(name.to_vec())),
            ("m_IsActive", V::Bool(true)),
            ("m_Layer", V::UInt(0)),
            ("m_Tag", V::UInt(0)),
            (
                "m_Component",
                vector(
                    component_pair((0, 0)),
                    components.iter().map(|p| component_pair(*p)).collect(),
                ),
            ),
        ],
    )
}
fn component_pair(ptr: (i32, i64)) -> V {
    st("ComponentPair", vec![("component", pptr("Component", ptr))])
}
/// The legacy 5.4 (class id, `PPtr`) component representation (Tier B).
#[must_use]
pub fn legacy_game_object(name: &[u8], components: &[(i32, (i32, i64))]) -> V {
    let mut go = game_object(name, &[]);
    set(
        &mut go,
        "m_Component",
        vector(
            pair(V::Int(0), pptr("Component", (0, 0))),
            components
                .iter()
                .map(|(id, ptr)| pair(V::Int(*id), pptr("Component", *ptr)))
                .collect(),
        ),
    );
    go
}
/// A vector3 or quaternion.
#[must_use]
pub fn xyz(ty: &str, values: &[f32]) -> V {
    st(
        ty,
        ["x", "y", "z", "w"]
            .iter()
            .zip(values)
            .map(|(n, x)| (*n, V::Float(*x)))
            .collect(),
    )
}
/// Identity local TRS, with caller-chosen hierarchy links. Change fields
/// with `ngui::set_field` for non-identity or hostile cases.
#[must_use]
pub fn transform(
    game_object: (i32, i64),
    father: (i32, i64),
    children: &[(i32, i64)],
    rect: bool,
) -> V {
    let mut value = st(
        if rect { "RectTransform" } else { "Transform" },
        vec![
            ("m_GameObject", pptr("GameObject", game_object)),
            ("m_LocalPosition", xyz("Vector3f", &[0., 0., 0.])),
            ("m_LocalRotation", xyz("Quaternionf", &[0., 0., 0., 1.])),
            ("m_LocalScale", xyz("Vector3f", &[1., 1., 1.])),
            ("m_Children", pointers("Transform", children)),
            ("m_Father", pptr("Transform", father)),
        ],
    );
    if rect {
        set(&mut value, "m_AnchorMin", xyz("Vector2f", &[0., 0.]));
        set(&mut value, "m_AnchorMax", xyz("Vector2f", &[1., 1.]));
        set(&mut value, "m_AnchoredPosition", xyz("Vector2f", &[0., 0.]));
        set(&mut value, "m_SizeDelta", xyz("Vector2f", &[100., 100.]));
        set(&mut value, "m_Pivot", xyz("Vector2f", &[0.5, 0.5]));
    }
    value
}
/// An ordered pointer array, including nulls.
#[must_use]
pub fn pointers(ty: &str, items: &[(i32, i64)]) -> V {
    vector(
        pptr(ty, (0, 0)),
        items.iter().map(|p| pptr(ty, *p)).collect(),
    )
}
/// `MeshFilter` facts.
#[must_use]
pub fn mesh_filter(go: (i32, i64), mesh: (i32, i64)) -> V {
    st(
        "MeshFilter",
        vec![
            ("m_GameObject", pptr("GameObject", go)),
            ("m_Mesh", pptr("Mesh", mesh)),
        ],
    )
}
/// Renderer facts, including a static batch and the legacy subset list.
#[must_use]
pub fn mesh_renderer(go: (i32, i64), materials: &[(i32, i64)], static_batch: (u16, u16)) -> V {
    st(
        "MeshRenderer",
        vec![
            ("m_GameObject", pptr("GameObject", go)),
            ("m_Enabled", V::Bool(true)),
            ("m_Materials", pointers("Material", materials)),
            (
                "m_StaticBatchInfo",
                st(
                    "StaticBatchInfo",
                    vec![
                        ("firstSubMesh", V::UInt(u32::from(static_batch.0))),
                        ("subMeshCount", V::UInt(u32::from(static_batch.1))),
                    ],
                ),
            ),
            ("m_SubsetIndices", vector(V::UInt(0), vec![])),
            ("m_StaticBatchRoot", pptr("Transform", (0, 0))),
        ],
    )
}
/// Skin references and blend weights, reusing renderer facts.
#[must_use]
pub fn skinned_mesh_renderer(
    go: (i32, i64),
    mesh: (i32, i64),
    bones: &[(i32, i64)],
    weights: &[f32],
) -> V {
    let V::Struct(_, fields) = mesh_renderer(go, &[], (0, 0)) else {
        return st("SkinnedMeshRenderer", vec![]);
    };
    let mut v = V::Struct("SkinnedMeshRenderer".into(), fields);
    for (name, value) in [
        ("m_Mesh", pptr("Mesh", mesh)),
        ("m_Bones", pointers("Transform", bones)),
        (
            "m_RootBone",
            pptr("Transform", bones.first().copied().unwrap_or((0, 0))),
        ),
        (
            "m_BlendShapeWeights",
            vector(V::Float(0.), weights.iter().map(|x| V::Float(*x)).collect()),
        ),
    ] {
        set(&mut v, name, value);
    }
    v
}
/// One LOD and an `LODGroup` with those levels in order.
#[must_use]
pub fn lod_group(go: (i32, i64), levels: &[(f32, Vec<(i32, i64)>)]) -> V {
    let level = |height, renderers: &[(i32, i64)]| {
        st(
            "LOD",
            vec![
                ("screenRelativeHeight", V::Float(height)),
                ("fadeTransitionWidth", V::Float(0.1)),
                (
                    "renderers",
                    vector(
                        st("LODRenderer", vec![("renderer", pptr("Renderer", (0, 0)))]),
                        renderers
                            .iter()
                            .map(|p| st("LODRenderer", vec![("renderer", pptr("Renderer", *p))]))
                            .collect(),
                    ),
                ),
            ],
        )
    };
    st(
        "LODGroup",
        vec![
            ("m_GameObject", pptr("GameObject", go)),
            ("m_Enabled", V::Bool(true)),
            ("m_LocalReferencePoint", xyz("Vector3f", &[0., 0., 0.])),
            ("m_Size", V::Float(2.)),
            (
                "m_LODs",
                vector(
                    level(0., &[]),
                    levels.iter().map(|(h, r)| level(*h, r)).collect(),
                ),
            ),
        ],
    )
}
/// A file with independently typed objects, preserving caller table order.
/// All trees are hand-built from these values; no external schema is read.
pub fn file(layout: Layout, externals: &[&str], objects: &[Object]) -> Result<Vec<u8>, BuildError> {
    let mut types = Vec::new();
    let mut rows = Vec::new();
    for (i, obj) in objects.iter().enumerate() {
        types.push(TypeSpec {
            class_id: obj.class_id,
            nodes: nodes(0, "Base", &obj.value)?,
            ..TypeSpec::default()
        });
        let mut writer = ByteWriter::le();
        write(&mut writer, &obj.value)?;
        let mut data = writer.into_inner();
        if let Some(cut) = obj.cut {
            data.truncate(cut);
        }
        data.resize(data.len() + obj.trailing, 0);
        rows.push(ObjectSpec {
            path_id: obj.path_id,
            type_id: i32::try_from(i).map_err(|_| BuildError::TooManyObjects(objects.len()))?,
            data,
            ..ObjectSpec::default()
        });
    }
    Ok(crate::serialized::build(&SerializedSpec {
        unity_version: layout.version().to_vec(),
        types,
        objects: rows,
        externals: externals
            .iter()
            .map(|s| ExternalSpec {
                path: s.as_bytes().to_vec(),
                ..ExternalSpec::default()
            })
            .collect(),
        ..SerializedSpec::default()
    }))
}

/// Set or append a dict field in a synthetic struct.
pub fn set(value: &mut V, name: &str, replacement: V) {
    if let V::Struct(_, fields) = value {
        if let Some((_, v)) = fields.iter_mut().find(|(n, _)| n == name) {
            *v = replacement;
        } else {
            fields.push((name.to_owned(), replacement));
        }
    }
}
