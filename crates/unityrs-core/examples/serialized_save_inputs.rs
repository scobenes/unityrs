//! Synthetic B1 inputs from the existing testkit builders, shared by the oracle.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use serde_json::{Value, json};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::serialized::{ExternalSpec, ObjectSpec, SerializedSpec, TypeSpec, build};

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, b| {
        write!(out, "{b:02x}").unwrap();
        out
    })
}
fn tree() -> Vec<NodeSpec> {
    vec![
        NodeSpec::new(0, "TextAsset", "Base"),
        NodeSpec::new(1, "int", "value"),
    ]
}
fn input(format: u32, big_endian: bool, trees: bool, big_id: bool) -> Vec<u8> {
    let script_class = if format < 16 { -7 } else { 114 };
    let mut spec = SerializedSpec {
        format,
        big_endian,
        type_trees: trees,
        types: vec![
            TypeSpec {
                class_id: 49,
                nodes: tree(),
                dependencies: vec![-1, 0, 7],
                ..TypeSpec::default()
            },
            TypeSpec {
                class_id: script_class,
                stripped: true,
                script_type_index: 2,
                script_id: [0x35; 16],
                old_type_hash: [0xab; 16],
                nodes: tree(),
                dependencies: vec![2],
                ..TypeSpec::default()
            },
        ],
        scripts: vec![
            (2, -37),
            (-1, if format >= 14 { 0x1_0000_0002 } else { 19 }),
        ],
        externals: vec![
            ExternalSpec {
                temp_empty: b"temporary-\xff".to_vec(),
                guid: [0x67; 16],
                kind: 2,
                path: b"library/file.assets".to_vec(),
            },
            ExternalSpec {
                path: b"archive:/CAB-other/entry".to_vec(),
                ..ExternalSpec::default()
            },
        ],
        ref_types: vec![TypeSpec {
            class_id: 49,
            script_type_index: 0,
            script_id: [0x42; 16],
            old_type_hash: [0x29; 16],
            nodes: tree(),
            ref_names: Some((b"Record".to_vec(), b"Space".to_vec(), b"Library".to_vec())),
            ..TypeSpec::default()
        }],
        big_id_enabled: i32::from(big_id),
        user_information: b"synthetic-\xce\xbb-\xff".to_vec(),
        ..SerializedSpec::default()
    };
    // Duplicate id: save keeps the last row, sorts by signed path id, and
    // uses the type's final script index rather than each object's old one.
    for (path, value, script) in [(9, 11i32, 1), (-3, -17, 2), (9, 29, 3), (4, 71, 5)] {
        let data = if big_endian {
            value.to_be_bytes()
        } else {
            value.to_le_bytes()
        };
        spec.objects.push(ObjectSpec {
            path_id: path,
            type_id: if format >= 16 { 0 } else { 49 },
            class_id: 49,
            data: data.to_vec(),
            script_type_index: Some(script),
            is_destroyed: Some(3),
            is_stripped: Some(-1),
            ..ObjectSpec::default()
        });
    }
    let mut bytes = build(&spec);
    if format >= 9 {
        bytes[17..20].copy_from_slice(&[3, 2, 1]);
    }
    if format >= 22 {
        bytes[40..48].copy_from_slice(&0x1122_3344_5566_7788i64.to_be_bytes());
    }
    bytes
}
fn boundary_input() -> Vec<u8> {
    let mut bytes = input(17, false, true, false);
    let key = b"temporary-\xff\0";
    let at = bytes.windows(key.len()).position(|w| w == key).unwrap();
    bytes.splice(at..at + key.len(), std::iter::repeat_n(b'a', 32_767));
    let delta = u32::try_from(32_767 - key.len()).unwrap();
    for at in [0, 4, 12] {
        let old = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
        bytes[at..at + 4].copy_from_slice(&(old + delta).to_be_bytes());
    }
    bytes
}

fn eof_boundary_input() -> Vec<u8> {
    let mut bytes = input(8, false, true, true);
    let key = b"synthetic-\xce\xbb-\xff\0";
    let at = bytes.windows(key.len()).position(|w| w == key).unwrap();
    bytes.splice(
        at..at + key.len(),
        std::iter::repeat_n(b'a', 32_767).chain(std::iter::once(0)),
    );
    let delta = u32::try_from(32_768 - key.len()).unwrap();
    for at in [0, 4] {
        let old = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
        bytes[at..at + 4].copy_from_slice(&(old + delta).to_be_bytes());
    }
    bytes
}

fn main() {
    let mut cases: Vec<Value> = vec![];
    for format in 1..=22 {
        for big in [false, true] {
            for parse_trees in [false, true] {
                for mutation in ["none", "raw", "empty", "typetree"] {
                    cases.push(
                        json!({"name":format!("v{format}-be{big}-parse{parse_trees}-{mutation}"),
                        "input":hex(&input(format,big,true,(7..14).contains(&format))),
                        "parse_trees":parse_trees,"mutation":mutation}),
                    );
                }
            }
            if format >= 13 {
                cases.push(json!({"name":format!("v{format}-be{big}-stripped"),
                    "input":hex(&input(format,big,false,false)),"parse_trees":true,"mutation":"raw"}));
            }
        }
    }
    cases.push(
        json!({"name":"v17-sequential-string-window","input":hex(&boundary_input()),
        "parse_trees":true,"mutation":"none","layout_reloads":false}),
    );
    cases.push(
        json!({"name":"v8-final-string-window","input":hex(&eof_boundary_input()),
        "parse_trees":true,"mutation":"none"}),
    );
    println!("{}", serde_json::to_string(&cases).unwrap());
}
