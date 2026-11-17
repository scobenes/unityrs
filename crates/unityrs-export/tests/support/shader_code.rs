use unityrs_testkit::{
    ngui::{Object, V, s, st, vector},
    scene3d::{self, Layout},
};

pub fn program(kind: i32, codes: &[&[u8]]) -> Vec<u8> {
    let mut blob = vec![0; 4 + 12 * codes.len()];
    blob[..4].copy_from_slice(&i32::try_from(codes.len()).unwrap().to_le_bytes());
    for (i, code) in codes.iter().enumerate() {
        let offset = i32::try_from(blob.len()).unwrap();
        blob[4 + 12 * i..8 + 12 * i].copy_from_slice(&offset.to_le_bytes());
        for n in [202_012_090i32, kind, 0, 0, 0, 0, 0] {
            blob.extend(n.to_le_bytes());
        }
        blob.extend(i32::try_from(code.len()).unwrap().to_le_bytes());
        blob.extend(*code);
        while !blob.len().is_multiple_of(4) {
            blob.push(0);
        }
    }
    blob
}

pub fn asset(kind: i32, blob: &[u8], count: i32) -> Vec<u8> {
    let platform = match kind {
        1 => 0,
        2..=4 => 9,
        5 => 5,
        6..=8 => 15,
        23 | 24 => 14,
        26..=30 => 2,
        _ => panic!("unsupported test kind"),
    };
    let metadata_kind = if kind == 24 { 23 } else { kind };
    let sub = |index| {
        st(
            "SerializedSubProgram",
            vec![
                ("m_BlobIndex", V::Int(index)),
                ("m_GpuProgramType", V::Int(metadata_kind)),
                ("m_ShaderHardwareTier", V::Int(index)),
            ],
        )
    };
    let tags = || st("SerializedTagMap", vec![("tags", vector(s(""), vec![]))]);
    let mut pass = vec![("m_Type", V::Int(0)), ("m_State", state())];
    for stage in [
        "progVertex",
        "progFragment",
        "progGeometry",
        "progHull",
        "progDomain",
    ] {
        pass.push((
            stage,
            st(
                "SerializedProgram",
                vec![(
                    "m_SubPrograms",
                    vector(
                        sub(0),
                        if stage == "progVertex" {
                            (0..count).map(sub).collect()
                        } else {
                            vec![]
                        },
                    ),
                )],
            ),
        ));
    }
    let pass = st("SerializedPass", pass);
    let subshader = st(
        "SerializedSubShader",
        vec![
            ("m_LOD", V::Int(0)),
            ("m_Tags", tags()),
            ("m_Passes", vector(pass.clone(), vec![pass])),
        ],
    );
    let packed = unityrs_testkit::bundle::compress(unityrs_testkit::bundle::TestCodec::Lz4, blob);
    scene3d::file(
        Layout::Unity2022,
        &[],
        &[Object::new(
            1,
            48,
            st(
                "Shader",
                vec![
                    ("compressedBlob", V::Bytes(packed.clone())),
                    ("platforms", vector(V::Int(0), vec![V::Int(platform)])),
                    ("offsets", vector(V::Int(0), vec![V::Int(0)])),
                    (
                        "compressedLengths",
                        vector(
                            V::Int(0),
                            vec![V::Int(i32::try_from(packed.len()).unwrap())],
                        ),
                    ),
                    (
                        "decompressedLengths",
                        vector(V::Int(0), vec![V::Int(i32::try_from(blob.len()).unwrap())]),
                    ),
                    ("m_ParsedForm", parsed_form(subshader)),
                ],
            ),
        )],
    )
    .unwrap()
}

fn parsed_form(subshader: V) -> V {
    st(
        "SerializedShader",
        vec![
            ("m_Name", s("Synthetic/Code")),
            (
                "m_PropInfo",
                st(
                    "SerializedProperties",
                    vec![("m_Props", vector(s(""), vec![]))],
                ),
            ),
            ("m_SubShaders", vector(subshader.clone(), vec![subshader])),
            ("m_FallbackName", s("")),
            ("m_CustomEditorName", s("")),
        ],
    )
}

fn state() -> V {
    let mut state = vec![
        ("m_Name", s("")),
        ("m_LOD", V::Int(0)),
        (
            "m_Tags",
            st("SerializedTagMap", vec![("tags", vector(s(""), vec![]))]),
        ),
        ("lighting", V::Bool(false)),
        ("gpuProgramID", V::Int(7)),
    ];
    for (key, value) in [
        ("alphaToMask", 0.),
        ("zClip", 1.),
        ("zTest", 4.),
        ("zWrite", 1.),
        ("culling", 2.),
        ("offsetFactor", 0.),
        ("offsetUnits", 0.),
    ] {
        state.push((
            key,
            st("SerializedShaderFloatValue", vec![("val", V::Float(value))]),
        ));
    }
    st("SerializedShaderState", state)
}
