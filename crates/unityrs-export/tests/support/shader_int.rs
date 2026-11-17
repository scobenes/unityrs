use unityrs_testkit::{
    ngui::{Object, V, s, st, vector},
    scene3d::{self, Layout},
};

pub fn shader_with_properties(kind: i32, defaults: &[f32], blob: bool) -> Vec<u8> {
    let property = |value| {
        st(
            "SerializedProperty",
            vec![
                (
                    "m_Attributes",
                    vector(s(""), vec![s("HDR"), s("Toggle(X)")]),
                ),
                ("m_Name", s("_Count")),
                ("m_Description", s("Count é")),
                ("m_Type", V::Int(kind)),
                ("m_Flags", V::UInt(0xffff)),
                ("m_DefValue[0]", V::Float(value)),
                ("m_DefValue[1]", V::Float(17.)),
                ("m_DefValue[2]", V::Float(23.)),
                ("m_DefValue[3]", V::Float(42.)),
            ],
        )
    };
    scene3d::file(
        Layout::Unity2022,
        &[],
        &[Object::new(
            1,
            48,
            st(
                "Shader",
                vec![
                    (
                        "compressedBlob",
                        V::Bytes(if blob { vec![1] } else { vec![] }),
                    ),
                    ("platforms", vector(V::Int(0), vec![])),
                    (
                        "m_ParsedForm",
                        st(
                            "SerializedShader",
                            vec![
                                ("m_Name", s("Synthetic/Int")),
                                (
                                    "m_PropInfo",
                                    st(
                                        "SerializedProperties",
                                        vec![(
                                            "m_Props",
                                            vector(
                                                property(0.),
                                                defaults.iter().copied().map(property).collect(),
                                            ),
                                        )],
                                    ),
                                ),
                                (
                                    "m_SubShaders",
                                    vector(st("SerializedSubShader", vec![]), vec![]),
                                ),
                                ("m_FallbackName", s("")),
                                ("m_CustomEditorName", s("")),
                            ],
                        ),
                    ),
                ],
            ),
        )],
    )
    .unwrap()
}
