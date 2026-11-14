use std::collections::BTreeMap;
use unityrs_core::{RawStr, limits::Meter};
use unityrs_mesh::{
    MeshError,
    obj::{self, ChargedBytes, MeteredWrite, ObjSink},
};
use unityrs_testkit::{
    ngui::{self, Object, V, pair, pptr, s, st, vector},
    scene3d::{self, mesh::MeshVersion},
};
mod support;
#[derive(Default)]
struct Sink(BTreeMap<Vec<u8>, Vec<u8>>);
impl ObjSink for Sink {
    fn file(
        &mut self,
        stem: &[u8],
        ext: &'static str,
        write: &mut dyn FnMut(&mut dyn MeteredWrite) -> Result<(), MeshError>,
    ) -> Result<(), MeshError> {
        let mut out = ChargedBytes::new(&Meter::unlimited());
        write(&mut out)?;
        let mut path = stem.to_vec();
        path.extend_from_slice(ext.as_bytes());
        self.0.insert(path, out.as_bytes().to_vec());
        Ok(())
    }
}
fn material() -> Object {
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
                                vector(
                                    pair(
                                        s(""),
                                        st(
                                            "ColorRGBA",
                                            vec![
                                                ("r", V::Float(1.)),
                                                ("g", V::Float(0.)),
                                                ("b", V::Float(0.)),
                                                ("a", V::Float(1.)),
                                            ],
                                        ),
                                    ),
                                    vec![],
                                ),
                            ),
                            ("m_Floats", vector(pair(s(""), V::Float(0.)), vec![])),
                            (
                                "m_TexEnvs",
                                vector(pair(s(""), st("UnityTexEnv", vec![])), vec![]),
                            ),
                        ],
                    ),
                ),
            ],
        ),
    )
}
#[test]
fn corrected_renderer_batch_alignment_and_optional_fields() {
    let mut mesh = scene3d::mesh::mesh(
        4,
        b"m",
        &[[0.; 3]; 3],
        &[0, 1, 2, 0, 1, 2],
        MeshVersion::V6000,
    );
    mesh = ngui::set_field(
        mesh,
        "m_SubMeshes",
        vector(
            scene3d::mesh::submesh(0, 3, 0, 0, 0, 3),
            vec![
                scene3d::mesh::submesh(0, 3, 0, 0, 0, 3),
                scene3d::mesh::submesh(6, 3, 0, 0, 0, 3),
            ],
        ),
        None,
    );
    let objects = vec![
        Object::new(1, 1, scene3d::game_object(b"go", &[(0, 2)])),
        Object::new(2, 33, scene3d::mesh_filter((0, 1), (0, 4))),
        Object::new(3, 23, scene3d::mesh_renderer((0, 1), &[(0, 5)], (1, 1))),
        mesh,
        material(),
    ];
    let (env, file) = support::load(MeshVersion::V6000, &objects);
    let mut sink = Sink::default();
    obj::export_mesh_renderer(&file.object(3).unwrap(), &env, file.meter(), &mut sink).unwrap();
    let text = std::str::from_utf8(&sink.0[b"m.obj".as_slice()]).unwrap();
    assert!(text.contains("g m_0\nf"));
    assert!(text.contains("g m_1\nusemtl mat\nf"));
    assert_eq!(
        std::str::from_utf8(&sink.0[b"m.mtl".as_slice()]).unwrap(),
        "newmtl mat\nKa 0.2000 0.2000 0.2000\nKd 0.8000 0.8000 0.8000\nKs 0.2000 0.2000 0.2000\nTr 0.0000\nNs 20.0000"
    );
    let _ = RawStr::from("m");
}
