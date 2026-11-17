//! Recorded from A1a head 4a15f9f9, with default limits and synthetic providers.
//! `UNITYRS_A1A_BIN` optionally repeats the comparison against a separate build.
use crate::support::BIN;
use serde_json::{Value, json};
use std::{collections::BTreeMap, ffi::OsStr, path::Path, process::Command};
use unityrs_testkit::{
    child,
    ngui::{Object, s, st},
    scene3d::{
        self, Layout,
        mesh::{MeshVersion, mesh},
    },
    scratch::Scratch,
};

fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
        .collect()
}
fn tree(root: &Path, at: &Path, out: &mut BTreeMap<String, Value>) {
    for entry in std::fs::read_dir(at).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            tree(root, &path, out);
        } else {
            let bytes = std::fs::read(&path).unwrap();
            out.insert(
                path.strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned(),
                json!({"size":bytes.len(),"sha256":unityrs_dump::json::sha256_hex(&bytes)}),
            );
        }
    }
}
#[allow(
    clippy::too_many_lines,
    reason = "one default-limit snapshot across the five legacy command contracts"
)]
fn snapshot(binary: &OsStr) -> Value {
    let tmp = Scratch::new("legacy-tpk-parity");
    let objects = vec![
        Object::new(1, 1, scene3d::game_object(b"synthetic", &[(0, 2)])),
        Object::new(2, 4, scene3d::transform((0, 1), (0, 0), &[], false)),
        mesh(
            3,
            b"triangle",
            &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
            &[0, 1, 2],
            MeshVersion::V2022,
        ),
        Object::new(
            4,
            49,
            st(
                "TextAsset",
                vec![("m_Name", s("leaf")), ("m_Script", s("payload"))],
            ),
        ),
    ];
    std::fs::write(
        tmp.join("input.assets"),
        scene3d::file(Layout::Unity2022, &[], &objects).unwrap(),
    )
    .unwrap();
    // values has no TPK/file-input option; its unchanged case protocol is
    // compared separately, beside all four commands that accept --tpk.
    std::fs::write(tmp.join("values.json"), json!([{"name":"synthetic","nodes":[[0,"Base","Base",0],[1,"int","x",0]],"hex":"07000000"}]).to_string()).unwrap();
    let providers: Value = serde_json::from_str(include_str!(
        "../../../unityrs-tpk/tests/fixtures/tpk/synthetic.json"
    ))
    .unwrap();
    let mut records = BTreeMap::new();
    for codec in ["root_flags", "lz4", "lzma"] {
        let provider = providers["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == codec)
            .unwrap();
        std::fs::write(
            tmp.join("types.tpk"),
            unhex(provider["tpk"].as_str().unwrap()),
        )
        .unwrap();
        for args in [
            vec![
                "dump",
                "input.assets",
                "-o",
                "result/dump.jsonl",
                "--tpk",
                "types.tpk",
                "-q",
            ],
            vec!["values", "values.json"],
            vec![
                "gltf",
                "input.assets",
                "--path-id",
                "1",
                "-o",
                "result/scene.glb",
                "--tpk",
                "types.tpk",
            ],
            vec![
                "obj",
                "input.assets",
                "--path-id",
                "3",
                "-o",
                "result/mesh.obj",
                "--tpk",
                "types.tpk",
            ],
            vec![
                "extract",
                "input.assets",
                "-o",
                "result",
                "--no-container",
                "--asset-filter",
                "TextAsset",
                "--tpk",
                "types.tpk",
            ],
        ] {
            std::fs::create_dir(tmp.join("result")).unwrap();
            let output =
                child::output(Command::new(binary).current_dir(tmp.path()).args(&args)).unwrap();
            assert!(
                output.status.success(),
                "{codec} {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let mut files = BTreeMap::new();
            tree(&tmp.join("result"), &tmp.join("result"), &mut files);
            let stderr = if args[0] == "dump" {
                // Elapsed wall time is the only nondeterministic field.
                let mut stats: Value = serde_json::from_slice(&output.stderr).unwrap();
                stats.as_object_mut().unwrap().remove("wall_s");
                stats.to_string()
            } else {
                String::from_utf8(output.stderr).unwrap()
            };
            records.insert(format!("{codec}/{}", args[0]), json!({"stdout":String::from_utf8(output.stdout).unwrap(),"stderr":stderr,"files":files}));
            std::fs::remove_dir_all(tmp.join("result")).unwrap();
        }
    }
    json!(records)
}

#[test]
fn default_tpk_limits_preserve_all_legacy_command_outputs() {
    let current = snapshot(OsStr::new(BIN));
    if let Some(binary) = std::env::var_os("UNITYRS_A1A_BIN") {
        let baseline = snapshot(&binary);
        println!("A1A_TPK_GOLDEN={baseline}");
        assert_eq!(current, baseline);
    }
    // The optional same-platform A1a comparison above includes the exact
    // glTF peak diagnostic. Its charged path bookkeeping differs by platform;
    // portable goldens retain the message while eliding only that number.
    let mut current = current;
    for (key, record) in current.as_object_mut().unwrap() {
        if key.ends_with("/gltf") {
            let (message, peak) = record["stderr"]
                .as_str()
                .unwrap()
                .split_once("; peak charged bytes ")
                .unwrap();
            assert!(peak.trim().parse::<u64>().unwrap() > 0);
            record["stderr"] = json!(format!("{message}; peak charged bytes <platform>\n"));
        }
    }
    let golden: Value =
        serde_json::from_str(include_str!("../fixtures/agent/tpk-default-parity.json")).unwrap();
    assert_eq!(current, golden);
}
