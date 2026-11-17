use super::support::{fixture, json, run};
use serde_json::Value;
use std::collections::BTreeSet;
pub fn docs_examples(examples: &[(&str, &Value)]) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/AGENTS.md");
    let mut text = std::fs::read_to_string(&path).unwrap();
    for (name, value) in examples {
        let start = format!("<!-- example:{name} -->\n");
        let end = format!("<!-- /example:{name} -->");
        let a = text.find(&start).unwrap() + start.len();
        let b = a + text[a..].find(&end).unwrap();
        let want = format!(
            "```json\n{}\n```\n",
            serde_json::to_string_pretty(value).unwrap()
        );
        if std::env::var_os("UNITYRS_UPDATE_AGENT_GOLDENS").is_some() {
            text.replace_range(a..b, &want);
        } else {
            assert_eq!(&text[a..b], want, "documentation example {name} drifted");
        }
    }
    if std::env::var_os("UNITYRS_UPDATE_AGENT_GOLDENS").is_some() {
        std::fs::write(path, text).unwrap();
    }
}
/// Keep parser acceptance distinct from loading a provider, profile, or input.
/// These paths deliberately do not exist; a parsed legacy invocation reaches IO.
fn legacy_args(command: &str, flag: &str) -> Vec<String> {
    let mut args: Vec<_> = match command {
        "dump" => vec![command, "arity-missing.assets", "-o", "dump.jsonl"],
        "extract" => vec![command, "arity-missing.assets", "-o", "out"],
        "obj" => vec![
            command,
            "arity-missing.assets",
            "--path-id",
            "-2",
            "-o",
            "out.obj",
        ],
        "gltf" => vec![
            command,
            "arity-missing.assets",
            "--scene",
            "arity-missing.assets",
            "-o",
            "out.glb",
        ],
        _ => vec![command],
    }
    .into_iter()
    .map(str::to_owned)
    .collect();
    if command == "dump" && flag == "--allow-index-mismatch" {
        args.push("--index".to_owned());
    }
    // glTF accepts exactly one selector; aliases for the dump output share a destination.
    if command == "gltf" && matches!(flag, "--scene" | "--path-id" | "--prefab") {
        let i = args.iter().position(|a| a == "--scene").unwrap();
        args.drain(i..i + 2);
    }
    let key = if command == "dump" && flag == "--output" {
        "-o"
    } else {
        flag
    };
    if let Some(i) = args.iter().position(|a| a == key) {
        args.drain(i..i + 2);
    }
    args
}
fn invoke(tmp: &unityrs_testkit::scratch::Scratch, args: &[String]) -> std::process::Output {
    run(tmp, &args.iter().map(String::as_str).collect::<Vec<_>>())
}
fn assert_missing_value(
    tmp: &unityrs_testkit::scratch::Scratch,
    name: &str,
    flag: &str,
    args: &[String],
) {
    // With the flag last, no later positional can masquerade as its value.
    let missing = invoke(tmp, args);
    let status = match name {
        "dump" | "inspect" | "show" | "describe" | "mcp" => 2,
        "batch" => 78,
        _ => 1,
    };
    assert_eq!(
        missing.status.code(),
        Some(status),
        "missing {name} {flag}: {missing:?}"
    );
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(
        stderr.contains("missing")
            || stderr.contains("expected one argument")
            || stderr.contains("bad_spec usage"),
        "missing {name} {flag}: {stderr}"
    );
}
fn assert_switch_value(
    tmp: &unityrs_testkit::scratch::Scratch,
    name: &str,
    flag: &str,
    args: &[String],
) {
    // Attached values are unambiguously a switch value, never another INPUT.
    let mut explicit = args.to_vec();
    *explicit.last_mut().unwrap() = format!("{flag}=true");
    let refused = invoke(tmp, &explicit);
    let status = match name {
        "dump" | "values" | "inspect" | "show" | "describe" | "mcp" => 2,
        "batch" => 78,
        _ => 1,
    };
    assert_eq!(
        refused.status.code(),
        Some(status),
        "switch value {name} {flag}: {refused:?}"
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    let reason = match name {
        "dump" => "ignored explicit argument",
        "values" => "expected one argument",
        "batch" => "bad_spec usage",
        "inspect" | "show" | "describe" => "unknown option",
        _ => "unexpected argument",
    };
    assert!(
        stderr.contains(reason),
        "switch value {name} {flag}: {stderr}"
    );
}
#[test]
fn every_advertised_option_has_its_declared_arity() {
    let tmp = fixture();
    let description = json(&tmp, &["describe"], 0);
    let cursor = json(&tmp, &["inspect", "synthetic.assets", "--limit", "1"], 0)["next_cursor"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut commands = BTreeSet::new();
    for command in description["commands"].as_array().unwrap() {
        let name = command["name"].as_str().unwrap();
        assert!(commands.insert(name));
        // MCP startup/framing option arities are exercised by the real stdio suite.
        if name == "mcp" {
            continue;
        }
        for option in command["options"].as_array().unwrap() {
            let flag = option["name"].as_str().unwrap();
            let mut args = match name {
                "inspect" => vec![name.to_owned(), "synthetic.assets".to_owned()],
                "show" => vec![
                    name.to_owned(),
                    "synthetic.assets".to_owned(),
                    "--path-id".to_owned(),
                    "-2".to_owned(),
                ],
                _ => legacy_args(name, flag),
            };
            if flag == "--help" || name == "batch" {
                args = vec![name.to_owned()];
            } else if name == "show" && flag == "--path-id" {
                args.truncate(2);
            }
            args.push(flag.to_owned());
            if option["value"].is_null() {
                assert_switch_value(&tmp, name, flag, &args);
            } else {
                assert_missing_value(&tmp, name, flag, &args);
                let value = match flag {
                    "--class" | "--asset-filter" => "TextAsset",
                    "--name" => "*",
                    "--path-id" => "-2",
                    "--scene" | "--prefab" | "--tpk" | "--profile" | "--builtin-file"
                    | "--vorbis-headers" => "arity-missing.assets",
                    "--file" => "synthetic.assets",
                    "--file-index" => "0:0",
                    "--cursor" => &cursor,
                    "--levels" => "L0",
                    "--unbounded" => "TreeDepth",
                    "--schema" if name == "describe" => "error",
                    "--check" => "arity-missing.job.json",
                    _ => option["default"].as_str().unwrap_or("1"),
                };
                args.push(value.to_owned());
            }
            let accepted = invoke(&tmp, &args);
            let stderr = String::from_utf8_lossy(&accepted.stderr);
            if flag == "--help" || (name == "batch" && flag == "--schema") || name == "describe" {
                assert!(
                    accepted.status.success(),
                    "accepted {name} {flag}: {accepted:?}"
                );
            } else if name == "batch" {
                assert_eq!(accepted.status.code(), Some(78));
                assert!(stderr.contains("bad_spec unreadable"), "{stderr}");
            } else if matches!(name, "inspect" | "show") {
                if flag == "--tpk" {
                    assert_eq!(accepted.status.code(), Some(1), "{name} {flag}");
                    let error: Value = serde_json::from_slice(&accepted.stdout).unwrap();
                    assert_ne!(error["code"], "usage", "accepted {name} {flag}: {error}");
                } else {
                    assert!(
                        accepted.status.success(),
                        "accepted {name} {flag}: {accepted:?}"
                    );
                }
            } else {
                assert!(
                    !accepted.status.success(),
                    "missing input should reach IO: {args:?}"
                );
                assert!(
                    stderr.contains("os error 2")
                        || stderr.contains("no such input: arity-missing.assets"),
                    "accepted {name} {flag}: {stderr}"
                );
            }
        }
    }
    assert_eq!(
        commands,
        BTreeSet::from([
            "dump", "values", "gltf", "obj", "extract", "batch", "inspect", "show", "describe",
            "mcp"
        ])
    );
}
#[test]
fn advertised_defaults_match_observable_defaults() {
    let tmp = fixture();
    let description = json(&tmp, &["describe"], 0);
    for name in ["inspect", "show"] {
        let record = description["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap();
        let mut args = vec![name, "synthetic.assets"];
        if name == "show" {
            args.extend(["--path-id", "-2"]);
        }
        let default = json(&tmp, &args, 0);
        for option in record["options"].as_array().unwrap() {
            if let Some(value) = option["default"].as_str() {
                args.extend([option["name"].as_str().unwrap(), value]);
            }
        }
        assert_eq!(default, json(&tmp, &args, 0), "{name} default drift");
    }
    // Real legacy parser defaults: explicit forms must produce identical files
    // and stdout to omissions. The corpus/parity parsers themselves stay untouched.
    let command = description["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "dump")
        .unwrap();
    let base = run(
        &tmp,
        &["dump", "synthetic.assets", "-o", "base.jsonl", "-q"],
    );
    assert!(base.status.success());
    let mut args = vec!["dump", "synthetic.assets", "-o", "explicit.jsonl", "-q"];
    for flag in ["--kind", "--levels", "--jobs"] {
        let o = command["options"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["name"] == flag)
            .unwrap();
        args.extend([flag, o["default"].as_str().unwrap()]);
    }
    assert!(run(&tmp, &args).status.success());
    assert_eq!(
        std::fs::read(tmp.join("base.jsonl")).unwrap(),
        std::fs::read(tmp.join("explicit.jsonl")).unwrap()
    );
    for name in ["gltf", "extract"] {
        let c = description["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap();
        let base = if name == "gltf" {
            vec![
                name,
                "synthetic.assets",
                "--scene",
                "synthetic.assets",
                "-o",
                "test.glb",
            ]
        } else {
            vec![name, "synthetic.assets", "-o", "out"]
        };
        let before = run(&tmp, &base);
        assert!(before.status.success(), "{name}: {before:?}");
        let mut args = base;
        for o in c["options"].as_array().unwrap() {
            if !o["value"].is_null()
                && let Some(v) = o["default"].as_str()
            {
                args.extend([o["name"].as_str().unwrap(), v]);
            }
        }
        let after = run(&tmp, &args);
        assert_eq!(before.stdout, after.stdout);
        assert_eq!(before.status.code(), after.status.code());
    }
    // Batch and values have no option defaults, and OBJ's optional TPK has none.
    for name in ["batch", "values", "obj", "describe"] {
        let c = description["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap();
        assert!(
            c["options"]
                .as_array()
                .unwrap()
                .iter()
                .all(|o| o["default"].is_null())
        );
    }
}
#[test]
fn schemas_allow_additions_and_reject_broken_contracts() {
    let tmp = fixture();
    for name in ["inspect", "show", "describe", "error"] {
        let schema: Value =
            serde_json::from_slice(&run(&tmp, &["describe", "--schema", name]).stdout).unwrap();
        assert_eq!(
            schema["$schema"],
            "https://json-schema.org/draft/2020-12/schema"
        );
        assert!(!super::support::valid(
            &serde_json::json!({"format":format!("unityrs.{name}"),"version":1}),
            &schema,
            &schema
        ));
    }
}
