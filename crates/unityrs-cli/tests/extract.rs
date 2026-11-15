//! The binary replays L9 digests and exercises the output root's safety boundary.
#![allow(clippy::format_collect, reason = "fixture digest spelling")]
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, os::unix::ffi::OsStrExt, path::Path, process::Command};
#[path = "extract/report.rs"]
mod expected_report;
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
use unityrs_testkit::{
    child,
    ngui::{Object, V, s, st},
    scene3d::{self, Layout},
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
fn tree(root: &Path) -> BTreeMap<String, Value> {
    fn visit(base: &Path, at: &Path, result: &mut BTreeMap<String, Value>) {
        for entry in std::fs::read_dir(at).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(base, &path, result);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                result.insert(hex(path.strip_prefix(base).unwrap().as_os_str().as_bytes()),json!({"size":bytes.len(),"sha256":Sha256::digest(&bytes).iter().map(|b|format!("{b:02x}")).collect::<String>()}));
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
#[test]
fn extract_cli_replays_the_l9_oracle() {
    let recorded: Value = serde_json::from_str(include_str!(
        "../../unityrs-export/tests/fixtures/extractor.json"
    ))
    .unwrap();
    for case in recorded["cases"].as_array().unwrap() {
        let tmp = Scratch::new("extract-cli");
        let input = tmp.join("synthetic.assets");
        let output = tmp.join("dst");
        std::fs::write(&input, unhex(case["input"].as_str().unwrap())).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_unityrs"));
        command.arg("extract").arg(&input).arg("-o").arg(&output);
        let o = &case["options"];
        if o["use_container"] == false {
            command.arg("--no-container");
        }
        if o["append_path_id"] == true {
            command.arg("--append-path-id");
        }
        if o["export_unknown_as_typetree"] == true {
            command.arg("--export-unknown-as-typetree");
        }
        if let Some(n) = o["ignore_first_container_dirs"].as_i64() {
            command.args(["--ignore-first-container-dirs", &n.to_string()]);
        }
        if let Some(ids) = o["asset_filter"].as_array() {
            for id in ids {
                command.args(["--asset-filter", &id.to_string()]);
            }
        }
        let result = child::output(&mut command).unwrap();
        assert!(
            result.status.success(),
            "{}: {}",
            case["case"],
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            String::from_utf8(result.stdout.clone()).unwrap(),
            String::from_utf8(expected_report::json(&case["expected"]["report"], &input)).unwrap(),
            "{}: complete ordered report",
            case["case"]
        );
        assert_eq!(
            json!(tree(&output)),
            case["expected"]["tree"],
            "{}",
            case["case"]
        );
    }
}
fn input(tmp: &Scratch, name: &[u8]) -> std::path::PathBuf {
    let object = Object::new(
        1,
        49,
        st(
            "TextAsset",
            vec![("m_Name", V::Str(name.to_vec())), ("m_Script", s("new"))],
        ),
    );
    let input = tmp.join("input.assets");
    std::fs::write(
        &input,
        scene3d::file(Layout::Unity2022, &[], &[object]).unwrap(),
    )
    .unwrap();
    input
}
fn run(input: &Path, output: &Path, extra: &[&str]) -> std::process::Output {
    child::output(
        Command::new(env!("CARGO_BIN_EXE_unityrs"))
            .arg("extract")
            .arg(input)
            .arg("-o")
            .arg(output)
            .arg("--no-container")
            .args(extra),
    )
    .unwrap()
}
#[test]
fn unsafe_names_and_symlink_parents_write_nothing_outside_root() {
    use std::os::unix::fs::symlink;
    for name in [
        b"../escape".as_slice(),
        b"/escape",
        b"a/../../escape",
        b"nul\0name",
        b"C:/escape",
        b"a\\escape",
        b"link/escape",
    ] {
        let tmp = Scratch::new("extract-path");
        let input = input(&tmp, name);
        let root = tmp.join("dst");
        let outside = tmp.join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        symlink(&outside, root.join("link")).unwrap();
        let result = run(&input, &root, &[]);
        assert!(result.status.success());
        let report: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(report["failures"].as_array().unwrap().len(), 1, "{name:?}");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        assert!(!tmp.join("escape.txt").exists());
    }
}
#[test]
fn existing_hardlinks_and_symlink_leaves_are_replaced_without_following() {
    use std::os::unix::fs::symlink;
    for link in [false, true] {
        let tmp = Scratch::new("extract-leaf");
        let input = input(&tmp, b"leaf");
        let root = tmp.join("dst");
        std::fs::create_dir(&root).unwrap();
        let outside = tmp.join("outside");
        std::fs::write(&outside, b"old").unwrap();
        if link {
            symlink(&outside, root.join("leaf.txt")).unwrap();
        } else {
            std::fs::hard_link(&outside, root.join("leaf.txt")).unwrap();
        }
        let result = run(&input, &root, &["--report", "report.json"]);
        assert!(result.status.success());
        assert_eq!(std::fs::read(&outside).unwrap(), b"old");
        assert_eq!(std::fs::read(root.join("leaf.txt")).unwrap(), b"new");
        assert_eq!(
            std::fs::read(root.join("report.json")).unwrap(),
            result.stdout
        );
    }
}
#[test]
fn help_bad_arguments_multiple_inputs_and_named_filter() {
    let help =
        child::output(Command::new(env!("CARGO_BIN_EXE_unityrs")).args(["extract", "--help"]))
            .unwrap();
    assert!(help.status.success());
    let tmp = Scratch::new("extract-args");
    let input = input(&tmp, b"leaf");
    let other = tmp.join("second.assets");
    std::fs::copy(&input, &other).unwrap();
    let result = child::output(
        Command::new(env!("CARGO_BIN_EXE_unityrs"))
            .arg("extract")
            .arg(&input)
            .arg(&other)
            .arg("-o")
            .arg(tmp.join("dst"))
            .args(["--no-container", "--asset-filter", "TextAsset"]),
    )
    .unwrap();
    assert!(result.status.success());
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["written"].as_array().unwrap().len(), 2);
    for options in [
        vec!["--wat"],
        vec!["--ignore-first-container-dirs", "invalid"],
        vec!["--asset-filter", "MissingClass"],
        vec!["--report", "../outside"],
    ] {
        let result = run(&input, &tmp.join("bad"), &options);
        assert_eq!(result.status.code(), Some(1));
    }
}

#[test]
fn container_paths_are_validated_before_dropping_their_prefix() {
    use unityrs_testkit::ngui::{pair, pptr, vector};
    for name in [
        b"a/../escape".as_slice(),
        b"/absolute",
        b"a/\0bad",
        b"C:/escape",
        b"a\\b",
        b"",
        b"valid/but/all/dropped",
    ] {
        let tmp = Scratch::new("extract-container-path");
        let info = st(
            "AssetInfo",
            vec![
                ("preloadIndex", V::Int(0)),
                ("preloadSize", V::Int(0)),
                ("asset", pptr("Object", (0, 1))),
            ],
        );
        let bundle = Object::new(
            2,
            142,
            st(
                "AssetBundle",
                vec![
                    ("m_Name", s("synthetic")),
                    ("m_PreloadTable", vector(pptr("Object", (0, 0)), vec![])),
                    (
                        "m_Container",
                        V::Vector(
                            "map".into(),
                            Box::new(pair(s(""), info.clone())),
                            vec![pair(V::Str(name.to_vec()), info)],
                        ),
                    ),
                ],
            ),
        );
        let text = Object::new(
            1,
            49,
            st(
                "TextAsset",
                vec![("m_Name", s("text")), ("m_Script", s("payload"))],
            ),
        );
        let input = tmp.join("input.assets");
        std::fs::write(
            &input,
            scene3d::file(Layout::Unity2022, &[], &[bundle, text]).unwrap(),
        )
        .unwrap();
        let output = tmp.join("dst");
        let result = child::output(
            Command::new(env!("CARGO_BIN_EXE_unityrs"))
                .arg("extract")
                .arg(&input)
                .arg("-o")
                .arg(&output)
                .args(["--ignore-first-container-dirs", "99"]),
        )
        .unwrap();
        assert!(
            result.status.success(),
            "{name:?}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(report["failures"][0]["reason"], "unsafe_path", "{name:?}");
        assert_eq!(std::fs::read_dir(output).unwrap().count(), 0);
    }
}

#[test]
fn tiny_extraction_budgets_are_fatal_and_named() {
    let tmp = Scratch::new("extract-budget");
    let input = input(&tmp, b"leaf");
    for option in [
        "--max-input-memory",
        "--max-decode-work",
        "--max-input-bytes",
    ] {
        let result = run(&input, &tmp.join("dst"), &[option, "1"]);
        assert_eq!(result.status.code(), Some(1));
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains("limit") || error.contains("budget refused"),
            "{option}: {error}"
        );
        assert!(!error.contains("unexpected argument"), "{error}");
        assert_eq!(result.stdout, [] as [u8; 0]);
    }
}

#[test]
fn retained_work_refusals_are_fatal_and_successful_runs_are_complete() {
    let tmp = Scratch::new("extract-work");
    let input = input(&tmp, b"leaf");
    let mut completed = false;
    for work in (256..8192).step_by(64) {
        let result = run(
            &input,
            &tmp.join("dst"),
            &["--max-decode-work", &work.to_string()],
        );
        if result.status.success() {
            let report: Value = serde_json::from_slice(&result.stdout).unwrap();
            assert_eq!(report["failures"], json!([]), "work {work}");
            assert_eq!(
                report["written"].as_array().unwrap().len(),
                1,
                "work {work}"
            );
            completed = true;
        } else {
            assert_eq!(result.status.code(), Some(1));
            assert_eq!(result.stdout, [] as [u8; 0]);
        }
    }
    assert!(completed);
}
