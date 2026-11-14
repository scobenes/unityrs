use unityrs_testkit::ngui::{Object, V, s, st, vector};
use unityrs_testkit::scene3d::{Layout, file};

pub struct Case {
    pub name: &'static str,
    pub class_id: i32,
    pub extension: &'static str,
    pub input: Vec<u8>,
}
pub fn build(class_id: i32, value: V) -> Vec<u8> {
    file(Layout::Unity2022, &[], &[Object::new(1, class_id, value)]).unwrap()
}
#[allow(clippy::too_many_lines, reason = "synthetic fixture table")]
pub fn cases() -> Vec<Case> {
    let mut cases = Vec::new();
    for (name, script, extension) in [
        ("text_unicode", "héllo 😀\n".as_bytes(), ""),
        ("text_surrogateescape", b"a\xff\xe4\xb8z\0", ".bytes"),
        ("text_empty", b"", ".json"),
        ("text_multiple_dots", b"multi", ".a.b"),
    ] {
        cases.push(Case {
            name,
            class_id: 49,
            extension,
            input: build(
                49,
                st(
                    "TextAsset",
                    vec![("m_Name", s("text")), ("m_Script", V::Str(script.to_vec()))],
                ),
            ),
        });
    }
    for (name, bytes) in [
        ("font_otto", &b"OTTO\0\xff"[..]),
        ("font_ttf", &b"\0\x01\0\0"[..]),
        ("font_empty", &b""[..]),
    ] {
        cases.push(Case {
            name,
            class_id: 128,
            extension: ".otf",
            input: build(
                128,
                st(
                    "Font",
                    vec![
                        ("m_Name", s("font")),
                        (
                            "m_FontData",
                            vector(V::UInt8(0), bytes.iter().map(|b| V::UInt8(*b)).collect()),
                        ),
                    ],
                ),
            ),
        });
    }
    for (name, bytes) in [
        ("font_bytes_otto", &b"OTTO\0\xff"[..]),
        ("font_bytes_ttf", &b"\0\x01\0\0"[..]),
    ] {
        cases.push(Case {
            name,
            class_id: 128,
            extension: "",
            input: build(
                128,
                st("Font", vec![("m_FontData", V::Bytes(bytes.to_vec()))]),
            ),
        });
    }
    for (name, cut, trailing) in [("mono_truncated", Some(2), 0), ("mono_trailing", None, 1)] {
        let mut object = Object::new(1, 114, st("MonoBehaviour", vec![("n", V::Int(7))]));
        object.cut = cut;
        object.trailing = trailing;
        cases.push(Case {
            name,
            class_id: 114,
            extension: "",
            input: file(Layout::Unity2022, &[], &[object]).unwrap(),
        });
    }
    let tree = st(
        "MonoBehaviour",
        vec![
            ("z", s("é😀\u{7f}\n\u{1}")),
            ("bad", V::Str(b"\xff\xed\xa0\x80".to_vec())),
            (
                "clé",
                vector(
                    V::Double(0.),
                    vec![
                        V::Double(-0.),
                        V::Double(f64::NAN),
                        V::Double(f64::INFINITY),
                        V::Double(f64::NEG_INFINITY),
                        V::Double(1e-5),
                        V::Double("1551563132437320.25".parse().unwrap()),
                        V::Double(5e-324),
                    ],
                ),
            ),
            ("float", V::Float(0.1)),
            ("pair", unityrs_testkit::ngui::pair(V::Int(1), s("x"))),
            ("empty", vector(V::Int(0), vec![])),
        ],
    );
    cases.push(Case {
        name: "mono_json",
        class_id: 114,
        extension: ".txt",
        input: build(114, tree),
    });
    cases.push(Case {
        name: "mono_bytes",
        class_id: 114,
        extension: "",
        input: build(
            114,
            st("MonoBehaviour", vec![("data", V::Bytes(vec![0, 255]))]),
        ),
    });
    let spec = unityrs_testkit::serialized::SerializedSpec {
        type_trees: false,
        types: vec![unityrs_testkit::serialized::TypeSpec {
            class_id: 114,
            ..Default::default()
        }],
        objects: vec![unityrs_testkit::serialized::ObjectSpec {
            path_id: 1,
            data: vec![1, 2, 3, 255],
            ..Default::default()
        }],
        ..Default::default()
    };
    cases.push(Case {
        name: "mono_no_tree",
        class_id: 114,
        extension: "",
        input: unityrs_testkit::serialized::build(&spec),
    });
    cases.extend(mono_breadth());
    cases
}

fn mono(name: &'static str, value: V) -> Case {
    Case {
        name,
        class_id: 114,
        extension: "",
        input: build(114, value),
    }
}

#[allow(clippy::too_many_lines, reason = "synthetic fixture table")]
fn mono_breadth() -> Vec<Case> {
    use unityrs_testkit::ngui::{pair, pptr};
    let empty = || st("Empty", vec![]);
    let floats = [
        1e16,
        1e-7,
        -0.,
        f64::from_bits(1),
        f64::MIN_POSITIVE,
        f64::from_bits(0x000f_ffff_ffff_ffff),
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    let widened = [
        0.1_f32,
        1e16_f32,
        1e-7_f32,
        -0.,
        f32::from_bits(1),
        f32::from_bits(0x007f_ffff),
        f32::MIN_POSITIVE,
        f32::MAX,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ];
    let escapes = "\"\\\u{8}\u{c}\n\r\t\u{0}\u{1}\u{1f}\u{7f}\u{2028}\u{2029}😀𐀀";
    // Typetree names are NUL-terminated; keep NUL in values only.
    let key = escapes.replace('\0', "");
    let strings = st(
        "MonoBehaviour",
        vec![
            ("escapes", s(escapes)),
            (&key, s("key")),
            ("invalid", V::Str(b"\xff\xed\xa0\x80\xe4\xb8".to_vec())),
        ],
    );
    vec![
        integer_case(),
        // Scalar bools align individually; a primitive bool run is packed
        // and aligned once, unlike ngui::V's per-value bool writer.
        Case {
            name: "mono_bools",
            class_id: 114,
            extension: "",
            input: raw_mono(
                vec![
                    unityrs_testkit::blob::NodeSpec::new(0, "MonoBehaviour", "Base"),
                    unityrs_testkit::blob::NodeSpec::new(1, "bool", "false").aligned(),
                    unityrs_testkit::blob::NodeSpec::new(1, "bool", "true").aligned(),
                    unityrs_testkit::blob::NodeSpec::new(1, "vector", "array"),
                    unityrs_testkit::blob::NodeSpec::new(2, "Array", "Array").aligned(),
                    unityrs_testkit::blob::NodeSpec::new(3, "int", "size"),
                    unityrs_testkit::blob::NodeSpec::new(3, "bool", "data"),
                ],
                vec![0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 0, 1, 0, 0],
            ),
        },
        mono(
            "mono_floats",
            st(
                "MonoBehaviour",
                vec![
                    (
                        "double",
                        vector(V::Double(0.), floats.into_iter().map(V::Double).collect()),
                    ),
                    (
                        "float32",
                        vector(V::Float(0.), widened.into_iter().map(V::Float).collect()),
                    ),
                    ("double_scalar", V::Double(1e16)),
                    ("float_scalar", V::Float(f32::from_bits(1))),
                ],
            ),
        ),
        // Invalid UTF-8 values are safe; invalid field names abort UnityPyBoost
        // and are pinned only by simple.rs's native export test.
        mono("mono_strings", strings),
        mono(
            "mono_containers",
            st(
                "MonoBehaviour",
                vec![
                    ("dict", empty()),
                    ("list", vector(V::Int(0), vec![])),
                    (
                        "nested",
                        st(
                            "Nested",
                            vec![("dict", empty()), ("list", vector(empty(), vec![empty()]))],
                        ),
                    ),
                    (
                        "map",
                        V::Vector(
                            "map".into(),
                            Box::new(pair(s(""), V::Int(0))),
                            vec![pair(s("one"), V::Int(1)), pair(s("two"), V::Int(-2))],
                        ),
                    ),
                    (
                        "empty_map",
                        V::Vector("map".into(), Box::new(pair(s(""), V::Int(0))), vec![]),
                    ),
                    ("pair", pair(empty(), vector(V::UInt8(0), vec![]))),
                    ("pointer", pptr("Object", (2, i64::MAX))),
                    ("null_pointer", pptr("Object", (0, 0))),
                ],
            ),
        ),
        mono(
            "mono_byte_array",
            st(
                "MonoBehaviour",
                vec![(
                    "array",
                    vector(V::UInt8(0), vec![V::UInt8(0), V::UInt8(127), V::UInt8(255)]),
                )],
            ),
        ),
        mono(
            "mono_empty_bytes",
            st("MonoBehaviour", vec![("data", V::Bytes(vec![]))]),
        ),
        mono("mono_empty_dict", empty()),
    ]
}

fn raw_mono(nodes: Vec<unityrs_testkit::blob::NodeSpec>, data: Vec<u8>) -> Vec<u8> {
    use unityrs_testkit::serialized::{ObjectSpec, SerializedSpec, TypeSpec};
    unityrs_testkit::serialized::build(&SerializedSpec {
        types: vec![TypeSpec {
            class_id: 114,
            nodes,
            ..Default::default()
        }],
        objects: vec![ObjectSpec {
            path_id: 1,
            data,
            ..Default::default()
        }],
        ..Default::default()
    })
}

fn integer_case() -> Case {
    use unityrs_testkit::blob::NodeSpec;
    let mut nodes = vec![NodeSpec::new(0, "MonoBehaviour", "Base")];
    let mut data = Vec::new();
    macro_rules! extremes {
        ($ty:ty, $unity:literal) => {{
            for (suffix, value) in [("min", <$ty>::MIN), ("max", <$ty>::MAX)] {
                nodes.push(NodeSpec::new(1, $unity, &format!("{}_{}", $unity, suffix)));
                data.extend_from_slice(&value.to_le_bytes());
            }
            nodes.push(NodeSpec::new(1, "vector", concat!($unity, "_array")));
            nodes.push(NodeSpec::new(2, "Array", "Array"));
            nodes.push(NodeSpec::new(3, "int", "size"));
            nodes.push(NodeSpec::new(3, $unity, "data"));
            data.extend_from_slice(&2_i32.to_le_bytes());
            data.extend_from_slice(&<$ty>::MIN.to_le_bytes());
            data.extend_from_slice(&<$ty>::MAX.to_le_bytes());
        }};
    }
    extremes!(i8, "SInt8");
    extremes!(u8, "UInt8");
    extremes!(i16, "SInt16");
    extremes!(u16, "UInt16");
    extremes!(i32, "SInt32");
    extremes!(u32, "UInt32");
    extremes!(i64, "SInt64");
    extremes!(u64, "UInt64");
    Case {
        name: "mono_integer_extremes",
        class_id: 114,
        extension: "",
        input: raw_mono(nodes, data),
    }
}
