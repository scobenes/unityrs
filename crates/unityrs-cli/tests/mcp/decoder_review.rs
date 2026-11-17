use crate::mcp_support::{initialize, session};
use serde_json::json;

#[test]
fn bounded_request_tree_and_duplicate_members() {
    session(&[], |s| {
        s.ready();
        for (body, code, message) in [
            (format!(r#"{{"jsonrpc":"2.0","id":9,"method":"ping","params":{{"flood":[{}]}}}}"#, vec!["0"; 20_000].join(",")), -32600, "node"),
            (format!(r#"{{"jsonrpc":"2.0","id":9,"method":"ping","params":{{"deep":{}0{}}}}}"#, "[".repeat(40), "]".repeat(40)), -32600, "depth"),
            (json!({"jsonrpc":"2.0","id":9,"method":"ping","params":{"long":"x".repeat(530_000)}}).to_string(), -32600, "string"),
            (r#"{"jsonrpc":"2.0","id":9,"method":"ping","method":"ping"}"#.into(), -32600, "Duplicate"),
            (r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"unityrs_show","arguments":{"inputs":["synthetic.assets"],"path_id":-2,"max_depth":0,"max_depth":1}}}"#.into(), -32602, "Duplicate"),
            (r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"unityrs_batch_check","arguments":{"job":{"threads":0,"threads":1}}}}"#.into(), -32602, "Duplicate"),
        ] {
            s.send(&format!("{body}\n"));
            let result = s.read();
            assert_eq!(result["error"]["code"], code, "{result}");
            assert!(result["error"]["message"].as_str().unwrap().contains(message), "{result}");
            assert_eq!(s.request("ping", json!({}))["result"], json!({}));
        }
    });
}

#[test]
fn tool_argument_errors_are_actionable_tool_results() {
    session(&["--out", "out"], |s| {
        s.ready();
        for (tool, args, key, bad, allowed) in [
            (
                "show",
                json!({"inputs":["synthetic.assets"],"path_id":-2,"max_depth":129}),
                "max_depth",
                "129",
                "128",
            ),
            (
                "inspect",
                json!({"inputs":["synthetic.assets"],"limit":0}),
                "limit",
                "0",
                "10000",
            ),
            (
                "show",
                json!({"inputs":["synthetic.assets"],"path_id":-2,"bytes":"bad"}),
                "bytes",
                "bad",
                "summary",
            ),
            (
                "show",
                json!({"inputs":["synthetic.assets"],"path_id":-2,"max_items":257}),
                "max_items",
                "257",
                "256",
            ),
            ("inspect", json!({"inputs":[7]}), "inputs[0]", "7", "string"),
            (
                "extract",
                json!({"inputs":["synthetic.assets"],"output":"x","unknown":true}),
                "unknown",
                "true",
                "allowed",
            ),
            (
                "extract",
                json!({"inputs":["synthetic.assets"],"output":"x","asset_filter":["bad"]}),
                "asset_filter[0]",
                "bad",
                "class",
            ),
            (
                "extract",
                json!({"inputs":["synthetic.assets"],"output":"../escape"}),
                "output",
                "../escape",
                "relative",
            ),
            (
                "extract",
                json!({"inputs":["synthetic.assets"],"output":"x","report":"/escape"}),
                "report",
                "/escape",
                "relative",
            ),
        ] {
            let result = s.request(
                "tools/call",
                json!({"name":format!("unityrs_{tool}"),"arguments":args}),
            );
            assert_eq!(result["result"]["isError"], true, "{result}");
            let doc = &result["result"]["structuredContent"];
            assert_eq!(doc["format"], "unityrs.error");
            let message = doc["message"].as_str().unwrap();
            for expected in [key, bad, allowed] {
                assert!(message.contains(expected), "missing {expected}: {message}");
            }
        }
        for params in [
            json!({}),
            json!({"name":"unknown"}),
            json!({"name":"unityrs_show","arguments":[]}),
        ] {
            assert_eq!(s.request("tools/call", params)["error"]["code"], -32602);
        }
    });
}

#[test]
fn initialization_state_is_not_changed_by_malformed_notifications() {
    session(&[], |s| {
        assert_eq!(s.request("tools/list", json!({}))["error"]["code"], -32600);
        s.request("initialize", initialize("2025-11-25"));
        assert_eq!(
            s.request("initialize", initialize("2025-11-25"))["error"]["code"],
            -32600
        );
        for params in [json!([]), json!({"unexpected":true}), json!({"_meta":7})] {
            s.send(&format!(
                "{}\n",
                json!({"jsonrpc":"2.0","method":"notifications/initialized","params":params})
            ));
            assert_eq!(s.request("tools/list", json!({}))["error"]["code"], -32600);
        }
        s.notify("notifications/initialized");
        assert!(s.request("tools/list", json!({}))["result"]["tools"].is_array());
    });
}

#[test]
fn framing_exact_line_cap() {
    session(
        &["--max-request-bytes", "256", "--max-response-bytes", "4096"],
        |s| {
            let ping = r#"{"jsonrpc":"2.0","id":9,"method":"ping"}"#;
            s.send(&format!("{ping}{}\n", " ".repeat(255 - ping.len())));
            assert_eq!(s.read()["result"], json!({}));
            s.send(&format!("{ping}{}\n", " ".repeat(256 - ping.len())));
            assert_eq!(s.read()["error"]["code"], -32600);
            assert_eq!(s.request("ping", json!({}))["result"], json!({}));
        },
    );
}

#[test]
fn request_tree_caps_accept_exact_boundaries_and_refuse_the_next_unit() {
    session(
        &[
            "--max-request-bytes",
            "4096",
            "--max-response-bytes",
            "8192",
        ],
        |s| {
            // 11 envelope/key/container nodes plus 53 scalar nodes = 4096 / 64.
            for (count, accepted) in [(53, true), (54, false)] {
                let params = json!({"flood": vec![0; count]});
                s.send(&format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":9,"method":"ping","params":params})
                ));
                let response = s.read();
                assert_eq!(response.get("result").is_some(), accepted, "{response}");
            }
            // Keys, version and method contribute 29 decoded UTF-8 bytes.
            for (length, accepted) in [(2019, true), (2020, false)] {
                s.send(&format!("{}\n", json!({"jsonrpc":"2.0","id":9,"method":"ping","params":{"s":"x".repeat(length)}})));
                let response = s.read();
                assert_eq!(response.get("result").is_some(), accepted, "{response}");
            }
            for (arrays, accepted) in [(30, true), (31, false)] {
                s.send(&format!(
                    r#"{{"jsonrpc":"2.0","id":9,"method":"ping","params":{{"deep":{}0{}}}}}
"#,
                    "[".repeat(arrays),
                    "]".repeat(arrays)
                ));
                let response = s.read();
                assert_eq!(response.get("result").is_some(), accepted, "{response}");
            }
            s.send(
                "{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\",\"\\u006dethod\":\"ping\"}\n",
            );
            assert_eq!(s.read()["error"]["code"], -32600);
        },
    );
}

#[test]
fn unusable_report_leaves_fail_before_extraction() {
    session(&["--out", "out"], |s| {
        s.ready();
        for report in [".", "dir/", "dir/."] {
            let result = s.request("tools/call", json!({"name":"unityrs_extract","arguments":{"inputs":["synthetic.assets"],"output":"not-created","report":report,"no_container":true}}));
            let doc = &result["result"]["structuredContent"];
            assert_eq!(doc["code"], "usage", "{result}");
            let message = doc["message"].as_str().unwrap();
            for expected in ["report", report, "filename"] {
                assert!(message.contains(expected), "{message}");
            }
            assert!(!s.directory.join("out/not-created").exists());
        }
    });
}

#[test]
fn duplicate_error_channel_tracks_the_params_subtree() {
    session(&[], |s| {
        for (line, code) in [
            (
                r#"{"jsonrpc":"2.0","id":{"x":1,"x":2},"method":"ping"}"#,
                -32600,
            ),
            (
                r#"{"jsonrpc":"2.0","id":9,"method":"ping","extension":{"x":1,"x":2}}"#,
                -32600,
            ),
            (
                r#"{"jsonrpc":"2.0","id":9,"method":"ping","params":{"nested":{"x":1,"x":2}}}"#,
                -32602,
            ),
        ] {
            s.send(&format!("{line}\n"));
            assert_eq!(s.read()["error"]["code"], code);
        }
    });
}
