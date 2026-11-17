//! Real stdio sessions using only generated Unity fixtures.
#[path = "mcp/support.rs"]
mod mcp_support;
#[path = "agent/support.rs"]
mod support;
use mcp_support::*;
use serde_json::json;

#[test]
fn lifecycle_and_recovery() {
    session(&[], |s| {
        let init = s.request("initialize", initialize("2025-11-25"));
        assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
        s.notify("notifications/initialized");
        let list = s.request("tools/list", json!({}));
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 4);
        assert!(tools.iter().all(|t| t["name"] != "unityrs_extract"));
        assert_eq!(s.request("ping", json!({}))["result"], json!({}));
        assert_eq!(s.request("missing", json!({}))["error"]["code"], -32601);
        assert_eq!(s.request("tools/call", json!({}))["error"]["code"], -32602);
        s.send("{broken\n");
        assert_eq!(s.read()["error"]["code"], -32700);
        s.send("[]\n");
        assert_eq!(s.read()["error"]["code"], -32600);
        s.notify("ping");
        assert_eq!(s.request("ping", json!({}))["result"], json!({}));
    });
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one end-to-end protocol and schema session"
)]
fn tools_documents_schemas_and_pagination() {
    session(&["--out", "out"], |s| {
        s.ready();
        let list = s.request("tools/list", json!({}));
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 5);
        for (name, arguments) in [
            ("describe", json!({})),
            ("describe", json!({"schema":"show"})),
            (
                "inspect",
                json!({"inputs":["synthetic.assets"],"class":["TextAsset"],"name":["a*"]}),
            ),
            (
                "show",
                json!({"inputs":["synthetic.assets"],"path_id":-2,"file_index":"0:0","value":"canonical","bytes":"full"}),
            ),
            (
                "extract",
                json!({"inputs":["synthetic.assets"],"output":"exports/text","no_container":true,"asset_filter":["TextAsset"]}),
            ),
            ("batch_check", json!({"job":job()})),
        ] {
            let tool = tools
                .iter()
                .find(|t| t["name"] == format!("unityrs_{name}"))
                .unwrap();
            assert!(support::valid(
                &arguments,
                &tool["inputSchema"],
                &tool["inputSchema"]
            ));
            let response = s.request(
                "tools/call",
                json!({"name":format!("unityrs_{name}"),"arguments":arguments}),
            );
            check_result(&response, tool, false);
        }
        assert_eq!(
            std::fs::read(s.directory.join("out/exports/text/alpha.txt")).unwrap(),
            b"hello"
        );
        let mut cursor: Option<serde_json::Value> = None;
        let mut ids = Vec::new();
        loop {
            let mut args = json!({"inputs":["synthetic.assets"],"limit":1});
            if let Some(token) = &cursor {
                args["cursor"] = token.clone();
            }
            let response = s.request(
                "tools/call",
                json!({"name":"unityrs_inspect","arguments":args}),
            );
            let tool = tools
                .iter()
                .find(|t| t["name"] == "unityrs_inspect")
                .unwrap();
            check_result(&response, tool, false);
            let doc = &response["result"]["structuredContent"];
            ids.extend(
                doc["objects"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|o| o["path_id"].as_i64().unwrap()),
            );
            cursor = doc["next_cursor"].as_str().map(|s| json!(s));
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(ids, [-2, 10, 30]);
        for (name, args) in [
            ("show", json!({"inputs":["synthetic.assets"],"path_id":999})),
            (
                "inspect",
                json!({"inputs":["synthetic.assets"],"cursor":"bad"}),
            ),
            (
                "extract",
                json!({"inputs":["missing.assets"],"output":"exports/missing"}),
            ),
            ("batch_check", json!({"job":{}})),
        ] {
            let tool = tools
                .iter()
                .find(|t| t["name"] == format!("unityrs_{name}"))
                .unwrap();
            let response = s.request(
                "tools/call",
                json!({"name":format!("unityrs_{name}"),"arguments":args}),
            );
            check_result(&response, tool, true);
        }
        // Every option's input property comes from the same describe/parser table.
        let desc = s.request("tools/call", json!({"name":"unityrs_describe"}));
        for command in desc["result"]["structuredContent"]["commands"]
            .as_array()
            .unwrap()
        {
            let name = command["name"].as_str().unwrap();
            if !["describe", "inspect", "show", "extract"].contains(&name) {
                continue;
            }
            let tool = tools
                .iter()
                .find(|t| t["name"] == format!("unityrs_{name}"))
                .unwrap();
            for opt in command["options"].as_array().unwrap() {
                let flag = opt["name"].as_str().unwrap();
                if ["--help", "-o"].contains(&flag) {
                    continue;
                }
                let key = flag.trim_start_matches('-').replace('-', "_");
                let property = &tool["inputSchema"]["properties"][&key];
                assert!(!property.is_null(), "{name}: {key}");
                assert_eq!(property["description"], opt["help"]);
            }
        }
    });
}
fn check_result(response: &serde_json::Value, tool: &serde_json::Value, failed: bool) {
    let result = &response["result"];
    assert_eq!(result["isError"], failed, "{response}");
    assert_eq!(result["content"].as_array().unwrap().len(), 1);
    assert_eq!(result["content"][0]["type"], "text");
    let text: serde_json::Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, result["structuredContent"]);
    assert!(
        support::valid(&text, &tool["outputSchema"], &tool["outputSchema"]),
        "{text}: {}",
        tool["outputSchema"]
    );
    if failed {
        assert_eq!(text["format"], "unityrs.error");
    }
}
fn job() -> serde_json::Value {
    json!({"spec_version":1,"run_id":"synthetic","attempt":1,"threads":1,"download_concurrency":1,
        "max_inflight_uncompressed_bytes":268_435_456,"max_decoded_bytes":16_777_216,
        "dep_cache_bytes":33_554_432,"max_view_bundles":4,
        "spool":{"dir":"/placeholder/spool","size_bytes":1_073_741_824,"reserve_bytes":0},
        "outputs":{"textures":{"key":"{input}/{name}.png"}},"sink":{"kind":"directory","root":"/placeholder/out"},
        "inputs":[{"id":"synthetic","path":"/placeholder/input"}]})
}

#[test]
fn confinement_and_budget_ceilings() {
    session(
        &[
            "--out",
            "out",
            "--max-input-bytes",
            "1048576",
            "--max-items",
            "8",
        ],
        |s| {
            s.ready();
            let outside = unityrs_testkit::scratch::Scratch::new("mcp-outside");
            std::fs::copy(
                s.directory.join("synthetic.assets"),
                outside.join("outside.assets"),
            )
            .unwrap();
            std::os::unix::fs::symlink(outside.path(), s.directory.join("escape")).unwrap();
            std::os::unix::fs::symlink(outside.path(), s.directory.join("out/escape")).unwrap();
            for path in [
                outside
                    .join("outside.assets")
                    .to_string_lossy()
                    .into_owned(),
                "../outside.assets".into(),
                "escape/outside.assets".into(),
                "/placeholder/elsewhere.assets".into(),
            ] {
                let response = s.request(
                    "tools/call",
                    json!({"name":"unityrs_show","arguments":{"inputs":[path],"path_id":-2}}),
                );
                assert_eq!(response["result"]["isError"], true, "{response}");
            }
            for output in [
                "../escape",
                "a/../../escape",
                "/placeholder/escape",
                "escape/sub",
            ] {
                let response = s.request("tools/call", json!({"name":"unityrs_extract","arguments":{"inputs":["synthetic.assets"],"output":output,"no_container":true}}));
                assert_eq!(response["result"]["isError"], true, "{response}");
            }
            for (tool, args) in [
                (
                    "show",
                    json!({"inputs":["synthetic.assets"],"path_id":-2,"max_input_bytes":1_048_577}),
                ),
                (
                    "show",
                    json!({"inputs":["synthetic.assets"],"path_id":-2,"max_items":9}),
                ),
                (
                    "inspect",
                    json!({"inputs":["synthetic.assets"],"max_decode_work":34_359_738_369_u64}),
                ),
                (
                    "extract",
                    json!({"inputs":["synthetic.assets"],"output":"budget","max_input_memory":8_589_934_593_u64}),
                ),
            ] {
                let response = s.request(
                    "tools/call",
                    json!({"name":format!("unityrs_{tool}"),"arguments":args}),
                );
                assert_eq!(response["result"]["isError"], true, "{response}");
                assert_eq!(response["result"]["structuredContent"]["code"], "budget");
            }
            let response = s.request("tools/call", json!({"name":"unityrs_show","arguments":{"inputs":["synthetic.assets"],"path_id":-2,"max_input_bytes":1}}));
            assert_eq!(response["result"]["structuredContent"]["code"], "budget");
            assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 1);
        },
    );
    session(&[], |s| {
        s.ready();
        assert_eq!(
            s.request(
                "tools/call",
                json!({"name":"unityrs_extract","arguments":{}})
            )["error"]["code"],
            -32602
        );
    });
}

#[test]
fn framing_version_negotiation_and_invalid_requests() {
    for version in ["2025-11-25", "2099-01-01", "2024-11-05"] {
        session(
            &[
                "--max-request-bytes",
                "4096",
                "--max-response-bytes",
                "8192",
            ],
            |s| {
                assert_eq!(
                    s.request("initialize", initialize(version))["result"]["protocolVersion"],
                    "2025-11-25"
                );
                s.notify("notifications/initialized");
                s.send(&format!("{}\n", "x".repeat(5000)));
                assert_eq!(s.read()["error"]["code"], -32600);
                for line in [
                    "null",
                    "{}",
                    "42",
                    "[]",
                    "[{}]",
                    r#"{"jsonrpc":"1.0","id":"old","method":"ping"}"#,
                    r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#,
                ] {
                    s.send(&format!("{line}\n"));
                    assert_eq!(s.read()["error"]["code"], -32600);
                }
                s.send(
                    r#"{"jsonrpc":"2.0","id":"string-id","method":"ping"}
"#,
                );
                assert_eq!(s.read()["id"], "string-id");
                assert_eq!(s.request("tools/list", json!({}))["error"]["code"], -32603);
                let response = s.request("tools/call", json!({"name":"unityrs_describe"}));
                assert_eq!(response["result"]["structuredContent"]["code"], "budget");
                assert_eq!(s.request("ping", json!({}))["result"], json!({}));
            },
        );
    }
    session(&[], |s| {
        assert_eq!(s.request("tools/list", json!({}))["error"]["code"], -32600);
        assert_eq!(s.request("initialize", json!({}))["error"]["code"], -32602);
        s.ready();
        for args in [
            json!({"inputs":[]}),
            json!({"inputs":["synthetic.assets"],"limit":0}),
            json!({"inputs":["synthetic.assets"],"limit":"1"}),
            json!({"inputs":["synthetic.assets"],"unknown":true}),
        ] {
            assert_eq!(
                s.request(
                    "tools/call",
                    json!({"name":"unityrs_inspect","arguments":args})
                )["error"]["code"],
                -32602
            );
        }
    });
}

#[test]
fn split_companions_providers_and_held_output_roots() {
    session(&["--out", "out"], |s| {
        s.ready();
        let bytes = std::fs::read(s.directory.join("synthetic.assets")).unwrap();
        let middle = bytes.len() / 2;
        std::fs::write(s.directory.join("parts.assets.split0"), &bytes[..middle]).unwrap();
        std::fs::write(s.directory.join("parts.assets.split1"), &bytes[middle..]).unwrap();
        for input in ["parts.assets", "parts.assets.split0", "parts.assets.split٠"] {
            let response = s.request(
                "tools/call",
                json!({"name":"unityrs_show","arguments":{"inputs":[input],"path_id":-2}}),
            );
            assert_eq!(
                response["result"]["structuredContent"]["object"]["file"],
                "parts.assets"
            );
            assert_eq!(
                response["result"]["structuredContent"]["value"]["m_Script"], "hello",
                "{response}"
            );
        }
        let outside = unityrs_testkit::scratch::Scratch::new("mcp-companions");
        std::fs::write(outside.join("tail"), &bytes[middle..]).unwrap();
        std::fs::remove_file(s.directory.join("parts.assets.split1")).unwrap();
        std::os::unix::fs::symlink(
            outside.join("tail"),
            s.directory.join("parts.assets.split1"),
        )
        .unwrap();
        let response = s.request(
            "tools/call",
            json!({"name":"unityrs_show","arguments":{"inputs":["parts.assets"],"path_id":-2}}),
        );
        assert_eq!(response["result"]["isError"], true);
        for (tool, field) in [
            ("show", "tpk"),
            ("extract", "tpk"),
            ("extract", "vorbis_headers"),
        ] {
            let mut args = json!({"inputs":["synthetic.assets"]});
            if tool == "show" {
                args["path_id"] = json!(-2);
            } else {
                args["output"] = json!("providers");
            }
            args[field] = json!(outside.join("tail").to_str().unwrap());
            let response = s.request(
                "tools/call",
                json!({"name":format!("unityrs_{tool}"),"arguments":args}),
            );
            assert_eq!(response["result"]["isError"], true, "{response}");
        }
        std::fs::rename(s.directory.join("out"), s.directory.join("held-out")).unwrap();
        std::os::unix::fs::symlink(outside.path(), s.directory.join("out")).unwrap();
        let response = s.request("tools/call", json!({"name":"unityrs_extract","arguments":{"inputs":["synthetic.assets"],"output":"held","no_container":true,"asset_filter":["TextAsset"]}}));
        assert_eq!(response["result"]["isError"], false, "{response}");
        assert_eq!(
            std::fs::read(s.directory.join("held-out/held/alpha.txt")).unwrap(),
            b"hello"
        );
        assert!(!outside.join("held").exists());
    });
}

#[test]
fn held_input_root_survives_pathname_replacement() {
    session(&[], |s| {
        s.ready();
        let original = s.directory.clone();
        let held = original.with_extension("held");
        // Scratch ownership remains with the original pathname: restore it
        // before assertions so a failing test also removes every fixture.
        std::fs::rename(&original, &held).unwrap();
        let replacement = unityrs_testkit::scratch::Scratch::new("mcp-replacement");
        std::fs::write(
            replacement.join("synthetic.assets"),
            b"untrusted replacement",
        )
        .unwrap();
        std::os::unix::fs::symlink(replacement.path(), &original).unwrap();
        let response = s.request("tools/call", json!({"name":"unityrs_show","arguments":{"inputs":[original.join("synthetic.assets").to_str().unwrap()],"path_id":-2}}));
        std::fs::remove_file(&original).unwrap();
        std::fs::rename(&held, &original).unwrap();
        assert_eq!(
            response["result"]["structuredContent"]["value"]["m_Script"], "hello",
            "{response}"
        );
    });
}

#[test]
fn startup_options_reject_unknown_missing_and_duplicate_values() {
    for option in [
        "--root",
        "--out",
        "--max-request-bytes",
        "--max-response-bytes",
        "--max-input-memory",
        "--max-input-bytes",
        "--max-decode-work",
        "--max-tree-depth",
        "--max-bytes",
        "--max-items",
        "--max-depth",
        "--limit",
    ] {
        let tmp = support::fixture();
        let output = support::run(&tmp, &["mcp", option]);
        assert_eq!(output.status.code(), Some(2), "{option}: {output:?}");
        assert_eq!(output.stdout, [] as [u8; 0]);
        assert!(String::from_utf8_lossy(&output.stderr).contains("missing"));
    }
    let tmp = support::fixture();
    for args in [
        vec!["mcp"],
        vec!["mcp", "--unknown"],
        vec!["mcp", "--root", ".", "--max-items", "1", "--max-items", "2"],
        vec!["mcp", "--root", ".", "--max-depth", "129"],
        vec!["mcp", "--root", ".", "--max-request-bytes", "1"],
        vec!["mcp", "--root", ".", "--max-response-bytes", "1024"],
    ] {
        assert_eq!(support::run(&tmp, &args).status.code(), Some(2));
    }
    // Repeated roots are explicitly allowed and still start a server.
    session(&["--root", "."], |s| {
        s.ready();
    });
}

#[test]
fn batch_preparation_and_response_use_server_budgets() {
    session(&["--max-input-memory", "262144"], |s| {
        s.ready();
        let ordinary = s.request(
            "tools/call",
            json!({"name":"unityrs_batch_check","arguments":{"job":job()}}),
        );
        assert_eq!(ordinary["result"]["isError"], false, "{ordinary}");
        let mut large = job();
        large["threads"] = json!(1024);
        large["download_concurrency"] = json!(256);
        let response = s.request(
            "tools/call",
            json!({"name":"unityrs_batch_check","arguments":{"job":large}}),
        );
        assert_eq!(
            response["result"]["structuredContent"]["code"], "budget",
            "{response}"
        );
    });
    session(&["--max-decode-work", "65536"], |s| {
        s.ready();
        let response = s.request(
            "tools/call",
            json!({"name":"unityrs_batch_check","arguments":{"job":job()}}),
        );
        assert_eq!(
            response["result"]["structuredContent"]["code"], "budget",
            "{response}"
        );
    });
    session(&["--max-input-memory", "1024"], |s| {
        s.ready();
        let response = s.request("tools/call", json!({"name":"unityrs_describe"}));
        assert_eq!(
            response["result"]["structuredContent"]["code"], "budget",
            "{response}"
        );
        assert_eq!(s.request("ping", json!({}))["result"], json!({}));
    });
    session(&["--out", "out"], |s| {
        s.ready();
        let response = s.request("tools/call", json!({"name":"unityrs_extract","arguments":{"inputs":["synthetic.assets"],"output":"budget","max_input_bytes":1}}));
        assert_eq!(
            response["result"]["structuredContent"]["code"], "budget",
            "{response}"
        );
    });
}
