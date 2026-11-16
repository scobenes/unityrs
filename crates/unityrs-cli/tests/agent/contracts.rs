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
fn quoted_options(mut source: &str, prefix: &str) -> BTreeSet<String> {
    let mut options = BTreeSet::new();
    while let Some(start) = source.find(prefix) {
        let rest = &source[start + prefix.len()..];
        let Some(end) = rest.find('"') else {
            break;
        };
        let value = &rest[..end];
        if value.starts_with('-') && value != "--" && value != "-" {
            options.insert(value.to_owned());
        }
        source = &rest[end + 1..];
    }
    options
}
#[test]
#[allow(
    clippy::too_many_lines,
    reason = "exercise the complete parser contract together"
)]
fn every_parser_and_describe_agree_on_options() {
    let tmp = fixture();
    let description = json(&tmp, &["describe"], 0);
    // Exact source slices cover the option tables/matches, excluding prose and tests.
    let dump = include_str!("../../src/args/mod.rs")
        .split("const STRINGS:")
        .nth(1)
        .unwrap()
        .split("];\n")
        .next()
        .unwrap();
    let extract = include_str!("../../src/extract.rs")
        .split("match arg.as_slice() {")
        .nth(1)
        .unwrap()
        .split("    let output = output.ok_or")
        .next()
        .unwrap();
    let gltf = include_str!("../../src/gltf/args.rs")
        .split("match arg.as_slice() {")
        .nth(1)
        .unwrap()
        .split("        if inputs == 0")
        .next()
        .unwrap();
    let obj = include_str!("../../src/obj.rs")
        .split("match arg.as_slice() {")
        .nth(1)
        .unwrap()
        .split("    let input = Path::new")
        .next()
        .unwrap();
    for (command, source, prefix) in [
        ("dump", dump, "(\""),
        ("extract", extract, "b\""),
        ("gltf", gltf, "b\""),
        ("obj", obj, "b\""),
    ] {
        let record = description["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == command)
            .unwrap();
        let mut expected = quoted_options(source, prefix);
        expected.remove("-h");
        expected.remove("--help");
        let actual: BTreeSet<_> = record["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["name"].as_str().unwrap().to_owned())
            .filter(|s| s != "--help")
            .collect();
        assert_eq!(actual, expected, "{command} options drifted from parser");
    }
    let batch = include_str!("../../src/batch.rs")
        .split("fn command(args:")
        .nth(1)
        .unwrap()
        .split("/// `unityrs batch`")
        .next()
        .unwrap();
    let batch_record = description["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "batch")
        .unwrap();
    assert_eq!(
        quoted_options(batch, "b\""),
        batch_record["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o["name"].as_str().unwrap().to_owned())
            .collect()
    );
    let values = include_str!("../../src/main.rs")
        .split("Some(b\"values\") =>")
        .nth(1)
        .unwrap()
        .split("Some(b\"-h\" | b\"--help\")")
        .next()
        .unwrap();
    assert_eq!(
        quoted_options(values, "b\""),
        BTreeSet::from(["--help".to_owned(), "-h".to_owned()])
    );
    let names: Vec<_> = description["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 9); // Six existing commands plus three additions.
    for command in description["commands"].as_array().unwrap() {
        let name = command["name"].as_str().unwrap();
        if name == "batch" {
            assert_eq!(command["options"].as_array().unwrap().len(), 2);
            assert_eq!(command["options"][0]["name"], "--check");
            assert_eq!(command["options"][1]["name"], "--schema");
            continue;
        }
        if name == "values" {
            assert_eq!(command["options"].as_array().unwrap().len(), 1);
        }
        // Exercise every advertised option through the real binary. A sentinel
        // path ensures option recognition is separate from successful loading.
        for option in command["options"].as_array().unwrap() {
            let flag = option["name"].as_str().unwrap();
            let mut args = vec![name, "synthetic.assets"];
            if name == "dump" {
                args.extend(["-o", "dump.jsonl"]);
            }
            if name == "extract" {
                args.extend(["-o", "out"]);
            }
            if name == "obj" {
                args.extend(["--path-id", "-2", "-o", "out.obj"]);
            }
            if name == "gltf" {
                args.extend(["--scene", "synthetic.assets", "-o", "out.glb"]);
            }
            if name == "show" {
                args.extend(["--path-id", "-2"]);
            }
            if name == "describe" {
                args = vec![name];
            }
            if flag == "--help" {
                args = vec![name, flag];
            } else {
                // Avoid duplicating required flags from the base argument set.
                if let Some(i) = args.iter().position(|a| *a == flag) {
                    args.drain(i..i + 2);
                }
                args.push(flag);
                if !option["value"].is_null() {
                    args.push(option["default"].as_str().unwrap_or(match flag {
                        "--class" | "--asset-filter" => "TextAsset",
                        "--name" => "*",
                        "--path-id" => "-2",
                        "--levels" => "L0",
                        "--unbounded" => "TreeDepth",
                        "--schema" => "error",
                        _ => "1",
                    }));
                }
            }
            let out = run(&tmp, &args);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(
                !stderr.contains("unrecognized arguments")
                    && !stderr.contains("unexpected argument")
                    && !stderr.contains("unknown option"),
                "{name} {flag}: {stderr}"
            );
        }
    }
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
