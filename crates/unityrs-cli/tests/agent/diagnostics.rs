//! Errors retain the identity and control needed to retry the operation.
use super::support::{fixture, json};
use serde_json::Value;
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_testkit::ngui::{self, Object, s, st};

fn contains(error: &Value, parts: &[&str]) {
    let message = error["message"].as_str().unwrap();
    for part in parts {
        assert!(message.contains(part), "missing {part:?}: {error}");
    }
}

fn address(error: &Value) {
    assert_eq!(error["detail"]["input"], 1, "{error}");
    assert_eq!(error["detail"]["file_index"], 0, "{error}");
    assert_eq!(error["detail"]["path_id"], 7, "{error}");
    contains(
        error,
        &["input 1", "large.assets", "file_index 0", "path_id 7"],
    );
}

#[test]
fn shared_provider_failures_do_not_blame_a_skipped_input() {
    let tmp = fixture();
    std::fs::write(tmp.join("bad.tpk"), [0; 8]).unwrap();
    for provider in ["missing.tpk", "bad.tpk"] {
        for command in ["show", "inspect"] {
            let mut args = vec![
                command,
                "missing.assets",
                "synthetic.assets",
                "--tpk",
                provider,
            ];
            if command == "show" {
                args.extend(["--path-id", "-2", "--file-index", "1:0"]);
            }
            let error = json(&tmp, &args, 1);
            assert_eq!(error["code"], "input");
            contains(&error, &["shared provider", "--tpk", provider]);
            assert_eq!(error["detail"]["tpk"], provider);
            assert!(error["detail"].get("input").is_none(), "{error}");
            assert!(
                !error["message"]
                    .as_str()
                    .unwrap()
                    .contains("missing.assets")
            );
        }
    }
}

#[test]
fn provider_budget_refusals_preserve_the_option_and_shared_context() {
    let tmp = fixture();
    let mut provider = b"TPK*".to_vec();
    provider.extend_from_slice(&[1, 0, 0, 0]);
    provider.extend_from_slice(&0_u32.to_le_bytes());
    provider.extend_from_slice(&12_u32.to_le_bytes());
    provider.extend_from_slice(&12_u32.to_le_bytes());
    provider.extend_from_slice(&0_i64.to_le_bytes());
    provider.extend_from_slice(&i32::MAX.to_le_bytes());
    std::fs::write(tmp.join("large.tpk"), provider).unwrap();
    let page = json(&tmp, &["inspect", "synthetic.assets", "--limit", "1"], 0);
    let token = page["next_cursor"].as_str().unwrap();
    for cursor in [false, true] {
        let mut args = vec!["inspect", "synthetic.assets", "--tpk", "large.tpk"];
        if cursor {
            args.extend(["--cursor", token]);
        }
        let error = json(&tmp, &args, 1);
        assert_eq!(error["code"], "budget");
        assert_eq!(error["detail"]["tpk"], "large.tpk");
        contains(
            &error,
            &[
                "fixed provider bound",
                "current:",
                "no CLI option raises it",
                "--tpk",
                "large.tpk",
            ],
        );
        args.extend(["--max-input-bytes", "1"]);
        let error = json(&tmp, &args, 1);
        assert_eq!(error["code"], "budget");
        assert_eq!(error["detail"]["tpk"], "large.tpk");
        contains(&error, &["raise --max-input-bytes", "current: 1"]);
    }
}

#[test]
fn show_render_work_refusal_retains_the_selected_address() {
    let tmp = fixture();
    std::fs::write(
        tmp.join("large.assets"),
        ngui::file(
            &[],
            &[Object::new(
                7,
                49,
                st(
                    "TextAsset",
                    vec![("m_Name", s("large")), ("m_Script", s(&"x".repeat(10_000)))],
                ),
            )],
            true,
        )
        .unwrap(),
    )
    .unwrap();
    // Prove that the same reader budget loads and parses the value. The
    // refusal is in response rendering, on its separate response meter.
    let mut opts = EnvOptions::unitypy();
    let mut context = (*opts.context).clone();
    context.limits.max_decode_work = 4096;
    opts.context = std::sync::Arc::new(context);
    let file = Environment::new(opts)
        .load(Source::Path(tmp.join("large.assets")))
        .unwrap();
    let unityrs_core::files::File::Serialized(file) = file else {
        panic!("serialized fixture")
    };
    file.object(7).unwrap().parse_as_dict(false).unwrap();
    for view in ["natural", "canonical"] {
        let error = json(
            &tmp,
            &[
                "show",
                "missing.assets",
                "large.assets",
                "--path-id",
                "7",
                "--file-index",
                "1:0",
                "--max-decode-work",
                "4096",
                "--value",
                view,
            ],
            1,
        );
        assert_eq!(error["code"], "budget");
        address(&error);
        contains(
            &error,
            &["raise --max-decode-work", "current: 4096", "DecodeWork"],
        );
    }
}

#[test]
fn show_render_memory_refusal_retains_the_selected_address() {
    let tmp = fixture();
    std::fs::write(
        tmp.join("large.assets"),
        ngui::file(
            &[],
            &[Object::new(
                7,
                49,
                st(
                    "TextAsset",
                    vec![("m_Name", s("large")), ("m_Script", s(&"\0".repeat(8000)))],
                ),
            )],
            true,
        )
        .unwrap(),
    )
    .unwrap();
    let mut opts = EnvOptions::unitypy();
    let mut context = (*opts.context).clone();
    context.limits.max_input_memory = 60000;
    opts.context = std::sync::Arc::new(context);
    let file = Environment::new(opts)
        .load(Source::Path(tmp.join("large.assets")))
        .unwrap();
    let unityrs_core::files::File::Serialized(file) = file else {
        panic!("serialized fixture")
    };
    file.object(7).unwrap().parse_as_dict(false).unwrap();
    let error = json(
        &tmp,
        &[
            "show",
            "missing.assets",
            "large.assets",
            "--path-id",
            "7",
            "--file-index",
            "1:0",
            "--max-input-memory",
            "60000",
        ],
        1,
    );
    assert_eq!(error["code"], "budget");
    address(&error);
    contains(&error, &["raise --max-input-memory", "current: 60000"]);
}

#[test]
fn object_read_failures_retain_addresses_and_provider_guidance() {
    let tmp = fixture();
    for trees in [true, false] {
        let object = Object::new(7, 49, st("TextAsset", vec![("m_Name", s("large"))]));
        let object = if trees { object.cut_to(2) } else { object };
        std::fs::write(
            tmp.join("large.assets"),
            ngui::file(&[], &[object], trees).unwrap(),
        )
        .unwrap();
        let error = json(
            &tmp,
            &[
                "show",
                "missing.assets",
                "large.assets",
                "--path-id",
                "7",
                "--file-index",
                "1:0",
            ],
            1,
        );
        assert_eq!(error["code"], "input");
        address(&error);
        if !trees {
            contains(&error, &["--tpk FILE", "current: absent"]);
        }
    }
}

#[test]
fn inspect_header_work_refusal_retains_the_object_after_parsing() {
    let tmp = fixture();
    let bytes = ngui::file(
        &[],
        &[Object::new(
            7,
            49,
            st("TextAsset", vec![("m_Name", s("large"))]),
        )],
        true,
    )
    .unwrap();
    std::fs::write(tmp.join("large.assets"), bytes).unwrap();
    let mut opts = EnvOptions::unitypy();
    let mut context = (*opts.context).clone();
    context.limits.max_decode_work = 4096;
    opts.context = std::sync::Arc::new(context);
    let file = Environment::new(opts)
        .load_observed(Source::Path(tmp.join("large.assets")), |data, meter| {
            meter.count_decode(data.len() as u64)?;
            let _ = unityrs_dump::json::sha256(data);
            Ok(())
        })
        .unwrap();
    let unityrs_core::files::File::Serialized(file) = file else {
        panic!("serialized fixture")
    };
    file.object(7).unwrap().parse_as_dict(false).unwrap();
    // Input hashing and parsing have enough work. Repeated class filters use
    // the response meter during fingerprinting, leaving little for headers.
    let filters = vec!["49"; 46];
    let mut args = vec![
        "inspect",
        "missing.assets",
        "large.assets",
        "--max-decode-work",
        "4096",
    ];
    for filter in &filters {
        args.extend(["--class", filter]);
    }
    let page = json(&tmp, &args, 0);
    let error = &page["inputs"][1]["error"];
    assert_eq!(error["code"], "budget", "{page}");
    address(error);
    contains(error, &["raise --max-decode-work", "current: 4096"]);
}

#[test]
fn budget_options_and_current_values_survive_all_phases() {
    let tmp = fixture();
    for option in [
        "--max-input-memory",
        "--max-input-bytes",
        "--max-decode-work",
        "--max-tree-depth",
    ] {
        for command in ["show", "inspect"] {
            let mut args = vec![command, "synthetic.assets", option, "0"];
            if command == "show" {
                args.extend(["--path-id", "-2"]);
            }
            let status = i32::from(
                command == "show" || matches!(option, "--max-input-memory" | "--max-decode-work"),
            );
            let doc = json(&tmp, &args, status);
            let error = if status == 0 {
                &doc["inputs"][0]["error"]
            } else {
                &doc
            };
            assert_eq!(error["code"], "budget");
            contains(error, &[option, "current: 0", "synthetic.assets"]);
        }
    }
    let error = json(
        &tmp,
        &["inspect", "missing.assets", "--max-input-memory", "256"],
        1,
    );
    assert_eq!(error["code"], "budget");
    contains(
        &error,
        &["--max-input-memory", "current: 256", "missing.assets"],
    );
    for command in ["show", "inspect"] {
        let mut args = vec![command, "synthetic.assets", "--max-bytes", "1"];
        if command == "show" {
            args.extend(["--path-id", "-2"]);
        }
        let error = json(&tmp, &args, 1);
        contains(
            &error,
            &["raise --max-bytes", "current: 1", "synthetic.assets"],
        );
        assert_eq!(error["detail"]["input"], 0);
        assert_eq!(error["detail"]["file_index"], 0);
        assert_eq!(error["detail"]["path_id"], -2);
    }
}

#[test]
fn selection_errors_name_the_requested_values() {
    let tmp = fixture();
    let error = json(
        &tmp,
        &[
            "show",
            "missing.assets",
            "synthetic.assets",
            "--path-id",
            "999",
            "--file-index",
            "1:0",
            "--file",
            "absent",
        ],
        1,
    );
    assert_eq!(error["code"], "not_found");
    contains(
        &error,
        &[
            "synthetic.assets",
            "--path-id '999'",
            "--file-index '1:0'",
            "--file 'absent'",
        ],
    );
    let error = json(&tmp, &["show", "synthetic.bundle", "--path-id", "-2"], 1);
    assert_eq!(error["code"], "ambiguous");
    contains(
        &error,
        &["synthetic.bundle", "--path-id '-2'", "--file-index"],
    );
}

#[test]
fn cursor_refusals_name_the_token_and_recovery() {
    let tmp = fixture();
    let error = json(
        &tmp,
        &["inspect", "synthetic.assets", "--cursor", "bad-token"],
        1,
    );
    contains(
        &error,
        &[
            "--cursor 'bad-token'",
            "synthetic.assets",
            "restart inspect without --cursor",
        ],
    );
    let page = json(&tmp, &["inspect", "synthetic.assets", "--limit", "1"], 0);
    let token = page["next_cursor"].as_str().unwrap();
    let error = json(
        &tmp,
        &[
            "inspect",
            "synthetic.assets",
            "--cursor",
            token,
            "--tpk",
            "missing.tpk",
        ],
        1,
    );
    assert_eq!(error["code"], "bad_cursor");
    assert_eq!(error["detail"]["tpk"], "missing.tpk");
    contains(
        &error,
        &["--tpk", "missing.tpk", "restart inspect without --cursor"],
    );
}

#[test]
fn duplicate_options_name_both_values() {
    let tmp = fixture();
    for args in [
        vec![
            "inspect",
            "synthetic.assets",
            "--limit",
            "2",
            "--limit",
            "3",
        ],
        vec!["describe", "--schema", "show", "--schema", "error"],
    ] {
        let error = json(&tmp, &args, 2);
        contains(
            &error,
            &[
                args[args.len() - 2],
                args[args.len() - 1],
                args[args.len() - 3],
            ],
        );
    }
}
