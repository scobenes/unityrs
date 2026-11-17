//! Extraction report and parser contracts over the real MCP transport.
use crate::{mcp_support::session, support};
use serde_json::{Value, json};
use unityrs_testkit::{
    bundle::{self, BundleSpec, EntrySpec},
    ngui::{Object, V, s, st},
    scene3d::{self, Layout},
};

fn text_file(name: &[u8]) -> Vec<u8> {
    scene3d::file(
        Layout::Unity2022,
        &[],
        &[Object::new(
            1,
            49,
            st(
                "TextAsset",
                vec![
                    ("m_Name", V::Str(name.to_vec())),
                    ("m_Script", s("payload")),
                ],
            ),
        )],
    )
    .unwrap()
}

#[test]
fn raw_member_names_are_tagged_in_mcp_reports_and_parity_stays_escaped() {
    session(&["--out", "out"], |s| {
        std::fs::write(
            s.directory.join("raw.bundle"),
            bundle::build(&BundleSpec {
                entries: vec![EntrySpec {
                    name: vec![0xff],
                    data: text_file(b"leaf"),
                    flags: 4,
                }],
                ..BundleSpec::default()
            }),
        )
        .unwrap();
        s.ready();
        let list = s.request("tools/list", json!({}));
        let tool = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "unityrs_extract")
            .unwrap();
        let response = s.request("tools/call", json!({"name":"unityrs_extract","arguments":{"inputs":["raw.bundle"],"output":"raw","no_container":true,"report":"report.json"}}));
        assert_eq!(response["result"]["isError"], false, "{response}");
        let report = &response["result"]["structuredContent"];
        assert_eq!(
            report["written"][0]["serialized_file"],
            json!({"t":"str","hex":"ff"})
        );
        assert_eq!(report["written"][0]["path"], "leaf.txt");
        assert_eq!(report["failures"], json!([]));
        assert_eq!(
            std::fs::read(s.directory.join("out/raw/leaf.txt")).unwrap(),
            b"payload"
        );
        let saved: Value = serde_json::from_slice(
            &std::fs::read(s.directory.join("out/raw/report.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(&saved, report);
        let text: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(&text, report);
        assert!(support::valid(
            report,
            &tool["outputSchema"],
            &tool["outputSchema"]
        ));
        // Structural comparison ties this metadata contract to agent rendering.
        let schema = s.request(
            "tools/call",
            json!({"name":"unityrs_describe","arguments":{"schema":"show"}}),
        );
        let rawstr = &schema["result"]["structuredContent"]["$defs"]["rawstr"];
        for group in ["written", "failures"] {
            let props =
                &tool["outputSchema"]["anyOf"][0]["properties"][group]["items"]["properties"];
            assert_eq!(&props["serialized_file"], rawstr);
            assert_eq!(&props["path"], rawstr);
        }
        let parity = unityrs_testkit::child::output(
            std::process::Command::new(support::BIN)
                .current_dir(&s.directory)
                .args(["extract", "raw.bundle", "-o", "parity", "--no-container"]),
        )
        .unwrap();
        assert!(parity.status.success(), "{parity:?}");
        assert!(
            String::from_utf8(parity.stdout)
                .unwrap()
                .contains("\"serialized_file\":\"\\udcff\"")
        );
    });
}

#[test]
fn container_directory_counts_have_exact_signed_parser_bounds() {
    session(&["--out", "out"], |s| {
        s.ready();
        let list = s.request("tools/list", json!({}));
        let tool = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "unityrs_extract")
            .unwrap();
        let prop = &tool["inputSchema"]["properties"]["ignore_first_container_dirs"];
        assert_eq!(prop["type"], "integer");
        assert_eq!(prop["minimum"], json!(i64::MIN));
        assert_eq!(prop["maximum"], json!(i64::MAX));
        let describe = s.request("tools/call", json!({"name":"unityrs_describe"}));
        let commands = describe["result"]["structuredContent"]["commands"]
            .as_array()
            .unwrap();
        let extract = commands.iter().find(|c| c["name"] == "extract").unwrap();
        for option in extract["options"].as_array().unwrap() {
            let name = option["name"].as_str().unwrap();
            // The CLI-only container aliases are not MCP switches (schema.rs).
            if ["--help", "-o", "--use-container", "--no-use-container"].contains(&name) {
                continue;
            }
            let key = name.trim_start_matches('-').replace('-', "_");
            let property = &tool["inputSchema"]["properties"][key];
            assert!(
                property["description"]
                    .as_str()
                    .unwrap()
                    .starts_with(option["help"].as_str().unwrap())
            );
            let kind = match option["value"].as_str() {
                None => "boolean",
                Some("N") => "integer",
                _ => "string",
            };
            let scalar = if name == "--asset-filter" {
                assert_eq!(property["type"], "array");
                &property["items"]
            } else {
                property
            };
            assert_eq!(scalar["type"], kind, "{name}");
            if let Some(default) = option["default"].as_str() {
                let expected = match kind {
                    "boolean" => json!(default.parse::<bool>().unwrap()),
                    "integer" => json!(default.parse::<i128>().unwrap()),
                    _ => json!(default),
                };
                assert_eq!(property["default"], expected, "{name}");
            }
        }
        for count in [json!(-1), json!(i64::MIN), json!(i64::MAX)] {
            let response = s.request("tools/call", json!({"name":"unityrs_extract","arguments":{"inputs":["synthetic.assets"],"output":"signed","no_container":true,"ignore_first_container_dirs":count}}));
            assert_eq!(response["result"]["isError"], false, "{count}: {response}");
        }
        for count in [
            json!(9_223_372_036_854_775_808_u64),
            json!(-9_223_372_036_854_775_809_f64),
        ] {
            let response = s.request("tools/call", json!({"name":"unityrs_extract","arguments":{"inputs":["synthetic.assets"],"output":"invalid","ignore_first_container_dirs":count}}));
            assert_eq!(response["result"]["isError"], true, "{count}: {response}");
        }
    });
}

#[test]
fn hostile_asset_names_remain_confined_and_report_failures() {
    session(&["--out", "out"], |s| {
        let absolute = s
            .directory
            .join("escaped")
            .to_str()
            .unwrap()
            .as_bytes()
            .to_vec();
        for (index, name) in [
            b"../../escaped".to_vec(),
            absolute,
            b"reserved:stream".to_vec(),
            b"C:/escaped".to_vec(),
            b"dir\\escaped".to_vec(),
            b"nul\0name".to_vec(),
        ]
        .iter()
        .enumerate()
        {
            std::fs::write(
                s.directory.join(format!("hostile{index}.assets")),
                text_file(name),
            )
            .unwrap();
        }
        s.ready();
        for index in 0..6 {
            let response = s.request("tools/call", json!({"name":"unityrs_extract","arguments":{"inputs":[format!("hostile{index}.assets")],"output":format!("hostile{index}"),"no_container":true}}));
            assert_eq!(response["result"]["isError"], false, "{response}");
            let report = &response["result"]["structuredContent"];
            assert_eq!(report["written"], json!([]));
            assert_eq!(report["failures"].as_array().unwrap().len(), 1);
            assert_eq!(report["failures"][0]["reason"], "unsafe_path");
            assert_eq!(
                std::fs::read_dir(s.directory.join(format!("out/hostile{index}")))
                    .unwrap()
                    .count(),
                0
            );
        }
        assert!(!s.directory.join("escaped.txt").exists());
        assert!(!s.directory.join("out/escaped.txt").exists());
    });
}

#[test]
fn invalid_utf8_container_output_name_is_tagged_and_written_beneath_output() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    use unityrs_testkit::ngui::{pair, pptr, s as string, vector};
    session(&["--out", "out"], |s| {
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
                    ("m_Name", string("synthetic")),
                    ("m_PreloadTable", vector(pptr("Object", (0, 0)), vec![])),
                    (
                        "m_Container",
                        V::Vector(
                            "map".into(),
                            Box::new(pair(string(""), info.clone())),
                            vec![pair(V::Str(b"folder/\xff".to_vec()), info)],
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
                vec![("m_Name", string("leaf")), ("m_Script", string("payload"))],
            ),
        );
        std::fs::write(
            s.directory.join("raw-container.assets"),
            scene3d::file(Layout::Unity2022, &[], &[bundle, text]).unwrap(),
        )
        .unwrap();
        let probe = s.directory.join(OsStr::from_bytes(b"raw-\xff-probe"));
        let raw_names_supported = match std::fs::write(&probe, b"probe") {
            Ok(()) => {
                std::fs::remove_file(probe).unwrap();
                true
            }
            Err(error) => {
                assert!(
                    matches!(error.raw_os_error(), Some(libc::EILSEQ | libc::EINVAL)),
                    "{error}"
                );
                false
            }
        };
        s.ready();
        let response = s.request("tools/call", json!({"name":"unityrs_extract","arguments":{"inputs":["raw-container.assets"],"output":"raw-container","report":"report.json"}}));
        assert_eq!(response["result"]["isError"], false, "{response}");
        let report = &response["result"]["structuredContent"];
        if !raw_names_supported {
            assert_eq!(report["written"], json!([]));
            assert_eq!(
                report["failures"][0]["path"],
                json!({"t":"str","hex":"666f6c6465722fff"})
            );
            assert_eq!(report["failures"][0]["reason"], "io");
            eprintln!("raw output filename unsupported by this filesystem; tagged refusal checked");
            return;
        }
        assert_eq!(report["failures"], json!([]));
        assert_eq!(
            report["written"][0]["path"],
            json!({"t":"str","hex":"666f6c6465722fff2e747874"})
        );
        let directory = s.directory.join("out/raw-container");
        assert_eq!(
            std::fs::read(directory.join(OsStr::from_bytes(b"folder/\xff.txt"))).unwrap(),
            b"payload"
        );
        let saved: Value =
            serde_json::from_slice(&std::fs::read(directory.join("report.json")).unwrap()).unwrap();
        assert_eq!(&saved, report);
        assert!(!s.directory.join("folder").exists());
        assert!(!s.directory.join("out/folder").exists());
    });
}

#[test]
fn output_subdirectory_limits_precede_all_creation() {
    session(&["--out", "out"], |s| {
        s.ready();
        for (output, limit) in [
            ("a/".repeat(65), "64 components"),
            (format!("a/{}", "b".repeat(4095)), "4096 bytes"),
            ("\"".repeat(400_000), "4096 bytes"),
        ] {
            let response = s.request("tools/call", json!({"name":"unityrs_extract","arguments":{"inputs":["synthetic.assets"],"output":output}}));
            assert_eq!(response["result"]["isError"], true, "{response}");
            let error = &response["result"]["structuredContent"];
            assert_eq!(error["code"], "usage", "{response}");
            assert!(
                error["message"].as_str().unwrap().contains(limit),
                "{response}"
            );
            assert_eq!(
                std::fs::read_dir(s.directory.join("out")).unwrap().count(),
                0
            );
        }
        let output = std::iter::repeat_n("a", 64).collect::<Vec<_>>().join("/");
        let response = s.request("tools/call", json!({"name":"unityrs_extract","arguments":{"inputs":["synthetic.assets"],"output":output,"no_container":true,"asset_filter":["TextAsset"]}}));
        assert_eq!(response["result"]["isError"], false, "{response}");
        assert_eq!(
            std::fs::read(s.directory.join("out").join(output).join("alpha.txt")).unwrap(),
            b"hello"
        );
    });
}

#[test]
fn asset_filter_count_cap_counts_duplicates_and_accepts_the_boundary() {
    session(&["--out", "out"], |s| {
        s.ready();
        let response = s.request("tools/call", json!({"name":"unityrs_extract","arguments":{"inputs":["synthetic.assets"],"output":"refused","asset_filter":vec!["TextAsset";1025]}}));
        assert_eq!(response["result"]["isError"], true, "{response}");
        assert_eq!(response["result"]["structuredContent"]["code"], "usage");
        let message = response["result"]["structuredContent"]["message"]
            .as_str()
            .unwrap();
        assert!(
            message.contains("asset_filter") && message.contains("1024"),
            "{response}"
        );
        assert!(!s.directory.join("out/refused").exists());
        let response = s.request("tools/call", json!({"name":"unityrs_extract","arguments":{"inputs":["synthetic.assets"],"output":"boundary","no_container":true,"asset_filter":vec!["TextAsset";1024]}}));
        assert_eq!(response["result"]["isError"], false, "{response}");
        assert_eq!(
            response["result"]["structuredContent"]["written"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            std::fs::read(s.directory.join("out/boundary/alpha.txt")).unwrap(),
            b"hello"
        );
    });
}

#[test]
fn output_directory_creation_exhausts_request_work_as_a_budget_failure() {
    session(&["--out", "out"], |s| {
        s.ready();
        let output = std::iter::repeat_n("a", 40).collect::<Vec<_>>().join("/");
        let response = s.request("tools/call", json!({"name":"unityrs_extract","arguments":{"inputs":["synthetic.assets"],"output":output,"max_decode_work":3000}}));
        assert_eq!(response["result"]["isError"], true, "{response}");
        assert_eq!(
            response["result"]["structuredContent"]["code"], "budget",
            "{response}"
        );
        assert!(
            s.directory.join("out/a").is_dir(),
            "directory traversal must have begun"
        );
        assert!(!s.directory.join("out").join(output).exists());
        assert_eq!(s.request("ping", json!({}))["result"], json!({}));
    });
}
