#![allow(dead_code, clippy::too_many_lines)]
use std::sync::Arc;
use unityrs_core::{
    Bytes, RawStr,
    env::{EnvOptions, Environment, Source},
    files::File,
    limits::Meter,
};
use unityrs_scene::{Graph, GraphOptions, graph_from_scene_file};
use unityrs_testkit::{
    ngui::{Object, V, st, vector},
    scene3d::{self as s, mesh as m},
};
pub fn objects(skin: bool, batch: bool) -> Vec<Object> {
    let mut parent = s::transform((0, 1), (0, 0), &[(0, 4)], false);
    s::set(
        &mut parent,
        "m_LocalPosition",
        s::xyz("Vector3f", &[5., 2., 0.]),
    );
    let mut child = s::transform((0, 3), (0, 2), &[], false);
    s::set(
        &mut child,
        "m_LocalPosition",
        s::xyz("Vector3f", &[2., 0., 0.]),
    );
    let renderer = if skin {
        s::skinned_mesh_renderer((0, 3), (0, 7), &[(0, 2)], &[25.])
    } else {
        s::mesh_renderer((0, 3), &[], if batch { (1, 1) } else { (0, 0) })
    };
    let mut mesh = m::mesh(
        7,
        b"mesh",
        &[[1., 0., 0.], [0., 1., 0.], [0., 0., 0.]],
        &[0, 1, 2, 0, 1, 2],
        m::MeshVersion::V2022,
    );
    s::set(
        &mut mesh.value,
        "m_SubMeshes",
        vector(
            m::submesh(0, 0, 0, 0, 0, 0),
            vec![m::submesh(0, 3, 0, 0, 0, 3), m::submesh(12, 3, 0, 0, 0, 3)],
        ),
    );
    let mut channels = Vec::new();
    let mut offset = 0u8;
    for i in 0..14 {
        let dimension = match i {
            0 | 1 => 3,
            2 | 12 | 13 => 4,
            4 => 2,
            _ => 0,
        };
        channels.push(st(
            "ChannelInfo",
            vec![
                ("stream", V::UInt8(0)),
                ("offset", V::UInt8(offset)),
                ("format", V::UInt8(if i == 13 { 10 } else { 0 })),
                ("dimension", V::UInt8(dimension)),
            ],
        ));
        offset += dimension * 4;
    }
    let mut data = Vec::new();
    for (i, pos) in [[1f32, 0., 0.], [0., 1., 0.], [0., 0., 0.]]
        .iter()
        .enumerate()
    {
        for x in pos
            .iter()
            .chain([0., 0., 1.].iter())
            .chain([1., 0., 0., 1.].iter())
            .chain([0.2 + f32::from(u8::try_from(i).unwrap()) * 0.1, 0.25].iter())
            .chain([1., 0., 0., 0.].iter())
        {
            data.extend(x.to_le_bytes());
        }
        for j in [0u32; 4] {
            data.extend(j.to_le_bytes());
        }
    }
    data.resize(data.len().next_multiple_of(16), 0);
    s::set(
        &mut mesh.value,
        "m_VertexData",
        st(
            "VertexData",
            vec![
                ("m_VertexCount", V::UInt(3)),
                ("m_Channels", vector(channels[0].clone(), channels)),
                ("m_DataSize", V::Bytes(data)),
            ],
        ),
    );
    s::set(
        &mut mesh.value,
        "m_BindPose",
        vector(
            m::matrix([0.; 16]),
            vec![m::matrix([
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ])],
        ),
    );
    s::set(
        &mut mesh.value,
        "m_Shapes",
        m::blend_shapes(
            vec![
                m::blend_vertex(1, [0.5, 0., 0.], [0.; 3], [0.; 3]),
                m::blend_vertex(2, [2., 0., 0.], [0.; 3], [0.; 3]),
            ],
            vec![
                m::blend_frame(0, 1, false, false),
                m::blend_frame(1, 1, false, false),
            ],
            vec![m::blend_channel(b"bend", 0, 0, 2)],
            vec![50., 100.],
        ),
    );
    vec![
        Object::new(1, 1, s::game_object(b"parent", &[(0, 2)])),
        Object::new(2, 4, parent),
        Object::new(3, 1, s::game_object(b"child", &[(0, 4), (0, 5), (0, 6)])),
        Object::new(4, 4, child),
        Object::new(5, 33, s::mesh_filter((0, 3), (0, 7))),
        Object::new(6, if skin { 137 } else { 23 }, renderer),
        mesh,
    ]
}
pub fn graph(objects: &[Object]) -> (Environment, Graph) {
    graph_in(objects, EnvOptions::unitypy())
}
pub fn graph_in(objects: &[Object], options: EnvOptions) -> (Environment, Graph) {
    let env = Environment::new(options);
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"synthetic"),
            data: Bytes::from(s::file(s::Layout::Unity2022, &[], objects).unwrap()),
        })
        .unwrap()
    else {
        panic!("fixture")
    };
    let g =
        graph_from_scene_file(&env, &file, &GraphOptions::default(), &Meter::unlimited()).unwrap();
    (env, g)
}
pub fn collect(skin: bool, batch: bool) -> unityrs_gltf::Scene {
    let (env, g) = graph(&objects(skin, batch));
    let meter = Meter::unlimited();
    unityrs_gltf::collect(&env, &g, &unityrs_gltf::Profile::new(&meter), &meter).unwrap()
}
pub fn floats(s: &unityrs_gltf::Scene, id: usize) -> &[f64] {
    let unityrs_gltf::Data::Float(v) = &s.accessors.as_slice()[id].data else {
        panic!("float")
    };
    v.as_slice()
}
pub fn write(s: &unityrs_gltf::Scene) -> Vec<u8> {
    let mut bytes = Vec::new();
    unityrs_gltf::write_glb(
        s,
        &unityrs_gltf::Options::default(),
        &Meter::unlimited(),
        &mut bytes,
    )
    .unwrap();
    bytes
}
pub fn meter() -> Arc<Meter> {
    Meter::unlimited()
}

/// Review repro: root -> hips -> spine, sibling body; optionally a root prop.
pub fn hierarchy_objects(cross_root: bool) -> Vec<Object> {
    let mut objects = objects(true, false);
    objects[0] = Object::new(1, 1, s::game_object(b"root", &[(0, 2)]));
    objects[1] = Object::new(2, 4, s::transform((0, 1), (0, 0), &[(0, 8), (0, 4)], false));
    s::set(
        &mut objects[1].value,
        "m_LocalPosition",
        s::xyz("Vector3f", &[1., 0., 0.]),
    );
    objects.extend([
        Object::new(9, 1, s::game_object(b"hips", &[(0, 8)])),
        Object::new(8, 4, s::transform((0, 9), (0, 2), &[(0, 10)], false)),
        Object::new(11, 1, s::game_object(b"spine", &[(0, 10)])),
        Object::new(10, 4, s::transform((0, 11), (0, 8), &[], false)),
        Object::new(13, 1, s::game_object(b"prop", &[(0, 12)])),
        Object::new(12, 4, s::transform((0, 13), (0, 0), &[], false)),
    ]);
    for index in [8, 10] {
        s::set(
            &mut objects[index].value,
            "m_LocalPosition",
            s::xyz("Vector3f", &[0., 1., 0.]),
        );
    }
    objects[5] = Object::new(
        6,
        137,
        s::skinned_mesh_renderer(
            (0, 3),
            (0, 7),
            &[(0, 8), (0, if cross_root { 12 } else { 10 })],
            &[25.],
        ),
    );
    s::set(
        &mut objects[5].value,
        "m_RootBone",
        unityrs_testkit::ngui::pptr("Transform", (0, 10)),
    );
    let identity = m::matrix([
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ]);
    s::set(
        &mut objects[6].value,
        "m_BindPose",
        vector(identity.clone(), vec![identity.clone(), identity]),
    );
    let V::Struct(_, fields) = &mut objects[6].value else {
        panic!("mesh")
    };
    let (_, V::Struct(_, vertex)) = fields
        .iter_mut()
        .find(|(k, _)| k == "m_VertexData")
        .unwrap()
    else {
        panic!("vertex")
    };
    let (_, V::Bytes(data)) = vertex.iter_mut().find(|(k, _)| k == "m_DataSize").unwrap() else {
        panic!("data")
    };
    // The middle vertex follows the second bone, exercising both joint roots.
    data[144..148].copy_from_slice(&1u32.to_le_bytes());
    objects
}

pub fn validator_clean(scene: &unityrs_gltf::Scene) {
    let bytes = write(scene);
    let Some(exe) = std::env::var_os("UNITYRS_GLTF_VALIDATOR") else {
        assert!(std::env::var_os("UNITYRS_GLTF_VALIDATOR_REQUIRED").is_none());
        eprintln!("review validator skipped: set UNITYRS_GLTF_VALIDATOR");
        return;
    };
    let scratch = unityrs_testkit::scratch::Scratch::new("gltf-review-validator");
    let path = scratch.path().join("scene.glb");
    std::fs::write(&path, bytes).unwrap();
    let out = unityrs_testkit::child::output(
        std::process::Command::new(exe)
            .args(["--stdout", "--all"])
            .arg(path),
    )
    .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["validatorVersion"], "2.0.0-dev.3.10");
    assert_eq!(report["issues"]["numErrors"], 0, "{report}");
    assert_eq!(report["issues"]["numWarnings"], 0, "{report}");
    assert_eq!(report["issues"]["truncated"], false);
    assert!(out.status.success(), "{report}");
}

pub fn repair_objects() -> Vec<Object> {
    let mut objects = hierarchy_objects(true);
    objects[0] = Object::new(1, 1, s::game_object(b"parent", &[(0, 2)]));
    objects[5] = Object::new(
        6,
        137,
        s::skinned_mesh_renderer((0, 3), (0, 7), &[(0, 8), (0, 12), (0, 8)], &[]),
    );
    let identity = m::matrix([
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ]);
    s::set(
        &mut objects[6].value,
        "m_BindPose",
        vector(identity.clone(), vec![identity; 3]),
    );
    objects
}
