#![allow(dead_code)]
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};
use unityrs_testkit::{
    bundle::{self, BundleSpec, EntrySpec},
    child,
    ngui::{self, Object, V, s, st, vector},
    scratch::Scratch,
};
pub const BIN: &str = env!("CARGO_BIN_EXE_unityrs");
pub fn fixture() -> Scratch {
    let tmp = Scratch::new("agent");
    let objects = [
        Object::new(
            30,
            1,
            st(
                "GameObject",
                vec![("m_Name", s("gamma")), ("active", V::Bool(true))],
            ),
        ),
        Object::new(
            -2,
            49,
            st(
                "TextAsset",
                vec![("m_Name", s("alpha")), ("m_Script", s("hello"))],
            ),
        ),
        Object::new(
            10,
            49,
            st(
                "TextAsset",
                vec![("m_Name", s("beta")), ("m_Script", s("world"))],
            ),
        ),
    ];
    let file = ngui::file(&[], &objects, true).unwrap();
    std::fs::write(tmp.join("synthetic.assets"), &file).unwrap();
    let rich = Object::new(
        7,
        114,
        st(
            "MonoBehaviour",
            vec![
                ("m_Name", s("rich")),
                (
                    "list",
                    vector(V::Int(0), vec![V::Int(1), V::Int(2), V::Int(3)]),
                ),
                (
                    "nested",
                    st(
                        "Nested",
                        vec![("child", st("Leaf", vec![("value", V::Int(42))]))],
                    ),
                ),
                ("bytes", V::Bytes((0..100).collect())),
                ("u8s", vector(V::UInt8(0), (0..70).map(V::UInt8).collect())),
                ("pair", ngui::pair(V::Int(1), s("two"))),
                ("float", V::Float(1.5)),
                ("bad_string", V::Str(vec![0xff, b'a'])),
                ("long", s(&"x".repeat(2000))),
            ],
        ),
    );
    std::fs::write(
        tmp.join("rich.assets"),
        ngui::file(&[], &[rich], true).unwrap(),
    )
    .unwrap();
    let entries = vec![
        EntrySpec {
            name: b"CAB-z".to_vec(),
            data: file.clone(),
            flags: 4,
        },
        EntrySpec {
            name: b"CAB-a".to_vec(),
            data: file,
            flags: 4,
        },
        EntrySpec {
            name: b"data.resS".to_vec(),
            data: vec![1, 2, 3],
            flags: 0,
        },
    ];
    std::fs::write(
        tmp.join("synthetic.bundle"),
        bundle::build(&BundleSpec {
            entries,
            ..BundleSpec::default()
        }),
    )
    .unwrap();
    std::fs::write(
        tmp.join("broken.bundle"),
        [b"UnityFS\0".as_slice(), &[0; 32]].concat(),
    )
    .unwrap();
    tmp
}
pub fn run(tmp: &Scratch, args: &[&str]) -> Output {
    child::output(Command::new(BIN).current_dir(tmp.path()).args(args)).unwrap()
}
pub fn json(tmp: &Scratch, args: &[&str], status: i32) -> Value {
    let output = run(tmp, args);
    assert_eq!(
        output.status.code(),
        Some(status),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::str::from_utf8(&output.stdout).unwrap();
    assert!(text.ends_with('\n'));
    assert_eq!(text.lines().count(), 1, "exactly one compact document");
    let value: Value = serde_json::from_str(text).unwrap();
    if status != 0 {
        assert_eq!(String::from_utf8_lossy(&output.stderr).lines().count(), 1);
    }
    let name = value["format"]
        .as_str()
        .unwrap()
        .strip_prefix("unityrs.")
        .unwrap();
    let schema_output = run(tmp, &["describe", "--schema", name]);
    assert!(schema_output.status.success());
    let schema: Value = serde_json::from_slice(&schema_output.stdout).unwrap();
    assert!(valid(&value, &schema, &schema), "{value}: {schema}");
    value
}
/// Small validator for every validation keyword emitted by our schemas.
/// Unknown annotation keywords have no validation effect in JSON Schema.
#[allow(
    clippy::collapsible_if,
    reason = "keep schema keyword rules independent and readable"
)]
pub fn valid(v: &Value, s: &Value, root: &Value) -> bool {
    if let Some(r) = s["$ref"].as_str() {
        return valid(v, root.pointer(r.strip_prefix('#').unwrap()).unwrap(), root);
    }
    if let Some(variants) = s["anyOf"].as_array() {
        if !variants.iter().any(|s| valid(v, s, root)) {
            return false;
        }
    }
    if let Some(c) = s.get("const") {
        if v != c {
            return false;
        }
    }
    if let Some(e) = s["enum"].as_array() {
        if !e.contains(v) {
            return false;
        }
    }
    if let Some(t) = s["type"].as_str() {
        if !match t {
            "object" => v.is_object(),
            "array" => v.is_array(),
            "string" => v.is_string(),
            "integer" => v.is_i64() || v.is_u64(),
            "boolean" => v.is_boolean(),
            "null" => v.is_null(),
            _ => panic!("unknown schema type {t}"),
        } {
            return false;
        }
    }
    if let Some(keys) = s["required"].as_array() {
        if keys.iter().any(|k| v.get(k.as_str().unwrap()).is_none()) {
            return false;
        }
    }
    if let Some(props) = s["properties"].as_object() {
        for (key, schema) in props {
            if let Some(value) = v.get(key) {
                if !valid(value, schema, root) {
                    return false;
                }
            }
        }
    }
    if let Some(items) = s.get("items") {
        if let Some(values) = v.as_array() {
            if !values.iter().all(|v| valid(v, items, root)) {
                return false;
            }
        }
    }
    true
}
pub fn golden_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/agent")
        .join(format!("{name}.json"))
}
pub fn golden(name: &str, value: &Value) {
    let text = format!("{}\n", serde_json::to_string_pretty(value).unwrap());
    let path = golden_path(name);
    if std::env::var_os("UNITYRS_UPDATE_AGENT_GOLDENS").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        text,
        "regenerate with UNITYRS_UPDATE_AGENT_GOLDENS=1 cargo test -p unityrs-cli --test agent goldens"
    );
}
pub fn cuts(v: &Value, counts: &mut [u64; 4]) {
    if v["$unityrs"] == "cut" {
        let i = match v["reason"].as_str().unwrap() {
            "bytes" => 0,
            "items" => 1,
            "depth" => 2,
            _ => panic!(),
        };
        counts[i] += 1;
    } else if v["$unityrs"] == "bytes" {
        counts[3] += 1;
    }
    match v {
        Value::Array(a) => {
            for v in a {
                cuts(v, counts);
            }
        }
        Value::Object(m) => {
            for v in m.values() {
                cuts(v, counts);
            }
        }
        _ => {}
    }
}
pub fn assert_cuts(v: &Value) {
    let mut counts = [0; 4];
    cuts(&v["value"], &mut counts);
    for (i, key) in ["bytes", "items", "depth", "binary"].iter().enumerate() {
        assert_eq!(v["truncated"][key], counts[i], "{v}");
    }
    assert_eq!(v["truncated"]["cuts"], counts.iter().sum::<u64>());
}
