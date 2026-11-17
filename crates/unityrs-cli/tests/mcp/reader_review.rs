//! The confined transport retains native file-reader rules for every shared
//! agent fixture and the existing stream/memory reader edge corpus.
use crate::{mcp_support::session, support};
use serde_json::{Value, json};
use std::process::Command;
use unityrs_testkit::child;

fn compare(s: &mut crate::mcp_support::Session, input: &str, path_id: i64) {
    for command in ["inspect", "show"] {
        let mut args = vec![command, input];
        let id = path_id.to_string();
        if command == "show" {
            args.extend(["--path-id", &id]);
        }
        let cli = child::output(
            Command::new(support::BIN)
                .current_dir(&s.directory)
                .args(args),
        )
        .unwrap();
        let expected: Value = serde_json::from_slice(&cli.stdout).unwrap();
        let mut arguments = json!({"inputs":[input]});
        if command == "show" {
            arguments["path_id"] = json!(path_id);
        }
        let response = s.request(
            "tools/call",
            json!({"name":format!("unityrs_{command}"),"arguments":arguments}),
        );
        assert_eq!(
            response["result"]["structuredContent"], expected,
            "{command} {input}: {response}"
        );
        let text: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(text, expected);
    }
}

#[test]
fn confined_and_native_documents_match_shared_fixtures_and_reader_edges() {
    session(&[], |s| {
        s.ready();
        for (input, id) in [
            ("synthetic.assets", -2),
            ("rich.assets", 7),
            ("synthetic.bundle", -2),
            ("broken.bundle", 1),
        ] {
            compare(s, input, id);
        }
        std::fs::write(s.directory.join("twenty.resS"), [b'a'; 20]).unwrap();
        compare(s, "twenty.resS", 1);
        let corpus: Value = serde_json::from_str(include_str!(
            "../../../unityrs-core/tests/fixtures/d/paths.json"
        ))
        .unwrap();
        for (index, case) in corpus["cases"].as_array().unwrap().iter().enumerate() {
            let directory = format!("edge{index}");
            std::fs::create_dir(s.directory.join(&directory)).unwrap();
            for file in case["files"].as_array().unwrap() {
                let bytes: Vec<u8> = file[1]
                    .as_array()
                    .unwrap()
                    .iter()
                    .flat_map(|r| {
                        std::iter::repeat_n(
                            u8::try_from(r[1].as_u64().unwrap()).unwrap(),
                            usize::try_from(r[0].as_u64().unwrap()).unwrap(),
                        )
                    })
                    .collect();
                let input = format!("{directory}/{}", file[0].as_str().unwrap());
                std::fs::write(s.directory.join(&input), bytes).unwrap();
                compare(s, &input, 1);
            }
        }
    });
}
