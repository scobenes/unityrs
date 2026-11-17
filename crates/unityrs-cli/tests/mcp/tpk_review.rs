//! A compressed provider must stay inside the request's own ceilings.
use crate::mcp_support::session;
use serde_json::json;
use unityrs_testkit::bundle::{TestCodec, compress};

fn compressed_provider(codec: TestCodec, compression: u8) -> Vec<u8> {
    // An empty, valid type-tree blob, with ignored padding that still costs
    // decompression work and memory. Its compressed file fits both ceilings.
    let payload = vec![0; 128 << 10];
    let compressed = compress(codec, &payload);
    let mut bytes = b"TPK*".to_vec();
    bytes.extend_from_slice(&[1, compression, 0, 0]);
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&u32::try_from(compressed.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(&compressed);
    assert!(bytes.len() < 4096);
    bytes
}

#[test]
fn compressed_tpk_obeys_each_tools_request_memory_and_work_ceiling() {
    session(&["--out", "out"], |s| {
        s.ready();
        for (codec, compression) in [(TestCodec::Lz4, 1), (TestCodec::Lzma, 2)] {
            std::fs::write(
                s.directory.join("compressed.tpk"),
                compressed_provider(codec, compression),
            )
            .unwrap();
            for tool in ["inspect", "show", "extract"] {
                let mut args = json!({"inputs":["synthetic.assets"],"tpk":"compressed.tpk"});
                if tool == "show" {
                    args["path_id"] = json!(-2);
                }
                if tool == "extract" {
                    args["output"] = json!("provider-budget");
                }
                let ordinary = s.request(
                    "tools/call",
                    json!({"name":format!("unityrs_{tool}"),"arguments":args}),
                );
                assert_ne!(ordinary["result"]["isError"], true, "{ordinary}");
                for (key, option) in [
                    ("max_input_memory", "--max-input-memory"),
                    ("max_decode_work", "--max-decode-work"),
                ] {
                    let mut limited = args.clone();
                    limited[key] = json!(65536);
                    let response = s.request(
                        "tools/call",
                        json!({"name":format!("unityrs_{tool}"),"arguments":limited}),
                    );
                    assert_eq!(
                        response["result"]["isError"], true,
                        "{tool} {key}: {response}"
                    );
                    let error = &response["result"]["structuredContent"];
                    assert_eq!(error["code"], "budget", "{response}");
                    let message = error["message"].as_str().unwrap();
                    assert!(message.contains(option), "{response}");
                    assert!(message.contains("65536"), "{response}");
                }
            }
        }
        assert_eq!(s.request("ping", json!({}))["result"], json!({}));
    });
}

#[test]
fn sparse_oversized_tpk_is_refused_from_metadata_before_any_file_allocation() {
    session(&["--out", "out"], |s| {
        let file = std::fs::File::create(s.directory.join("sparse.tpk")).unwrap();
        file.set_len(1 << 30).unwrap();
        s.ready();
        for tool in ["inspect", "show", "extract"] {
            let mut args =
                json!({"inputs":["synthetic.assets"],"tpk":"sparse.tpk","max_input_memory":65536});
            if tool == "show" {
                args["path_id"] = json!(-2);
            }
            if tool == "extract" {
                args["output"] = json!("sparse");
            }
            let response = s.request(
                "tools/call",
                json!({"name":format!("unityrs_{tool}"),"arguments":args}),
            );
            assert_eq!(response["result"]["isError"], true, "{response}");
            let message = response["result"]["structuredContent"]["message"]
                .as_str()
                .unwrap();
            assert!(
                message.contains("fixed provider bound") && message.contains("16777216"),
                "{response}"
            );
        }
        for command in ["inspect", "show", "extract"] {
            let mut args = vec![
                command,
                "synthetic.assets",
                "--tpk",
                "sparse.tpk",
                "--max-input-memory",
                "65536",
            ];
            if command == "show" {
                args.extend(["--path-id", "-2"]);
            }
            if command == "extract" {
                args.extend(["-o", "ordinary"]);
            }
            let output = unityrs_testkit::child::output(
                std::process::Command::new(crate::support::BIN)
                    .current_dir(&s.directory)
                    .args(args),
            )
            .unwrap();
            assert_eq!(output.status.code(), Some(1));
            let diagnostic = if command == "extract" {
                String::from_utf8(output.stderr).unwrap()
            } else {
                String::from_utf8(output.stdout).unwrap()
            };
            assert!(
                diagnostic.contains("the TPK's file: 1073741824 > 16777216, the provider's bound"),
                "{command}: {diagnostic}"
            );
        }
    });
}

#[test]
fn ordinary_extract_keeps_the_legacy_missing_tpk_diagnostic() {
    let tmp = crate::support::fixture();
    let output = crate::support::run(
        &tmp,
        &[
            "extract",
            "synthetic.assets",
            "-o",
            "out",
            "--tpk",
            "missing.tpk",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    let message = String::from_utf8(output.stderr).unwrap();
    assert!(
        message.contains("unityrs extract: reading the TPK: No such file or directory"),
        "{message}"
    );
    assert!(!tmp.join("out").exists());
}
