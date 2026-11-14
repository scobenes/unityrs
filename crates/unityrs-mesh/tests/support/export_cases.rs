// Ported from UnityPy 1.25.2 (MIT): UnityPy/classes/generated.py's required renderer field lists. See THIRD_PARTY_LICENSES.
//! Synthetic export inputs shared by the recorder example and Rust tests.
use unityrs_testkit::{
    ngui::{Object, V, pair, pptr, s, set_field, st, texture, vector},
    scene3d::{
        self,
        mesh::{self, MeshVersion},
    },
};
pub struct Case {
    pub name: &'static str,
    pub kind: &'static str,
    pub objects: Vec<Object>,
    pub target: i64,
    pub names: Option<Vec<Option<Vec<u8>>>>,
}
fn channel(offset: u8, dim: u8) -> V {
    st(
        "ChannelInfo",
        vec![
            ("stream", V::UInt8(0)),
            ("offset", V::UInt8(offset)),
            ("format", V::UInt8(0)),
            ("dimension", V::UInt8(dim)),
        ],
    )
}
fn geometry(name: &[u8], positions: bool, normals: bool, uv: bool) -> Object {
    let mut m = mesh::mesh(4, name, &[[0.; 3]; 3], &[0, 1, 2], MeshVersion::V6000);
    let mut channels = vec![channel(0, 0); 14];
    let mut stride = 0u8;
    for (i, dim, on) in [(0, 3, positions), (1, 3, normals), (4, 2, uv)] {
        if on {
            channels[i] = channel(stride, dim);
            stride += dim * 4;
        }
    }
    let mut data = Vec::new();
    for i in 0..3 {
        if positions {
            for f in [
                [0., 1e-10, f32::NAN],
                [f32::INFINITY, -1.5, 0.1],
                [1., 2., 3.],
            ][i]
            {
                data.extend(f.to_le_bytes());
            }
        }
        if normals {
            for f in [[1., 0., 0.], [0., 1., 0.], [f32::NAN, 0., -1.]][i] {
                data.extend(f.to_le_bytes());
            }
        }
        if uv {
            for f in [[0.5f32, f32::NAN], [0.25, 1.], [0., 0.]][i] {
                data.extend(f.to_le_bytes());
            }
        }
    }
    data.resize(data.len().next_multiple_of(16), 0);
    m = set_field(
        m,
        "m_VertexData",
        st(
            "VertexData",
            vec![
                ("m_VertexCount", V::UInt(3)),
                ("m_Channels", vector(channel(0, 0), channels)),
                ("m_DataSize", V::Bytes(data)),
            ],
        ),
        None,
    );
    m
}
fn color(r: f32, g: f32, b: f32, a: f32) -> V {
    st(
        "ColorRGBA",
        vec![
            ("r", V::Float(r)),
            ("g", V::Float(g)),
            ("b", V::Float(b)),
            ("a", V::Float(a)),
        ],
    )
}
fn texenv(id: i64) -> V {
    st(
        "UnityTexEnv",
        vec![
            ("m_Texture", pptr("Texture", (0, id))),
            (
                "m_Scale",
                st("Vector2f", vec![("x", V::Float(1.)), ("y", V::Float(1.))]),
            ),
            (
                "m_Offset",
                st("Vector2f", vec![("x", V::Float(0.)), ("y", V::Float(0.))]),
            ),
        ],
    )
}
pub fn material(colors: bool, textures: bool, fast: bool) -> Object {
    let key = |v: &str| {
        if fast {
            st("FastPropertyName", vec![("name", s(v))])
        } else {
            s(v)
        }
    };
    let colors = if colors {
        vec![
            pair(key("_Color"), color(0.25, 0.5, 0.75, 1.)),
            pair(key("_SColor"), color(-0., 1., 2., 1.)),
            pair(key("_SpecularColor"), color(0.1, 0.2, 0.3, 1.)),
            pair(key("_Color"), color(1., 0., 0., 1.)),
        ]
    } else {
        vec![]
    };
    let textures = if textures {
        vec![
            pair(key("_MainTex"), texenv(7)),
            pair(key("_MainTex"), texenv(6)),
            pair(key("_BumpMap"), texenv(7)),
            pair(key("_SpecularTex"), texenv(6)),
            pair(key("_NormalTex"), texenv(6)),
            pair(key("ab"), texenv(0)),
        ]
    } else {
        vec![]
    };
    Object::new(
        5,
        21,
        st(
            "Material",
            vec![
                ("m_Name", s("mat")),
                ("m_Shader", pptr("Shader", (0, 0))),
                (
                    "m_SavedProperties",
                    st(
                        "UnityPropertySheet",
                        vec![
                            (
                                "m_Colors",
                                vector(pair(key(""), color(0., 0., 0., 1.)), colors),
                            ),
                            (
                                "m_Floats",
                                vector(
                                    pair(key(""), V::Float(0.)),
                                    vec![
                                        pair(key("_Shininess"), V::Float(0.03125)),
                                        pair(key("_Transparency"), V::Float(-0.)),
                                    ],
                                ),
                            ),
                            ("m_TexEnvs", vector(pair(key(""), texenv(0)), textures)),
                        ],
                    ),
                ),
            ],
        ),
    )
}
fn obj(name: &'static str, m: Object, names: Option<Vec<Option<Vec<u8>>>>) -> Case {
    Case {
        name,
        kind: "obj",
        objects: vec![m],
        target: 4,
        names,
    }
}
fn complete_renderer(r: &mut V) {
    // The graph builders contain just graph facts. Object-mode oracle inputs
    // also need every required field of the generated attrs classes.
    for (name, value) in [
        ("m_CastShadows", V::Int(1)),
        ("m_LightmapIndex", V::Int(0)),
        (
            "m_LightmapTilingOffset",
            scene3d::xyz("Vector4f", &[1., 1., 0., 0.]),
        ),
        ("m_ReceiveShadows", V::Bool(true)),
    ] {
        scene3d::set(r, name, value);
    }
}
fn renderer(
    name: &'static str,
    first: u16,
    subset: Option<Vec<u32>>,
    colors: bool,
    textures: bool,
    fast: bool,
) -> Case {
    let mut m = mesh::mesh(
        4,
        b"m",
        &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
        &[0, 1, 2, 0, 1, 2, 0, 1, 2],
        MeshVersion::V6000,
    );
    m = set_field(
        m,
        "m_SubMeshes",
        vector(
            mesh::submesh(0, 0, 0, 0, 0, 3),
            (0..3)
                .map(|i| mesh::submesh(i * 12, 3, 0, 0, 0, 3))
                .collect(),
        ),
        None,
    );
    let mut r = scene3d::mesh_renderer((0, 1), &[(0, 5), (0, 0)], (first, u16::from(first > 0)));
    complete_renderer(&mut r);
    if let V::Struct(_, fields) = &mut r {
        fields.retain(|(n, _)| n != "m_SubsetIndices");
    }
    if let Some(subset) = subset {
        if let V::Struct(_, fields) = &mut r {
            fields.retain(|(n, _)| n != "m_StaticBatchInfo");
        }
        scene3d::set(
            &mut r,
            "m_SubsetIndices",
            vector(V::UInt(0), subset.into_iter().map(V::UInt).collect()),
        );
    }
    Case {
        name,
        kind: "renderer",
        objects: vec![
            Object::new(1, 1, scene3d::game_object(b"go", &[(0, 2)])),
            Object::new(2, 33, scene3d::mesh_filter((0, 1), (0, 4))),
            Object::new(3, 23, r),
            m,
            material(colors, textures, fast),
            texture(6, b"tex", 4, (1, 2), vec![255, 0, 0, 255, 0, 0, 255, 255]),
            texture(7, b"", 4, (1, 1), vec![0, 255, 0, 255]),
        ],
        target: 3,
        names: None,
    }
}
#[allow(clippy::too_many_lines, reason = "table of synthetic export cases")]
pub fn cases() -> Vec<Case> {
    let mut cases = vec![
        obj("raw_specials", geometry(b"m", true, true, true), None),
        obj(
            "missing_uv_normals",
            geometry(b"plain", true, false, false),
            None,
        ),
        obj(
            "raw_name",
            geometry(b"raw\xff", true, false, false),
            Some(vec![Some(b"mat\xfe".to_vec())]),
        ),
        obj(
            "short_materials",
            geometry(b"m", true, true, true),
            Some(vec![Some(b"mat".to_vec())]),
        ),
        obj(
            "empty_materials",
            geometry(b"m", true, false, false),
            Some(vec![]),
        ),
        obj(
            "null_material",
            geometry(b"m", true, false, false),
            Some(vec![None]),
        ),
        obj(
            "empty_material",
            geometry(b"m", true, false, false),
            Some(vec![Some(vec![])]),
        ),
        obj("no_positions", geometry(b"empty", false, true, true), None),
        obj(
            "no_vertices",
            mesh::mesh(4, b"empty", &[], &[], MeshVersion::V6000),
            None,
        ),
    ];
    for (name, topology, indices) in [
        ("strip", 1, vec![0, 1, 2]),
        ("quad", 2, vec![0, 1, 2, 0]),
        ("lines", 3, vec![0, 1]),
        ("points", 5, vec![0]),
        ("cut_triangle", 0, vec![0, 1]),
    ] {
        let mut m = mesh::mesh(4, b"m", &[[0.; 3]; 3], &indices, MeshVersion::V6000);
        m = set_field(
            m,
            "m_SubMeshes",
            vector(
                mesh::submesh(0, 0, 0, 0, 0, 0),
                vec![mesh::submesh(
                    0,
                    indices.len().try_into().unwrap(),
                    topology,
                    0,
                    0,
                    3,
                )],
            ),
            None,
        );
        cases.push(obj(name, m, None));
    }
    let mut m = geometry(b"compressed", true, false, false);
    let mut compressed = mesh::compressed();
    let packed = st(
        "PackedBitVector",
        vec![
            ("m_NumItems", V::UInt(2)),
            ("m_Range", V::Float(0.)),
            ("m_Start", V::Float(0.)),
            ("m_Data", vector(V::UInt8(0), vec![V::UInt8(0)])),
            ("m_BitSize", V::UInt8(1)),
        ],
    );
    let mut positions = packed.clone();
    scene3d::set(&mut positions, "m_NumItems", V::UInt(9));
    scene3d::set(
        &mut positions,
        "m_Data",
        vector(V::UInt8(0), vec![V::UInt8(0); 2]),
    );
    scene3d::set(&mut compressed, "m_Vertices", positions);
    scene3d::set(&mut compressed, "m_Normals", packed);
    scene3d::set(
        &mut compressed,
        "m_NormalSigns",
        st(
            "PackedBitVector",
            vec![
                ("m_NumItems", V::UInt(1)),
                ("m_Data", vector(V::UInt8(0), vec![V::UInt8(1)])),
                ("m_BitSize", V::UInt8(1)),
            ],
        ),
    );
    m = set_field(m, "m_CompressedMesh", compressed, None);
    cases.push(obj("compressed_int_zero_normals", m, None));
    cases.extend([
        renderer("batch_offset", 1, None, false, false, false),
        renderer("unbatched_unity6", 0, None, false, false, false),
        renderer("legacy_subset", 0, Some(vec![1, 2]), false, false, false),
        renderer("empty_subset", 0, Some(vec![]), false, false, false),
        renderer("colors", 0, None, true, false, false),
        renderer("textures", 0, None, false, true, false),
        renderer("fast_keys", 1, None, true, true, true),
    ]);
    let mut no_mesh = renderer("no_mesh", 0, None, false, false, false);
    no_mesh.objects[1] = Object::new(2, 33, scene3d::mesh_filter((0, 1), (0, 0)));
    cases.push(no_mesh);
    let mut skin = renderer("skinned", 0, None, false, false, false);
    let mut r = scene3d::skinned_mesh_renderer((0, 1), (0, 4), &[], &[]);
    complete_renderer(&mut r);
    for (name, value) in [
        (
            "m_AABB",
            st(
                "AABB",
                vec![
                    ("m_Center", scene3d::xyz("Vector3f", &[0.; 3])),
                    ("m_Extent", scene3d::xyz("Vector3f", &[1.; 3])),
                ],
            ),
        ),
        ("m_DirtyAABB", V::Bool(false)),
        ("m_Quality", V::Int(1)),
        ("m_UpdateWhenOffscreen", V::Bool(false)),
    ] {
        scene3d::set(&mut r, name, value);
    }
    scene3d::set(
        &mut r,
        "m_Materials",
        scene3d::pointers("Material", &[(0, 5)]),
    );
    skin.objects[2] = Object::new(3, 137, r);
    // Only the SkinnedMeshRenderer branch can find this mesh.
    skin.objects[0] = Object::new(1, 1, scene3d::game_object(b"go", &[]));
    cases.push(skin);
    let mut empty_name = renderer("empty_texture_name", 1, None, false, false, false);
    let V::Struct(_, fields) = &mut empty_name.objects[4].value else {
        panic!("material")
    };
    let saved = &mut fields
        .iter_mut()
        .find(|(n, _)| n == "m_SavedProperties")
        .unwrap()
        .1;
    scene3d::set(
        saved,
        "m_TexEnvs",
        vector(pair(s(""), texenv(0)), vec![pair(s(""), texenv(7))]),
    );
    cases.push(empty_name);
    cases
}
